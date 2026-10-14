"""Exact static temporal Pad profile; admission never grants qualification.

The old profile is used only after reversing the one bounded Pad operation.
Its validator remains the authority for every unchanged Conv/Add/Relu edge.
"""

from __future__ import annotations

import copy
import hashlib
import math
import tempfile
from pathlib import Path
from typing import Any

import numpy as np
import onnx
from onnx import AttributeProto, TensorProto, helper, numpy_helper

from . import marblenet_graph as old
from .audit import audit_model_bytes, canonical_json
from .errors import ErrorCode, HybridError

PROFILE_ID = "fp16-marblenet-pad-v1"
MARKER = "wave.native_profile"
NATIVE_POLICY = {"id": "flat-height-depthwise-symmetric-v1"}
ORIGINAL_FULL_SHA256 = (
    "f5d1cdfe2c1f3ae8b3feb3ffa7568403839a7c59e8ff363846e3fde46cfad46a"
)
CHECKPOINT_SHA256 = "84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706"
PAD_RECIPE = {
    "id": "explicit-pad28-k29-d2-valid-v1",
    "profile_id": PROFILE_ID,
    "source_original_model_sha256": ORIGINAL_FULL_SHA256,
    "checkpoint_sha256": CHECKPOINT_SHA256,
    "full_depthwise_node_index": 27,
    "pads": [0, 0, 28, 0, 0, 28],
    "pads_constant": "marblenet_pad_pads",
    "value_constant": "marblenet_pad_zero",
    "value_dtype": "FLOAT16",
    "value_payload_hex": "0000",
    "native_policy": NATIVE_POLICY,
}
_PAD_BYTES = np.array([0, 0, 28, 0, 0, 28], dtype="<i8").tobytes()
_DW_VALID = {
    "dilations": [2],
    "group": 64,
    "kernel_shape": [29],
    "pads": [0, 0],
    "strides": [1],
}


def _require(ok, message, code=ErrorCode.ONNX_GRAPH_INVALID):
    old._require(ok, message, code)


def _seal(record):
    record["lowering_sha256"] = hashlib.sha256(
        canonical_json({k: v for k, v in record.items() if k != "lowering_sha256"})
    ).hexdigest()
    return record


def add_profile_marker(model: onnx.ModelProto) -> onnx.ModelProto:
    _require(
        not any(p.key == MARKER for p in model.metadata_props),
        "profile marker already present",
    )
    result = copy.deepcopy(model)
    result.metadata_props.add(key=MARKER, value=PROFILE_ID)
    return result


def rewrite_explicit_pad(model: onnx.ModelProto, index: int) -> onnx.ModelProto:
    """Preserve every source initializer bit and move only temporal padding."""
    result = add_profile_marker(model)
    _require(
        type(index) is int and 0 <= index < len(result.graph.node),
        "invalid rewrite index",
    )
    node = result.graph.node[index]
    _require(
        node.op_type == "Conv"
        and len(node.input) == 2
        and old._attrs(node) == {**_DW_VALID, "pads": [28, 28]},
        "rewrite requires trained29/rate2 implicit-pad Conv",
    )
    constants = {t.name: t for t in result.graph.initializer}
    _require(node.input[1] in constants, "rewrite weight missing")
    weights = numpy_helper.to_array(constants[node.input[1]])
    _require(
        weights.shape == (64, 1, 29)
        and weights.dtype in (np.dtype("<f2"), np.dtype("<f4")),
        "rewrite depthwise weights invalid",
    )
    pads, zero = PAD_RECIPE["pads_constant"], PAD_RECIPE["value_constant"]
    padded = node.input[0] + "_explicit_pad28"
    _require(
        not (
            {pads, zero, padded}
            & (set(constants) | {o for n in result.graph.node for o in n.output})
        ),
        "rewrite identifiers already present",
    )
    result.graph.initializer.extend(
        [
            numpy_helper.from_array(
                np.frombuffer(_PAD_BYTES, dtype="<i8").copy(), pads
            ),
            numpy_helper.from_array(np.array(0.0, dtype=weights.dtype), zero),
        ]
    )
    pad = helper.make_node(
        "Pad",
        [node.input[0], pads, zero],
        [padded],
        name="marblenet_pad28",
        mode="constant",
    )
    node.input[0] = padded
    next(a for a in node.attribute if a.name == "pads").ints[:] = [0, 0]
    result.graph.node.insert(index, pad)
    onnx.checker.check_model(result, full_check=True)
    return result


