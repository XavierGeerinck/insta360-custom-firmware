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


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    receipt = prepare_isolation(args.prepared, args.output)
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
