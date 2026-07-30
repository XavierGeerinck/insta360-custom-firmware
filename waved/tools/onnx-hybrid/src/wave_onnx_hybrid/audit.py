"""Strict ONNX admission, graph signing, and fail-closed placement."""

from __future__ import annotations

import hashlib
import json
import math
import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import onnx
from onnx import AttributeProto, ModelProto, TensorProto

from .errors import ErrorCode, HybridError

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
POLICY_ROOT_KEYS = frozenset(
    {
        "schema_version",
        "policy_id",
        "compiler_manifest",
        "compiler_qualified",
        "approved_npu_signatures",
        "limits",
    }
)
COMPILER_REFERENCE_KEYS = frozenset({"path", "sha256"})
LIMIT_KEYS = frozenset(
    {
        "max_model_bytes",
        "max_nodes",
        "max_inputs",
        "max_outputs",
        "max_initializers",
        "max_initializer_bytes",
        "max_total_initializer_bytes",
        "max_rank",
        "max_dimension",
        "max_tensor_bytes",
    }
)

FUTURE_CANDIDATE_OPERATORS = frozenset(
    {
        "Add",
        "AveragePool",
        "BatchNormalization",
        "Clip",
        "Concat",
        "Conv",
        "DequantizeLinear",
        "Div",
        "Flatten",
        "Gather",
        "Gemm",
        "GlobalAveragePool",
        "Identity",
        "LeakyRelu",
        "MatMul",
        "MaxPool",
        "Mul",
        "QuantizeLinear",
        "Relu",
        "Reshape",
        "Sigmoid",
        "Softmax",
        "Split",
        "Sub",
        "Tanh",
        "Transpose",
        "Unsqueeze",
    }
)
FUTURE_CANDIDATE_DTYPES = frozenset(
    {
        TensorProto.FLOAT,
        TensorProto.FLOAT16,
        TensorProto.INT8,
        TensorProto.UINT8,
        TensorProto.INT32,
    }
)
QUANTIZATION_OPERATORS = frozenset(
    {"QuantizeLinear", "DequantizeLinear", "QLinearConv", "QLinearMatMul"}
)

DTYPE_BYTES = {
    TensorProto.FLOAT: 4,
    TensorProto.UINT8: 1,
    TensorProto.INT8: 1,
    TensorProto.UINT16: 2,
    TensorProto.INT16: 2,
    TensorProto.INT32: 4,
    TensorProto.INT64: 8,
    TensorProto.BOOL: 1,
    TensorProto.FLOAT16: 2,
    TensorProto.DOUBLE: 8,
    TensorProto.UINT32: 4,
    TensorProto.UINT64: 8,
    TensorProto.COMPLEX64: 8,
    TensorProto.COMPLEX128: 16,
    TensorProto.BFLOAT16: 2,
    TensorProto.FLOAT8E4M3FN: 1,
    TensorProto.FLOAT8E4M3FNUZ: 1,
    TensorProto.FLOAT8E5M2: 1,
    TensorProto.FLOAT8E5M2FNUZ: 1,
}


@dataclass(frozen=True)
class AdmissionLimits:
    max_model_bytes: int = 64 * 1024 * 1024
    max_nodes: int = 4096
    max_inputs: int = 64
    max_outputs: int = 64
    max_initializers: int = 4096
    max_initializer_bytes: int = 64 * 1024 * 1024
    max_total_initializer_bytes: int = 256 * 1024 * 1024
    max_rank: int = 8
    max_dimension: int = 1 << 20
    max_tensor_bytes: int = 256 * 1024 * 1024

    def __post_init__(self) -> None:
        for name, value in self.to_document().items():
            if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
                raise HybridError(
                    ErrorCode.POLICY_INVALID,
                    f"limit {name} must be a positive integer",
                )

    def to_document(self) -> dict[str, int]:
        return {
            "max_model_bytes": self.max_model_bytes,
            "max_nodes": self.max_nodes,
            "max_inputs": self.max_inputs,
            "max_outputs": self.max_outputs,
            "max_initializers": self.max_initializers,
            "max_initializer_bytes": self.max_initializer_bytes,
            "max_total_initializer_bytes": self.max_total_initializer_bytes,
            "max_rank": self.max_rank,
            "max_dimension": self.max_dimension,
            "max_tensor_bytes": self.max_tensor_bytes,
        }

    @classmethod
    def from_document(cls, document: Mapping[str, Any]) -> AdmissionLimits:
        _require_exact_keys(
            document, LIMIT_KEYS, "policy.limits", ErrorCode.POLICY_INVALID
        )
        return cls(**dict(document))


