"""Frozen real-weight SqueezeNet fixtures; no device or native-library calls."""

from __future__ import annotations

import hashlib
import importlib
import io
import json
from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, numpy_helper

from .fixtures import deterministic_npz_bytes

CHECKPOINT_NAME = "squeezenet1_1-b8a52dc0.pth"
CHECKPOINT_SHA256 = "b8a52dc049b60e4b6ab68ad0df457362afab8b6304b2febdc1650a5dab4d7e7b"
SOURCE_SHA256 = "6a98c53c42399ebff8f776c042c476e39128af368c6175504aa75cbdec807f2d"
PROFILE = "fp16-squeezenet-v1"
IMAGES = ("dog", "dog1", "dog2", "grace_hopper")
OPS = {"Conv", "Relu", "MaxPool", "Concat", "GlobalAveragePool", "Flatten"}


class SqueezeNetError(ValueError):
    """The frozen source, preprocessing or parity contract was violated."""


def _require(ok, message):
    if not ok:
        raise SqueezeNetError(message)


def _bytes(path):
    path = Path(path)
    _require(
        path.is_file()
        and not path.is_symlink()
        and path.stat().st_size <= 64 * 1024 * 1024,
        "nonregular or oversized fixture input",
    )
    return path.read_bytes()


def _sha(payload):
    return hashlib.sha256(payload).hexdigest()


def _torch():
    import torch
    import torchvision

    _require(
        torch.__version__.split("+")[0] == "2.5.1"
        and torchvision.__version__.split("+")[0] == "0.20.1",
        "frozen Torch/torchvision versions changed",
    )
    source = Path(importlib.import_module("torchvision.models.squeezenet").__file__)
    _require(_sha(_bytes(source)) == SOURCE_SHA256, "frozen torchvision source changed")
    torch.set_num_threads(1)
    return torch, torchvision


def fp16_model(payload: bytes, control: str) -> onnx.ModelProto:
    """Convert all declared FP32 tensors, preserving each resulting half bit."""
    _require(control in {"20a", "20b", "20c", "20d", "20e"}, "unknown control")
    model = onnx.load_model_from_string(payload)
    _require(
        len(model.opset_import) == 1
        and model.opset_import[0].domain == ""
        and model.opset_import[0].version == 13,
        "frozen standard opset changed",
    )
    _require(
        all(n.domain == "" and n.op_type in OPS for n in model.graph.node),
        "unknown or training-only evaluation operator",
    )
    for tensor in model.graph.initializer:
        _require(
            tensor.data_type == TensorProto.FLOAT and not tensor.external_data,
            "non-FP32 or external coefficient",
        )
        array = numpy_helper.to_array(tensor)
        _require(np.all(np.isfinite(array)), "nonfinite source coefficient")
        converted = array.astype("<f2")
        _require(np.all(np.isfinite(converted)), "FP16 coefficient overflow")
        tensor.CopyFrom(numpy_helper.from_array(converted, tensor.name))
    for value in (*model.graph.input, *model.graph.output, *model.graph.value_info):
        _require(
            value.type.HasField("tensor_type")
            and value.type.tensor_type.elem_type == TensorProto.FLOAT,
            "unsupported declared tensor type",
        )
        value.type.tensor_type.elem_type = TensorProto.FLOAT16
    model.ir_version = 8
    del model.metadata_props[:]
    for key, value in {
        "wave.native_profile": PROFILE,
        "wave.checkpoint_sha256": CHECKPOINT_SHA256,
        "wave.control": control,
    }.items():
        entry = model.metadata_props.add()
        entry.key, entry.value = key, value
    model = onnx.shape_inference.infer_shapes(model, strict_mode=True, data_prop=True)
    onnx.checker.check_model(model, full_check=True)
    return model


def _probabilities(values):
    values = values.astype(np.float64)
    exp = np.exp(values - values.max())
    return exp / exp.sum()


def compare_classification(original: bytes, observed: bytes) -> dict:
    """Native/derived FP16 output against original FP32, fixed before runs."""
    _require(
        len(original) == 4000 and len(observed) == 2000,
        "classification byte count differs",
    )
    reference = np.frombuffer(original, dtype="<f4").astype(np.float64)
    actual = np.frombuffer(observed, dtype="<f2").astype(np.float64)
    _require(
        np.isfinite(reference).all() and np.isfinite(actual).all(),
        "nonfinite classification output",
    )
    errors = np.abs(reference - actual)
    logit_pass = bool(np.all(errors <= 0.05 + 0.02 * np.abs(reference)))
    ref_order, actual_order = (
        np.argsort(-reference, kind="stable"),
        np.argsort(-actual, kind="stable"),
    )
    overlap = len(set(ref_order[:5]) & set(actual_order[:5]))
    probability_error = float(
        np.max(np.abs(_probabilities(reference) - _probabilities(actual)))
    )
    result = {
        "status": "PASS"
        if logit_pass
        and probability_error <= 0.01
        and ref_order[0] == actual_order[0]
        and overlap >= 4
        else "FAIL",
        "max_abs_logit_error": float(errors.max()),
        "max_abs_probability_error": probability_error,
        "reference_top1": int(ref_order[0]),
        "observed_top1": int(actual_order[0]),
        "reference_top5": ref_order[:5].tolist(),
        "observed_top5": actual_order[:5].tolist(),
        "top5_overlap": overlap,
        "reference_margin": float(reference[ref_order[0]] - reference[ref_order[1]]),
        "reference_sha256": _sha(original),
        "observed_sha256": _sha(observed),
        "policy": {
            "logit_atol": 0.05,
            "logit_rtol": 0.02,
            "maximum_probability_error": 0.01,
            "top1_identical": True,
            "minimum_top5_overlap": 4,
        },
    }
    _require(
        result["status"] == "PASS",
        "classification parity failed: " + json.dumps(result),
    )
    return result