def _marker(model):
    found = [p.value for p in model.metadata_props if p.key == MARKER]
    _require(found == [PROFILE_ID], "exact explicit Pad profile marker required")


def _structural(tensor, dtype, shape, wanted):
    _require(
        tensor.data_type == dtype and list(tensor.dims) == shape,
        "Pad structural constant dtype/shape invalid",
    )
    _require(
        not tensor.HasField("segment")
        and tensor.data_location != TensorProto.EXTERNAL
        and not tensor.external_data,
        "Pad constants must be inline",
        ErrorCode.ONNX_EXTERNAL_DATA,
    )
    fields = [
        k
        for k in (
            "raw_data",
            "float_data",
            "int32_data",
            "string_data",
            "int64_data",
            "double_data",
            "uint64_data",
        )
        if getattr(tensor, k)
    ]
    _require(len(fields) == 1, "Pad constant requires exactly one payload")
    try:
        value = numpy_helper.to_array(tensor)
    except Exception as exc:
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID, "cannot decode Pad constant"
        ) from exc
    raw = value.tobytes()
    _require(raw == wanted, "Pad constant value or positive-zero bits invalid")
    return {
        "name": tensor.name,
        "dtype": "INT64" if dtype == TensorProto.INT64 else "FLOAT16",
        "shape": shape,
        "byte_size": len(raw),
        "payload_hex": raw.hex(),
        "sha256": hashlib.sha256(raw).hexdigest(),
    }