@dataclass(frozen=True)
class AuditPolicy:
    policy_id: str
    policy_sha256: str
    compiler_manifest_sha256: str
    compiler_qualified: bool
    approved_npu_signatures: frozenset[str]
    limits: AdmissionLimits


@dataclass(frozen=True)
class AuditResult:
    model_sha256: str
    graph_signature: str
    document: dict[str, Any]


def canonical_json(value: Any) -> bytes:
    """Return canonical evidence JSON and reject non-finite numbers."""

    try:
        return (
            json.dumps(
                value,
                allow_nan=False,
                ensure_ascii=False,
                indent=2,
                sort_keys=True,
            )
            + "\n"
        ).encode()
    except (TypeError, ValueError) as exc:
        raise HybridError(
            ErrorCode.EVIDENCE_INVALID,
            f"cannot serialize canonical evidence: {exc}",
        ) from exc


def _reject_duplicate_keys(pairs: Sequence[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json(path: Path, code: ErrorCode) -> Mapping[str, Any]:
    try:
        document = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_reject_duplicate_keys,
        )
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise HybridError(code, f"cannot load {path}: {exc}") from exc
    if not isinstance(document, Mapping):
        raise HybridError(code, f"{path} root must be an object")
    return document


def _require_exact_keys(
    value: Mapping[str, Any],
    expected: frozenset[str],
    location: str,
    code: ErrorCode,
) -> None:
    actual = frozenset(value)
    if actual == expected:
        return
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    details: list[str] = []
    if missing:
        details.append(f"missing keys {missing}")
    if unknown:
        details.append(f"unknown keys {unknown}")
    raise HybridError(code, f"{location}: {', '.join(details)}")


def _contained_path(root: Path, path: Path, location: str) -> Path:
    root_resolved = root.resolve()
    resolved = path.resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            f"{location} resolves outside the repository",
        ) from exc
    return resolved


def load_policy(
    path: Path | str,
    *,
    repository_root: Path | str,
) -> AuditPolicy:
    """Load a policy and bind it to the current compiler refusal manifest."""

    policy_path = Path(path)
    repository = Path(repository_root).resolve()
    _contained_path(repository, policy_path, "policy path")
    try:
        policy_payload = policy_path.read_bytes()
    except OSError as exc:
        raise HybridError(
            ErrorCode.POLICY_INVALID, f"cannot read policy: {exc}"
        ) from exc
    document = _load_json(policy_path, ErrorCode.POLICY_INVALID)
    _require_exact_keys(document, POLICY_ROOT_KEYS, "policy", ErrorCode.POLICY_INVALID)
    if document["schema_version"] != 1:
        raise HybridError(ErrorCode.POLICY_INVALID, "policy.schema_version must be 1")
    policy_id = document["policy_id"]
    if not isinstance(policy_id, str) or not policy_id:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.policy_id must be a non-empty string",
        )

    compiler_reference = document["compiler_manifest"]
    if not isinstance(compiler_reference, Mapping):
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.compiler_manifest must be an object",
        )
    _require_exact_keys(
        compiler_reference,
        COMPILER_REFERENCE_KEYS,
        "policy.compiler_manifest",
        ErrorCode.POLICY_INVALID,
    )
    relative_text = compiler_reference["path"]
    if not isinstance(relative_text, str) or not relative_text:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.compiler_manifest.path must be a non-empty string",
        )
    relative_path = Path(relative_text)
    if relative_path.is_absolute() or ".." in relative_path.parts:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.compiler_manifest.path must be repository-relative",
        )
    compiler_path = _contained_path(
        repository,
        repository / relative_path,
        "policy.compiler_manifest.path",
    )
    expected_hash = compiler_reference["sha256"]
    if not isinstance(expected_hash, str) or SHA256_RE.fullmatch(expected_hash) is None:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.compiler_manifest.sha256 must be lowercase SHA-256",
        )
    try:
        compiler_payload = compiler_path.read_bytes()
    except OSError as exc:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            f"cannot read compiler manifest: {exc}",
        ) from exc
    observed_hash = hashlib.sha256(compiler_payload).hexdigest()
    if observed_hash != expected_hash:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "compiler manifest SHA-256 mismatch",
        )
    compiler_document = _load_json(compiler_path, ErrorCode.POLICY_INVALID)
    authorization = compiler_document.get("authorization")
    compiler_authorized = (
        isinstance(authorization, Mapping)
        and authorization.get("compiler_execution") is True
    )

    compiler_qualified = document["compiler_qualified"]
    if not isinstance(compiler_qualified, bool):
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.compiler_qualified must be boolean",
        )
    signatures = document["approved_npu_signatures"]
    if not isinstance(signatures, list):
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy.approved_npu_signatures must be an array",
        )
    approved: list[str] = []
    for index, signature in enumerate(signatures):
        if not isinstance(signature, str) or SHA256_RE.fullmatch(signature) is None:
            raise HybridError(
                ErrorCode.POLICY_INVALID,
                f"approved_npu_signatures[{index}] must be lowercase SHA-256",
            )
        approved.append(signature)
    if approved != sorted(set(approved)):
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "approved_npu_signatures must be unique and sorted",
        )
    if compiler_qualified and not compiler_authorized:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "policy cannot qualify a manifest that refuses compiler execution",
        )
    if approved and not compiler_qualified:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "unqualified compiler policy cannot approve NPU signatures",
        )

    limits_document = document["limits"]
    if not isinstance(limits_document, Mapping):
        raise HybridError(ErrorCode.POLICY_INVALID, "policy.limits must be an object")
    return AuditPolicy(
        policy_id=policy_id,
        policy_sha256=hashlib.sha256(policy_payload).hexdigest(),
        compiler_manifest_sha256=expected_hash,
        compiler_qualified=compiler_qualified,
        approved_npu_signatures=frozenset(approved),
        limits=AdmissionLimits.from_document(limits_document),
    )