def prepare_squeezenet(assets: Path, output: Path) -> dict:
    """Prepare four images and five unchanged-weight graph candidates once."""
    assets, output = Path(assets), Path(output)
    _require(
        not output.exists() and not output.is_symlink(), "fixture output already exists"
    )
    _require(not assets.is_symlink(), "linked fixture input directory")
    checkpoint = _bytes(assets / CHECKPOINT_NAME)
    _require(
        _sha(checkpoint) == CHECKPOINT_SHA256, "checkpoint differs from pinned model"
    )
    _require(
        not output.resolve().is_relative_to(assets.resolve()),
        "fixture output aliases source",
    )
    torch, torchvision = _torch()
    import onnxruntime as ort
    from PIL import Image

    state = torch.load(io.BytesIO(checkpoint), map_location="cpu", weights_only=True)
    network = torchvision.models.squeezenet1_1(weights=None).eval()
    network.load_state_dict(state, strict=True)
    _require(
        sum(p.numel() for p in network.parameters()) == 1235496 and len(state) == 52,
        "trained parameter accounting differs",
    )
    transforms = torchvision.models.SqueezeNet1_1_Weights.IMAGENET1K_V1.transforms()
    originals, inputs, arrays = {}, {}, {}
    for image in IMAGES:
        payload = _bytes(assets / (image + ".jpg"))
        with Image.open(io.BytesIO(payload)) as source:
            rgb = source.convert("RGB")
            arrays[image] = transforms(rgb).unsqueeze(0).contiguous().numpy()
            originals[image] = {
                "sha256": _sha(payload),
                "bytes": len(payload),
                "original_size": list(source.size),
            }
        _require(
            arrays[image].shape == (1, 3, 224, 224)
            and np.isfinite(arrays[image]).all(),
            "image preprocessing contract changed",
        )
        inputs[image] = arrays[image].astype("<f2")
    output.mkdir(parents=True)
    files = {}

    def save(name, payload):
        with (output / name).open("xb") as handle:
            handle.write(payload)
        files[name] = {"bytes": len(payload), "sha256": _sha(payload)}

    def export(module, example):
        buffer = io.BytesIO()
        with torch.inference_mode():
            torch.onnx.export(
                module,
                torch.from_numpy(example.astype("<f4")),
                buffer,
                opset_version=13,
                input_names=["input"],
                output_names=["output"],
                do_constant_folding=True,
                keep_initializers_as_inputs=False,
                training=torch.onnx.TrainingMode.EVAL,
            )
        model = onnx.load_model_from_string(buffer.getvalue())
        model.ir_version = 8
        return onnx.shape_inference.infer_shapes(
            model, strict_mode=True, data_prop=True
        )

    full32 = export(network, arrays["dog"])
    _require(
        len(full32.graph.initializer) == 52
        and sum(numpy_helper.to_array(t).size for t in full32.graph.initializer)
        == 1235496,
        "export omitted or duplicated parameters",
    )
    for tensor in full32.graph.initializer:
        _require(
            tensor.name in state
            and numpy_helper.to_array(tensor).tobytes()
            == state[tensor.name].numpy().tobytes(),
            "export changed learned parameter bits",
        )
    save("squeezenet_fp32.onnx", full32.SerializeToString())
    full16 = fp16_model(full32.SerializeToString(), "20e")
    save("20e.onnx", full16.SerializeToString())
    options = ort.SessionOptions()
    options.intra_op_num_threads = options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    session32 = ort.InferenceSession(
        full32.SerializeToString(), options, providers=["CPUExecutionProvider"]
    )
    session16 = ort.InferenceSession(
        full16.SerializeToString(), options, providers=["CPUExecutionProvider"]
    )
    classifications = {}
    for image in IMAGES:
        with torch.inference_mode():
            original = network(torch.from_numpy(arrays[image])).numpy().astype("<f4")
        actual32 = session32.run(["output"], {"input": arrays[image]})[0]
        _require(
            np.allclose(original, actual32, atol=0.0001, rtol=0.00001),
            "original Torch/ORT FP32 mismatch",
        )
        half = session16.run(["output"], {"input": inputs[image]})[0].astype("<f2")
        classifications[image] = compare_classification(
            original.tobytes(), half.tobytes()
        )
        save(
            f"20e_{image}_input.npz", deterministic_npz_bytes({"input": inputs[image]})
        )
        buffer = io.BytesIO()
        np.save(buffer, arrays[image], allow_pickle=False)
        save(f"{image}_input_fp32.npy", buffer.getvalue())
        save(f"20e_{image}_original_fp32.bin", original.tobytes())
        save(f"20e_{image}_ort_fp16.bin", half.tobytes())
    save("20e_input.npz", deterministic_npz_bytes({"input": inputs["dog"]}))

    shapes = {
        v.name: v
        for v in (*full16.graph.input, *full16.graph.value_info, *full16.graph.output)
    }
    first_relu = next(n.output[0] for n in full16.graph.node if n.op_type == "Relu")
    first_pool = next(n.output[0] for n in full16.graph.node if n.op_type == "MaxPool")
    classifier_conv = next(
        n
        for n in full16.graph.node
        if n.op_type == "Conv" and "classifier.1.weight" in n.input
    )
    augmented = onnx.ModelProto()
    augmented.CopyFrom(full16)
    for name in (first_relu, first_pool, classifier_conv.input[0]):
        augmented.graph.output.add().CopyFrom(shapes[name])
    activation_session = ort.InferenceSession(
        augmented.SerializeToString(), options, providers=["CPUExecutionProvider"]
    )
    conv_activation, pool_activation, final_activation = activation_session.run(
        [first_relu, first_pool, classifier_conv.input[0]], {"input": inputs["dog"]}
    )

    class Classifier(torch.nn.Module):
        def __init__(self, classifier):
            super().__init__()
            self.classifier = classifier

        def forward(self, value):
            return torch.flatten(self.classifier(value), 1)

    controls = {
        "20a": (
            torch.nn.Sequential(network.features[0], network.features[1]).eval(),
            inputs["dog"][:, :, :17, :13].copy(),
        ),
        "20b": (
            torch.nn.Sequential(network.features[2]).eval(),
            conv_activation[:, :, :15, :19].copy(),
        ),
        "20c": (network.features[3].eval(), pool_activation[:, :, :7, :9].copy()),
        "20d": (Classifier(network.classifier).eval(), final_activation.copy()),
    }
    inventory = {}
    for control, (module, example) in controls.items():
        graph = fp16_model(export(module, example).SerializeToString(), control)
        session = ort.InferenceSession(
            graph.SerializeToString(), options, providers=["CPUExecutionProvider"]
        )
        expected = session.run(["output"], {"input": example})[0].astype("<f2")
        save(control + ".onnx", graph.SerializeToString())
        save(control + "_input.npz", deterministic_npz_bytes({"input": example}))
        save(control + "_ort_fp16.bin", expected.tobytes())
        inventory[control] = {
            "input_shape": list(example.shape),
            "output_shape": list(expected.shape),
            "operators": {
                op: sum(n.op_type == op for n in graph.graph.node)
                for op in sorted({n.op_type for n in graph.graph.node})
            },
        }
    inventory["20e"] = {
        "input_shape": [1, 3, 224, 224],
        "output_shape": [1, 1000],
        "operators": {
            op: sum(n.op_type == op for n in full16.graph.node)
            for op in sorted({n.op_type for n in full16.graph.node})
        },
    }
    classes = torchvision.models.SqueezeNet1_1_Weights.IMAGENET1K_V1.meta["categories"]
    save(
        "classes.json",
        (json.dumps(classes, ensure_ascii=False, indent=2) + "\n").encode(),
    )
    receipt = {
        "schema_version": 1,
        "checkpoint_sha256": CHECKPOINT_SHA256,
        "torchvision_source_sha256": SOURCE_SHA256,
        "parameters": 1235496,
        "initializer_tensors": 52,
        "fp16_parameter_bytes": 2470992,
        "trained_parameters_preserved": True,
        "images": originals,
        "preprocessing": {
            "resize": 256,
            "crop": 224,
            "interpolation": "bilinear",
            "antialias": True,
            "mean": [0.485, 0.456, 0.406],
            "std": [0.229, 0.224, 0.225],
            "RGB": True,
        },
        "versions": {
            "torch": torch.__version__,
            "torchvision": torchvision.__version__,
            "onnx": onnx.__version__,
            "ort": ort.__version__,
            "numpy": np.__version__,
        },
        "inventory": inventory,
        "classifications": classifications,
        "fp16_reference_authority": (
            "independent NumPy; actual ORT agreement must pass native "
            "element policy before launch"
        ),
        "files": files,
    }
    with (output / "preparation.json").open("x") as handle:
        json.dump(receipt, handle, sort_keys=True, indent=2, allow_nan=False)
        handle.write("\n")
    return receipt
