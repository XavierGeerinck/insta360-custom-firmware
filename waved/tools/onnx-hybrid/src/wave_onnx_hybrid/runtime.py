"""CPU oracle and fail-closed hybrid session construction."""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any, Protocol

import numpy as np
import onnxruntime as ort

from .audit import AuditResult
from .errors import ErrorCode, HybridError


class ExecutionMode(StrEnum):
    AUDIT = "audit"
    CPU = "cpu"
    HYBRID = "hybrid"
    STRICT = "strict"


class RunnableSession(Protocol):
    def run(self, inputs: Mapping[str, np.ndarray]) -> dict[str, np.ndarray]: ...

    @property
    def providers(self) -> list[str]: ...


@dataclass(frozen=True)
class SessionSelection:
    session: Any
    classification: str
    npu_attempts: int
    cpu_sessions: int
    npu_error: str | None


@dataclass(frozen=True)
class RunResult:
    outputs: dict[str, np.ndarray]
    evidence: dict[str, Any]


def _construct_cpu(cpu_factory: Callable[[], Any]) -> Any:
    try:
        return cpu_factory()
    except HybridError:
        raise
    except Exception as exc:
        raise HybridError(
            ErrorCode.CPU_FALLBACK_FAILURE,
            f"CPU session construction failed: {exc}",
        ) from exc


def construct_session(
    *,
    mode: ExecutionMode,
    npu_eligible: bool,
    cpu_factory: Callable[[], Any],
    npu_factory: Callable[[], Any] | None,
) -> SessionSelection:
    """Apply one-attempt session construction without executing inference."""

    if mode is ExecutionMode.AUDIT:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "audit mode does not construct an inference session",
        )
    if mode is ExecutionMode.CPU:
        return SessionSelection(
            session=_construct_cpu(cpu_factory),
            classification="cpu-explicit",
            npu_attempts=0,
            cpu_sessions=1,
            npu_error=None,
        )
    if not npu_eligible:
        if mode is ExecutionMode.STRICT:
            raise HybridError(
                ErrorCode.PARTITION_NOT_NPU_RESIDENT,
                "strict mode requires an approved NPU-resident partition",
            )
        return SessionSelection(
            session=_construct_cpu(cpu_factory),
            classification="cpu-preflight-fallback",
            npu_attempts=0,
            cpu_sessions=1,
            npu_error=None,
        )
    if npu_factory is None:
        raise HybridError(
            ErrorCode.PARTITION_COMPILE_FAILURE,
            "eligible partition has no NPU session factory",
        )
    try:
        npu_session = npu_factory()
    except Exception as exc:
        if mode is ExecutionMode.STRICT:
            raise HybridError(
                ErrorCode.PARTITION_COMPILE_FAILURE,
                f"NPU session construction failed: {exc}",
            ) from exc
        return SessionSelection(
            session=_construct_cpu(cpu_factory),
            classification="cpu-session-construction-fallback",
            npu_attempts=1,
            cpu_sessions=1,
            npu_error=str(exc),
        )
    return SessionSelection(
        session=npu_session,
        classification=("npu-strict" if mode is ExecutionMode.STRICT else "npu-hybrid"),
        npu_attempts=1,
        cpu_sessions=0,
        npu_error=None,
    )


NUMPY_DTYPE_BY_ONNX_NAME = {
    "FLOAT": np.dtype(np.float32),
    "UINT8": np.dtype(np.uint8),
    "INT8": np.dtype(np.int8),
    "UINT16": np.dtype(np.uint16),
    "INT16": np.dtype(np.int16),
    "INT32": np.dtype(np.int32),
    "INT64": np.dtype(np.int64),
    "BOOL": np.dtype(np.bool_),
    "FLOAT16": np.dtype(np.float16),
    "DOUBLE": np.dtype(np.float64),
    "UINT32": np.dtype(np.uint32),
    "UINT64": np.dtype(np.uint64),
    "COMPLEX64": np.dtype(np.complex64),
    "COMPLEX128": np.dtype(np.complex128),
}


def _validate_array(
    name: str,
    array: Any,
    specification: Mapping[str, Any],
    *,
    code: ErrorCode,
    max_tensor_bytes: int,
) -> np.ndarray:
    if not isinstance(array, np.ndarray):
        raise HybridError(code, f"tensor {name!r} must be a NumPy array")
    if not array.flags.c_contiguous:
        raise HybridError(code, f"tensor {name!r} must be C-contiguous")
    expected_dtype = NUMPY_DTYPE_BY_ONNX_NAME.get(specification.get("dtype"))
    if expected_dtype is None or array.dtype != expected_dtype:
        raise HybridError(
            code,
            f"tensor {name!r} dtype {array.dtype} does not match "
            f"{specification.get('dtype')}",
        )
    expected_shape = specification.get("shape")
    if not isinstance(expected_shape, list) or array.ndim != len(expected_shape):
        raise HybridError(code, f"tensor {name!r} rank does not match model")
    for index, (observed, expected) in enumerate(
        zip(array.shape, expected_shape, strict=True)
    ):
        if isinstance(expected, int) and observed != expected:
            raise HybridError(
                code,
                f"tensor {name!r} dimension {index} is {observed}, expected {expected}",
            )
        if observed < 0:
            raise HybridError(code, f"tensor {name!r} has a negative dimension")
    if array.nbytes > max_tensor_bytes:
        raise HybridError(code, f"tensor {name!r} exceeds byte limit")
    if np.issubdtype(array.dtype, np.floating) and not np.isfinite(array).all():
        raise HybridError(code, f"tensor {name!r} contains non-finite values")
    return array


