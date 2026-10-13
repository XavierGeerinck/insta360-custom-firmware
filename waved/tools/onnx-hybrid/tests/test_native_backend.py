from __future__ import annotations

import copy
import hashlib
from pathlib import Path

import numpy as np
import pytest

from wave_onnx_hybrid.errors import ErrorCode, HybridError
from wave_onnx_hybrid.native_backend import NativeSession
from wave_onnx_hybrid.native_import import import_native_model, independent_oracle
from wave_onnx_hybrid.runtime import ExecutionMode

FIXTURES = Path(__file__).parents[1] / "fixtures/native-poc17"


class Cpu:
    def __init__(self, record):
        self.record = record
        self.providers = ["CPUExecutionProvider"]

    def run(self, inputs):
        return independent_oracle(self.record, inputs)


def setup_session(
    *, mode=ExecutionMode.STRICT, executor=None, loader=None, control="17b2"
):
    model = FIXTURES / f"{control}.onnx"
    record = import_native_model(model)
    with np.load(FIXTURES / f"{control}_input.npz") as archive:
        inputs = {name: archive[name].copy() for name in archive.files}
    package = {
        "directory": "/mock/package",
        "record": record,
        "package_sha256": "a" * 64,
        "package": {},
        "build": {"status": "built"},
        "compiled": {"status": "PASS", "nbg_sha256": "b" * 64},
        "qualification": {
            "status": "PASS",
            "nbg_sha256": "b" * 64,
            "package_sha256": "a" * 64,
            "model_sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
        },
    }
    calls = {"native": 0, "cpu": 0, "loads": 0}

    def load(*args, **kwargs):
        calls["loads"] += 1
        assert kwargs["require_qualification"] is True
        return copy.deepcopy(package)

    def execute(package_dir, values, run_id, **kwargs):
        calls["native"] += 1
        output = next(iter(independent_oracle(record, values).values())).tobytes()
        identity = {
            "package_sha256": package["package_sha256"],
            "source_model_sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
            "nbg_sha256": package["compiled"]["nbg_sha256"],
            "input_sha256": hashlib.sha256(
                next(iter(values.values())).tobytes()
            ).hexdigest(),
            "output_sha256": hashlib.sha256(output).hexdigest(),
        }
        envelope = {
            "native_identity": identity,
            "status": "PASS",
            "execution_attempts": 1,
            "errors": [],
            "health_differences": [],
            "kernel_alerts": [],
            "transport": {"remote_exit": 0, "timed_out": False},
            "result": {
                "input": list(next(iter(values.values())).tobytes()),
                "output": list(output),
                "output_hex": output.hex(),
                "iterations": 1,
                "graph_us": 25,
            },
        }
        return {
            "output_bytes": output,
            "envelope": envelope,
            "run_directory": "/mock/run",
        }

    def cpu_factory():
        calls["cpu"] += 1
        return Cpu(record)

    session = NativeSession(
        model,
        Path("/mock/package"),
        run_id="unit-test",
        mode=mode,
        executor=executor or execute,
        package_loader=loader or load,
        cpu_factory=cpu_factory,
    )
    return session, inputs, calls, package


def test_qualified_native_session_validates_output_and_evidence():
    session, inputs, calls, _ = setup_session()
    output = session.run(inputs)
    assert calls["native"] == 1
    assert calls["cpu"] == 0
    assert session.providers == ["WaveNativeBackend"]
    assert session.evidence["classification"] == "npu-strict"
    assert session.evidence["native_calls"] == 1
    np.testing.assert_array_equal(
        output["output"], np.array([[30, 24, 18, 27]], dtype=np.uint8)
    )


def test_failed_run_latches_cpu_without_second_native_attempt():
    attempted = []

    def failing(*args, **kwargs):
        attempted.append(True)
        raise RuntimeError("native run failed")

    session, inputs, calls, _ = setup_session(
        mode=ExecutionMode.HYBRID, executor=failing
    )
    first = session.run(inputs)
    second = session.run(inputs)
    assert len(attempted) == 1
    assert calls["cpu"] == 1
    np.testing.assert_array_equal(first["output"], second["output"])
    assert session.providers == ["CPUExecutionProvider"]
    assert session.evidence["classification"] == "cpu-native-execution-fallback"