def _dtype_name(element_type: int) -> str:
    try:
        return TensorProto.DataType.Name(element_type)
    except ValueError:
        return f"UNKNOWN_{element_type}"


def _tensor_value_document(
    value_info: onnx.ValueInfoProto,
    limits: AdmissionLimits,
) -> dict[str, Any]:
    value_type = value_info.type
    if not value_type.HasField("tensor_type"):
        return {
            "name": value_info.name,
            "kind": "non-tensor",
            "dtype": None,
            "dtype_id": None,
            "shape": None,
            "static": False,
            "estimated_bytes": None,
        }
    tensor_type = value_type.tensor_type
    element_type = tensor_type.elem_type
    dimensions: list[int | dict[str, str] | None] = []
    static = True
    if tensor_type.HasField("shape"):
        if len(tensor_type.shape.dim) > limits.max_rank:
            raise HybridError(
                ErrorCode.ONNX_MODEL_LIMIT,
                f"value {value_info.name!r} exceeds max rank",
            )
        for dimension in tensor_type.shape.dim:
            kind = dimension.WhichOneof("value")
            if kind == "dim_value":
                value = dimension.dim_value
                if value < 0 or value > limits.max_dimension:
                    raise HybridError(
                        ErrorCode.ONNX_MODEL_LIMIT,
                        f"value {value_info.name!r} has invalid dimension {value}",
                    )
                dimensions.append(value)
            elif kind == "dim_param":
                dimensions.append({"symbol": dimension.dim_param})
                static = False
            else:
                dimensions.append(None)
                static = False
    else:
        static = False

    estimated_bytes: int | None = None
    width = DTYPE_BYTES.get(element_type)
    if static and width is not None:
        element_count = 1
        for dimension in dimensions:
            assert isinstance(dimension, int)
            element_count *= dimension
            if element_count * width > limits.max_tensor_bytes:
                raise HybridError(
                    ErrorCode.ONNX_MODEL_LIMIT,
                    f"value {value_info.name!r} exceeds tensor-byte limit",
                )
        estimated_bytes = element_count * width

    return {
        "name": value_info.name,
        "kind": "tensor",
        "dtype": _dtype_name(element_type),
        "dtype_id": element_type,
        "shape": dimensions,
        "static": static,
        "estimated_bytes": estimated_bytes,
    }


def _walk_graphs(
    root: onnx.GraphProto,
) -> list[tuple[str, onnx.GraphProto, int]]:
    """Return every graph and attribute subgraph in deterministic DFS order."""

    result: list[tuple[str, onnx.GraphProto, int]] = []
    pending: list[tuple[str, onnx.GraphProto, int]] = [("graph", root, 0)]
    while pending:
        path, graph, depth = pending.pop()
        result.append((path, graph, depth))
        children: list[tuple[str, onnx.GraphProto, int]] = []
        for node_index, node in enumerate(graph.node):
            node_name = node.name or f"node-{node_index:04d}"
            for attribute in node.attribute:
                attribute_name = attribute.name or "unnamed"
                if attribute.type == AttributeProto.GRAPH:
                    children.append(
                        (
                            f"{path}/{node_name}:{attribute_name}",
                            attribute.g,
                            depth + 1,
                        )
                    )
                elif attribute.type == AttributeProto.GRAPHS:
                    children.extend(
                        (
                            f"{path}/{node_name}:{attribute_name}[{graph_index}]",
                            child,
                            depth + 1,
                        )
                        for graph_index, child in enumerate(attribute.graphs)
                    )
        pending.extend(reversed(children))
    return result


