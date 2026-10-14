"""Frozen kernel/dilation isolation controls derived from trained PoC18 block18C.

These diagnostic variants are not equivalents of the complete trained model.
No device, compiler or vendor runtime is contacted by this module.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import stat
from pathlib import Path
from typing import Any

import numpy as np
import onnx
import onnxruntime as ort
from onnx import numpy_helper

from .audit import canonical_json
from .errors import ErrorCode, HybridError
from .fixtures import deterministic_npz_bytes
from .marblenet_graph import (
    import_graph,
    independent_graph_oracle,
    validate_graph_output,
)

PREPARATION_SHA256 = "4fcc00ccc0f1983f0a43c88053586da7ec3b5154ff13e4829f55972912db38cb"
SOURCE_MODEL_SHA256 = "561b5f00666d3790afb82f03b49a615d22ff979cfcb958b1512f4e9295fcba27"
SOURCE_INPUT_SHA256 = "13a01263dcbf489a71f1afe8c1446b1833abde1fa8631dd0768bc08cfe8a5d6e"
REPOSITORY_ROOT = Path(__file__).resolve().parents[5]
OUTPUT_ROOT = REPOSITORY_ROOT / "artifacts/npu/poc19"
_VARIANTS = {
    "19a": {"kernel": 13, "dilation": 2, "pads": [12, 12], "tap_slice": [8, 21]},
    "19b": {"kernel": 29, "dilation": 1, "pads": [14, 14], "tap_slice": [0, 29]},
}


def _require(ok: bool, message: str, code=ErrorCode.EVIDENCE_INVALID) -> None:
    if not ok:
        raise HybridError(code, message)


def _sha(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _regular(path: Path, maximum: int = 8 * 1024 * 1024) -> bytes:
    _require(
        path.is_file() and not path.is_symlink() and stat.S_ISREG(path.stat().st_mode),
        f"artifact is not a regular unlinked file: {path}",
    )
    with path.open("rb") as handle:
        payload = handle.read(maximum + 1)
    _require(len(payload) <= maximum, f"artifact exceeds bound: {path}")
    return payload


def _write(path: Path, payload: bytes) -> None:
    try:
        with path.open("xb") as handle:
            handle.write(payload)
    except FileExistsError as exc:
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS, f"isolation artifact already exists: {path}"
        ) from exc


def _frozen(prepared: Path):
    _require(
        prepared.is_dir() and not prepared.is_symlink(),
        "frozen preparation directory is missing/linked",
    )
    receipt_bytes = _regular(prepared / "preparation.json")
    _require(
        _sha(receipt_bytes) == PREPARATION_SHA256, "frozen PoC18 preparation changed"
    )
    receipt = json.loads(receipt_bytes)
    for name, identity in receipt["files"].items():
        _require(
            Path(name).name == name and name not in {".", ".."},
            "unsafe frozen artifact name",
        )
        payload = _regular(prepared / name)
        _require(
            len(payload) == identity["bytes"] and _sha(payload) == identity["sha256"],
            f"frozen artifact changed: {name}",
        )
    source_bytes = _regular(prepared / "18c.onnx")
    _require(_sha(source_bytes) == SOURCE_MODEL_SHA256, "frozen18C model changed")
    model = onnx.load_model_from_string(source_bytes)
    record = import_graph(prepared / "18c.onnx")
    _require(
        record["profile_id"] == "fp16-marblenet-v1"
        and record["onnx_input"]["shape"] == [1, 64, 200]
        and record["onnx_output"]["shape"] == [1, 128, 200]
        and len(record["nodes"]) == 3,
        "frozen18C tensor/graph contract changed",
    )
    input_npz = _regular(prepared / "18c_input.npz")
    with np.load(prepared / "18c_input.npz", allow_pickle=False) as archive:
        _require(archive.files == ["input"], "frozen18C input names changed")
        values = np.array(archive["input"], copy=True, order="C")
    _require(
        values.dtype == np.dtype("<f2")
        and values.shape == (1, 64, 200)
        and np.isfinite(values).all()
        and _sha(values.tobytes()) == SOURCE_INPUT_SHA256,
        "frozen18C input dtype/shape/finite/identity changed",
    )
    return model, record, values, input_npz


def _first_weight(model: onnx.ModelProto):
    name = model.graph.node[0].input[1]
    return next(t for t in model.graph.initializer if t.name == name)


def _change_attributes(node: onnx.NodeProto, spec: dict) -> None:
    changed = {
        "kernel_shape": [spec["kernel"]],
        "dilations": [spec["dilation"]],
        "pads": spec["pads"],
    }
    _require(
        {a.name for a in node.attribute} >= set(changed),
        "source depthwise attributes missing",
    )
    for attr in node.attribute:
        if attr.name in changed:
            del attr.ints[:]
            attr.ints.extend(changed[attr.name])


def _derive_variant(source: onnx.ModelProto, control: str) -> onnx.ModelProto:
    _require(control in _VARIANTS, "unknown isolation control")
    _require(
        _sha(source.SerializeToString()) == SOURCE_MODEL_SHA256,
        "isolation source is not frozen18C",
    )
    spec = _VARIANTS[control]
    result = copy.deepcopy(source)
    _change_attributes(result.graph.node[0], spec)
    if control == "19a":
        original = _first_weight(source)
        weights = numpy_helper.to_array(original)
        sliced = weights[..., 8:21].copy()
        _first_weight(result).CopyFrom(numpy_helper.from_array(sliced, original.name))
    return result


def _validate_variant(
    source: onnx.ModelProto, variant: onnx.ModelProto, control: str
) -> None:
    """Prove exact tap bits and that only the approved first Conv changed."""
    _require(
        control in _VARIANTS
        and _sha(source.SerializeToString()) == SOURCE_MODEL_SHA256,
        "variant requires frozen18C source",
    )
    spec = _VARIANTS[control]
    source_weight = _first_weight(source)
    weights = numpy_helper.to_array(source_weight)
    _require(
        weights.dtype == np.dtype("<f2") and weights.shape == (64, 1, 29),
        "source depthwise weights changed",
    )
    first = _first_weight(variant)
    wanted = copy.deepcopy(source_weight)
    if control == "19a":
        wanted.CopyFrom(
            numpy_helper.from_array(weights[..., 8:21].copy(), source_weight.name)
        )
    _require(
        first.SerializeToString() == wanted.SerializeToString(),
        "isolation tap bits/dtype/metadata changed",
    )
    wanted_node = copy.deepcopy(source.graph.node[0])
    _change_attributes(wanted_node, spec)
    _require(
        variant.graph.node[0].SerializeToString() == wanted_node.SerializeToString(),
        "isolation kernel/dilation/padding/wiring changed",
    )
    restored = copy.deepcopy(variant)
    _first_weight(restored).CopyFrom(source_weight)
    restored.graph.node[0].CopyFrom(source.graph.node[0])
    _require(
        restored.SerializeToString() == source.SerializeToString(),
        "downstream nodes/constants or model metadata changed",
    )
    onnx.checker.check_model(variant, full_check=True)


def _references(payload: bytes, record: dict, values: np.ndarray):
    _require(
        ort.__version__ == "1.27.0",
        "isolation reference requires pinned ORT1.27.0",
        ErrorCode.MODEL_STATE_VERSION_MISMATCH,
    )
    session = ort.InferenceSession(payload, providers=["CPUExecutionProvider"])
    _require(
        session.get_providers() == ["CPUExecutionProvider"],
        "isolation reference provider differs",
    )
    observed = session.run(["output"], {"input": values})[0]
    expected = independent_graph_oracle(record, {"input": values})["output"]
    _require(
        isinstance(observed, np.ndarray)
        and observed.dtype == np.dtype("<f2")
        and observed.shape == (1, 128, 200)
        and np.isfinite(observed).all(),
        "ORT diagnostic output contract/nonfinite failure",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    observed = np.ascontiguousarray(observed)
    comparison = validate_graph_output(record, expected.tobytes(), observed.tobytes())
    return expected, observed, comparison


def prepare_isolation(prepared: Path, output: Path) -> dict[str, Any]:
    """Emit both reviewed diagnostic candidates and actual host references."""
    prepared, output = Path(prepared), Path(output)
    _require(
        not output.is_symlink(), "isolation output is linked", ErrorCode.OUTPUT_EXISTS
    )
    resolved_prepared, resolved_output = prepared.resolve(), output.resolve()
    _require(
        resolved_output != resolved_prepared
        and resolved_prepared not in resolved_output.parents
        and resolved_output not in resolved_prepared.parents,
        "isolation output aliases original preparation",
        ErrorCode.POLICY_INVALID,
    )
    _require(
        OUTPUT_ROOT.resolve() in resolved_output.parents,
        "isolation output must remain under PoC19",
        ErrorCode.POLICY_INVALID,
    )
    _require(
        not output.exists(), "isolation output already exists", ErrorCode.OUTPUT_EXISTS
    )
    source, source_record, values, input_npz = _frozen(prepared)
    output = resolved_output
    output.mkdir(parents=True, exist_ok=False)
    controls = {}
    for control, spec in _VARIANTS.items():
        model = _derive_variant(source, control)
        _validate_variant(source, model, control)
        payload = model.SerializeToString()
        model_path = output / (control + ".onnx")
        _write(model_path, payload)
        record = import_graph(model_path)
        expected, observed, comparison = _references(payload, record, values)
        files = {
            control + "_lowering.json": canonical_json(record),
            control + "_input.npz": input_npz,
            control + "_input.bin": values.tobytes(),
            control + "_expected.npz": deterministic_npz_bytes({"output": expected}),
            control + "_independent_output.bin": expected.tobytes(),
            control + "_ort_output.bin": observed.tobytes(),
        }
        for name, data in files.items():
            _write(output / name, data)
        controls[control] = {
            "diagnostic_only": True,
            "full_model_equivalent": False,
            "source_model_sha256": SOURCE_MODEL_SHA256,
            "model": str(model_path),
            "model_sha256": record["source_model_sha256"],
            "graph_signature": record["graph_signature"],
            "lowering_sha256": record["lowering_sha256"],
            "lowering": str(output / (control + "_lowering.json")),
            "input": str(output / (control + "_input.npz")),
            "expected": str(output / (control + "_expected.npz")),
            "input_sha256": _sha(values.tobytes()),
            "independent_output_sha256": _sha(expected.tobytes()),
            "ort_output_sha256": _sha(observed.tobytes()),
            "input_shape": [1, 64, 200],
            "output_shape": [1, 128, 200],
            "depthwise": {
                "kernel": spec["kernel"],
                "dilation": spec["dilation"],
                "pads": list(spec["pads"]),
                "group": 64,
                "stride": 1,
                "source_tap_slice": list(spec["tap_slice"]),
                "weights_sha256": record["constants"][record["nodes"][0]["weights"]][
                    "sha256"
                ],
            },
            "downstream_model_fields_identical": True,
            "cpu_comparison": comparison,
        }
    receipt = {
        "schema_version": 1,
        "experiment": "poc19-kernel-dilation-isolation",
        "diagnostic_only": True,
        "full_model_equivalent": False,
        "source_preparation_sha256": PREPARATION_SHA256,
        "source_model_sha256": SOURCE_MODEL_SHA256,
        "source_lowering_sha256": source_record["lowering_sha256"],
        "source_input_sha256": SOURCE_INPUT_SHA256,
        "onnx_version": onnx.__version__,
        "ort_version": ort.__version__,
        "numpy_version": np.__version__,
        "controls": controls,
        "files": {
            p.name: {"sha256": _sha(p.read_bytes()), "bytes": p.stat().st_size}
            for p in sorted(output.iterdir())
            if p.is_file()
        },
    }
    _write(output / "preparation.json", canonical_json(receipt))
    return receipt


def _extract(model, start, stop, input_name, input_shape, output_shape):
    """Copy an exact trained subgraph, renaming only its boundary tensors."""
    from onnx import helper

    result = copy.deepcopy(model)
    nodes = list(result.graph.node)[start:stop]
    output_name = nodes[-1].output[0]
    for node in nodes:
        for i, name in enumerate(node.input):
            if name == input_name:
                node.input[i] = "input"
        for i, name in enumerate(node.output):
            if name == output_name:
                node.output[i] = "output"
    used = {name for n in nodes for name in n.input}
    constants = [copy.deepcopy(t) for t in model.graph.initializer if t.name in used]
    del result.graph.node[:]
    result.graph.node.extend(nodes)
    del result.graph.initializer[:]
    result.graph.initializer.extend(constants)
    del result.graph.value_info[:]
    dtype = model.graph.input[0].type.tensor_type.elem_type
    del result.graph.input[:]
    result.graph.input.extend(
        [helper.make_tensor_value_info("input", dtype, input_shape)]
    )
    del result.graph.output[:]
    result.graph.output.extend(
        [helper.make_tensor_value_info("output", dtype, output_shape)]
    )
    onnx.checker.check_model(result, full_check=True)
    return result


def _fp32(model):
    from onnx import TensorProto

    result = copy.deepcopy(model)
    for t in result.graph.initializer:
        if t.data_type == TensorProto.FLOAT16:
            t.CopyFrom(
                numpy_helper.from_array(
                    numpy_helper.to_array(t).astype(np.float32), t.name
                )
            )
    for value in [*result.graph.input, *result.graph.output, *result.graph.value_info]:
        if value.type.tensor_type.elem_type == TensorProto.FLOAT16:
            value.type.tensor_type.elem_type = TensorProto.FLOAT
    return result


def _cpu_session(model):
    session = ort.InferenceSession(
        model.SerializeToString(), providers=["CPUExecutionProvider"]
    )
    _require(
        session.get_providers() == ["CPUExecutionProvider"],
        "Pad candidate reference provider differs",
    )
    return session


def _fp32_comparison(reference, observed):
    _require(
        reference.shape == observed.shape
        and reference.dtype == observed.dtype == np.float32
        and np.isfinite(reference).all()
        and np.isfinite(observed).all(),
        "FP32 Pad comparison contract/nonfinite failure",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    error = np.abs(reference - observed)
    bad = error > 1e-4 + 1e-5 * np.abs(reference)
    result = {
        "status": "PASS" if not bad.any() else "FAIL",
        "elements": reference.size,
        "max_abs_error": float(error.max()),
        "mismatched_elements": int(bad.sum()),
        "policy": {"atol": 1e-4, "rtol": 1e-5},
        "reference_sha256": _sha(reference.tobytes()),
        "observed_sha256": _sha(observed.tobytes()),
    }
    _require(
        not bad.any(),
        "FP32 Pad arithmetic exceeds frozen policy",
        ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
    )
    return result


def _probability(logits):
    _require(
        logits.shape == (1, 2, 200) and np.isfinite(logits).all(),
        "invalid full Pad logits",
    )
    x = logits.astype(np.float32)
    exp = np.exp(x - x.max(axis=1, keepdims=True))
    return np.ascontiguousarray(
        (exp / exp.sum(axis=1, keepdims=True))[0, 1], np.float32
    )


def _probability_comparison(reference, observed):
    error = np.abs(reference - observed)
    disagree = int(np.sum((reference >= 0.5) != (observed >= 0.5)))
    agreement = 1 - disagree / 200
    result = {
        "status": "PASS" if error.max() <= 0.02 and agreement >= 0.99 else "FAIL",
        "elements": 200,
        "max_abs_error": float(error.max()),
        "decision_agreement": agreement,
        "disagreements": disagree,
        "policy": {"max_abs_error": 0.02, "minimum_decision_agreement": 0.99},
    }
    _require(
        result["status"] == "PASS",
        "Pad candidate probability gate failed",
        ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
    )
    return result


def _learned_accounting(source, candidate):
    original = {t.name: t for t in source.graph.initializer}
    learned = [t for t in candidate.graph.initializer if t.name in original]
    _require(
        all(
            t.SerializeToString() == original[t.name].SerializeToString()
            for t in learned
        ),
        "learned initializer bits changed",
    )
    return {
        "initializer_count": len(learned),
        "elements": sum(numpy_helper.to_array(t).size for t in learned),
        "bytes": sum(numpy_helper.to_array(t).nbytes for t in learned),
        "bits_identical": True,
        "initializers": {
            t.name: {
                "shape": list(t.dims),
                "sha256": _sha(numpy_helper.to_array(t).tobytes()),
            }
            for t in learned
        },
    }


def prepare_pad_candidate(originalprep: Path, output: Path) -> dict[str, Any]:
    """Freeze19c/d/e/f and exact full-model proof, without device/native calls."""
    from .marblenet_pad import (
        PAD_RECIPE,
        PROFILE_ID,
        add_profile_marker,
        rewrite_explicit_pad,
        validate_full_pad_model,
    )

    originalprep, output = Path(originalprep), Path(output)
    original, target = originalprep.resolve(), output.resolve()
    _require(
        not output.is_symlink() and not output.exists(),
        "Pad output exists/linked",
        ErrorCode.OUTPUT_EXISTS,
    )
    _require(
        target != original
        and original not in target.parents
        and target not in original.parents
        and OUTPUT_ROOT.resolve() in target.parents,
        "Pad output aliases original or leaves PoC19",
        ErrorCode.POLICY_INVALID,
    )
    source_c, _, input_c, input_c_npz = _frozen(originalprep)
    _require(
        ort.__version__ == "1.27.0"
        and onnx.__version__ == "1.22.0"
        and np.__version__ == "2.5.1",
        "Pad preparation requires pinned host versions",
        ErrorCode.MODEL_STATE_VERSION_MISMATCH,
    )
    source_full = onnx.load_model_from_string(_regular(originalprep / "18d.onnx"))
    folded32 = onnx.load_model_from_string(
        _regular(originalprep / "marblenet_folded_fp32.onnx")
    )
    source_d = _extract(source_full, 0, 3, "input", [1, 80, 400], [1, 128, 200])
    source_e = _extract(
        source_full, 11, 27, "activation_11", [1, 64, 200], [1, 64, 200]
    )
    models = {
        "19c": rewrite_explicit_pad(source_c, 0),
        "19d": add_profile_marker(source_d),
        "19e": add_profile_marker(source_e),
        "19f": rewrite_explicit_pad(source_full, 27),
    }
    full_identity = validate_full_pad_model(models["19f"].SerializeToString())
    output = target
    output.mkdir(parents=True, exist_ok=False)
    files = {}

    def retain(name, data):
        _write(output / name, data)
        files[name] = {"sha256": _sha(data), "bytes": len(data)}

    records = {}
    for name, model in models.items():
        retain(name + ".onnx", model.SerializeToString())
        records[name] = import_graph(output / (name + ".onnx"), profile_id=PROFILE_ID)
        retain(name + "_lowering.json", canonical_json(records[name]))
    speech32 = np.load(originalprep / "speech_features_fp32.npy", allow_pickle=False)
    capture = _extract(folded32, 0, 11, "input", [1, 80, 400], [1, 64, 200])
    capture32 = np.ascontiguousarray(
        _cpu_session(capture).run(None, {"input": speech32})[0]
    )
    input_e = capture32.astype(np.float16)
    retain("19e_capture_prefix_fp32.onnx", capture.SerializeToString())
    retain("19e_capture_activation_fp32.bin", capture32.tobytes())
    source_models = {
        "19c": source_c,
        "19d": source_d,
        "19e": source_e,
        "19f": source_full,
    }
    sessions = {name: _cpu_session(model) for name, model in models.items()}
    originals = {name: _cpu_session(model) for name, model in source_models.items()}
    input_d_npz = _regular(originalprep / "18d_speech_input.npz")
    with np.load(originalprep / "18d_speech_input.npz", allow_pickle=False) as z:
        input_d = np.array(z["input"], copy=True, order="C")
    inputs = {"19c": input_c, "19d": input_d, "19e": input_e, "19f": input_d}
    input_archives = {
        "19c": input_c_npz,
        "19d": input_d_npz,
        "19e": deterministic_npz_bytes({"input": input_e}),
        "19f": input_d_npz,
    }
    controls = {}
    for name, model in models.items():
        values = inputs[name]
        record = records[name]
        expected = independent_graph_oracle(record, {"input": values})["output"]
        observed = np.ascontiguousarray(sessions[name].run(None, {"input": values})[0])
        reference = np.ascontiguousarray(
            originals[name].run(None, {"input": values})[0]
        )
        _require(
            observed.dtype == np.float16
            and list(observed.shape) == record["onnx_output"]["shape"],
            "Pad observed output contract mismatch",
            ErrorCode.ONNX_OUTPUT_INVALID,
        )
        vs_independent = validate_graph_output(
            record, expected.tobytes(), observed.tobytes()
        )
        vs_original = validate_graph_output(
            record, reference.tobytes(), observed.tobytes()
        )
        retain(name + "_input.npz", input_archives[name])
        retain(name + "_input.bin", values.tobytes())
        retain(name + "_expected.npz", deterministic_npz_bytes({"output": expected}))
        retain(name + "_independent_output.bin", expected.tobytes())
        retain(name + "_ort_output.bin", observed.tobytes())
        controls[name] = {
            "profile_id": PROFILE_ID,
            "model": str(output / (name + ".onnx")),
            "model_sha256": record["source_model_sha256"],
            "lowering": str(output / (name + "_lowering.json")),
            "lowering_sha256": record["lowering_sha256"],
            "graph_signature": record["graph_signature"],
            "input": str(output / (name + "_input.npz")),
            "input_sha256": _sha(values.tobytes()),
            "expected": str(output / (name + "_expected.npz")),
            "independent_output_sha256": _sha(expected.tobytes()),
            "ort_output_sha256": _sha(observed.tobytes()),
            "input_shape": record["onnx_input"]["shape"],
            "output_shape": record["onnx_output"]["shape"],
            "nodes": len(record["nodes"]),
            "learned_initializers": _learned_accounting(source_models[name], model),
            "cpu_vs_independent": vs_independent,
            "cpu_vs_original_fp16": vs_original,
            "full_model": name == "19f",
        }
    layer32_original = _cpu_session(_fp32(source_c))
    layer32_padded = _cpu_session(_fp32(models["19c"]))
    layer32_parity = _fp32_comparison(
        layer32_original.run(None, {"input": input_c.astype(np.float32)})[0],
        layer32_padded.run(None, {"input": input_c.astype(np.float32)})[0],
    )
    padded32 = rewrite_explicit_pad(folded32, 27)
    retain("19f_folded_fp32.onnx", padded32.SerializeToString())
    original32_session, padded32_session = (
        _cpu_session(folded32),
        _cpu_session(padded32),
    )
    clips = {}
    for clip in ("silence", "noise", "speech", "noisy-speech"):
        features = np.load(
            originalprep / (clip + "_features_fp32.npy"), allow_pickle=False
        )
        values = features.astype(np.float16)
        expected = independent_graph_oracle(records["19f"], {"input": values})["output"]
        observed = np.ascontiguousarray(sessions["19f"].run(None, {"input": values})[0])
        old_half = np.ascontiguousarray(
            originals["19f"].run(None, {"input": values})[0]
        )
        padded_full32 = np.ascontiguousarray(
            padded32_session.run(None, {"input": features})[0]
        )
        folded_full32 = np.ascontiguousarray(
            original32_session.run(None, {"input": features})[0]
        )
        original_full32_bytes = _regular(
            originalprep / ("18d_" + clip + "_original_fp32.bin")
        )
        original_full32 = np.frombuffer(original_full32_bytes, dtype="<f4").reshape(
            1, 2, 200
        )
        prefix = "19f_" + clip
        retain(
            prefix + "_input.npz",
            _regular(originalprep / ("18d_" + clip + "_input.npz")),
        )
        retain(prefix + "_expected.npz", deterministic_npz_bytes({"output": expected}))
        retain(prefix + "_independent_output.bin", expected.tobytes())
        retain(prefix + "_ort_output.bin", observed.tobytes())
        retain(prefix + "_original_fp32.bin", original_full32_bytes)
        retain(prefix + "_padded_fp32.bin", padded_full32.tobytes())
        clips[clip] = {
            "input": str(output / (prefix + "_input.npz")),
            "expected": str(output / (prefix + "_expected.npz")),
            "features_fp32_sha256": _sha(features.tobytes()),
            "input_sha256": _sha(values.tobytes()),
            "cpu_fp16_vs_original_fp16": validate_graph_output(
                records["19f"], old_half.tobytes(), observed.tobytes()
            ),
            "cpu_fp16_vs_independent": validate_graph_output(
                records["19f"], expected.tobytes(), observed.tobytes()
            ),
            "padded_fp32_vs_folded_fp32": _fp32_comparison(
                folded_full32, padded_full32
            ),
            "padded_fp32_vs_original_unfused_fp32": _fp32_comparison(
                original_full32, padded_full32
            ),
            "cpu_fp16_probabilities": _probability_comparison(
                _probability(original_full32), _probability(observed)
            ),
            "independent_fp16_probabilities": _probability_comparison(
                _probability(original_full32), _probability(expected)
            ),
        }
    receipt = {
        "schema_version": 1,
        "experiment": "poc19-exact-marblenet-explicit-pad",
        "profile_id": PROFILE_ID,
        "qualification": "UNQUALIFIED",
        "source_preparation_sha256": PREPARATION_SHA256,
        "source_original_model_sha256": full_identity["source_original_model_sha256"],
        "checkpoint_sha256": full_identity["checkpoint_sha256"],
        "recipe": PAD_RECIPE,
        "recipe_sha256": full_identity["recipe_sha256"],
        "full_identity": full_identity,
        "versions": {
            "onnx": onnx.__version__,
            "onnxruntime": ort.__version__,
            "numpy": np.__version__,
        },
        "controls": controls,
        "clips": clips,
        "layer_fp32_parity": layer32_parity,
        "19e_input_origin": {
            "kind": "captured-original-folded-fp32-prefix-activation11-cast-fp16",
            "prefix_model_sha256": _sha(capture.SerializeToString()),
            "features_fp32_sha256": _sha(speech32.tobytes()),
            "activation_fp32_sha256": _sha(capture32.tobytes()),
            "activation_fp16_sha256": _sha(input_e.tobytes()),
        },
        "files": files,
    }
    _write(output / "preparation.json", canonical_json(receipt))
    return receipt


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--candidate", choices=["isolation", "pad"], default="isolation"
    )
    args = parser.parse_args(argv)
    receipt = (
        prepare_isolation if args.candidate == "isolation" else prepare_pad_candidate
    )(args.prepared, args.output)
    print(
        json.dumps(
            {
                "preparation": str(args.output / "preparation.json"),
                "controls": receipt["controls"],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