def _normalize(model):
    """Reverse Pad for structural validation; return its bound canonical fields."""
    _marker(model)
    normalized = copy.deepcopy(model)
    properties = [
        copy.deepcopy(p) for p in normalized.metadata_props if p.key != MARKER
    ]
    del normalized.metadata_props[:]
    normalized.metadata_props.extend(properties)
    positions = [i for i, n in enumerate(model.graph.node) if n.op_type == "Pad"]
    _require(len(positions) <= 1, "profile permits at most one exact Pad")
    if not positions:
        return normalized, None, {}
    index = positions[0]
    pad = model.graph.node[index]
    _require(
        pad.domain in {"", "ai.onnx"}
        and len(pad.input) in {2, 3}
        and len(pad.output) == 1,
        "Pad arity/domain invalid",
    )
    _require(
        not pad.attribute
        or (
            len(pad.attribute) == 1
            and pad.attribute[0].name == "mode"
            and pad.attribute[0].type == AttributeProto.STRING
            and pad.attribute[0].s == b"constant"
            and not pad.attribute[0].ref_attr_name
        ),
        "only constant Pad is supported",
    )
    constants = {t.name: t for t in model.graph.initializer}
    _require(
        len(constants) == len(model.graph.initializer) and pad.input[1] in constants,
        "Pad constants missing/duplicated",
    )
    structural = {
        pad.input[1]: _structural(
            constants[pad.input[1]], TensorProto.INT64, [6], _PAD_BYTES
        )
    }
    zero = pad.input[2] if len(pad.input) == 3 else None
    if zero is not None:
        _require(zero in constants and zero != pad.input[1], "Pad zero missing")
        structural[zero] = _structural(
            constants[zero], TensorProto.FLOAT16, [], b"\x00\x00"
        )
    _require(
        index + 1 < len(model.graph.node),
        "Pad requires immediate trained depthwise consumer",
    )
    conv = model.graph.node[index + 1]
    _require(
        conv.op_type == "Conv"
        and len(conv.input) == 2
        and conv.input[0] == pad.output[0]
        and old._attrs(conv) == _DW_VALID,
        "Pad must feed exact trained29/rate2 VALID depthwise Conv",
    )
    _require(
        sum(pad.output[0] in n.input for n in model.graph.node) == 1,
        "padded tensor must have one consumer",
    )
    for value in model.graph.value_info:
        if value.name == pad.output[0]:
            tensor = value.type.tensor_type
            _require(
                tensor.elem_type == TensorProto.FLOAT16
                and all(d.WhichOneof("value") == "dim_value" for d in tensor.shape.dim)
                and [d.dim_value for d in tensor.shape.dim] == [1, 64, 256],
                "declared Pad intermediate shape/type differs",
            )
    lowered = {
        "type": "Pad",
        "inputs": [pad.input[0]],
        "output": pad.output[0],
        "input_shape": [1, 64, 200],
        "output_shape": [1, 64, 256],
        "pads": [28, 28],
        "value": 0.0,
        "pads_constant": pad.input[1],
        "value_constant": zero,
    }
    target = normalized.graph.node[index + 1]
    target.input[0] = pad.input[0]
    next(a for a in target.attribute if a.name == "pads").ints[:] = [28, 28]
    del normalized.graph.node[index]
    kept = [
        copy.deepcopy(t)
        for t in normalized.graph.initializer
        if t.name not in structural
    ]
    del normalized.graph.initializer[:]
    normalized.graph.initializer.extend(kept)
    declared = [
        copy.deepcopy(v) for v in normalized.graph.value_info if v.name != pad.output[0]
    ]
    del normalized.graph.value_info[:]
    normalized.graph.value_info.extend(declared)
    return normalized, (index, lowered), structural


def import_pad_graph(path: Path) -> dict[str, Any]:
    with Path(path).open("rb") as handle:
        payload = handle.read(old._POLICY.limits.max_model_bytes + 1)
    audit = audit_model_bytes(payload, source_name=Path(path).name, policy=old._POLICY)
    model = onnx.load_model_from_string(payload)
    _require(
        len(model.graph.node) <= 48 and len(model.graph.initializer) <= 96,
        "Pad graph exceeds node/constant bound",
    )
    _require(
        audit.document["graph_inventory"]["graphs"] == 1
        and audit.document["graph_inventory"]["attribute_tensors"] == 0,
        "nested graph/attribute tensors prohibited",
    )
    normalized, pad_info, structural = _normalize(model)
    with tempfile.TemporaryDirectory(prefix="wave-marblenet-pad-") as temporary:
        proxy = Path(temporary) / "normalized.onnx"
        proxy.write_bytes(normalized.SerializeToString())
        record = old.import_graph(proxy)
    if pad_info is not None:
        index, pad = pad_info
        conv = record["nodes"][index]
        _require(
            conv["input_shape"] == pad["input_shape"], "Pad source shape must be64/200"
        )
        conv["inputs"] = [pad["output"]]
        conv["input_shape"] = pad["output_shape"]
        conv["pads"] = [0, 0]
        record["nodes"].insert(index, pad)
    record["constants"] = {
        t.name: structural[t.name]
        if t.name in structural
        else record["constants"][t.name]
        for t in model.graph.initializer
    }
    for node in record["nodes"]:
        if node["type"] == "Conv":
            node["native_strides"] = [
                node["stride"],
                2
                if node["stride"] == 2 and node["group"] == node["input_shape"][1]
                else 1,
            ]
    record.update(
        profile_id=PROFILE_ID,
        source_model_sha256=audit.model_sha256,
        graph_signature=audit.graph_signature,
        native_policy=dict(NATIVE_POLICY),
        operation={"type": "ConvGraph", "node_count": len(record["nodes"])},
    )
    _seal(record)
    validate_pad_record(record)
    return record