def _graph_constant_tensors(
    graph_path: str,
    graph: onnx.GraphProto,
) -> list[tuple[str, TensorProto]]:
    """Return initializers and tensor-valued attributes owned by one graph."""

    tensors: list[tuple[str, TensorProto]] = []
    tensors.extend(
        (f"{graph_path}.initializer[{index}]", tensor)
        for index, tensor in enumerate(graph.initializer)
    )
    for index, sparse in enumerate(graph.sparse_initializer):
        tensors.append(
            (f"{graph_path}.sparse_initializer[{index}].values", sparse.values)
        )
        tensors.append(
            (f"{graph_path}.sparse_initializer[{index}].indices", sparse.indices)
        )
    for node_index, node in enumerate(graph.node):
        node_name = node.name or f"node-{node_index:04d}"
        for attribute in node.attribute:
            prefix = f"{graph_path}/{node_name}:{attribute.name or 'unnamed'}"
            if attribute.type == AttributeProto.TENSOR:
                tensors.append((prefix, attribute.t))
            elif attribute.type == AttributeProto.TENSORS:
                tensors.extend(
                    (f"{prefix}[{index}]", tensor)
                    for index, tensor in enumerate(attribute.tensors)
                )
            elif attribute.type == AttributeProto.SPARSE_TENSOR:
                tensors.append((f"{prefix}.values", attribute.sparse_tensor.values))
                tensors.append((f"{prefix}.indices", attribute.sparse_tensor.indices))
            elif attribute.type == AttributeProto.SPARSE_TENSORS:
                for index, sparse in enumerate(attribute.sparse_tensors):
                    tensors.append((f"{prefix}[{index}].values", sparse.values))
                    tensors.append((f"{prefix}[{index}].indices", sparse.indices))
    return tensors


def _reject_external_data(model: ModelProto) -> None:
    for graph_path, graph, _depth in _walk_graphs(model.graph):
        for location, tensor in _graph_constant_tensors(graph_path, graph):
            if tensor.data_location == TensorProto.EXTERNAL or tensor.external_data:
                raise HybridError(
                    ErrorCode.ONNX_EXTERNAL_DATA,
                    f"external tensor data is forbidden at {location}",
                )


def _initializer_document(
    tensor: TensorProto,
    limits: AdmissionLimits,
) -> dict[str, Any]:
    if len(tensor.dims) > limits.max_rank:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            f"initializer {tensor.name!r} exceeds max rank",
        )
    for dimension in tensor.dims:
        if dimension < 0 or dimension > limits.max_dimension:
            raise HybridError(
                ErrorCode.ONNX_MODEL_LIMIT,
                f"initializer {tensor.name!r} has invalid dimension",
            )
    normalized = TensorProto()
    normalized.CopyFrom(tensor)
    normalized.ClearField("name")
    normalized.ClearField("doc_string")
    normalized.ClearField("external_data")
    normalized.data_location = TensorProto.DEFAULT
    payload = normalized.SerializeToString(deterministic=True)
    if len(payload) > limits.max_initializer_bytes:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            f"initializer {tensor.name!r} exceeds byte limit",
        )
    return {
        "name": tensor.name,
        "dtype": _dtype_name(tensor.data_type),
        "dtype_id": tensor.data_type,
        "shape": list(tensor.dims),
        "encoded_bytes": len(payload),
        "payload_sha256": hashlib.sha256(payload).hexdigest(),
    }


