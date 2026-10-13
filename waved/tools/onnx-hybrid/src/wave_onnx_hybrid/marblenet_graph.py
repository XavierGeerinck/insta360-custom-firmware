"""Bounded FP16 NCW Conv/Add/Relu import and independent NumPy evaluation.

Graph admission never grants qualification. Constants retain ONNX OIK byte order.
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

PROFILE_ID = "fp16-marblenet-v1"
NUMERICAL_POLICY = {"atol": 0.02, "rtol": 0.02}
CHANNELS = {2, 64, 80, 128}
WIDTHS = {200, 400}
KERNELS = {1, 11, 13, 15, 17, 29}
_POLICY = AuditPolicy(
    policy_id="wave-marblenet-import-v1-unqualified",
    policy_sha256=hashlib.sha256(b"wave-marblenet-import-v1-unqualified").hexdigest(),
    compiler_manifest_sha256="0" * 64,
    compiler_qualified=False,
    approved_npu_signatures=frozenset(),
    limits=AdmissionLimits(
        max_nodes=48,
        max_inputs=1,
        max_outputs=1,
        max_rank=3,
        max_dimension=400,
        max_initializer_bytes=8 * 1024 * 1024,
        max_total_initializer_bytes=8 * 1024 * 1024,
        max_tensor_bytes=8 * 1024 * 1024,
    ),
)


def _require(ok: bool, message: str, code=ErrorCode.ONNX_GRAPH_INVALID) -> None:
    if not ok:
        raise HybridError(code, message)


def _keys(value: Any, keys: set[str], message: str) -> None:
    _require(isinstance(value, dict) and set(value) == keys, message)


def _shape(shape: Any) -> None:
    _require(
        isinstance(shape, list)
        and len(shape) == 3
        and all(type(v) is int for v in shape)
        and shape[0] == 1
        and shape[1] in CHANNELS
        and shape[2] in WIDTHS,
        "graph requires bounded static NCW shapes",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )


def _contract(name: str, shape: list[int]) -> dict[str, Any]:
    return {
        "name": name,
        "dtype": "FLOAT16",
        "shape": shape,
        "byte_size": int(np.prod(shape)) * 2,
    }


def _io(value: onnx.ValueInfoProto) -> dict[str, Any]:
    _require(value.type.HasField("tensor_type"), "graph requires tensor I/O")
    t = value.type.tensor_type
    _require(
        t.elem_type == TensorProto.FLOAT16,
        "graph requires FP16 I/O",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        t.HasField("shape")
        and all(d.WhichOneof("value") == "dim_value" for d in t.shape.dim),
        "graph requires static dimensions",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )
    shape = [d.dim_value for d in t.shape.dim]
    _shape(shape)
    return _contract(value.name, shape)


def _attrs(node: onnx.NodeProto) -> dict[str, Any]:
    allowed = {
        "pads": AttributeProto.INTS,
        "strides": AttributeProto.INTS,
        "dilations": AttributeProto.INTS,
        "kernel_shape": AttributeProto.INTS,
        "group": AttributeProto.INT,
    }
    result = {}
    for attr in node.attribute:
        _require(
            attr.name in allowed and attr.name not in result,
            "unknown or duplicate graph Conv attribute",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        _require(
            attr.type == allowed[attr.name] and not attr.ref_attr_name,
            "invalid graph Conv attribute",
        )
        result[attr.name] = (
            attr.i if attr.type == AttributeProto.INT else list(attr.ints)
        )
    return result


def _constant(tensor: TensorProto) -> dict[str, Any]:
    _require(
        tensor.data_type == TensorProto.FLOAT16,
        "graph constants require FP16",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        not tensor.HasField("segment")
        and tensor.data_location != TensorProto.EXTERNAL
        and not tensor.external_data,
        "graph requires inline constants",
        ErrorCode.ONNX_EXTERNAL_DATA,
    )
    payload_fields = [
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
    _require(len(payload_fields) == 1, "graph constant must have exactly one payload")
    values = numpy_helper.to_array(tensor)
    _require(
        values.dtype == np.dtype("<f2") and np.isfinite(values).all(),
        "graph constant is not finite FP16",
    )
    b = np.ascontiguousarray(values, dtype="<f2").tobytes()
    return {
        **_contract(tensor.name, list(tensor.dims)),
        "payload_hex": b.hex(),
        "sha256": hashlib.sha256(b).hexdigest(),
    }


def import_graph(path: Path) -> dict[str, Any]:
    """Extract a bounded graph from the same audited bytes, without execution."""
    with Path(path).open("rb") as handle:
        payload = handle.read(_POLICY.limits.max_model_bytes + 1)
    audit = audit_model_bytes(payload, source_name=Path(path).name, policy=_POLICY)
    model = onnx.load_model_from_string(payload)
    g = model.graph
    _require(
        len(model.opset_import) == 1
        and model.opset_import[0].domain in {"", "ai.onnx"}
        and model.opset_import[0].version == 13,
        "graph requires standard opset13",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        not model.functions and not model.training_info,
        "graph functions/training info unsupported",
    )
    _require(
        audit.document["graph_inventory"]["graphs"] == 1
        and audit.document["graph_inventory"]["attribute_tensors"] == 0,
        "nested graphs/attribute constants unsupported",
    )
    _require(
        len(g.input) == len(g.output) == 1 and 1 <= len(g.node) <= 48,
        "graph requires one input/output and at most48 nodes",
    )
    input_spec, output_spec = _io(g.input[0]), _io(g.output[0])
    constants = {t.name: _constant(t) for t in g.initializer}
    _require(len(constants) == len(g.initializer), "duplicate graph constants")
    _require(input_spec["name"] not in constants, "overridable constants unsupported")
    shapes = {input_spec["name"]: input_spec["shape"]}
    nodes = []
    for node in g.node:
        _require(
            node.domain in {"", "ai.onnx"} and node.op_type in {"Conv", "Add", "Relu"},
            "unsupported graph operation",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        _require(
            len(node.output) == 1
            and node.output[0]
            and node.output[0] not in shapes
            and node.output[0] not in constants,
            "invalid or duplicate node output",
        )
        _require(
            node.input and node.input[0] in shapes,
            "node input is not a preceding activation",
        )
        s = shapes[node.input[0]]
        if node.op_type == "Conv":
            _require(
                len(node.input) in {2, 3} and node.input[1] in constants,
                "Conv requires inline weights",
            )
            attrs = _attrs(node)
            w = constants[node.input[1]]["shape"]
            _require(len(w) == 3, "Conv weights require OIK layout")
            k = w[2]
            stride = attrs.get("strides", [1])
            dilation = attrs.get("dilations", [1])
            pads = attrs.get("pads")
            _require(
                len(stride) == len(dilation) == 1
                and pads is not None
                and len(pads) == 2,
                "Conv requires explicit temporal attributes",
            )
            _require(
                attrs.get("kernel_shape", [k]) == [k],
                "kernel_shape disagrees with weights",
            )
            _require(
                stride[0] in {1, 2}
                and dilation[0] in {1, 2}
                and all(type(p) is int and 0 <= p <= 56 for p in pads),
                "unsupported Conv stride/dilation/padding",
            )
            out = [
                1,
                w[0],
                (s[2] + sum(pads) - dilation[0] * (k - 1) - 1) // stride[0] + 1,
            ]
            bias = node.input[2] if len(node.input) == 3 and node.input[2] else None
            _require(bias is None or bias in constants, "Conv bias must be inline")
            lowered = {
                "type": "Conv",
                "inputs": [node.input[0]],
                "output": node.output[0],
                "input_shape": s,
                "output_shape": out,
                "weights": node.input[1],
                "bias": bias,
                "kernel": k,
                "stride": stride[0],
                "dilation": dilation[0],
                "pads": pads,
                "group": attrs.get("group", 1),
            }
        elif node.op_type == "Relu":
            _require(
                len(node.input) == 1 and not node.attribute,
                "Relu requires one input and no attributes",
            )
            out = s
            lowered = {
                "type": "Relu",
                "inputs": list(node.input),
                "output": node.output[0],
                "input_shape": s,
                "output_shape": out,
            }
        else:
            _require(
                len(node.input) == 2
                and all(n in shapes for n in node.input)
                and not node.attribute,
                "Add requires two preceding activations",
            )
            out = s
            lowered = {
                "type": "Add",
                "inputs": list(node.input),
                "output": node.output[0],
                "input_shapes": [shapes[n] for n in node.input],
                "output_shape": out,
            }
        _shape(out)
        shapes[node.output[0]] = out
        nodes.append(lowered)
    declared = [*g.input, *g.output, *g.value_info]
    _require(
        len({v.name for v in declared}) == len(declared),
        "duplicate declared value names",
    )
    for value in declared:
        contract = _io(value)
        _require(
            value.name in shapes and contract["shape"] == shapes[value.name],
            "declared activation contract disagrees with graph",
        )
    record = {
        "schema_version": 1,
        "profile_id": PROFILE_ID,
        "source_model_sha256": audit.model_sha256,
        "graph_signature": audit.graph_signature,
        "onnx_input": input_spec,
        "onnx_output": output_spec,
        "native_input": _contract(
            "input", [input_spec["shape"][2], 1, input_spec["shape"][1], 1]
        ),
        "native_output": _contract(
            "output", [output_spec["shape"][2], 1, output_spec["shape"][1], 1]
        ),
        "constants": constants,
        "nodes": nodes,
        "operation": {"type": "ConvGraph", "node_count": len(nodes)},
        "numerical_policy": dict(NUMERICAL_POLICY),
    }
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    validate_graph_record(record)
    return record


def validate_graph_record(record: dict[str, Any]) -> None:
    """Check identity and every structural/constant edge before native use."""
    _keys(
        record,
        {
            "schema_version",
            "profile_id",
            "source_model_sha256",
            "graph_signature",
            "onnx_input",
            "onnx_output",
            "native_input",
            "native_output",
            "constants",
            "nodes",
            "operation",
            "numerical_policy",
            "lowering_sha256",
        },
        "invalid graph record keys",
    )
    _require(
        type(record["schema_version"]) is int
        and record["schema_version"] == 1
        and record["profile_id"] == PROFILE_ID,
        "invalid graph profile",
    )
    for key in ("source_model_sha256", "graph_signature", "lowering_sha256"):
        v = record[key]
        _require(
            isinstance(v, str)
            and len(v) == 64
            and all(c in "0123456789abcdef" for c in v),
            "invalid graph identity",
        )
    identity = {k: v for k, v in record.items() if k != "lowering_sha256"}
    _require(
        record["lowering_sha256"]
        == hashlib.sha256(canonical_json(identity)).hexdigest(),
        "graph lowering identity mismatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    _require(
        record["numerical_policy"] == NUMERICAL_POLICY,
        "graph numerical policy changed",
        ErrorCode.EVIDENCE_INVALID,
    )
    for key in ("onnx_input", "onnx_output"):
        spec = record[key]
        _keys(
            spec, {"name", "dtype", "shape", "byte_size"}, "invalid graph I/O contract"
        )
        _shape(spec["shape"])
        _require(
            isinstance(spec["name"], str)
            and bool(spec["name"])
            and spec == _contract(spec["name"], spec["shape"]),
            "invalid graph I/O contract",
        )
        native = record[key.replace("onnx", "native")]
        s = spec["shape"]
        _require(
            native
            == _contract(
                "input" if key == "onnx_input" else "output", [s[2], 1, s[1], 1]
            ),
            "native layout disagrees with NCW",
        )
    constants = record["constants"]
    _require(
        isinstance(constants, dict) and len(constants) <= 96, "invalid graph constants"
    )
    for name, c in constants.items():
        _keys(
            c,
            {"name", "dtype", "shape", "byte_size", "payload_hex", "sha256"},
            "invalid constant contract",
        )
        _require(
            isinstance(name, str)
            and bool(name)
            and c["name"] == name
            and c["dtype"] == "FLOAT16",
            "invalid constant identity",
        )
        _require(
            isinstance(c["shape"], list)
            and 1 <= len(c["shape"]) <= 3
            and all(type(d) is int and 1 <= d <= 400 for d in c["shape"]),
            "invalid constant shape",
        )
        _require(
            c["byte_size"] == int(np.prod(c["shape"])) * 2
            and c["byte_size"] <= 8 * 1024 * 1024,
            "constant size mismatch",
        )
        try:
            b = bytes.fromhex(c["payload_hex"])
        except (TypeError, ValueError) as exc:
            raise HybridError(
                ErrorCode.EVIDENCE_INVALID, "invalid constant payload"
            ) from exc
        _require(
            len(b) == c["byte_size"]
            and hashlib.sha256(b).hexdigest() == c["sha256"]
            and np.isfinite(np.frombuffer(b, dtype="<f2")).all(),
            "constant hash/finite check failed",
            ErrorCode.EVIDENCE_INVALID,
        )
    _require(
        sum(c["byte_size"] for c in constants.values()) <= 8 * 1024 * 1024,
        "graph constants exceed total byte bound",
    )
    nodes = record["nodes"]
    _require(
        isinstance(nodes, list) and 1 <= len(nodes) <= 48, "invalid graph node count"
    )
    _keys(record["operation"], {"type", "node_count"}, "invalid graph operation")
    _require(
        type(record["operation"].get("node_count")) is int
        and record["operation"] == {"type": "ConvGraph", "node_count": len(nodes)},
        "graph operation count mismatch",
    )
    shapes = {record["onnx_input"]["name"]: record["onnx_input"]["shape"]}
    used = set()
    producers = {}
    conv_count = 0
    _require(not (set(shapes) & set(constants)), "overridable constants")
    for node in nodes:
        _require(
            isinstance(node, dict) and node.get("type") in {"Conv", "Relu", "Add"},
            "unsupported record node",
        )
        typ = node["type"]
        keys = {"type", "inputs", "output", "output_shape"}
        keys |= {"input_shapes"} if typ == "Add" else {"input_shape"}
        if typ == "Conv":
            keys |= {"weights", "bias", "kernel", "stride", "dilation", "pads", "group"}
        _keys(node, keys, "invalid node record keys")
        ins = node["inputs"]
        out = node["output"]
        _require(
            isinstance(ins, list)
            and len(ins) == (2 if typ == "Add" else 1)
            and all(isinstance(n, str) and n in shapes for n in ins),
            "invalid graph edge",
        )
        _require(
            isinstance(out, str)
            and bool(out)
            and out not in shapes
            and out not in constants,
            "duplicate activation output",
        )
        _shape(node["output_shape"])
        if typ == "Add":
            _require(
                node["input_shapes"] == [shapes[n] for n in ins]
                and shapes[ins[0]] == shapes[ins[1]] == node["output_shape"],
                "Add broadcasting prohibited",
            )
        else:
            s = shapes[ins[0]]
            _require(node["input_shape"] == s, "node input contract mismatch")
            if typ == "Relu":
                _require(node["output_shape"] == s, "Relu shape mismatch")
            else:
                conv_count += 1
                _require(
                    isinstance(node["weights"], str)
                    and node["weights"] in constants
                    and (
                        node["bias"] is None
                        or (isinstance(node["bias"], str) and node["bias"] in constants)
                    ),
                    "missing Conv constant",
                )
                k, stride, dilation, group = (
                    node["kernel"],
                    node["stride"],
                    node["dilation"],
                    node["group"],
                )
                _require(
                    all(type(v) is int for v in (k, stride, dilation, group))
                    and k in KERNELS
                    and stride in {1, 2}
                    and dilation in {1, 2}
                    and not (stride == 2 and dilation == 2),
                    "unsupported Conv parameters",
                )
                pads = node["pads"]
                _require(
                    isinstance(pads, list)
                    and len(pads) == 2
                    and all(type(p) is int and 0 <= p <= 56 for p in pads)
                    and pads[0] == pads[1],
                    "explicit symmetric pads required",
                )
                o = node["output_shape"][1]
                _require(
                    group == 1 or (group == s[1] and o == s[1]),
                    "only dense or multiplier-one depthwise Conv supported",
                )
                _require(
                    constants[node["weights"]]["shape"] == [o, s[1] // group, k],
                    "Conv OIK/group layout mismatch",
                )
                _require(
                    node["bias"] is None or constants[node["bias"]]["shape"] == [o],
                    "Conv bias shape mismatch",
                )
                width = (s[2] + sum(pads) - dilation * (k - 1) - 1) // stride + 1
                _require(
                    node["output_shape"] == [1, o, width],
                    "Conv output geometry mismatch",
                )
                used.add(node["weights"])
                if node["bias"] is not None:
                    used.add(node["bias"])
        shapes[out] = node["output_shape"]
        producers[out] = ins
    output = record["onnx_output"]
    _require(
        conv_count > 0
        and shapes.get(output["name"]) == output["shape"]
        and output["name"] in producers,
        "graph output mismatch",
    )
    _require(used == set(constants), "unused graph constants")
    live = {output["name"]}
    pending = [output["name"]]
    while pending:
        for name in producers.get(pending.pop(), []):
            if name not in live:
                live.add(name)
                pending.append(name)
    _require(set(producers) <= live, "unused graph nodes")


def independent_graph_oracle(
    record: dict[str, Any], inputs: dict[str, np.ndarray]
) -> dict[str, np.ndarray]:
    """Evaluate FP16 stages with FP32 Conv accumulation and no ORT session."""
    validate_graph_record(record)
    spec = record["onnx_input"]
    _require(
        set(inputs) == {spec["name"]},
        "graph input names mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    x = inputs[spec["name"]]
    _require(
        isinstance(x, np.ndarray)
        and x.dtype == np.dtype("<f2")
        and list(x.shape) == spec["shape"]
        and np.isfinite(x).all(),
        "graph input shape/dtype/finite mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    values = {spec["name"]: np.ascontiguousarray(x)}
    constants = {
        name: np.frombuffer(bytes.fromhex(c["payload_hex"]), dtype="<f2")
        .reshape(c["shape"])
        .astype(np.float32)
        for name, c in record["constants"].items()
    }
    with np.errstate(over="ignore", invalid="ignore"):
        for n in record["nodes"]:
            x = values[n["inputs"][0]].astype(np.float32)
            if n["type"] == "Conv":
                padded = np.pad(x, ((0, 0), (0, 0), tuple(n["pads"])))
                size = n["dilation"] * (n["kernel"] - 1) + 1
                windows = np.lib.stride_tricks.sliding_window_view(
                    padded, size, axis=2
                )[:, :, :: n["stride"], :: n["dilation"]]
                w = constants[n["weights"]]
                if n["group"] == 1:
                    result = np.einsum("oik,nitk->not", w, windows, optimize=True)
                else:
                    result = np.einsum(
                        "ck,nctk->nct", w[:, 0, :], windows, optimize=True
                    )
                if n["bias"] is not None:
                    result += constants[n["bias"]][None, :, None]
            elif n["type"] == "Add":
                result = x + values[n["inputs"][1]].astype(np.float32)
            else:
                result = np.maximum(x, 0)
            result = np.ascontiguousarray(result, dtype="<f2")
            _require(
                list(result.shape) == n["output_shape"] and np.isfinite(result).all(),
                "nonfinite graph oracle activation",
                ErrorCode.ONNX_OUTPUT_INVALID,
            )
            values[n["output"]] = result
    return {record["onnx_output"]["name"]: values[record["onnx_output"]["name"]]}


def validate_graph_output(
    record: dict[str, Any], expected: bytes, observed: bytes
) -> dict[str, Any]:
    """Bind and qualify actual bytes using the profile's immutable tolerance."""
    validate_graph_record(record)
    size = record["onnx_output"]["byte_size"]
    _require(
        isinstance(expected, bytes)
        and isinstance(observed, bytes)
        and len(expected) == len(observed) == size,
        "graph output byte size mismatch",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    _require(
        observed != b"\xa5" * size,
        "graph output equals untouched canary",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    reference = np.frombuffer(expected, dtype="<f2").astype(np.float32)
    actual = np.frombuffer(observed, dtype="<f2").astype(np.float32)
    _require(
        np.isfinite(reference).all() and np.isfinite(actual).all(),
        "nonfinite graph output",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    errors = np.abs(actual - reference)
    bounds = 0.02 + 0.02 * np.abs(reference)
    bad = errors > bounds
    metrics = {
        "status": "PASS" if not bad.any() else "FAIL",
        "passed": not bool(bad.any()),
        "reference_sha256": hashlib.sha256(expected).hexdigest(),
        "observed_sha256": hashlib.sha256(observed).hexdigest(),
        "max_abs_error": float(errors.max()),
        "elements": len(reference),
        "mismatched_elements": int(bad.sum()),
        "policy": dict(NUMERICAL_POLICY),
    }
    if bad.any():
        raise HybridError(
            ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
            "graph output exceeds frozen numerical tolerance",
            details=metrics,
        )
    return metrics
