"""Host-only lowering for the two explicitly bounded Wave native profiles.

Importability is a compile candidate, never evidence of device qualification.
All identities use the auditor's canonical JSON and actual initializer bytes.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

import numpy as np
import onnx
from onnx import AttributeProto, TensorProto, numpy_helper

from .audit import AdmissionLimits, AuditPolicy, audit_model_bytes, canonical_json
from .errors import ErrorCode, HybridError

SCHEMA_VERSION = 1
_DTYPE = {"FLOAT16": "<f2", "FLOAT32": "<f4", "UINT8": "u1", "INT32": "<i4"}
_NATIVE_POLICY = AuditPolicy(
    policy_id="wave-native-import-v1-unqualified",
    policy_sha256=hashlib.sha256(b"wave-native-import-v1-unqualified").hexdigest(),
    compiler_manifest_sha256="0" * 64,
    compiler_qualified=False,
    approved_npu_signatures=frozenset(),
    limits=AdmissionLimits(),
)


def _require(
    condition: bool, message: str, code: ErrorCode = ErrorCode.ONNX_GRAPH_INVALID
) -> None:
    if not condition:
        raise HybridError(code, message)


def _tensor(
    name: str, dtype: str, shape: list[int], *, quantized: bool = False
) -> dict[str, Any]:
    result = {
        "name": name,
        "dtype": dtype,
        "shape": shape,
        "byte_size": int(np.prod(shape)) * np.dtype(_DTYPE[dtype]).itemsize,
    }
    if quantized:
        result["quantization"] = {"type": "ASYMMETRIC", "scale": 1.0, "zero_point": 0}
    return result


def _constant(
    name: str,
    dtype: str,
    shape: list[int],
    values: np.ndarray,
    *,
    quantized: bool = False,
) -> dict[str, Any]:
    result = _tensor(name, dtype, shape, quantized=quantized)
    payload = np.ascontiguousarray(values, dtype=_DTYPE[dtype]).tobytes()
    _require(len(payload) == result["byte_size"], "lowered constant byte-size mismatch")
    result.update(payload_hex=payload.hex(), sha256=hashlib.sha256(payload).hexdigest())
    return result


def _value_contract(
    value: onnx.ValueInfoProto, dtype: str, shapes: list[list[int]]
) -> dict[str, Any]:
    _require(
        value.type.HasField("tensor_type"),
        "native profile requires tensor I/O",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    tensor = value.type.tensor_type
    expected_type = {"FLOAT16": TensorProto.FLOAT16, "UINT8": TensorProto.UINT8}[dtype]
    _require(
        tensor.elem_type == expected_type,
        f"native profile requires {dtype} I/O",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        tensor.HasField("shape")
        and all(d.WhichOneof("value") == "dim_value" for d in tensor.shape.dim),
        "source I/O must have static dimensions",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )
    shape = [d.dim_value for d in tensor.shape.dim]
    _require(
        shape in shapes,
        f"unsupported native profile shape {shape}",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )
    return _tensor(value.name, dtype, shape)


def _decode(tensor: TensorProto, dtype: int, shape: list[int]) -> np.ndarray:
    _require(
        tensor.data_type == dtype,
        f"initializer {tensor.name!r} has wrong dtype",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        list(tensor.dims) == shape,
        f"initializer {tensor.name!r} has wrong shape",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )
    _require(not tensor.HasField("segment"), "segmented initializers are unsupported")
    populated = [
        name
        for name in (
            "raw_data",
            "float_data",
            "int32_data",
            "string_data",
            "int64_data",
            "double_data",
            "uint64_data",
        )
        if getattr(tensor, name)
    ]
    _require(len(populated) == 1, "initializer must contain exactly one inline payload")
    try:
        values = numpy_helper.to_array(tensor)
    except Exception as exc:
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID, "cannot decode inline initializer"
        ) from exc
    _require(list(values.shape) == shape, "decoded initializer shape mismatch")
    _require(bool(np.isfinite(values).all()), "non-finite initializer is unsupported")
    return values


def _attributes(node: onnx.NodeProto) -> dict[str, Any]:
    expected = {
        "auto_pad": AttributeProto.STRING,
        "pads": AttributeProto.INTS,
        "strides": AttributeProto.INTS,
        "dilations": AttributeProto.INTS,
        "kernel_shape": AttributeProto.INTS,
        "group": AttributeProto.INT,
    }
    attrs: dict[str, Any] = {}
    for attr in node.attribute:
        _require(
            attr.name in expected and attr.name not in attrs,
            "unknown or duplicate Conv attribute",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        _require(
            attr.type == expected[attr.name] and not attr.ref_attr_name,
            "invalid Conv attribute type",
        )
        if attr.type == AttributeProto.STRING:
            _require(
                attr.s in {b"SAME_UPPER", b"NOTSET"},
                "only SAME_UPPER or explicit symmetric padding is supported",
                ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
            )
            attrs[attr.name] = attr.s.decode("ascii")
        elif attr.type == AttributeProto.INT:
            attrs[attr.name] = attr.i
        else:
            attrs[attr.name] = list(attr.ints)
    return attrs


def _conv(graph: onnx.GraphProto, constants: dict[str, TensorProto]) -> dict[str, Any]:
    node = graph.node[0]
    _require(
        len(node.input) in {2, 3} and bool(node.input[1]),
        "Conv requires inline weights and optional inline bias",
    )
    input_spec = _value_contract(graph.input[0], "FLOAT16", [[1, 1, 16], [1, 1, 1, 16]])
    output_spec = _value_contract(graph.output[0], "FLOAT16", [input_spec["shape"]])
    rank = len(input_spec["shape"])
    kernel = [3] if rank == 3 else [1, 3]
    weight_shape = [1, 1, *kernel]
    attrs = _attributes(node)
    _require(
        attrs.get("group", 1) == 1,
        "Conv group must be 1",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    for name in ("strides", "dilations"):
        _require(
            attrs.get(name, [1] * (rank - 2)) == [1] * (rank - 2),
            f"Conv {name} must be 1",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
    _require(
        attrs.get("kernel_shape", kernel) == kernel,
        "Conv kernel_shape mismatch",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    auto_pad = attrs.get("auto_pad", "NOTSET")
    symmetric = [1, 1] if rank == 3 else [0, 1, 0, 1]
    _require(
        not (auto_pad == "SAME_UPPER" and "pads" in attrs),
        "conflicting Conv pads/auto_pad",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        auto_pad == "SAME_UPPER" or attrs.get("pads") == symmetric,
        "Conv requires equivalent explicit symmetric padding",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    weights = _decode(constants[node.input[1]], TensorProto.FLOAT16, weight_shape)
    bias = (
        _decode(constants[node.input[2]], TensorProto.FLOAT16, [1]).astype("<f4")
        if len(node.input) == 3 and node.input[2]
        else np.zeros(1, dtype="<f4")
    )
    return {
        "profile_id": "fp16-conv-v1",
        "onnx_input": input_spec,
        "onnx_output": output_spec,
        "native_input": _tensor("input", "FLOAT16", [16, 1, 1, 1]),
        "native_output": _tensor("output", "FLOAT16", [16, 1, 1, 1]),
        "weights": _constant("weights", "FLOAT16", [3, 1, 1, 1], weights),
        "bias": _constant("bias", "FLOAT32", [1], bias),
        "operation": {
            "type": "Conv2d",
            "padding": "SAME",
            "strides": [1, 1],
            "dilations": [0, 0],
            "group": 1,
            "kernel": [3, 1],
        },
    }


def _fc(graph: onnx.GraphProto, constants: dict[str, TensorProto]) -> dict[str, Any]:
    node = graph.node[0]
    _require(
        len(node.input) == 8 and all(node.input),
        "QLinearMatMul requires eight explicit inputs",
    )
    _require(
        not node.attribute,
        "QLinearMatMul attributes are unsupported",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    input_spec = _value_contract(graph.input[0], "UINT8", [[1, 16]])
    output_spec = _value_contract(graph.output[0], "UINT8", [[1, 4]])
    for index in (1, 4, 6):
        scale = _decode(constants[node.input[index]], TensorProto.FLOAT, [])
        _require(
            float(scale) == 1.0,
            "quantizer scale must be scalar FLOAT 1.0",
            ErrorCode.ONNX_QUANTIZATION_UNSUPPORTED,
        )
    for index in (2, 5, 7):
        zero_point = _decode(constants[node.input[index]], TensorProto.UINT8, [])
        _require(
            int(zero_point) == 0,
            "quantizer zero point must be scalar UINT8 0",
            ErrorCode.ONNX_QUANTIZATION_UNSUPPORTED,
        )
    weights = _decode(constants[node.input[3]], TensorProto.UINT8, [16, 4])
    return {
        "profile_id": "u8-fc-v1",
        "onnx_input": input_spec,
        "onnx_output": output_spec,
        "native_input": _tensor("input", "UINT8", [16, 1], quantized=True),
        "native_output": _tensor("output", "UINT8", [4, 1], quantized=True),
        "weights": _constant("weights", "UINT8", [16, 4], weights.T, quantized=True),
        "bias": _constant(
            "bias", "INT32", [4], np.zeros(4, dtype="<i4"), quantized=True
        ),
        "operation": {"type": "FullyConnected", "axis": 0, "units": 4},
    }


def import_native_model(path: Path) -> dict[str, Any]:
    """Admit one ONNX model and extract actual parameters without native calls."""
    path = Path(path)
    try:
        with path.open("rb") as handle:
            payload = handle.read(_NATIVE_POLICY.limits.max_model_bytes + 1)
    except OSError as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE, f"cannot read model: {exc}"
        ) from exc
    audit = audit_model_bytes(payload, source_name=path.name, policy=_NATIVE_POLICY)
    # Lower the same audited bytes, retaining the source's declared I/O shapes.
    model = onnx.load_model_from_string(payload)
    graph = model.graph
    vision_markers = [
        p.value for p in model.metadata_props if p.key == "wave.native_profile"
    ]
    if "fp16-squeezenet-mean-v1" in vision_markers:
        _require(
            vision_markers == ["fp16-squeezenet-mean-v1"],
            "duplicate native mean profile marker",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        from .vision_mean import import_graph as import_mean

        return import_mean(path)
    if "fp16-squeezenet-v1" in vision_markers:
        _require(
            vision_markers == ["fp16-squeezenet-v1"],
            "duplicate native vision profile marker",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        from .vision_graph import import_graph as import_vision

        return import_vision(path)
    # Preserve the two original profile admissions; route only bounded NCW
    # graph candidates to the separate, independently validated importer.
    if len(graph.node) > 1 or (
        len(graph.input) == 1
        and graph.input[0].type.HasField("tensor_type")
        and graph.input[0].type.tensor_type.elem_type == TensorProto.FLOAT16
        and len(graph.input[0].type.tensor_type.shape.dim) == 3
        and graph.input[0].type.tensor_type.shape.dim[1].dim_value in {2, 64, 80, 128}
    ):
        from .marblenet_graph import import_graph

        markers = [
            p.value for p in model.metadata_props if p.key == "wave.native_profile"
        ]
        if markers:
            _require(
                markers == ["fp16-marblenet-pad-v1"],
                "unsupported or duplicate native graph profile marker",
                ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
            )
            return import_graph(path, profile_id=markers[0])
        return import_graph(path)
    _require(
        len(model.opset_import) == 1
        and model.opset_import[0].domain in {"", "ai.onnx"}
        and model.opset_import[0].version == 13,
        "native profiles require only standard ONNX opset 13",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        audit.document["graph_inventory"]["graphs"] == 1
        and audit.document["graph_inventory"]["attribute_tensors"] == 0,
        "nested graphs and attribute constants are unsupported",
    )
    _require(
        len(graph.node) == len(graph.input) == len(graph.output) == 1,
        "native profiles require one node, input and output",
    )
    names = [v.name for v in [*graph.input, *graph.output, *graph.value_info]]
    _require(len(names) == len(set(names)), "duplicate value names are unsupported")
    constants = {t.name: t for t in graph.initializer}
    _require(
        not (set(constants) & {v.name for v in graph.input}),
        "overridable initializers are unsupported",
    )
    node = graph.node[0]
    _require(
        node.domain in {"", "ai.onnx"},
        "native profiles require the standard ONNX domain",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        node.op_type in {"Conv", "QLinearMatMul"},
        "operator has no native import profile",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        node.input
        and node.input[0] == graph.input[0].name
        and list(node.output) == [graph.output[0].name],
        "native node must consume and produce the external I/O",
    )
    _require(
        set(node.input[1:]) - {""} == set(constants),
        "all parameters must be used inline initializers",
    )
    result = (
        _conv(graph, constants) if node.op_type == "Conv" else _fc(graph, constants)
    )
    result.update(
        schema_version=SCHEMA_VERSION,
        source_model_sha256=audit.model_sha256,
        graph_signature=audit.graph_signature,
    )
    result["lowering_sha256"] = hashlib.sha256(canonical_json(result)).hexdigest()
    return result


def independent_oracle(
    record: dict[str, Any], inputs: dict[str, np.ndarray]
) -> dict[str, np.ndarray]:
    """Compute ONNX-shaped output in NumPy without ORT or any native library.

    Conv accumulates three half operands in FP32 then rounds once to FP16.
    FC widens before multiplication and saturates the integer sum to UINT8.
    """
    if record.get("profile_id") == "fp16-squeezenet-mean-v1":
        from .vision_mean import independent_graph_oracle as mean_oracle

        return mean_oracle(record, inputs)
    if record.get("profile_id") == "fp16-squeezenet-v1":
        from .vision_graph import independent_graph_oracle as vision_oracle

        return vision_oracle(record, inputs)
    if record.get("profile_id") in {"fp16-marblenet-v1", "fp16-marblenet-pad-v1"}:
        from .marblenet_graph import independent_graph_oracle

        return independent_graph_oracle(record, inputs)
    identity = {k: v for k, v in record.items() if k != "lowering_sha256"}
    _require(
        record.get("lowering_sha256")
        == hashlib.sha256(canonical_json(identity)).hexdigest(),
        "lowering identity mismatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    spec = record["onnx_input"]
    _require(
        set(inputs) == {spec["name"]},
        "input tensor names mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    values = inputs[spec["name"]]
    _require(
        isinstance(values, np.ndarray)
        and values.dtype == np.dtype(_DTYPE[spec["dtype"]])
        and list(values.shape) == spec["shape"],
        "input tensor dtype/shape mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    _require(
        bool(np.isfinite(values).all()),
        "input tensor is non-finite",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    coefficients = record["weights"]
    weights = np.frombuffer(
        bytes.fromhex(coefficients["payload_hex"]), dtype=_DTYPE[coefficients["dtype"]]
    )
    if record["profile_id"] == "fp16-conv-v1":
        flat = values.reshape(16).astype(np.float32)
        padded = np.pad(flat, (1, 1))
        bias = np.frombuffer(bytes.fromhex(record["bias"]["payload_hex"]), dtype="<f4")[
            0
        ]
        output = np.full(16, bias, dtype=np.float32)
        for tap in range(3):
            output += padded[tap : tap + 16] * np.float32(weights[tap])
        output = output.astype(np.float16)
    elif record["profile_id"] == "u8-fc-v1":
        output = np.clip(
            weights.reshape(4, 16).astype(np.int32)
            @ values.reshape(16).astype(np.int32),
            0,
            255,
        ).astype(np.uint8)
    else:
        raise HybridError(ErrorCode.ONNX_OPERATOR_UNSUPPORTED, "unknown oracle profile")
    spec = record["onnx_output"]
    return {spec["name"]: output.reshape(spec["shape"])}