def _graph_inventory_document(
    model: ModelProto,
    limits: AdmissionLimits,
) -> dict[str, int]:
    """Bound and summarize the complete graph/subgraph tree."""

    graphs = _walk_graphs(model.graph)
    total_nodes = 0
    constant_tensors: list[tuple[str, TensorProto]] = []
    graph_initializers = 0
    attribute_tensors = 0
    for graph_path, graph, _depth in graphs:
        total_nodes += len(graph.node)
        if len(graph.input) > limits.max_inputs:
            raise HybridError(
                ErrorCode.ONNX_MODEL_LIMIT,
                f"{graph_path} exceeds input limit",
            )
        if len(graph.output) > limits.max_outputs:
            raise HybridError(
                ErrorCode.ONNX_MODEL_LIMIT,
                f"{graph_path} exceeds output limit",
            )
        node_names = [node.name for node in graph.node if node.name]
        if len(node_names) != len(set(node_names)):
            raise HybridError(
                ErrorCode.ONNX_GRAPH_INVALID,
                f"{graph_path} has duplicate non-empty node names",
            )
        owned_tensors = _graph_constant_tensors(graph_path, graph)
        constant_tensors.extend(owned_tensors)
        graph_tensor_count = len(graph.initializer) + 2 * len(graph.sparse_initializer)
        graph_initializers += graph_tensor_count
        attribute_tensors += len(owned_tensors) - graph_tensor_count

    if total_nodes > limits.max_nodes:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            "model graph tree exceeds node limit",
        )
    if len(constant_tensors) > limits.max_initializers:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            "model graph tree exceeds constant-tensor limit",
        )
    constant_tensor_bytes = sum(
        _initializer_document(tensor, limits)["encoded_bytes"]
        for _location, tensor in constant_tensors
    )
    if constant_tensor_bytes > limits.max_total_initializer_bytes:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            "model graph tree constant tensors exceed total byte limit",
        )
    top_level_nodes = len(model.graph.node)
    return {
        "graphs": len(graphs),
        "max_depth": max(depth for _path, _graph, depth in graphs),
        "top_level_nodes": top_level_nodes,
        "nested_nodes": total_nodes - top_level_nodes,
        "total_nodes": total_nodes,
        "graph_initializer_tensors": graph_initializers,
        "attribute_tensors": attribute_tensors,
        "constant_tensors": len(constant_tensors),
        "constant_tensor_bytes": constant_tensor_bytes,
    }


def _float_hex(value: float, location: str) -> str:
    if not math.isfinite(value):
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID,
            f"non-finite float at {location}",
        )
    return float(value).hex()


def _normalized_tensor_payload(
    tensor: TensorProto,
    *,
    clear_name: bool,
) -> bytes:
    normalized = TensorProto()
    normalized.CopyFrom(tensor)
    if clear_name:
        normalized.ClearField("name")
    normalized.ClearField("doc_string")
    normalized.ClearField("external_data")
    normalized.data_location = TensorProto.DEFAULT
    return normalized.SerializeToString(deterministic=True)


def _normalized_sparse_tensor_payload(sparse: onnx.SparseTensorProto) -> bytes:
    normalized = onnx.SparseTensorProto()
    normalized.CopyFrom(sparse)
    for tensor in (normalized.values, normalized.indices):
        tensor.ClearField("name")
        tensor.ClearField("doc_string")
        tensor.ClearField("external_data")
        tensor.data_location = TensorProto.DEFAULT
    return normalized.SerializeToString(deterministic=True)


def _normalized_graph_payload(graph: onnx.GraphProto) -> bytes:
    normalized = onnx.GraphProto()
    normalized.CopyFrom(graph)
    for _path, nested, _depth in _walk_graphs(normalized):
        nested.ClearField("doc_string")
        for value in list(nested.input) + list(nested.output) + list(nested.value_info):
            value.ClearField("doc_string")
        for tensor in nested.initializer:
            tensor.ClearField("doc_string")
            tensor.ClearField("external_data")
            tensor.data_location = TensorProto.DEFAULT
        for sparse in nested.sparse_initializer:
            for tensor in (sparse.values, sparse.indices):
                tensor.ClearField("doc_string")
                tensor.ClearField("external_data")
                tensor.data_location = TensorProto.DEFAULT
        for node in nested.node:
            node.ClearField("doc_string")
            for attribute in node.attribute:
                attribute.ClearField("doc_string")
                if attribute.type == AttributeProto.TENSOR:
                    attribute.t.ClearField("name")
                    attribute.t.ClearField("doc_string")
                elif attribute.type == AttributeProto.TENSORS:
                    for tensor in attribute.tensors:
                        tensor.ClearField("name")
                        tensor.ClearField("doc_string")
    return normalized.SerializeToString(deterministic=True)


