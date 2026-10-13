"""Explicit, qualification-bound Wave native session adapter."""

from __future__ import annotations

import hashlib
import sys
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

import numpy as np

from .audit import audit_model_path, load_policy
from .errors import ErrorCode, HybridError
from .native_import import import_native_model, independent_oracle
from .runtime import ExecutionMode, _OrtCpuSession, _validate_array

REPOSITORY_ROOT = Path(__file__).resolve().parents[5]
DEFAULT_POLICY = REPOSITORY_ROOT / "docs/npu/onnx/hybrid-policy.json"
_DTYPES = {"FLOAT16": np.dtype("<f2"), "UINT8": np.dtype("u1")}


def native_bridge() -> Any:
    """Load the repository lab bridge only when explicit native work is requested."""
    script_dir = REPOSITORY_ROOT / "waved/scripts/npu"
    if str(script_dir) not in sys.path:
        sys.path.insert(0, str(script_dir))
    import npu_onnx_native_lab

    return npu_onnx_native_lab


def _digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _failure(code: ErrorCode, message: str) -> HybridError:
    return HybridError(code, message)


class NativeSession:
    """One supported ONNX graph, with a latched CPU fallback in hybrid mode."""

    def __init__(
        self,
        model_path: Path | str,
        package_dir: Path | str,
        *,
        run_id: str,
        mode: ExecutionMode,
        executor: Callable[..., dict[str, Any]] | None = None,
        package_loader: Callable[..., dict[str, Any]] | None = None,
        cpu_factory: Callable[[], Any] | None = None,
    ) -> None:
        self.model_path = Path(model_path)
        self.package_dir = Path(package_dir)
        self.run_id = run_id
        self.mode = ExecutionMode(mode)
        if self.mode not in (ExecutionMode.STRICT, ExecutionMode.HYBRID):
            raise _failure(
                ErrorCode.POLICY_INVALID,
                "native backend requires strict or hybrid mode",
            )
        self.audit = audit_model_path(
            self.model_path,
            policy=load_policy(DEFAULT_POLICY, repository_root=REPOSITORY_ROOT),
        )
        self._model_payload = self.model_path.read_bytes()
        if _digest(self._model_payload) != self.audit.model_sha256:
            raise _failure(ErrorCode.ONNX_GRAPH_INVALID, "model changed after audit")
        self._executor = executor
        self._loader = package_loader
        self._cpu_factory = cpu_factory or (
            lambda: _OrtCpuSession(
                self._model_payload,
                [item["name"] for item in self.audit.document["outputs"]],
            )
        )
        self._cpu: Any | None = None
        self._latched_error: HybridError | None = None
        self._package: dict[str, Any] | None = None
        self._record: dict[str, Any] | None = None
        self._native_attempts = 0
        self._native_calls: int | None = 0
        self._cpu_sessions = 0
        self._classification = "native-preflight"
        self._last_envelope: dict[str, Any] | None = None
        try:
            self._record = import_native_model(self.model_path)
            self._package = self._load_checked()
        except Exception as exc:
            error = self._as_error(exc, "native package preflight failed")
            if self.mode is ExecutionMode.STRICT:
                raise error from exc
            self._latched_error = error
            self._classification = "cpu-native-preflight-fallback"

    @staticmethod
    def _as_error(
        exc: Exception,
        context: str,
        code: ErrorCode = ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
    ) -> HybridError:
        if isinstance(exc, HybridError):
            return exc
        return _failure(code, f"{context}: {exc}")

    def _load_checked(self) -> dict[str, Any]:
        loader = self._loader or native_bridge().load_package
        package = loader(self.package_dir, require_qualification=True)
        record = self._record
        if record is None or not isinstance(package, dict):
            raise _failure(
                ErrorCode.MODEL_PACKAGE_INVALID, "native package record is missing"
            )
        stored = package.get("record")
        compiled = package.get("compiled")
        qualification = package.get("qualification")
        if (
            stored != record
            or Path(package.get("directory", "/invalid")).resolve()
            != self.package_dir.resolve()
            or not isinstance(compiled, dict)
            or not isinstance(qualification, dict)
            or compiled.get("status") != "PASS"
            or qualification.get("status") != "PASS"
            or qualification.get("nbg_sha256") != compiled.get("nbg_sha256")
            or qualification.get("package_sha256") != package.get("package_sha256")
            or qualification.get("model_sha256") != record["source_model_sha256"]
            or qualification.get("lowering_sha256") != record["lowering_sha256"]
        ):
            raise _failure(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "native package/model qualification differs",
            )
        if _digest(self.model_path.read_bytes()) != record["source_model_sha256"]:
            raise _failure(
                ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
                "ONNX model changed after native admission",
            )
        return package

    @property
    def providers(self) -> list[str]:
        if self._cpu is not None or (
            self._latched_error is not None and self.mode is ExecutionMode.HYBRID
        ):
            return ["CPUExecutionProvider"]
        return ["WaveNativeBackend"]

    @property
    def evidence(self) -> dict[str, Any]:
        package = self._package or {}
        return {
            "schema_version": 1,
            "mode": self.mode.value,
            "classification": self._classification,
            "providers": self.providers,
            "native_calls": self._native_calls,
            "native_attempts": self._native_attempts,
            "cpu_sessions": self._cpu_sessions,
            "npu_error": str(self._latched_error) if self._latched_error else None,
            "native_eligibility": {
                "importable": self._record is not None,
                "compiled": package.get("compiled") is not None,
                "qualified": package.get("qualification") is not None,
            },
            "model_sha256": self.audit.model_sha256,
            "graph_signature": self.audit.graph_signature,
            "package_sha256": package.get("package_sha256"),
            "nbg_sha256": (package.get("compiled") or {}).get("nbg_sha256"),
            "run_id": self.run_id if self._native_attempts else None,
            "native_run": self._last_envelope,
        }

    def _validate_inputs(
        self, inputs: Mapping[str, np.ndarray]
    ) -> dict[str, np.ndarray]:
        specifications = (
            [self._record["onnx_input"]]
            if self._record
            else self.audit.document["inputs"]
        )
        expected = {spec["name"]: spec for spec in specifications}
        if set(inputs) != set(expected):
            raise _failure(
                ErrorCode.ONNX_INPUT_INVALID,
                f"input names must be exactly {sorted(expected)}",
            )
        limit = self.audit.document["policy"]["limits"]["max_tensor_bytes"]
        return {
            name: _validate_array(
                name,
                inputs[name],
                spec,
                code=ErrorCode.ONNX_INPUT_INVALID,
                max_tensor_bytes=limit,
            )
            for name, spec in expected.items()
        }

    def _run_cpu(self, inputs: dict[str, np.ndarray]) -> dict[str, np.ndarray]:
        if self._cpu is None:
            try:
                self._cpu = self._cpu_factory()
                self._cpu_sessions += 1
            except Exception as exc:
                raise _failure(
                    ErrorCode.CPU_FALLBACK_FAILURE,
                    f"CPU fallback construction failed: {exc}",
                ) from exc
        outputs = self._cpu.run(inputs)
        specifications = {spec["name"]: spec for spec in self.audit.document["outputs"]}
        if not isinstance(outputs, dict) or set(outputs) != set(specifications):
            raise _failure(
                ErrorCode.ONNX_OUTPUT_INVALID, "CPU fallback output names differ"
            )
        limit = self.audit.document["policy"]["limits"]["max_tensor_bytes"]
        return {
            name: _validate_array(
                name,
                outputs[name],
                spec,
                code=ErrorCode.ONNX_OUTPUT_INVALID,
                max_tensor_bytes=limit,
            )
            for name, spec in specifications.items()
        }

    def _validate_result(
        self, result: Any, package: dict[str, Any], inputs: dict[str, np.ndarray]
    ) -> dict[str, np.ndarray]:
        if not isinstance(result, dict) or not isinstance(
            result.get("output_bytes"), bytes
        ):
            raise _failure(
                ErrorCode.ONNX_OUTPUT_INVALID, "native output bytes are missing"
            )
        record = self._record
        assert record is not None
        spec = record["onnx_output"]
        dtype = _DTYPES[spec["dtype"]]
        expected_bytes = int(np.prod(spec["shape"])) * dtype.itemsize
        payload = result["output_bytes"]
        if len(payload) != expected_bytes:
            raise _failure(
                ErrorCode.ONNX_OUTPUT_INVALID,
                "native output byte count differs from ONNX contract",
            )
        expected = independent_oracle(record, inputs)[spec["name"]].tobytes(order="C")
        if payload != expected:
            raise _failure(
                ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
                "native output differs from independent ONNX oracle",
            )
        envelope = result.get("envelope")
        if not isinstance(envelope, dict):
            raise _failure(
                ErrorCode.EVIDENCE_INVALID, "native execution envelope is missing"
            )
        identity = envelope.get("native_identity")
        input_bytes = next(iter(inputs.values())).tobytes(order="C")
        expected_identity = {
            "package_sha256": package["package_sha256"],
            "source_model_sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
            "nbg_sha256": package["compiled"]["nbg_sha256"],
            "input_sha256": _digest(input_bytes),
            "output_sha256": _digest(payload),
        }
        run_result = envelope.get("result")
        transport = envelope.get("transport")
        if not isinstance(run_result, dict) or not isinstance(transport, dict):
            raise _failure(
                ErrorCode.EVIDENCE_INVALID, "native run result/transport is missing"
            )
        if (
            identity != expected_identity
            or envelope.get("status") != "PASS"
            or envelope.get("execution_attempts") != 1
            or envelope.get("errors") != []
            or envelope.get("health_differences") != []
            or envelope.get("kernel_alerts") != []
            or transport.get("remote_exit") != 0
            or transport.get("timed_out") is not False
            or run_result.get("output") != list(payload)
            or run_result.get("input") != list(input_bytes)
            or run_result.get("iterations") != 1
            or run_result.get("output_hex") != payload.hex()
            or not isinstance(run_result.get("graph_us"), int)
        ):
            raise _failure(
                ErrorCode.EVIDENCE_INVALID,
                "native execution output/evidence identity differs",
            )
        self._last_envelope = envelope
        self._native_calls = 1
        return {
            spec["name"]: np.frombuffer(payload, dtype=dtype)
            .copy()
            .reshape(spec["shape"])
        }

    def run(self, inputs: Mapping[str, np.ndarray]) -> dict[str, np.ndarray]:
        values = self._validate_inputs(inputs)
        if self._latched_error is not None:
            if self.mode is ExecutionMode.STRICT:
                raise self._latched_error
            return self._run_cpu(values)
        try:
            package = self._load_checked()
        except Exception as exc:
            error = self._as_error(exc, "native package changed")
            if self.mode is ExecutionMode.STRICT:
                raise error from exc
            self._latched_error = error
            self._classification = "cpu-native-preflight-fallback"
            return self._run_cpu(values)
        assert self._record is not None
        expected_output = next(iter(independent_oracle(self._record, values).values()))
        if expected_output.tobytes(order="C") == b"\xa5" * expected_output.nbytes:
            error = _failure(
                ErrorCode.ONNX_OUTPUT_INVALID,
                "native output would equal the unread-output canary",
            )
            if self.mode is ExecutionMode.STRICT:
                raise error
            self._latched_error = error
            self._classification = "cpu-native-preflight-fallback"
            return self._run_cpu(values)
        try:
            executor = self._executor or native_bridge().execute_package
            self._native_attempts += 1
            result = executor(
                self.package_dir, values, self.run_id, require_qualification=True
            )
            outputs = self._validate_result(result, package, values)
        except Exception as exc:
            error = self._as_error(
                exc, "native execution failed", ErrorCode.PARTITION_COMPILE_FAILURE
            )
            self._native_calls = None
            self._latched_error = error
            if self.mode is ExecutionMode.STRICT:
                raise error from exc
            self._classification = "cpu-native-execution-fallback"
            return self._run_cpu(values)
        self._classification = (
            "npu-strict" if self.mode is ExecutionMode.STRICT else "npu-hybrid"
        )
        return outputs
