"""Five fixed SqueezeNet1.1 FP16 graph recipes, without native execution.

C-order NCHW/OIHW bytes map directly to width-first WHCN/WHIO. Admission
establishes structural compatibility only; it never establishes native support.
"""

from __future__ import annotations

import hashlib
import math
import re
from pathlib import Path
from typing import Any

import numpy as np
import onnx
from onnx import AttributeProto, TensorProto, numpy_helper

from .audit import AdmissionLimits, AuditPolicy, audit_model_bytes, canonical_json
from .errors import ErrorCode, HybridError

PROFILE_ID = "fp16-squeezenet-v1"
CHECKPOINT_SHA256 = "b8a52dc049b60e4b6ab68ad0df457362afab8b6304b2febdc1650a5dab4d7e7b"
NUMERICAL_POLICY = {"atol": 0.03, "rtol": 0.015}
NATIVE_POLICY = {"id": "whcn-spatial-ceil-equivalent-v1", "flatten": "explicit-reshape"}
CONTRACTS = {
    "20a": ([1, 3, 17, 13], [1, 64, 8, 6]),
    "20b": ([1, 64, 15, 19], [1, 64, 7, 9]),
    "20c": ([1, 64, 7, 9], [1, 128, 7, 9]),
    "20d": ([1, 512, 13, 13], [1, 1000]),
    "20e": ([1, 3, 224, 224], [1, 1000]),
}
_BYTE_BOUND = 8 * 1024 * 1024
_POLICY = AuditPolicy(
    policy_id="wave-squeezenet-import-v1-unqualified",
    policy_sha256=hashlib.sha256(b"wave-squeezenet-import-v1-unqualified").hexdigest(),
    compiler_manifest_sha256="0" * 64,
    compiler_qualified=False,
    approved_npu_signatures=frozenset(),
    limits=AdmissionLimits(
        max_nodes=96,
        max_inputs=1,
        max_outputs=1,
        max_initializers=192,
        max_rank=4,
        max_dimension=1000,
        max_initializer_bytes=_BYTE_BOUND,
        max_total_initializer_bytes=_BYTE_BOUND,
        max_tensor_bytes=_BYTE_BOUND,
    ),
)


def _require(ok: bool, message: str, code=ErrorCode.ONNX_GRAPH_INVALID) -> None:
    if not ok:
        raise HybridError(code, message)


def _keys(value: Any, keys: set[str], message: str) -> None:
    _require(isinstance(value, dict) and set(value) == keys, message)


def _shape(shape: Any, *, ranks=(2, 4)) -> None:
    _require(
        isinstance(shape, list)
        and len(shape) in ranks
        and all(type(d) is int and 1 <= d <= 1000 for d in shape)
        and shape[0] == 1
        and math.prod(shape) * 2 <= _BYTE_BOUND,
        "bounded static batch1 spatial shape required",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )


def native_shape(shape: list[int]) -> list[int]:
    return [shape[3], shape[2], shape[1], 1] if len(shape) == 4 else [shape[1], 1]


def _contract(name: str, shape: list[int]) -> dict[str, Any]:
    return {
        "name": name,
        "dtype": "FLOAT16",
        "shape": shape,
        "byte_size": math.prod(shape) * 2,
    }


def _io(value: onnx.ValueInfoProto) -> dict[str, Any]:
    _require(value.type.HasField("tensor_type"), "tensor activation required")
    t = value.type.tensor_type
    _require(
        t.elem_type == TensorProto.FLOAT16,
        "FP16 activation required",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        t.HasField("shape")
        and all(d.WhichOneof("value") == "dim_value" for d in t.shape.dim),
        "static activation dimensions required",
        ErrorCode.ONNX_SHAPE_UNSUPPORTED,
    )
    shape = [d.dim_value for d in t.shape.dim]
    _shape(shape)
    return _contract(value.name, shape)