def _attribute_document(attribute: AttributeProto) -> dict[str, Any]:
    attribute_type = attribute.type
    value: Any
    if attribute_type == AttributeProto.FLOAT:
        value = _float_hex(attribute.f, attribute.name)
    elif attribute_type == AttributeProto.INT:
        value = attribute.i
    elif attribute_type == AttributeProto.STRING:
        value = attribute.s.hex()
    elif attribute_type == AttributeProto.FLOATS:
        value = [
            _float_hex(item, f"{attribute.name}[{index}]")
            for index, item in enumerate(attribute.floats)
        ]
    elif attribute_type == AttributeProto.INTS:
        value = list(attribute.ints)
    elif attribute_type == AttributeProto.STRINGS:
        value = [item.hex() for item in attribute.strings]
    elif attribute_type == AttributeProto.TENSOR:
        payload = _normalized_tensor_payload(attribute.t, clear_name=True)
        value = {
            "protobuf_bytes": len(payload),
            "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
        }
    elif attribute_type == AttributeProto.TENSORS:
        value = []
        for tensor in attribute.tensors:
            payload = _normalized_tensor_payload(tensor, clear_name=True)
            value.append(
                {
                    "protobuf_bytes": len(payload),
                    "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
                }
            )
    elif attribute_type == AttributeProto.GRAPH:
        payload = _normalized_graph_payload(attribute.g)
        value = {
            "protobuf_bytes": len(payload),
            "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
        }
    elif attribute_type == AttributeProto.GRAPHS:
        value = []
        for graph in attribute.graphs:
            payload = _normalized_graph_payload(graph)
            value.append(
                {
                    "protobuf_bytes": len(payload),
                    "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
                }
            )
    elif attribute_type == AttributeProto.SPARSE_TENSOR:
        payload = _normalized_sparse_tensor_payload(attribute.sparse_tensor)
        value = {
            "protobuf_bytes": len(payload),
            "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
        }
    elif attribute_type == AttributeProto.SPARSE_TENSORS:
        value = []
        for sparse in attribute.sparse_tensors:
            payload = _normalized_sparse_tensor_payload(sparse)
            value.append(
                {
                    "protobuf_bytes": len(payload),
                    "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
                }
            )
    else:
        normalized = AttributeProto()
        normalized.CopyFrom(attribute)
        normalized.ClearField("name")
        normalized.ClearField("doc_string")
        payload = normalized.SerializeToString(deterministic=True)
        value = {
            "protobuf_bytes": len(payload),
            "protobuf_sha256": hashlib.sha256(payload).hexdigest(),
        }
    return {
        "name": attribute.name,
        "type": AttributeProto.AttributeType.Name(attribute_type),
        "value": value,
    }


def _value_map(
    graph: onnx.GraphProto,
    limits: AdmissionLimits,
) -> dict[str, dict[str, Any]]:
    values: dict[str, dict[str, Any]] = {}
    for value_info in list(graph.input) + list(graph.value_info) + list(graph.output):
        document = _tensor_value_document(value_info, limits)
        existing = values.get(value_info.name)
        if existing is not None and existing != document:
            raise HybridError(
                ErrorCode.ONNX_GRAPH_INVALID,
                f"conflicting value metadata for {value_info.name!r}",
            )
        values[value_info.name] = document
    return values


def _graph_signature_document(
    model: ModelProto,
    inputs: list[dict[str, Any]],
    outputs: list[dict[str, Any]],
    initializers: list[dict[str, Any]],
) -> dict[str, Any]:
    nodes: list[dict[str, Any]] = []
    for index, node in enumerate(model.graph.node):
        nodes.append(
            {
                "index": index,
                "name": node.name,
                "domain": node.domain or "ai.onnx",
                "op_type": node.op_type,
                "inputs": list(node.input),
                "outputs": list(node.output),
                "attributes": sorted(
                    (_attribute_document(item) for item in node.attribute),
                    key=lambda item: item["name"],
                ),
            }
        )
    return {
        "schema_version": 1,
        "ir_version": model.ir_version,
        "opsets": sorted(
            (
                {
                    "domain": item.domain or "ai.onnx",
                    "version": item.version,
                }
                for item in model.opset_import
            ),
            key=lambda item: item["domain"],
        ),
        "inputs": inputs,
        "outputs": outputs,
        "initializers": initializers,
        "nodes": nodes,
    }


def _node_reasons(
    node: onnx.NodeProto,
    values: Mapping[str, dict[str, Any]],
    initializers: Mapping[str, dict[str, Any]],
    *,
    compiler_qualified: bool,
    graph_approved: bool,
) -> list[str]:
    reasons: set[str] = set()
    if (node.domain or "ai.onnx") != "ai.onnx":
        reasons.add(ErrorCode.ONNX_OPERATOR_UNSUPPORTED.value)
    if node.op_type not in FUTURE_CANDIDATE_OPERATORS:
        reasons.add(ErrorCode.ONNX_OPERATOR_UNSUPPORTED.value)
    if node.op_type in QUANTIZATION_OPERATORS:
        reasons.add(ErrorCode.ONNX_QUANTIZATION_UNSUPPORTED.value)

    for name in list(node.input) + list(node.output):
        if not name or name in initializers:
            continue
        value = values.get(name)
        if value is None or value["kind"] != "tensor":
            reasons.add(ErrorCode.ONNX_TYPE_UNSUPPORTED.value)
            reasons.add(ErrorCode.ONNX_SHAPE_UNSUPPORTED.value)
            continue
        if value["dtype_id"] not in FUTURE_CANDIDATE_DTYPES:
            reasons.add(ErrorCode.ONNX_TYPE_UNSUPPORTED.value)
        if not value["static"]:
            reasons.add(ErrorCode.ONNX_SHAPE_UNSUPPORTED.value)

    if not graph_approved:
        reasons.add(ErrorCode.PARTITION_NOT_PREFLIGHTED.value)
    if not compiler_qualified:
        reasons.add("COMPILER_DEPENDENCY_MISMATCH")
    return sorted(reasons)


