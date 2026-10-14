"""Metadata-only SqueezeNet derivation with a bounded approximate native mean.

The original independent mean remains authoritative. Fixed native half
coefficients approximate1/169; no learned coefficient or ONNX operator changes.
"""

from __future__ import annotations

import copy
import hashlib
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any

import numpy as np
import onnx
from onnx import TensorProto, helper

from . import vision_graph as original
from .audit import audit_model_bytes, canonical_json
from .errors import ErrorCode, HybridError

PROFILE_ID = "fp16-squeezenet-mean-v1"
MEAN_HALF_BITS = 0x1E0F
ORIGINAL_MODEL_SHA256 = {
    "20d": "e5a3ffa3ecdde8b68bae5dafd7f5672bf383fd2bd1e65fe6ff7acebc934cf498",
    "20e": "f5ac6b08b8304372c06fb897c2cad63e0c30191e9bc866680b1777e5bb62e081",
}
ORIGINAL_LOWERING_SHA256 = {
    "20d": "f1f50a50acf8a759131dbfbc5dad02277617448dd0cac3ce48e2e9efb66bff1a",
    "20e": "9c79b484eacbcaa32af3a24a525360be4cc0e80843e3a0b57f831590d2745b94",
}
DERIVED_MODEL_SHA256 = {
    "20d": "d872fc6d1e1b12412986e5eb6e7337584eb0ac1fb622d3ec84d8e0bb0b07ff9b",
    "20e": "b18443c69e389b10152bf294ee0decfbe9d3f000da0abc90ff521a4cf679ab99",
}
_BASE_KEYS = {
    "schema_version",
    "profile_id",
    "source_model_sha256",
    "graph_signature",
    "control_id",
    "checkpoint_sha256",
    "onnx_input",
    "onnx_output",
    "native_input",
    "native_output",
    "constants",
    "nodes",
    "operation",
    "numerical_policy",
    "native_policy",
    "lowering_sha256",
}
_require = original._require


def native_policy(channels: int) -> dict[str, Any]:
    return {
        "id": "fixed-depthwise-mean13-v1",
        "half_bits": MEAN_HALF_BITS,
        "weight_shape": [13, 13, channels, 1],
        "group": channels,
        "multiplier": 1,
        "strides": [1, 1],
        "dilations": [1, 1],
        "pads": [0, 0, 0, 0],
        "bias_dtype": "FLOAT32",
        "bias_half_promotion": False,
        "bias_bits": 0,
    }


def _seal(record: dict[str, Any]) -> dict[str, Any]:
    record.pop("lowering_sha256", None)
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    return record


def _metadata(model: onnx.ModelProto) -> dict[str, str]:
    props = {p.key: p.value for p in model.metadata_props}
    _require(
        len(props) == len(model.metadata_props)
        and set(props)
        == {"wave.native_profile", "wave.checkpoint_sha256", "wave.control"}
        and props["wave.checkpoint_sha256"] == original.CHECKPOINT_SHA256,
        "invalid mean model metadata",
        ErrorCode.EVIDENCE_INVALID,
    )
    return props


def _payload(payload: Any) -> None:
    _require(
        isinstance(payload, bytes) and 0 < len(payload) <= 64 * 1024 * 1024,
        "invalid bounded mean model bytes",
        ErrorCode.ONNX_PARSE_FAILURE,
    )


def derive_mean_model(payload: bytes) -> bytes:
    """Change only the profile marker of an exact pinned20d or20e model."""
    _payload(payload)
    digest = hashlib.sha256(payload).hexdigest()
    controls = [
        control for control, sha in ORIGINAL_MODEL_SHA256.items() if digest == sha
    ]
    _require(
        len(controls) == 1,
        "mean derivation requires exact pinned parent",
        ErrorCode.EVIDENCE_INVALID,
    )
    model = onnx.load_model_from_string(payload)
    props = _metadata(model)
    control = controls[0]
    _require(
        props["wave.control"] == control
        and props["wave.native_profile"] == original.PROFILE_ID,
        "invalid parent control/profile marker",
        ErrorCode.EVIDENCE_INVALID,
    )
    marker = next(p for p in model.metadata_props if p.key == "wave.native_profile")
    marker.value = PROFILE_ID
    derived = model.SerializeToString()
    _require(
        hashlib.sha256(derived).hexdigest() == DERIVED_MODEL_SHA256[control],
        "unexpected metadata-only derived identity",
        ErrorCode.EVIDENCE_INVALID,
    )
    marker.value = original.PROFILE_ID
    _require(
        model.SerializeToString() == payload,
        "parent bytes did not reconstruct exactly",
        ErrorCode.EVIDENCE_INVALID,
    )
    return derived