class _OrtCpuSession:
    def __init__(self, model_payload: bytes, output_names: list[str]) -> None:
        options = ort.SessionOptions()
        options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
        options.graph_optimization_level = (
            ort.GraphOptimizationLevel.ORT_ENABLE_EXTENDED
        )
        options.intra_op_num_threads = 1
        options.inter_op_num_threads = 1
        options.enable_mem_pattern = True
        options.enable_cpu_mem_arena = True
        self._session = ort.InferenceSession(
            model_payload,
            sess_options=options,
            providers=["CPUExecutionProvider"],
        )
        self._providers = self._session.get_providers()
        if self._providers != ["CPUExecutionProvider"]:
            raise HybridError(
                ErrorCode.CPU_FALLBACK_FAILURE,
                f"unexpected ONNX Runtime providers: {self._providers}",
            )
        self._output_names = output_names

    @property
    def providers(self) -> list[str]:
        return list(self._providers)

    def run(self, inputs: Mapping[str, np.ndarray]) -> dict[str, np.ndarray]:
        try:
            values = self._session.run(
                self._output_names,
                dict(inputs),
            )
        except Exception as exc:
            raise HybridError(
                ErrorCode.CPU_FALLBACK_FAILURE,
                f"CPU inference failed: {exc}",
            ) from exc
        return dict(zip(self._output_names, values, strict=True))


def _tensor_evidence(array: np.ndarray) -> dict[str, Any]:
    payload = np.ascontiguousarray(array).tobytes(order="C")
    return {
        "dtype": str(array.dtype),
        "shape": list(array.shape),
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def execute_cpu_model(
    model_path: Path | str,
    audit: AuditResult,
    inputs: Mapping[str, np.ndarray],
    *,
    mode: ExecutionMode,
) -> RunResult:
    """Execute the CPU oracle under explicit CPU or fail-closed hybrid mode."""

    if mode is ExecutionMode.AUDIT:
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "use the auditor directly for audit mode",
        )
    path = Path(model_path)
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise HybridError(
            ErrorCode.ONNX_PARSE_FAILURE,
            f"cannot read model {path}: {exc}",
        ) from exc
    observed_hash = hashlib.sha256(payload).hexdigest()
    if observed_hash != audit.model_sha256:
        raise HybridError(
            ErrorCode.ONNX_GRAPH_INVALID,
            "model changed after audit",
        )
    expected_inputs = {item["name"]: item for item in audit.document["inputs"]}
    if set(inputs) != set(expected_inputs):
        raise HybridError(
            ErrorCode.ONNX_INPUT_INVALID,
            f"input names must be exactly {sorted(expected_inputs)}",
        )
    limits = audit.document["policy"]["limits"]
    validated_inputs = {
        name: _validate_array(
            name,
            inputs[name],
            specification,
            code=ErrorCode.ONNX_INPUT_INVALID,
            max_tensor_bytes=limits["max_tensor_bytes"],
        )
        for name, specification in expected_inputs.items()
    }
    output_specs = {item["name"]: item for item in audit.document["outputs"]}
    output_names = list(output_specs)
    npu_eligible = audit.document["summary"]["npu_partitions"] > 0
    selection = construct_session(
        mode=mode,
        npu_eligible=npu_eligible,
        cpu_factory=lambda: _OrtCpuSession(payload, output_names),
        npu_factory=None,
    )
    if not isinstance(selection.session, _OrtCpuSession):
        raise HybridError(
            ErrorCode.PARTITION_NOT_NPU_RESIDENT,
            "this host-only runner cannot execute an NPU session",
        )
    outputs = selection.session.run(validated_inputs)
    if set(outputs) != set(output_specs):
        raise HybridError(
            ErrorCode.ONNX_OUTPUT_INVALID,
            f"output names differ from model: {sorted(outputs)}",
        )
    validated_outputs = {
        name: _validate_array(
            name,
            outputs[name],
            specification,
            code=ErrorCode.ONNX_OUTPUT_INVALID,
            max_tensor_bytes=limits["max_tensor_bytes"],
        )
        for name, specification in output_specs.items()
    }
    evidence = {
        "schema_version": 1,
        "mode": mode.value,
        "classification": selection.classification,
        "model_sha256": audit.model_sha256,
        "graph_signature": audit.graph_signature,
        "providers": selection.session.providers,
        "onnxruntime_version": ort.__version__,
        "session_options": {
            "execution_mode": "ORT_SEQUENTIAL",
            "graph_optimization_level": "ORT_ENABLE_EXTENDED",
            "intra_op_num_threads": 1,
            "inter_op_num_threads": 1,
        },
        "npu_attempts": selection.npu_attempts,
        "cpu_sessions": selection.cpu_sessions,
        "npu_error": selection.npu_error,
        "inputs": {
            name: _tensor_evidence(array) for name, array in validated_inputs.items()
        },
        "outputs": {
            name: _tensor_evidence(array) for name, array in validated_outputs.items()
        },
        "non_claims": [
            "No NPU provider, compiler, graph, driver, or device was used.",
            (
                "Hybrid mode is CPU preflight fallback while no exact NPU "
                "graph signature is approved."
            ),
        ],
    }
    return RunResult(outputs=validated_outputs, evidence=evidence)