def _partition_documents(
    node_documents: list[dict[str, Any]],
    graph: onnx.GraphProto,
    value_documents: Mapping[str, dict[str, Any]],
    initializer_names: set[str],
) -> list[dict[str, Any]]:
    if not node_documents:
        return []
    partitions: list[dict[str, Any]] = []
    current_nodes: list[dict[str, Any]] = []
    current_placement = node_documents[0]["placement"]
    for node in node_documents:
        if node["placement"] != current_placement:
            partitions.append(
                {
                    "index": len(partitions),
                    "placement": current_placement,
                    "nodes": [item["index"] for item in current_nodes],
                }
            )
            current_nodes = []
            current_placement = node["placement"]
        current_nodes.append(node)
    partitions.append(
        {
            "index": len(partitions),
            "placement": current_placement,
            "nodes": [item["index"] for item in current_nodes],
        }
    )

    graph_outputs = {item.name for item in graph.output}
    producer: dict[str, int] = {}
    consumers: dict[str, set[int]] = {}
    for node in node_documents:
        for output in node["outputs"]:
            if output:
                producer[output] = node["index"]
        for input_name in node["inputs"]:
            if input_name:
                consumers.setdefault(input_name, set()).add(node["index"])
    node_to_partition = {
        node_index: partition["index"]
        for partition in partitions
        for node_index in partition["nodes"]
    }
    for partition in partitions:
        partition_nodes = set(partition["nodes"])
        boundary_inputs: set[str] = set()
        boundary_outputs: set[str] = set()
        for node_index in partition_nodes:
            node = node_documents[node_index]
            for name in node["inputs"]:
                if not name or name in initializer_names:
                    continue
                source_node = producer.get(name)
                if source_node is None or source_node not in partition_nodes:
                    boundary_inputs.add(name)
            for name in node["outputs"]:
                if not name:
                    continue
                outside_consumer = any(
                    consumer not in partition_nodes
                    for consumer in consumers.get(name, set())
                )
                if outside_consumer or name in graph_outputs:
                    boundary_outputs.add(name)
        partition["inputs"] = sorted(boundary_inputs)
        partition["outputs"] = sorted(boundary_outputs)
        partition["input_bytes"] = sum(
            value_documents.get(name, {}).get("estimated_bytes") or 0
            for name in boundary_inputs
        )
        partition["output_bytes"] = sum(
            value_documents.get(name, {}).get("estimated_bytes") or 0
            for name in boundary_outputs
        )
        partition["upstream_partitions"] = sorted(
            {
                node_to_partition[source]
                for name in boundary_inputs
                if (source := producer.get(name)) is not None
                and node_to_partition[source] != partition["index"]
            }
        )
    return partitions