def make_mean_control() -> bytes:
    """Return the fixed one-channel diagnostic definition; input stays external."""
    graph = helper.make_graph(
        [helper.make_node("GlobalAveragePool", ["input"], ["output"])],
        "squeezenet_mean13_diagnostic",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT16, [1, 1, 13, 13])],
        [helper.make_tensor_value_info("output", TensorProto.FLOAT16, [1, 1, 1, 1])],
    )
    model = helper.make_model(
        graph,
        opset_imports=[helper.make_opsetid("", 13)],
        producer_name="wave-fixed-mean-control-v1",
    )
    model.ir_version = 8
    for key, value in (
        ("wave.native_profile", PROFILE_ID),
        ("wave.checkpoint_sha256", original.CHECKPOINT_SHA256),
        ("wave.control", "21a"),
    ):
        prop = model.metadata_props.add()
        prop.key, prop.value = key, value
    return model.SerializeToString()


def _parent_payload(payload: bytes) -> tuple[str, bytes]:
    model = onnx.load_model_from_string(payload)
    props = _metadata(model)
    control = props["wave.control"]
    _require(
        props["wave.native_profile"] == PROFILE_ID and control in ORIGINAL_MODEL_SHA256,
        "unsupported derived mean control/profile",
        ErrorCode.EVIDENCE_INVALID,
    )
    _require(
        hashlib.sha256(payload).hexdigest() == DERIVED_MODEL_SHA256[control],
        "derived mean bytes changed",
        ErrorCode.EVIDENCE_INVALID,
    )
    next(
        p for p in model.metadata_props if p.key == "wave.native_profile"
    ).value = original.PROFILE_ID
    parent = model.SerializeToString()
    _require(
        hashlib.sha256(parent).hexdigest() == ORIGINAL_MODEL_SHA256[control],
        "original parent reconstruction failed",
        ErrorCode.EVIDENCE_INVALID,
    )
    return control, parent


def _diagnostic_record(payload: bytes) -> dict[str, Any]:
    _require(
        payload == make_mean_control(),
        "diagnostic definition changed",
        ErrorCode.EVIDENCE_INVALID,
    )
    audit = audit_model_bytes(payload, source_name="21a.onnx", policy=original._POLICY)
    incoming, outgoing = [1, 1, 13, 13], [1, 1, 1, 1]
    return _seal(
        {
            "schema_version": 1,
            "profile_id": PROFILE_ID,
            "source_model_sha256": audit.model_sha256,
            "graph_signature": audit.graph_signature,
            "source_original_model_sha256": None,
            "control_id": "21a",
            "checkpoint_sha256": original.CHECKPOINT_SHA256,
            "onnx_input": original._contract("input", incoming),
            "onnx_output": original._contract("output", outgoing),
            "native_input": original._contract(
                "input", original.native_shape(incoming)
            ),
            "native_output": original._contract(
                "output", original.native_shape(outgoing)
            ),
            "constants": {},
            "nodes": [
                {
                    "type": "GlobalAveragePool",
                    "inputs": ["input"],
                    "output": "output",
                    "input_shape": incoming,
                    "output_shape": outgoing,
                }
            ],
            "operation": {"type": "VisionGraph", "node_count": 1},
            "numerical_policy": dict(original.NUMERICAL_POLICY),
            "native_policy": native_policy(1),
        }
    )


def import_graph(path: Path) -> dict[str, Any]:
    """Import exact metadata-only derivations or the fixed scalar diagnostic."""
    try:
        with Path(path).open("rb") as handle:
            payload = handle.read(64 * 1024 * 1024 + 1)
    except OSError as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE, f"cannot read mean model: {exc}"
        ) from exc
    _payload(payload)
    audit_model_bytes(payload, source_name=Path(path).name, policy=original._POLICY)
    props = _metadata(onnx.load_model_from_string(payload))
    if props["wave.control"] == "21a":
        record = _diagnostic_record(payload)
    else:
        control, parent = _parent_payload(payload)
        with TemporaryDirectory(prefix="wave-mean-parent-") as directory:
            parent_path = Path(directory) / "original.onnx"
            parent_path.write_bytes(parent)
            record = original.import_graph(parent_path)
        _require(
            record["lowering_sha256"] == ORIGINAL_LOWERING_SHA256[control],
            "original parent lowering changed",
            ErrorCode.EVIDENCE_INVALID,
        )
        record.update(
            profile_id=PROFILE_ID,
            source_model_sha256=hashlib.sha256(payload).hexdigest(),
            source_original_model_sha256=ORIGINAL_MODEL_SHA256[control],
            native_policy=native_policy(1000),
        )
        _seal(record)
    validate_graph_record(record)
    return record