def _attrs(node: onnx.NodeProto, allowed: dict[str, int]) -> dict[str, Any]:
    result = {}
    for a in node.attribute:
        _require(
            a.name in allowed and a.name not in result,
            "unknown or duplicate spatial attribute",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        _require(
            a.type == allowed[a.name] and not a.ref_attr_name,
            "invalid spatial attribute type",
        )
        result[a.name] = a.i if a.type == AttributeProto.INT else list(a.ints)
    return result


def _constant(t: TensorProto) -> dict[str, Any]:
    _require(
        t.data_type == TensorProto.FLOAT16,
        "FP16 constants required",
        ErrorCode.ONNX_TYPE_UNSUPPORTED,
    )
    _require(
        not t.HasField("segment")
        and t.data_location != TensorProto.EXTERNAL
        and not t.external_data,
        "inline constants required",
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
        if getattr(t, k)
    ]
    _require(len(fields) == 1, "exactly one constant payload required")
    _require(
        fields[0] in {"raw_data", "int32_data"},
        "FP16 constant requires raw bytes or uint16-valued int32 data",
    )
    elements = math.prod(t.dims)
    _require(
        len(t.dims) in (1, 4)
        and all(1 <= d <= 1000 for d in t.dims)
        and 2 * elements <= _BYTE_BOUND,
        "invalid constant dimensions",
    )
    _require(
        len(t.raw_data) == 2 * elements
        if fields[0] == "raw_data"
        else len(t.int32_data) == elements
        and all(0 <= value <= 65535 for value in t.int32_data),
        "invalid FP16 payload size or bit range",
    )
    a = numpy_helper.to_array(t)
    _require(
        a.dtype == np.dtype("<f2") and np.isfinite(a).all(),
        "finite FP16 constants required",
    )
    b = np.ascontiguousarray(a, dtype="<f2").tobytes()
    return {
        **_contract(t.name, list(t.dims)),
        "payload_hex": b.hex(),
        "sha256": hashlib.sha256(b).hexdigest(),
    }


def import_graph(path: Path) -> dict[str, Any]:
    """Admit the exact metadata-marked control recipe from audited model bytes."""
    try:
        with Path(path).open("rb") as handle:
            payload = handle.read(_POLICY.limits.max_model_bytes + 1)
    except OSError as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE, f"cannot read vision model: {exc}"
        ) from exc
    audit = audit_model_bytes(payload, source_name=Path(path).name, policy=_POLICY)
    m = onnx.load_model_from_string(payload)
    _require(
        m.ir_version == 8
        and len(m.opset_import) == 1
        and m.opset_import[0].domain in {"", "ai.onnx"}
        and m.opset_import[0].version == 13,
        "vision requires IR8 and standard opset13",
        ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
    )
    _require(
        not m.functions
        and not m.training_info
        and audit.document["graph_inventory"]["graphs"] == 1
        and audit.document["graph_inventory"]["attribute_tensors"] == 0,
        "nested graphs/functions/training/attribute constants unsupported",
    )
    metadata = {p.key: p.value for p in m.metadata_props}
    _require(
        len(metadata) == len(m.metadata_props)
        and set(metadata)
        == {"wave.native_profile", "wave.checkpoint_sha256", "wave.control"}
        and metadata["wave.native_profile"] == PROFILE_ID
        and metadata["wave.checkpoint_sha256"] == CHECKPOINT_SHA256
        and metadata["wave.control"] in CONTRACTS,
        "invalid fixed vision metadata",
        ErrorCode.EVIDENCE_INVALID,
    )
    g = m.graph
    _require(
        len(g.input) == len(g.output) == 1 and 1 <= len(g.node) <= 96,
        "vision requires one I/O and bounded nodes",
    )
    incoming, outgoing = _io(g.input[0]), _io(g.output[0])
    constants = {t.name: _constant(t) for t in g.initializer}
    _require(len(constants) == len(g.initializer), "duplicate initializer")
    shapes = {incoming["name"]: incoming["shape"]}
    _require(not set(shapes) & set(constants), "overridable constants unsupported")
    nodes = []
    for n in g.node:
        _require(
            n.domain in {"", "ai.onnx"}
            and n.op_type
            in {"Conv", "Relu", "MaxPool", "Concat", "GlobalAveragePool", "Flatten"},
            "unsupported vision operator",
            ErrorCode.ONNX_OPERATOR_UNSUPPORTED,
        )
        _require(
            len(n.output) == 1
            and n.output[0]
            and n.output[0] not in shapes
            and n.output[0] not in constants,
            "invalid node output",
        )
        _require(n.input and n.input[0] in shapes, "missing preceding activation")
        s = shapes[n.input[0]]
        node = {"type": n.op_type, "inputs": list(n.input), "output": n.output[0]}
        if n.op_type == "Conv":
            _require(
                len(n.input) == 3
                and n.input[1] in constants
                and n.input[2] in constants,
                "trained Conv requires inline weight and bias",
            )
            a = _attrs(
                n,
                {
                    k: AttributeProto.INTS
                    for k in ("pads", "strides", "dilations", "kernel_shape")
                }
                | {"group": AttributeProto.INT},
            )
            w = constants[n.input[1]]["shape"]
            _require(len(w) == 4 and len(s) == 4, "OIHW Conv and NCHW required")
            kernel, strides, dilations, pads = (
                a.get("kernel_shape", w[2:]),
                a.get("strides", [1, 1]),
                a.get("dilations", [1, 1]),
                a.get("pads", [0] * 4),
            )
            _require(
                len(kernel) == len(strides) == len(dilations) == 2
                and len(pads) == 4
                and all(v > 0 for v in (*kernel, *strides, *dilations)),
                "invalid Conv spatial parameters",
            )
            out = [
                1,
                w[0],
                *(
                    (
                        s[2 + i]
                        + pads[i]
                        + pads[i + 2]
                        - dilations[i] * (kernel[i] - 1)
                        - 1
                    )
                    // strides[i]
                    + 1
                    for i in range(2)
                ),
            ]
            node.update(
                inputs=[n.input[0]],
                input_shape=s,
                output_shape=out,
                weights=n.input[1],
                bias=n.input[2],
                kernel=kernel,
                strides=strides,
                dilations=dilations,
                pads=pads,
                group=a.get("group", 1),
            )
        elif n.op_type == "Concat":
            _require(
                len(n.input) == 2 and all(v in shapes for v in n.input),
                "Concat requires two preceding activations",
            )
            a = _attrs(n, {"axis": AttributeProto.INT})
            _require(
                a == {"axis": 1} and all(len(shapes[v]) == 4 for v in n.input),
                "Concat requires channel axis1",
            )
            out = [1, sum(shapes[v][1] for v in n.input), s[2], s[3]]
            node.update(
                input_shapes=[shapes[v] for v in n.input], output_shape=out, axis=1
            )
        else:
            _require(
                len(n.input) == 1 and len(s) == 4, "single NCHW activation required"
            )
            out = s[:]
            if n.op_type == "MaxPool":
                a = _attrs(
                    n,
                    {
                        k: AttributeProto.INTS
                        for k in ("pads", "strides", "dilations", "kernel_shape")
                    }
                    | {"ceil_mode": AttributeProto.INT},
                )
                kernel, strides, dilations, pads = (
                    a.get("kernel_shape"),
                    a.get("strides", [1, 1]),
                    a.get("dilations", [1, 1]),
                    a.get("pads", [0] * 4),
                )
                _require(
                    kernel == [3, 3]
                    and strides == [2, 2]
                    and dilations == [1, 1]
                    and pads == [0] * 4
                    and a.get("ceil_mode", 0) == 1,
                    "fixed MaxPool3 stride2 ceil1 required",
                )
                out = [1, s[1], (s[2] - 3) // 2 + 1, (s[3] - 3) // 2 + 1]
                node.update(
                    kernel=kernel,
                    strides=strides,
                    dilations=dilations,
                    pads=pads,
                    ceil_mode=1,
                )
            elif n.op_type == "Flatten":
                _require(
                    _attrs(n, {"axis": AttributeProto.INT}) == {"axis": 1},
                    "Flatten axis1 required",
                )
                out = [1, math.prod(s[1:])]
                node["axis"] = 1
            else:
                _require(not n.attribute, "unexpected unary attributes")
                if n.op_type == "GlobalAveragePool":
                    out = [1, s[1], 1, 1]
            node.update(input_shape=s, output_shape=out)
        _shape(out)
        shapes[n.output[0]] = out
        nodes.append(node)
    declared = [*g.input, *g.output, *g.value_info]
    _require(
        len({v.name for v in declared}) == len(declared),
        "duplicate declared activation",
    )
    for v in declared:
        c = _io(v)
        _require(
            v.name in shapes and c["shape"] == shapes[v.name],
            "declared shape disagrees with operation",
        )
    record = {
        "schema_version": 1,
        "profile_id": PROFILE_ID,
        "source_model_sha256": audit.model_sha256,
        "graph_signature": audit.graph_signature,
        "control_id": metadata["wave.control"],
        "checkpoint_sha256": CHECKPOINT_SHA256,
        "onnx_input": incoming,
        "onnx_output": outgoing,
        "native_input": _contract("input", native_shape(incoming["shape"])),
        "native_output": _contract("output", native_shape(outgoing["shape"])),
        "constants": constants,
        "nodes": nodes,
        "operation": {"type": "VisionGraph", "node_count": len(nodes)},
        "numerical_policy": dict(NUMERICAL_POLICY),
        "native_policy": dict(NATIVE_POLICY),
    }
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    validate_graph_record(record)
    return record


def _recipe(control: str) -> list[dict[str, Any]]:
    """Independent numeric recipe: input is -1, each output is its node index."""
    nodes = []

    def unary(typ, inp, **attrs):
        nodes.append({"type": typ, "inputs": [inp], **attrs})
        return len(nodes) - 1

    def conv(inp, co, kernel, stride=1, pad=0):
        c = unary(
            "Conv",
            inp,
            channels=co,
            kernel=[kernel, kernel],
            strides=[stride, stride],
            dilations=[1, 1],
            pads=[pad] * 4,
            group=1,
        )
        return unary("Relu", c)

    def pool(inp):
        return unary(
            "MaxPool",
            inp,
            kernel=[3, 3],
            strides=[2, 2],
            dilations=[1, 1],
            pads=[0] * 4,
            ceil_mode=1,
        )

    def fire(inp, squeeze, expand):
        s = conv(inp, squeeze, 1)
        a, b = conv(s, expand, 1), conv(s, expand, 3, pad=1)
        nodes.append({"type": "Concat", "inputs": [a, b], "axis": 1})
        return len(nodes) - 1

    def head(inp):
        return unary("Flatten", unary("GlobalAveragePool", conv(inp, 1000, 1)), axis=1)

    if control == "20a":
        conv(-1, 64, 3, stride=2)
    elif control == "20b":
        pool(-1)
    elif control == "20c":
        fire(-1, 16, 64)
    elif control == "20d":
        head(-1)
    else:
        last = pool(conv(-1, 64, 3, stride=2))
        for index, (s, e) in enumerate(
            (
                (16, 64),
                (16, 64),
                (32, 128),
                (32, 128),
                (48, 192),
                (48, 192),
                (64, 256),
                (64, 256),
            )
        ):
            last = fire(last, s, e)
            if index in (1, 3):
                last = pool(last)
        head(last)
    return nodes


def validate_graph_record(r: dict[str, Any]) -> None:
    """Reject altered policy, constants, edges, layout, topology and geometry."""
    _keys(
        r,
        {
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
        },
        "invalid vision record keys",
    )
    _require(
        type(r["schema_version"]) is int
        and r["schema_version"] == 1
        and r["profile_id"] == PROFILE_ID
        and isinstance(r["control_id"], str)
        and r["control_id"] in CONTRACTS
        and r["checkpoint_sha256"] == CHECKPOINT_SHA256,
        "invalid vision profile identity",
    )
    for k in ("source_model_sha256", "graph_signature", "lowering_sha256"):
        _require(
            isinstance(r[k], str) and re.fullmatch(r"[0-9a-f]{64}", r[k]) is not None,
            "invalid vision SHA",
        )
    _require(
        r["lowering_sha256"]
        == hashlib.sha256(
            canonical_json({k: v for k, v in r.items() if k != "lowering_sha256"})
        ).hexdigest(),
        "vision lowering identity mismatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    _require(
        r["numerical_policy"] == NUMERICAL_POLICY
        and r["native_policy"] == NATIVE_POLICY,
        "vision policy changed",
        ErrorCode.EVIDENCE_INVALID,
    )
    for index, direction in enumerate(("input", "output")):
        s = r[f"onnx_{direction}"]
        _keys(s, {"name", "dtype", "shape", "byte_size"}, "invalid I/O contract")
        _shape(s["shape"])
        _require(
            isinstance(s["name"], str)
            and 0 < len(s["name"]) <= 512
            and type(s["byte_size"]) is int
            and s == _contract(s["name"], s["shape"])
            and s["shape"] == CONTRACTS[r["control_id"]][index],
            "wrong fixed control I/O",
        )
        native = r[f"native_{direction}"]
        _keys(
            native, {"name", "dtype", "shape", "byte_size"}, "invalid native contract"
        )
        _require(
            type(native["byte_size"]) is int
            and isinstance(native["shape"], list)
            and all(type(v) is int for v in native["shape"])
            and native == _contract(direction, native_shape(s["shape"])),
            "native layout mismatch",
        )
    constants = r["constants"]
    _require(isinstance(constants, dict) and len(constants) <= 192, "invalid constants")
    for name, c in constants.items():
        _keys(
            c,
            {"name", "dtype", "shape", "byte_size", "payload_hex", "sha256"},
            "invalid constant keys",
        )
        _require(
            isinstance(name, str)
            and 0 < len(name) <= 512
            and c["name"] == name
            and c["dtype"] == "FLOAT16",
            "invalid constant identity",
        )
        s = c["shape"]
        _require(
            isinstance(s, list)
            and len(s) in (1, 4)
            and all(type(v) is int and 1 <= v <= 1000 for v in s)
            and type(c["byte_size"]) is int
            and c["byte_size"] == math.prod(s) * 2 <= _BYTE_BOUND,
            "invalid constant shape/size",
        )
        _require(
            isinstance(c["payload_hex"], str)
            and len(c["payload_hex"]) == c["byte_size"] * 2
            and re.fullmatch(r"[0-9a-f]+", c["payload_hex"]) is not None,
            "invalid constant payload",
            ErrorCode.EVIDENCE_INVALID,
        )
        b = bytes.fromhex(c["payload_hex"])
        _require(
            c["sha256"] == hashlib.sha256(b).hexdigest()
            and np.isfinite(np.frombuffer(b, dtype="<f2")).all(),
            "constant hash/finite mismatch",
            ErrorCode.EVIDENCE_INVALID,
        )
    _require(
        sum(c["byte_size"] for c in constants.values()) <= _BYTE_BOUND,
        "initializer byte bound exceeded",
    )
    nodes = r["nodes"]
    recipe = _recipe(r["control_id"])
    _require(
        isinstance(nodes, list) and len(nodes) == len(recipe) <= 96,
        "wrong fixed recipe node count",
    )
    _keys(r["operation"], {"type", "node_count"}, "invalid operation")
    _require(
        type(r["operation"]["node_count"]) is int
        and r["operation"] == {"type": "VisionGraph", "node_count": len(nodes)},
        "invalid operation count",
    )
    shapes = {r["onnx_input"]["name"]: r["onnx_input"]["shape"]}
    ids = {-1: r["onnx_input"]["name"]}
    _require(not set(shapes) & set(constants), "overridable constant")
    used = set()
    for index, (n, spec) in enumerate(zip(nodes, recipe, strict=True)):
        _require(
            isinstance(n, dict) and n.get("type") == spec["type"],
            "wrong recipe operation",
        )
        typ = n["type"]
        keys = {"type", "inputs", "output", "output_shape"} | (
            {"input_shapes"} if typ == "Concat" else {"input_shape"}
        )
        keys |= (
            {"weights", "bias", "kernel", "strides", "dilations", "pads", "group"}
            if typ == "Conv"
            else set()
        )
        keys |= (
            {"kernel", "strides", "dilations", "pads", "ceil_mode"}
            if typ == "MaxPool"
            else set()
        )
        keys |= {"axis"} if typ in {"Concat", "Flatten"} else set()
        _keys(n, keys, "invalid recipe node keys")
        _require(
            isinstance(n["inputs"], list)
            and n["inputs"] == [ids[v] for v in spec["inputs"]],
            "wrong recipe edge or branch order",
        )
        out = n["output"]
        _require(
            isinstance(out, str)
            and 0 < len(out) <= 512
            and out not in shapes
            and out not in constants,
            "duplicate recipe output",
        )
        s = shapes[n["inputs"][0]]
        _require(len(s) == 4, "rank4 recipe input required")
        _shape(n["output_shape"])
        for k, v in spec.items():
            if k in {"type", "inputs", "channels"}:
                continue
            actual = n[k]
            _require(
                actual == v
                and (
                    type(actual) is int
                    if type(v) is int
                    else isinstance(actual, list)
                    and all(type(t) is int for t in actual)
                ),
                "wrong recipe spatial parameter",
            )
        if typ == "Concat":
            ss = [shapes[v] for v in n["inputs"]]
            _require(
                isinstance(n["input_shapes"], list) and len(n["input_shapes"]) == 2,
                "invalid Concat input shape list",
            )
            for shape in n["input_shapes"]:
                _shape(shape, ranks=(4,))
            _require(
                n["input_shapes"] == ss
                and all(x[0] == s[0] and x[2:] == s[2:] for x in ss),
                "Concat spatial mismatch",
            )
            output = [1, sum(x[1] for x in ss), s[2], s[3]]
        else:
            _shape(n["input_shape"], ranks=(4,))
            _require(n["input_shape"] == s, "recipe input shape mismatch")
            output = s[:]
            if typ == "Conv":
                _require(
                    isinstance(n["weights"], str)
                    and n["weights"] in constants
                    and isinstance(n["bias"], str)
                    and n["bias"] in constants,
                    "missing trained Conv constants",
                )
                co = spec["channels"]
                _require(
                    constants[n["weights"]]["shape"] == [co, s[1], *n["kernel"]]
                    and constants[n["bias"]]["shape"] == [co],
                    "OIHW or bias shape mismatch",
                )
                output = [
                    1,
                    co,
                    *(
                        (s[2 + i] + n["pads"][i] + n["pads"][i + 2] - n["kernel"][i])
                        // n["strides"][i]
                        + 1
                        for i in range(2)
                    ),
                ]
                used.update((n["weights"], n["bias"]))
            elif typ == "MaxPool":
                _require(
                    all((extent - 3) % 2 == 0 for extent in s[2:]),
                    "ceil/floor equivalence required",
                )
                output = [1, s[1], *((extent - 3) // 2 + 1 for extent in s[2:])]
            elif typ == "GlobalAveragePool":
                _require(s == [1, 1000, 13, 13], "fixed classifier GAP required")
                output = [1, s[1], 1, 1]
            elif typ == "Flatten":
                _require(s == [1, 1000, 1, 1], "fixed post-GAP Flatten required")
                output = [1, 1000]
        _require(n["output_shape"] == output, "recipe output geometry mismatch")
        shapes[out], ids[index] = output, out
    _require(used == set(constants), "unused constants")
    _require(
        r["onnx_output"]["name"] == ids[len(nodes) - 1]
        and r["onnx_output"]["shape"] == shapes[ids[len(nodes) - 1]],
        "wrong recipe graph output",
    )
    if r["control_id"] == "20e":
        _require(
            sum(c["byte_size"] for c in constants.values()) == 2470992,
            "full trained coefficient accounting mismatch",
        )


def independent_graph_oracle(
    record: dict[str, Any], inputs: dict[str, np.ndarray]
) -> dict[str, np.ndarray]:
    """FP32 accumulation and one F16 rounding per declared tensor, without ORT."""
    validate_graph_record(record)
    spec = record["onnx_input"]
    _require(
        isinstance(inputs, dict) and set(inputs) == {spec["name"]},
        "vision input names mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    x = inputs[spec["name"]]
    _require(
        isinstance(x, np.ndarray)
        and x.dtype == np.dtype("<f2")
        and list(x.shape) == spec["shape"]
        and np.isfinite(x).all(),
        "vision input shape/type/finite mismatch",
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
            typ = n["type"]
            if typ == "Conv":
                pt, pl, pb, pr = n["pads"]
                padded = np.pad(x, ((0, 0), (0, 0), (pt, pb), (pl, pr)))
                windows = np.lib.stride_tricks.sliding_window_view(
                    padded, tuple(n["kernel"]), axis=(2, 3)
                )[:, :, :: n["strides"][0], :: n["strides"][1]]
                result = np.einsum(
                    "ockl,ncyxkl->noyx", constants[n["weights"]], windows, optimize=True
                )
                result += constants[n["bias"]][None, :, None, None]
            elif typ == "MaxPool":
                windows = np.lib.stride_tricks.sliding_window_view(
                    x, (3, 3), axis=(2, 3)
                )[:, :, ::2, ::2]
                result = windows.max(axis=(-2, -1))
            elif typ == "Concat":
                result = np.concatenate([values[v] for v in n["inputs"]], axis=1)
            elif typ == "GlobalAveragePool":
                result = np.mean(x, axis=(2, 3), keepdims=True, dtype=np.float32)
            elif typ == "Flatten":
                result = x.reshape(1, 1000)
            else:
                result = np.maximum(x, 0)
            result = np.ascontiguousarray(result, dtype="<f2")
            _require(
                list(result.shape) == n["output_shape"] and np.isfinite(result).all(),
                "nonfinite vision oracle activation",
                ErrorCode.ONNX_OUTPUT_INVALID,
            )
            values[n["output"]] = result
    name = record["onnx_output"]["name"]
    return {name: values[name]}


def validate_graph_output(
    record: dict[str, Any], expected: bytes, observed: bytes
) -> dict[str, Any]:
    """Compare actual FP16 readback against the immutable native tolerance."""
    validate_graph_record(record)
    size = record["onnx_output"]["byte_size"]
    _require(
        isinstance(expected, bytes)
        and isinstance(observed, bytes)
        and len(expected) == len(observed) == size,
        "vision output size mismatch",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    reference, actual = (
        np.frombuffer(b, dtype="<f2").astype(np.float32) for b in (expected, observed)
    )
    _require(
        np.isfinite(reference).all() and np.isfinite(actual).all(),
        "nonfinite vision output",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    _require(
        observed != b"\xa5" * size,
        "untouched output canary",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    errors = np.abs(actual - reference)
    bad = errors > NUMERICAL_POLICY["atol"] + NUMERICAL_POLICY["rtol"] * np.abs(
        reference
    )
    metrics = {
        "status": "FAIL" if bad.any() else "PASS",
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
            "vision output exceeds frozen tolerance",
            details=metrics,
        )
    return metrics