def audit_model_bytes(
    payload: bytes,
    *,
    source_name: str,
    policy: AuditPolicy,
) -> AuditResult:
    """Validate, infer, sign, and place a bounded in-memory ONNX model."""

    if not isinstance(payload, bytes):
        raise HybridError(ErrorCode.ONNX_PARSE_FAILURE, "model payload must be bytes")
    if not payload:
        raise HybridError(ErrorCode.ONNX_PARSE_FAILURE, "model payload is empty")
    if len(payload) > policy.limits.max_model_bytes:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            f"model exceeds {policy.limits.max_model_bytes} bytes",
        )
    model_hash = hashlib.sha256(payload).hexdigest()
    try:
        model = onnx.load_model_from_string(payload)
    except Exception as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE, f"cannot parse ONNX: {exc}"
        ) from exc
    if not model.HasField("graph"):
        raise HybridError(ErrorCode.ONNX_GRAPH_INVALID, "model has no graph")
    _reject_external_data(model)
    if model.functions or model.training_info:
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID,
            "local functions and training_info are outside this PoC",
        )
    graph = model.graph
    limits = policy.limits
    graph_inventory = _graph_inventory_document(model, limits)
    if graph.sparse_initializer:
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID,
            "sparse initializers are outside this PoC",
        )
    try:
        onnx.checker.check_model(model, full_check=True)
    except Exception as exc:
        raise HybridError(
            ErrorCode.ONNX_CHECK_FAILURE, f"ONNX checker rejected model: {exc}"
        ) from exc
    try:
        inferred = onnx.shape_inference.infer_shapes(
            model,
            check_type=True,
            strict_mode=True,
            data_prop=False,
        )
    except Exception as exc:
        raise HybridError(
            ErrorCode.ONNX_CHECK_FAILURE,
            f"strict shape inference failed: {exc}",
        ) from exc
    graph = inferred.graph

    initializer_documents = [
        _initializer_document(tensor, limits)
        for tensor in sorted(graph.initializer, key=lambda item: item.name)
    ]
    total_initializer_bytes = sum(
        item["encoded_bytes"] for item in initializer_documents
    )
    if total_initializer_bytes > limits.max_total_initializer_bytes:
        raise HybridError(
            ErrorCode.ONNX_MODEL_LIMIT,
            "total initializer bytes exceed limit",
        )
    initializer_names = {item["name"] for item in initializer_documents}
    if len(initializer_names) != len(initializer_documents):
        raise HybridError(ErrorCode.ONNX_GRAPH_INVALID, "duplicate initializer names")

    values = _value_map(graph, limits)
    input_documents = [
        values[item.name] for item in graph.input if item.name not in initializer_names
    ]
    output_documents = [values[item.name] for item in graph.output]
    signature_document = _graph_signature_document(
        inferred,
        input_documents,
        output_documents,
        initializer_documents,
    )
    graph_signature = hashlib.sha256(canonical_json(signature_document)).hexdigest()
    graph_approved = (
        policy.compiler_qualified and graph_signature in policy.approved_npu_signatures
    )

    node_documents: list[dict[str, Any]] = []
    for index, node in enumerate(graph.node):
        reasons = _node_reasons(
            node,
            values,
            {name: {} for name in initializer_names},
            compiler_qualified=policy.compiler_qualified,
            graph_approved=graph_approved,
        )
        placement = "npu" if graph_approved and not reasons else "cpu"
        node_documents.append(
            {
                "index": index,
                "name": node.name or f"node-{index:04d}",
                "domain": node.domain or "ai.onnx",
                "op_type": node.op_type,
                "inputs": list(node.input),
                "outputs": list(node.output),
                "placement": placement,
                "reasons": reasons,
            }
        )
    partitions = _partition_documents(
        node_documents,
        graph,
        values,
        initializer_names,
    )
    cpu_nodes = sum(node["placement"] == "cpu" for node in node_documents)
    npu_nodes = len(node_documents) - cpu_nodes
    cpu_partitions = sum(partition["placement"] == "cpu" for partition in partitions)
    npu_partitions = len(partitions) - cpu_partitions
    document = {
        "schema_version": 1,
        "tool": {
            "name": "wave-onnx-hybrid",
            "component": "capability-auditor",
            "onnx_version": onnx.__version__,
        },
        "source_model": {
            "name": source_name,
            "bytes": len(payload),
            "sha256": model_hash,
        },
        "policy": {
            "policy_id": policy.policy_id,
            "sha256": policy.policy_sha256,
            "compiler_manifest_sha256": (policy.compiler_manifest_sha256),
            "compiler_qualified": policy.compiler_qualified,
            "approved_signature_count": len(policy.approved_npu_signatures),
            "limits": policy.limits.to_document(),
        },
        "graph_signature": graph_signature,
        "signature_document": signature_document,
        "graph_inventory": graph_inventory,
        "inputs": input_documents,
        "outputs": output_documents,
        "initializers": initializer_documents,
        "nodes": node_documents,
        "partitions": partitions,
        "summary": {
            "cpu_admissible": True,
            "cpu_nodes": cpu_nodes,
            "cpu_partitions": cpu_partitions,
            "npu_nodes": npu_nodes,
            "npu_partitions": npu_partitions,
        },
        "non_claims": sorted(
            [
                "No compiler or NPU provider was loaded.",
                "No graph was compiled or executed by this audit.",
                "Operator names do not establish Wave NPU support.",
                (
                    "All nodes remain CPU until an exact graph signature and "
                    "compiler/profile combination are separately approved."
                ),
            ]
        ),
    }
    canonical_json(document)
    return AuditResult(
        model_sha256=model_hash,
        graph_signature=graph_signature,
        document=document,
    )


def audit_model_path(
    path: Path | str,
    *,
    policy: AuditPolicy,
) -> AuditResult:
    model_path = Path(path)
    try:
        with model_path.open("rb") as handle:
            payload = handle.read(policy.limits.max_model_bytes + 1)
    except OSError as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE,
            f"cannot read model {model_path}: {exc}",
        ) from exc
    return audit_model_bytes(
        payload,
        source_name=model_path.name,
        policy=policy,
    )