def _original_record(record: dict[str, Any]) -> dict[str, Any]:
    result = copy.deepcopy(record)
    result.pop("source_original_model_sha256")
    control = result["control_id"]
    result.update(
        profile_id=original.PROFILE_ID,
        source_model_sha256=ORIGINAL_MODEL_SHA256[control],
        native_policy=dict(original.NATIVE_POLICY),
        lowering_sha256=ORIGINAL_LOWERING_SHA256[control],
    )
    return result


def validate_graph_record(record: dict[str, Any]) -> None:
    """Bind policy and all learned bits to the frozen original record identity."""
    original._keys(
        record,
        _BASE_KEYS | {"source_original_model_sha256"},
        "invalid mean record keys",
    )
    _require(
        record["profile_id"] == PROFILE_ID
        and type(record["schema_version"]) is int
        and record["schema_version"] == 1
        and record["control_id"] in ("21a", "20d", "20e"),
        "invalid mean record profile",
    )
    unsigned = {k: v for k, v in record.items() if k != "lowering_sha256"}
    _require(
        record["lowering_sha256"]
        == hashlib.sha256(canonical_json(unsigned)).hexdigest(),
        "mean lowering identity mismatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    diagnostic = record["control_id"] == "21a"
    policy = native_policy(1 if diagnostic else 1000)
    _require(
        record["native_policy"] == policy
        and canonical_json(record["native_policy"]) == canonical_json(policy),
        "mean native policy changed",
        ErrorCode.EVIDENCE_INVALID,
    )
    if diagnostic:
        _require(
            record == _diagnostic_record(make_mean_control())
            and canonical_json(record)
            == canonical_json(_diagnostic_record(make_mean_control())),
            "diagnostic record changed",
            ErrorCode.EVIDENCE_INVALID,
        )
    else:
        control = record["control_id"]
        _require(
            record["source_original_model_sha256"] == ORIGINAL_MODEL_SHA256[control]
            and record["source_model_sha256"] == DERIVED_MODEL_SHA256[control],
            "mean parent/source identity changed",
            ErrorCode.EVIDENCE_INVALID,
        )
        original.validate_graph_record(_original_record(record))


def independent_graph_oracle(
    record: dict[str, Any], inputs: dict[str, np.ndarray]
) -> dict[str, np.ndarray]:
    """Evaluate the original mean, never the approximate native coefficients."""
    validate_graph_record(record)
    if record["control_id"] != "21a":
        return original.independent_graph_oracle(_original_record(record), inputs)
    _require(
        isinstance(inputs, dict) and set(inputs) == {"input"},
        "mean input names mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    x = inputs["input"]
    _require(
        isinstance(x, np.ndarray)
        and x.dtype == np.dtype("<f2")
        and list(x.shape) == [1, 1, 13, 13]
        and np.isfinite(x).all(),
        "mean input shape/type/finite mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    result = np.mean(
        x.astype(np.float32), axis=(2, 3), keepdims=True, dtype=np.float32
    ).astype("<f2")
    _require(
        np.isfinite(result).all(),
        "nonfinite mean oracle",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    return {"output": np.ascontiguousarray(result)}


def compare_output(
    record: dict[str, Any], reference: bytes, actual: bytes
) -> dict[str, Any]:
    """Compare actual readback to original mean under the unchanged policy."""
    validate_graph_record(record)
    size = record["onnx_output"]["byte_size"]
    _require(
        isinstance(reference, bytes)
        and isinstance(actual, bytes)
        and len(reference) == len(actual) == size,
        "mean output size mismatch",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    expected, observed = (
        np.frombuffer(b, dtype="<f2").astype(np.float32) for b in (reference, actual)
    )
    _require(
        np.isfinite(expected).all() and np.isfinite(observed).all(),
        "nonfinite mean output",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    _require(
        actual != b"\xa5" * size,
        "untouched mean output canary",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    errors = np.abs(observed - expected)
    bad = errors > 0.03 + 0.015 * np.abs(expected)
    metrics = {
        "status": "FAIL" if bad.any() else "PASS",
        "passed": not bool(bad.any()),
        "reference_sha256": hashlib.sha256(reference).hexdigest(),
        "observed_sha256": hashlib.sha256(actual).hexdigest(),
        "max_abs_error": float(errors.max()),
        "elements": len(expected),
        "mismatched_elements": int(bad.sum()),
        "policy": dict(original.NUMERICAL_POLICY),
    }
    if bad.any():
        raise HybridError(
            ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
            "mean output exceeds frozen tolerance",
            details=metrics,
        )
    return metrics