def _project_record(record):
    _require(
        isinstance(record, dict) and record.get("profile_id") == PROFILE_ID,
        "invalid explicit Pad profile",
    )
    _require(
        record.get("native_policy") == NATIVE_POLICY, "native stride policy changed"
    )
    _require(
        record.get("lowering_sha256")
        == hashlib.sha256(
            canonical_json({k: v for k, v in record.items() if k != "lowering_sha256"})
        ).hexdigest(),
        "Pad lowering identity mismatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    projected = copy.deepcopy(record)
    del projected["native_policy"]
    projected["profile_id"] = old.PROFILE_ID
    nodes = projected.get("nodes")
    constants = projected.get("constants")
    _require(
        isinstance(nodes, list) and isinstance(constants, dict),
        "invalid Pad graph fields",
    )
    _require(
        1 <= len(nodes) <= 48 and len(constants) <= 96,
        "Pad graph exceeds node/constant bound",
    )
    positions = [
        i for i, n in enumerate(nodes) if isinstance(n, dict) and n.get("type") == "Pad"
    ]
    _require(len(positions) <= 1, "only one exact Pad is admitted")
    if positions:
        index = positions[0]
        pad = nodes[index]
        old._keys(
            pad,
            {
                "type",
                "inputs",
                "output",
                "input_shape",
                "output_shape",
                "pads",
                "value",
                "pads_constant",
                "value_constant",
            },
            "invalid Pad record fields",
        )
        _require(
            isinstance(pad["inputs"], list)
            and len(pad["inputs"]) == 1
            and isinstance(pad["inputs"][0], str)
            and isinstance(pad["output"], str)
            and bool(pad["output"]),
            "invalid Pad edge",
        )
        _require(
            pad["input_shape"] == [1, 64, 200]
            and pad["output_shape"] == [1, 64, 256]
            and all(type(d) is int for d in pad["input_shape"] + pad["output_shape"])
            and pad["pads"] == [28, 28]
            and all(type(p) is int for p in pad["pads"])
            and type(pad["value"]) is float
            and pad["value"] == 0.0
            and math.copysign(1, pad["value"]) == 1,
            "only positive-zero Pad28/28 on64/200 is admitted",
        )
        names = [(pad["pads_constant"], "INT64", [6], _PAD_BYTES)]
        if pad["value_constant"] is not None:
            names.append((pad["value_constant"], "FLOAT16", [], b"\x00\x00"))
        _require(
            len({n[0] for n in names}) == len(names),
            "duplicate Pad structural reference",
        )
        for name, dtype, shape, wanted in names:
            _require(
                isinstance(name, str) and name in constants,
                "missing Pad structural constant",
            )
            c = constants[name]
            old._keys(
                c,
                {"name", "dtype", "shape", "byte_size", "payload_hex", "sha256"},
                "invalid structural constant fields",
            )
            _require(
                type(c["byte_size"]) is int
                and isinstance(c["shape"], list)
                and all(type(d) is int for d in c["shape"])
                and c
                == {
                    "name": name,
                    "dtype": dtype,
                    "shape": shape,
                    "byte_size": len(wanted),
                    "payload_hex": wanted.hex(),
                    "sha256": hashlib.sha256(wanted).hexdigest(),
                },
                "Pad structural constant identity changed",
            )
            del constants[name]
        _require(index + 1 < len(nodes), "Pad consumer missing")
        conv = nodes[index + 1]
        _require(
            conv.get("type") == "Conv"
            and conv.get("inputs") == [pad["output"]]
            and conv.get("input_shape") == [1, 64, 256]
            and conv.get("output_shape") == [1, 64, 200]
            and all(
                type(v) is int
                for v in conv["input_shape"] + conv["output_shape"] + conv["pads"]
            )
            and all(
                conv.get(k) == v
                for k, v in {
                    "kernel": 29,
                    "dilation": 2,
                    "stride": 1,
                    "pads": [0, 0],
                    "group": 64,
                    "bias": None,
                }.items()
            ),
            "Pad consumer must be trained29/rate2 VALID depthwise",
        )
        _require(
            sum(
                pad["output"] in n.get("inputs", [])
                for n in nodes
                if isinstance(n, dict)
            )
            == 1
            and pad["output"] not in record["constants"]
            and pad["output"] != record["onnx_input"]["name"]
            and sum(n.get("output") == pad["output"] for n in nodes) == 1,
            "Pad output is duplicated or has other consumers",
        )
        conv["inputs"] = list(pad["inputs"])
        conv["input_shape"] = list(pad["input_shape"])
        conv["pads"] = [28, 28]
        del nodes[index]
    for node in nodes:
        if isinstance(node, dict) and node.get("type") == "Conv":
            strides = node.pop("native_strides", None)
            _require(
                isinstance(node.get("input_shape"), list)
                and len(node["input_shape"]) == 3,
                "invalid Conv input shape",
            )
            expected = [
                node.get("stride"),
                2
                if node.get("stride") == 2
                and node.get("group") == node["input_shape"][1]
                else 1,
            ]
            _require(
                isinstance(strides, list)
                and strides == expected
                and all(type(s) is int for s in strides),
                "native flat-height stride mapping changed",
            )
    projected["operation"] = {"type": "ConvGraph", "node_count": len(nodes)}
    _seal(projected)
    old.validate_graph_record(projected)
    _require(
        record["operation"] == {"type": "ConvGraph", "node_count": len(record["nodes"])}
        and type(record["operation"]["node_count"]) is int,
        "Pad operation count mismatch",
    )
    return projected


def validate_pad_record(record):
    _project_record(record)


def independent_pad_oracle(record, inputs):
    return old.independent_graph_oracle(_project_record(record), inputs)


def validate_full_pad_model(payload: bytes) -> dict[str, Any]:
    """Reverse the exact full recipe and cryptographically bind old18D bytes.

    No ORT session, checkpoint loader, transport or device call is created.
    Mutable/resealed receipts cannot authorize another same-shape graph.
    """
    _require(
        isinstance(payload, bytes)
        and len(payload) <= old._POLICY.limits.max_model_bytes,
        "full model payload invalid",
    )
    try:
        model = onnx.load_model_from_string(payload)
    except Exception as exc:
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID, "cannot decode full Pad model"
        ) from exc
    normalized, pad, _ = _normalize(model)
    _require(
        pad is not None and pad[0] == PAD_RECIPE["full_depthwise_node_index"],
        "full trained Pad position changed",
        ErrorCode.MODEL_PACKAGE_INVALID,
    )
    original = normalized.SerializeToString()
    _require(
        hashlib.sha256(original).hexdigest() == ORIGINAL_FULL_SHA256,
        "full Pad inverse does not reconstruct pinned original18D",
        ErrorCode.MODEL_PACKAGE_INVALID,
    )
    _require(
        rewrite_explicit_pad(normalized, 27).SerializeToString() == payload,
        "full model does not match deterministic Pad recipe",
        ErrorCode.MODEL_PACKAGE_INVALID,
    )
    return {
        "profile_id": PROFILE_ID,
        "model_sha256": hashlib.sha256(payload).hexdigest(),
        "source_original_model_sha256": ORIGINAL_FULL_SHA256,
        "checkpoint_sha256": CHECKPOINT_SHA256,
        "recipe_id": PAD_RECIPE["id"],
        "recipe_sha256": hashlib.sha256(canonical_json(PAD_RECIPE)).hexdigest(),
        "learned_initializer_count": len(normalized.graph.initializer),
        "learned_initializer_bytes": sum(
            numpy_helper.to_array(t).nbytes for t in normalized.graph.initializer
        ),
    }
