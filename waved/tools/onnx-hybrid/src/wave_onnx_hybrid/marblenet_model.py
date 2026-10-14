"""Pinned MarbleNet checkpoint reconstruction, exact CPU features and exports.

Preparation uses optional PyTorch; this is a source-matched reconstruction,
not execution of the complete NeMo framework. No native libraries are loaded.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import tarfile
from pathlib import Path
from typing import Any

import numpy as np
import onnx
import onnxruntime as ort
import soundfile as sf
import torch
import yaml
from onnx import TensorProto, helper, numpy_helper
from torch import nn

from .audit import canonical_json
from .errors import ErrorCode, HybridError
from .marblenet_graph import (
    NUMERICAL_POLICY,
    import_graph,
    independent_graph_oracle,
    validate_graph_output,
)

CHECKPOINT_SHA256 = "84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706"
NEMO_REVISION = "ceeafa4f2b8cece8a0fd47b29051ad659e55b76c"
HF_REVISION = "9d25eb877d9edab5101ccc843edd04787a6c72c4"
SPEECH_URL = "https://dldata-public.s3.us-east-2.amazonaws.com/2086-149220-0033.wav"
SPEECH_SHA256 = "5fceacff0315d49cb59fcc505bcecf1ed5f2f35c2897b1e65a59f30e5d922150"
SAMPLES = 63840
BN_EPSILON = 0.001
FP32_POLICY = {"atol": 0.0001, "rtol": 0.00001}
PROBABILITY_POLICY = {"max_abs_error": 0.02, "minimum_decision_agreement": 0.99}
# (input channels, output channels, repeat, temporal kernel, stride, dilation,
# separable, projected residual)
BLOCKS = [
    (80, 128, 1, 11, 2, 1, True, False),
    (128, 64, 2, 13, 1, 1, True, True),
    (64, 64, 2, 15, 1, 1, True, True),
    (64, 64, 2, 17, 1, 1, True, True),
    (64, 128, 1, 29, 1, 2, True, False),
    (128, 128, 1, 1, 1, 1, False, False),
]


def _require(ok: bool, message: str, code=ErrorCode.MODEL_PACKAGE_INVALID) -> None:
    if not ok:
        raise HybridError(code, message)


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def expected_state_shapes() -> dict[str, tuple[int, ...]]:
    """Account for every state tensor, including BN state and CPU buffers."""
    shapes = {
        "preprocessor.featurizer.window": (400,),
        "preprocessor.featurizer.fb": (1, 80, 257),
    }

    def bn(prefix, c):
        for name in ("weight", "bias", "running_mean", "running_var"):
            shapes[prefix + "." + name] = (c,)
        shapes[prefix + ".num_batches_tracked"] = ()

    for b, (cin, cout, repeat, k, _, _, sep, res) in enumerate(BLOCKS):
        prefix = f"encoder.encoder.{b}"
        for r in range(repeat):
            ci = cin if r == 0 else cout
            idx = r * 5 if sep else r * 4
            if sep:
                shapes[f"{prefix}.mconv.{idx}.conv.weight"] = (ci, 1, k)
                shapes[f"{prefix}.mconv.{idx + 1}.conv.weight"] = (cout, ci, 1)
                bn(f"{prefix}.mconv.{idx + 2}", cout)
            else:
                shapes[f"{prefix}.mconv.{idx}.conv.weight"] = (cout, ci, k)
                bn(f"{prefix}.mconv.{idx + 1}", cout)
        if res:
            shapes[f"{prefix}.res.0.0.conv.weight"] = (cout, cin, 1)
            bn(f"{prefix}.res.0.1", cout)
    shapes.update({"decoder.layer0.weight": (2, 128), "decoder.layer0.bias": (2,)})
    return shapes


def validate_state_dict(state: dict[str, torch.Tensor]) -> None:
    expected = expected_state_shapes()
    _require(
        isinstance(state, dict) and set(state) == set(expected) and len(state) == 84,
        "checkpoint requires exactly84 known tensors",
    )
    for key, shape in expected.items():
        x = state[key]
        dtype = torch.int64 if key.endswith("num_batches_tracked") else torch.float32
        _require(
            isinstance(x, torch.Tensor)
            and tuple(x.shape) == shape
            and x.dtype == dtype
            and x.device.type == "cpu"
            and x.is_contiguous(),
            f"checkpoint tensor contract mismatch: {key}",
        )
        _require(bool(torch.isfinite(x).all()), f"checkpoint nonfinite tensor: {key}")
        if key.endswith("running_var"):
            _require(bool((x >= 0).all()), f"negative BN running variance: {key}")


def load_checkpoint(checkpoint: Path) -> dict[str, Any]:
    """Read only exact regular members and restricted CPU tensor state."""
    with Path(checkpoint).open("rb") as handle:
        payload = handle.read(501761)
    _require(
        len(payload) == 501760 and _sha(payload) == CHECKPOINT_SHA256,
        "checkpoint identity mismatch",
        ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
    )
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
        members = archive.getmembers()
        _require(
            len(members) == 3
            and [m.name for m in members]
            == [".", "./model_config.yaml", "./model_weights.ckpt"],
            "unexpected checkpoint archive inventory",
        )
        _require(
            members[0].isdir()
            and all(
                m.isfile()
                and not m.issym()
                and not m.islnk()
                and m.size < 2 * 1024 * 1024
                for m in members[1:]
            ),
            "unsafe checkpoint member",
        )
        config_bytes = archive.extractfile(members[1]).read()
        weights_bytes = archive.extractfile(members[2]).read()
    config = yaml.safe_load(config_bytes)
    _require(
        config["nemo_version"] == "2.1.0rc0"
        and config["encoder"]["feat_in"] == 80
        and config["encoder"]["activation"] == "relu"
        and config["encoder"]["conv_mask"] is True
        and config["decoder"]["num_layers"] == 1
        and config["decoder"]["log_softmax"] is False,
        "checkpoint architecture mismatch",
    )
    state = torch.load(io.BytesIO(weights_bytes), map_location="cpu", weights_only=True)
    validate_state_dict(state)
    return {
        "state": state,
        "config": config,
        "checkpoint_sha256": CHECKPOINT_SHA256,
        "config_sha256": _sha(config_bytes),
        "weights_sha256": _sha(weights_bytes),
    }


def preprocess_audio(
    waveform: np.ndarray, state: dict[str, torch.Tensor]
) -> np.ndarray:
    """Evaluation preprocessing with the exact serialized Hann/mel buffers."""
    _require(
        isinstance(waveform, np.ndarray)
        and waveform.dtype == np.dtype("<f4")
        and waveform.shape == (SAMPLES,)
        and np.isfinite(waveform).all(),
        "audio requires63840 finite mono float32 samples",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    with torch.no_grad():
        x = torch.from_numpy(np.ascontiguousarray(waveform))[None]
        x = torch.cat((x[:, :1], x[:, 1:] - 0.97 * x[:, :-1]), dim=1)
        z = torch.stft(
            x,
            n_fft=512,
            hop_length=160,
            win_length=400,
            window=state["preprocessor.featurizer.window"],
            center=True,
            pad_mode="reflect",
            normalized=False,
            onesided=True,
            return_complex=True,
        )
        magnitude = torch.sqrt(torch.view_as_real(z).pow(2).sum(-1))
        features = torch.log(
            torch.matmul(state["preprocessor.featurizer.fb"], magnitude.pow(2)) + 2**-24
        )
    _require(
        tuple(features.shape) == (1, 80, 400) and bool(torch.isfinite(features).all()),
        "feature extraction contract mismatch",
    )
    return np.ascontiguousarray(features.numpy(), dtype=np.float32)


def _conv(state, key, cin, cout, k, stride=1, dilation=1, groups=1, bias=None):
    conv = nn.Conv1d(
        cin,
        cout,
        k,
        stride=stride,
        padding=dilation * (k - 1) // 2,
        dilation=dilation,
        groups=groups,
        bias=bias is not None,
    )
    with torch.no_grad():
        conv.weight.copy_(state[key])
        if bias is not None:
            conv.bias.copy_(state[bias])
    return conv


def _bn(state, prefix, c):
    bn = nn.BatchNorm1d(c, eps=BN_EPSILON, momentum=0.1)
    bn.load_state_dict(
        {
            k: state[prefix + "." + k]
            for k in (
                "weight",
                "bias",
                "running_mean",
                "running_var",
                "num_batches_tracked",
            )
        }
    )
    return bn.eval()


def _fold(conv, bn):
    result = nn.Conv1d(
        conv.in_channels,
        conv.out_channels,
        conv.kernel_size,
        conv.stride,
        conv.padding,
        conv.dilation,
        conv.groups,
        bias=True,
    )
    with torch.no_grad():
        scale = bn.weight / torch.sqrt(bn.running_var + BN_EPSILON)
        result.weight.copy_(conv.weight * scale[:, None, None])
        bias = conv.bias if conv.bias is not None else torch.zeros_like(bn.running_mean)
        result.bias.copy_(bn.bias + scale * (bias - bn.running_mean))
    return result


class _Unit(nn.Module):
    def __init__(self, state, prefix, idx, cin, cout, k, s, d, sep, folded):
        super().__init__()
        self.depthwise = (
            _conv(state, f"{prefix}.mconv.{idx}.conv.weight", cin, cin, k, s, d, cin)
            if sep
            else nn.Identity()
        )
        self.pointwise = _conv(
            state,
            f"{prefix}.mconv.{idx + 1 if sep else idx}.conv.weight",
            cin,
            cout,
            1 if sep else k,
            1 if sep else s,
            1 if sep else d,
        )
        self.bn = _bn(state, f"{prefix}.mconv.{idx + 2 if sep else idx + 1}", cout)
        if folded:
            self.pointwise = _fold(self.pointwise, self.bn)
            self.bn = nn.Identity()

    def forward(self, x):
        return self.bn(self.pointwise(self.depthwise(x)))


class _Block(nn.Module):
    def __init__(self, state, b, folded):
        super().__init__()
        cin, cout, repeat, k, s, d, sep, res = BLOCKS[b]
        prefix = f"encoder.encoder.{b}"
        self.units = nn.ModuleList(
            [
                _Unit(
                    state,
                    prefix,
                    r * 5 if sep else r * 4,
                    cin if r == 0 else cout,
                    cout,
                    k,
                    s,
                    d,
                    sep,
                    folded,
                )
                for r in range(repeat)
            ]
        )
        self.projection = None
        if res:
            conv = _conv(state, f"{prefix}.res.0.0.conv.weight", cin, cout, 1)
            bn = _bn(state, f"{prefix}.res.0.1", cout)
            self.projection = _fold(conv, bn) if folded else nn.Sequential(conv, bn)

    def forward(self, x):
        y = x
        for i, unit in enumerate(self.units):
            y = unit(y)
            if i < len(self.units) - 1:
                y = torch.relu(y)
        if self.projection is not None:
            y = y + self.projection(x)
        return torch.relu(y)


class SourceMatchedMarbleNet(nn.Module):
    """Unfused checkpoint eval semantics reconstructed from pinned NeMo source."""

    def __init__(self, state):
        super().__init__()
        validate_state_dict(state)
        self.blocks = nn.ModuleList([_Block(state, b, False) for b in range(6)])
        self.decoder = nn.Linear(128, 2)
        with torch.no_grad():
            self.decoder.weight.copy_(state["decoder.layer0.weight"])
            self.decoder.bias.copy_(state["decoder.layer0.bias"])
        self.eval()

    def activations(self, x):
        values = []
        for block in self.blocks:
            values.append(x)
            x = block(x)
        return x, values

    def forward(self, x):
        x, _ = self.activations(x)
        return self.decoder(x.transpose(1, 2))


class FoldedMarbleNet(nn.Module):
    """Folded FP32 backbone/classifier returning NCW logits."""

    def __init__(self, state):
        super().__init__()
        validate_state_dict(state)
        self.blocks = nn.ModuleList([_Block(state, b, True) for b in range(6)])
        self.classifier = nn.Conv1d(128, 2, 1)
        with torch.no_grad():
            self.classifier.weight.copy_(state["decoder.layer0.weight"][:, :, None])
            self.classifier.bias.copy_(state["decoder.layer0.bias"])
        self.eval()

    def forward(self, x):
        for block in self.blocks:
            x = block(x)
        return self.classifier(x)


class _GraphBuilder:
    def __init__(self, state, dtype, folded):
        self.state = state
        self.dtype = dtype
        self.folded = folded
        self.nodes = []
        self.constants = []
        self.counter = 0

    def constant(self, name, array):
        self.constants.append(
            numpy_helper.from_array(np.ascontiguousarray(array, dtype=self.dtype), name)
        )
        return name

    def output(self):
        self.counter += 1
        return f"activation_{self.counter}"

    def conv(self, x, weight, bn=None, *, stride=1, dilation=1, group=1, bias=None):
        w = self.state[weight].numpy().copy()
        if w.ndim == 2:
            w = w[:, :, None]
        b = self.state[bias].numpy().copy() if bias else None
        if bn and self.folded:
            a = self.state[bn + ".weight"].numpy() / np.sqrt(
                self.state[bn + ".running_var"].numpy() + np.float32(BN_EPSILON)
            )
            w = w * a[:, None, None]
            b = self.state[bn + ".bias"].numpy() + a * (
                (b if b is not None else 0) - self.state[bn + ".running_mean"].numpy()
            )
        name = self.output()
        inputs = [x, self.constant(name + "_weight", w)]
        if b is not None:
            inputs.append(self.constant(name + "_bias", b))
        k = w.shape[2]
        pad = dilation * (k - 1) // 2
        self.nodes.append(
            helper.make_node(
                "Conv",
                inputs,
                [name],
                kernel_shape=[k],
                strides=[stride],
                dilations=[dilation],
                pads=[pad, pad],
                group=group,
            )
        )
        if bn and not self.folded:
            out = self.output()
            ins = [name]
            for key in ("weight", "bias", "running_mean", "running_var"):
                ins.append(
                    self.constant(out + "_" + key, self.state[bn + "." + key].numpy())
                )
            self.nodes.append(
                helper.make_node(
                    "BatchNormalization", ins, [out], epsilon=BN_EPSILON, momentum=0.9
                )
            )
            name = out
        return name

    def relu(self, x):
        out = self.output()
        self.nodes.append(helper.make_node("Relu", [x], [out]))
        return out

    def block(self, x, b):
        cin, cout, repeat, _k, s, d, sep, res = BLOCKS[b]
        prefix = f"encoder.encoder.{b}"
        start = x
        for r in range(repeat):
            ci = cin if r == 0 else cout
            idx = r * 5 if sep else r * 4
            if sep:
                x = self.conv(
                    x,
                    f"{prefix}.mconv.{idx}.conv.weight",
                    stride=s,
                    dilation=d,
                    group=ci,
                )
                x = self.conv(
                    x,
                    f"{prefix}.mconv.{idx + 1}.conv.weight",
                    f"{prefix}.mconv.{idx + 2}",
                )
            else:
                x = self.conv(
                    x,
                    f"{prefix}.mconv.{idx}.conv.weight",
                    f"{prefix}.mconv.{idx + 1}",
                    stride=s,
                    dilation=d,
                )
            if r < repeat - 1:
                x = self.relu(x)
        if res:
            skip = self.conv(
                start, f"{prefix}.res.0.0.conv.weight", f"{prefix}.res.0.1"
            )
            out = self.output()
            self.nodes.append(helper.make_node("Add", [x, skip], [out]))
            x = out
        return self.relu(x)

    def model(self, control):
        if control == "18a":
            input_shape = [1, 80, 200]
            output_shape = [1, 128, 200]
            out = self.relu(
                self.conv(
                    "input",
                    "encoder.encoder.0.mconv.1.conv.weight",
                    "encoder.encoder.0.mconv.2",
                )
            )
        elif control in {"18b", "18c"}:
            b = 1 if control == "18b" else 4
            input_shape = [1, BLOCKS[b][0], 200]
            output_shape = [1, BLOCKS[b][1], 200]
            out = self.block("input", b)
        else:
            input_shape = [1, 80, 400]
            output_shape = [1, 2, 200]
            out = "input"
            for b in range(6):
                out = self.block(out, b)
            out = self.conv(out, "decoder.layer0.weight", bias="decoder.layer0.bias")
        if control == "original":
            name = self.output()
            self.nodes.append(
                helper.make_node("Transpose", [out], [name], perm=[0, 2, 1])
            )
            out = name
            output_shape = [1, 200, 2]
        self.nodes[-1].output[0] = "output"
        dtype = TensorProto.FLOAT16 if self.dtype == np.float16 else TensorProto.FLOAT
        graph = helper.make_graph(
            self.nodes,
            control,
            [helper.make_tensor_value_info("input", dtype, input_shape)],
            [helper.make_tensor_value_info("output", dtype, output_shape)],
            self.constants,
        )
        model = helper.make_model(
            graph,
            opset_imports=[helper.make_opsetid("", 13)],
            ir_version=8,
            producer_name="wave-source-matched-marblenet-v1",
        )
        helper.set_model_props(
            model,
            {
                "checkpoint_sha256": CHECKPOINT_SHA256,
                "nemo_source_revision": NEMO_REVISION,
                "reference_kind": "source-matched-reconstruction",
                "valid_feature_frames": "400",
            },
        )
        onnx.checker.check_model(model, full_check=True)
        return model


def export_model(
    state, path: Path, control="18d", *, dtype=np.float16, folded=True
) -> None:
    onnx.save_model(_GraphBuilder(state, dtype, folded).model(control), path)


def _fp32_parity(reference, observed, label):
    reference = np.asarray(reference, dtype=np.float32)
    observed = np.asarray(observed, dtype=np.float32)
    error = np.abs(observed - reference)
    bad = error > FP32_POLICY["atol"] + FP32_POLICY["rtol"] * np.abs(reference)
    _require(
        reference.shape == observed.shape
        and np.isfinite(reference).all()
        and np.isfinite(observed).all()
        and not bad.any(),
        f"FP32 parity failed: {label}",
        ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
    )
    return {
        "status": "PASS",
        "max_abs_error": float(error.max()),
        "elements": reference.size,
        "policy": dict(FP32_POLICY),
    }


def probabilities(logits: np.ndarray) -> np.ndarray:
    """Map NCW two-class logits to frame speech probabilities on CPU."""
    _require(
        logits.shape == (1, 2, 200) and np.isfinite(logits).all(),
        "invalid full-model logits",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    x = logits.astype(np.float32)
    x = x - x.max(axis=1, keepdims=True)
    ex = np.exp(x)
    return np.ascontiguousarray(
        (ex / ex.sum(axis=1, keepdims=True))[0, 1], dtype=np.float32
    )


def probability_comparison(
    reference: np.ndarray, observed: np.ndarray
) -> dict[str, Any]:
    _require(
        reference.shape == observed.shape == (200,)
        and np.isfinite(reference).all()
        and np.isfinite(observed).all(),
        "invalid speech probability arrays",
    )
    error = np.abs(reference - observed)
    agreement = float(np.mean((reference >= 0.5) == (observed >= 0.5)))
    result = {
        "status": "PASS" if error.max() <= 0.02 and agreement >= 0.99 else "FAIL",
        "max_abs_error": float(error.max()),
        "decision_agreement": agreement,
        "disagreements": int(np.sum((reference >= 0.5) != (observed >= 0.5))),
        "elements": 200,
        "policy": dict(PROBABILITY_POLICY),
    }
    _require(
        result["status"] == "PASS",
        "speech probabilities exceed frozen tolerance",
        ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
    )
    return result


def prepare_marblenet(
    checkpoint: Path, output_dir: Path, speech_wav: Path
) -> dict[str, Any]:
    """Freeze real trained exports and four audio inputs in an exclusive dir."""
    _require(
        str(torch.__version__).split("+")[0] == "2.5.1"
        and onnx.__version__ == "1.22.0"
        and ort.__version__ == "1.27.0"
        and np.__version__ == "2.5.1",
        "preparation requires the pinned Torch/ONNX/ORT/NumPy environment",
        ErrorCode.MODEL_STATE_VERSION_MISMATCH,
    )
    torch.set_num_threads(1)
    data = load_checkpoint(checkpoint)
    state = data["state"]
    speech_bytes = Path(speech_wav).read_bytes()
    _require(
        _sha(speech_bytes) == SPEECH_SHA256,
        "public speech fixture identity mismatch",
        ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
    )
    speech, sample_rate = sf.read(speech_wav, dtype="float32", always_2d=False)
    _require(
        sample_rate == 16000 and speech.ndim == 1 and len(speech) >= SAMPLES,
        "public speech fixture format mismatch",
    )
    output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=False)
    learned_parameters = sum(
        tensor.numel()
        for name, tensor in state.items()
        if not name.startswith("preprocessor.")
        and not any(
            term in name
            for term in ("running_mean", "running_var", "num_batches_tracked")
        )
    )
    _require(learned_parameters == 91378, "learned parameter coverage mismatch")
    source = SourceMatchedMarbleNet(state)
    folded = FoldedMarbleNet(state)
    original_path = output_dir / "marblenet_original_fp32.onnx"
    folded_path = output_dir / "marblenet_folded_fp32.onnx"
    export_model(state, original_path, "original", dtype=np.float32, folded=False)
    export_model(state, folded_path, "18d", dtype=np.float32)
    controls = {}
    for name in ("18a", "18b", "18c", "18d"):
        path = output_dir / (name + ".onnx")
        export_model(state, path, name)
        record = import_graph(path)
        (output_dir / (name + "_lowering.json")).write_bytes(canonical_json(record))
        controls[name] = {
            "model": str(path),
            "model_sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
            "lowering": str(output_dir / (name + "_lowering.json")),
            "nodes": len(record["nodes"]),
            "constant_count": len(record["constants"]),
        }
    sessions = {
        name: ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
        for name, path in [
            ("original", original_path),
            ("folded", folded_path),
            *[(n, output_dir / (n + ".onnx")) for n in controls],
        ]
    }
    rng = np.random.default_rng(180018)
    noise = rng.normal(0, 0.005, SAMPLES).astype(np.float32)
    speech = np.ascontiguousarray(speech[:SAMPLES], dtype=np.float32)
    clips = {
        "silence": np.zeros(SAMPLES, np.float32),
        "noise": noise,
        "speech": speech,
        "noisy-speech": np.clip(speech + noise, -1, 1).astype(np.float32),
    }
    clip_receipts = {}
    default_arrays = None
    full_record = import_graph(output_dir / "18d.onnx")
    for name, wave in clips.items():
        features = preprocess_audio(wave, state)
        half = features.astype(np.float16)
        with torch.no_grad():
            tensor = torch.from_numpy(features)
            reference = source(tensor).numpy().transpose(0, 2, 1).copy()
            torch_folded = folded(tensor).numpy()
            _, inputs = source.activations(tensor)
            cut_inputs = {
                "18a": source.blocks[0]
                .units[0]
                .depthwise(tensor)
                .numpy()
                .astype(np.float16),
                "18b": inputs[1].numpy().astype(np.float16),
                "18c": inputs[4].numpy().astype(np.float16),
            }
        orig_ort = (
            sessions["original"]
            .run(None, {"input": features})[0]
            .transpose(0, 2, 1)
            .copy()
        )
        folded_ort = sessions["folded"].run(None, {"input": features})[0]
        fp16_ort = sessions["18d"].run(None, {"input": half})[0]
        independent = independent_graph_oracle(full_record, {"input": half})["output"]
        parity = {
            "torch_folded": _fp32_parity(reference, torch_folded, "torch-folded"),
            "ort_original": _fp32_parity(reference, orig_ort, "ort-original"),
            "ort_folded": _fp32_parity(reference, folded_ort, "ort-folded"),
            "fp16_ort_vs_independent": validate_graph_output(
                full_record, independent.tobytes(), fp16_ort.tobytes()
            ),
        }
        prob_ref = probabilities(reference)
        prob_half = probabilities(fp16_ort)
        prob_ind = probabilities(independent)
        parity["fp16_probabilities"] = probability_comparison(prob_ref, prob_half)
        parity["independent_fp16_probabilities"] = probability_comparison(
            prob_ref, prob_ind
        )
        np.save(output_dir / (name + "_waveform.npy"), wave, allow_pickle=False)
        np.save(
            output_dir / (name + "_features_fp32.npy"), features, allow_pickle=False
        )
        np.save(
            output_dir / (name + "_probabilities_fp32.npy"),
            prob_ref,
            allow_pickle=False,
        )
        np.save(
            output_dir / (name + "_probabilities_fp16.npy"),
            prob_half,
            allow_pickle=False,
        )
        np.savez(output_dir / ("18d_" + name + "_input.npz"), input=half)
        np.savez(output_dir / ("18d_" + name + "_expected.npz"), output=independent)
        (output_dir / ("18d_" + name + "_original_fp32.bin")).write_bytes(
            reference.astype("<f4").tobytes()
        )
        (output_dir / ("18d_" + name + "_ort_fp16.bin")).write_bytes(fp16_ort.tobytes())
        clip_receipts[name] = {
            "synthetic": name in {"silence", "noise", "noisy-speech"},
            "sample_rate": 16000,
            "samples": SAMPLES,
            "valid_frames": 400,
            "waveform_sha256": _sha(wave.tobytes()),
            "features_fp32_sha256": _sha(features.tobytes()),
            "features_fp16_sha256": _sha(half.tobytes()),
            "probabilities_fp32_sha256": _sha(prob_ref.tobytes()),
            "probabilities_fp16_sha256": _sha(prob_half.tobytes()),
            "independent_fp16_sha256": _sha(independent.tobytes()),
            "input": str(output_dir / ("18d_" + name + "_input.npz")),
            "expected": str(output_dir / ("18d_" + name + "_expected.npz")),
            "reference_speech_frames": int((prob_ref >= 0.5).sum()),
            "fp16_speech_frames": int((prob_half >= 0.5).sum()),
            "parity": parity,
        }
        if name == "speech":
            default_arrays = (half, independent, cut_inputs)
    half, independent, cut_inputs = default_arrays
    for name in controls:
        x = half if name == "18d" else cut_inputs[name]
        record = import_graph(output_dir / (name + ".onnx"))
        oracle = independent_graph_oracle(record, {"input": x})["output"]
        ort_output = sessions[name].run(None, {"input": x})[0]
        metric = validate_graph_output(record, oracle.tobytes(), ort_output.tobytes())
        np.savez(output_dir / (name + "_input.npz"), input=x)
        np.savez(output_dir / (name + "_expected.npz"), output=oracle)
        (output_dir / (name + "_ort_fp16.bin")).write_bytes(ort_output.tobytes())
        controls[name].update(
            input=str(output_dir / (name + "_input.npz")),
            expected=str(output_dir / (name + "_expected.npz")),
            input_sha256=_sha(x.tobytes()),
            independent_output_sha256=_sha(oracle.tobytes()),
            ort_output_sha256=_sha(ort_output.tobytes()),
            cpu_parity=metric,
        )
    manifest = {
        "schema_version": 1,
        "reference_kind": "source-matched-reconstruction",
        "checkpoint_sha256": CHECKPOINT_SHA256,
        "weights_sha256": data["weights_sha256"],
        "config_sha256": data["config_sha256"],
        "nemo_source_revision": NEMO_REVISION,
        "hf_revision": HF_REVISION,
        "speech_source": {
            "url": SPEECH_URL,
            "sha256": SPEECH_SHA256,
            "samples_total": len(sf.read(speech_wav)[0]),
            "selected_samples": [0, SAMPLES],
        },
        "tensor_count": 84,
        "state_tensors": {
            name: {
                "shape": list(tensor.shape),
                "dtype": str(tensor.dtype),
                "sha256": _sha(tensor.numpy().tobytes()),
            }
            for name, tensor in state.items()
        },
        "learned_parameters": learned_parameters,
        "bn_epsilon": BN_EPSILON,
        "feature_contract": [1, 80, 400],
        "logit_contract": [1, 2, 200],
        "fp32_policy": dict(FP32_POLICY),
        "numerical_policy": dict(NUMERICAL_POLICY),
        "probability_policy": dict(PROBABILITY_POLICY),
        "context": {
            "receptive_field_feature_frames": 291,
            "future_feature_frames": 145,
            "future_seconds_without_stft": 1.45,
        },
        "versions": {
            "torch": torch.__version__,
            "onnx": onnx.__version__,
            "onnxruntime": ort.__version__,
            "numpy": np.__version__,
        },
        "controls": controls,
        "clips": clip_receipts,
        "fp32_models": {
            "original": {
                "path": str(original_path),
                "sha256": _sha(original_path.read_bytes()),
            },
            "folded": {
                "path": str(folded_path),
                "sha256": _sha(folded_path.read_bytes()),
            },
        },
    }
    manifest["files"] = {
        f.name: {"sha256": _sha(f.read_bytes()), "bytes": f.stat().st_size}
        for f in sorted(output_dir.iterdir())
        if f.is_file()
    }
    (output_dir / "preparation.json").write_bytes(canonical_json(manifest))
    return manifest


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("prepare")
    prepare.add_argument("--checkpoint", type=Path, required=True)
    prepare.add_argument("--output-dir", type=Path, required=True)
    prepare.add_argument("--speech-wav", type=Path, required=True)
    args = parser.parse_args(argv)
    manifest = prepare_marblenet(args.checkpoint, args.output_dir, args.speech_wav)
    print(
        json.dumps(
            {
                "preparation": str(args.output_dir / "preparation.json"),
                "controls": manifest["controls"],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