def test_strict_execution_failure_never_constructs_cpu_or_retries():
    attempts = []

    def failing(*args, **kwargs):
        attempts.append(1)
        raise RuntimeError("native failed")

    session, inputs, calls, _ = setup_session(executor=failing)
    for _ in range(2):
        with pytest.raises(HybridError, match="native failed"):
            session.run(inputs)
    assert attempts == [1]
    assert calls["cpu"] == 0


def test_unqualified_package_refuses_before_executor():
    def unqualified(*args, **kwargs):
        raise HybridError(ErrorCode.NBG_SIGNATURE_NOT_APPROVED, "unqualified")

    with pytest.raises(HybridError, match="unqualified"):
        setup_session(loader=unqualified)


def test_unqualified_package_hybrid_is_cpu_preflight_fallback():
    def unqualified(*args, **kwargs):
        raise HybridError(ErrorCode.NBG_SIGNATURE_NOT_APPROVED, "unqualified")

    session, inputs, calls, _ = setup_session(
        mode=ExecutionMode.HYBRID, loader=unqualified
    )
    session.run(inputs)
    assert calls["native"] == 0
    assert session.evidence["classification"] == "cpu-native-preflight-fallback"


def test_package_change_after_construction_refuses_before_native():
    session, inputs, calls, package = setup_session()

    def changed(*args, **kwargs):
        changed_package = copy.deepcopy(package)
        changed_package["record"]["source_model_sha256"] = "f" * 64
        return changed_package

    session._loader = changed
    with pytest.raises(HybridError, match=r"package|model"):
        session.run(inputs)
    assert calls["native"] == 0


@pytest.mark.parametrize("bad", [b"", b"12345", b"123"])
def test_malformed_native_output_is_refused(bad):
    def malformed(*args, **kwargs):
        return {"output_bytes": bad, "envelope": {}, "run_directory": "/mock/run"}

    session, inputs, calls, _ = setup_session(executor=malformed)
    with pytest.raises(HybridError, match="output"):
        session.run(inputs)
    assert calls["cpu"] == 0


def test_bad_input_refuses_before_native():
    session, _, calls, _ = setup_session()
    with pytest.raises(HybridError, match="dtype"):
        session.run({"input": np.zeros((1, 16), dtype=np.float32)})
    assert calls["native"] == 0


def test_all_canary_oracle_refuses_before_native_and_hybrid_uses_cpu():
    session, inputs, calls, _ = setup_session(mode=ExecutionMode.HYBRID, control="17b1")
    inputs["input"][:] = 165
    expected = independent_oracle(session._record, inputs)["output"]
    assert expected.tobytes() == b"\xa5" * expected.nbytes
    output = session.run(inputs)
    assert calls["native"] == 0
    assert calls["cpu"] == 1
    np.testing.assert_array_equal(output["output"], expected)
    assert session.evidence["classification"] == "cpu-native-preflight-fallback"


def test_all_canary_oracle_strict_refuses_without_cpu():
    session, inputs, calls, _ = setup_session(control="17b1")
    inputs["input"][:] = 165
    with pytest.raises(HybridError, match="canary"):
        session.run(inputs)
    assert calls["native"] == 0
    assert calls["cpu"] == 0


def test_unsupported_model_hybrid_uses_existing_cpu_contract(tmp_path, monkeypatch):
    from wave_onnx_hybrid import native_backend
    from wave_onnx_hybrid.fixtures import build_fixture_models, serialize_model

    monkeypatch.setattr(
        native_backend,
        "native_bridge",
        lambda: pytest.fail("unsupported graph must not load native bridge"),
    )

    model = tmp_path / "unsupported.onnx"
    model.write_bytes(serialize_model(build_fixture_models()["dynamic_identity.onnx"]))
    session = NativeSession(
        model, tmp_path / "absent", run_id="unsupported", mode=ExecutionMode.HYBRID
    )
    inputs = {"input": np.arange(3, dtype=np.float32).reshape(1, 1, 3)}
    # Derive the published input name from the CPU admission document.
    inputs = {session.audit.document["inputs"][0]["name"]: next(iter(inputs.values()))}
    result = session.run(inputs)
    np.testing.assert_array_equal(
        next(iter(result.values())), next(iter(inputs.values()))
    )
    assert session.evidence["native_calls"] == 0
    assert session.evidence["native_eligibility"]["importable"] is False
