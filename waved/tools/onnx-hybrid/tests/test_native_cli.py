from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

from wave_onnx_hybrid import cli, native_backend, native_cli


def test_lifecycle_commands_dispatch_exact_arguments(monkeypatch, tmp_path):
    calls = []

    class Bridge:
        def import_package(self, *args):
            calls.append(("import", args))

        def build_package(self, *args):
            calls.append(("build", args))

        def compile_package(self, *args):
            calls.append(("compile", args))

        def qualify_package(self, *args):
            calls.append(("qualify", args))

    monkeypatch.setattr(native_cli, "native_bridge", lambda: Bridge())
    model = tmp_path / "model.onnx"
    package = tmp_path / "package"
    inputs = tmp_path / "input.npz"
    assert cli.main(["import-native", str(model), "--package-dir", str(package)]) == 0
    assert cli.main(["build-native", str(package)]) == 0
    assert cli.main(["compile-native", str(package), "--run-id", "compile-1"]) == 0
    assert (
        cli.main(
            [
                "qualify-native",
                str(package),
                "--inputs",
                str(inputs),
                "--run-id",
                "qualify-1",
            ]
        )
        == 0
    )
    assert calls == [
        ("import", (model, package)),
        ("build", (package,)),
        ("compile", (package, "compile-1")),
        ("qualify", (package, inputs, "qualify-1")),
    ]


def _run_args(tmp_path: Path) -> list[str]:
    model = tmp_path / "model.onnx"
    model.write_bytes(b"mock")
    inputs = tmp_path / "input.npz"
    np.savez(inputs, input=np.arange(16, dtype=np.uint8).reshape(1, 16))
    return [
        "run",
        str(model),
        "--backend",
        "wave-native",
        "--package",
        str(tmp_path / "package"),
        "--inputs",
        str(inputs),
        "--mode",
        "strict",
        "--outputs",
        str(tmp_path / "out.npz"),
        "--evidence",
        str(tmp_path / "evidence.json"),
        "--run-id",
        "run-1",
    ]


def test_native_run_writes_actual_backend_evidence(monkeypatch, tmp_path):
    calls = []

    class Session:
        def __init__(self, model, package, *, run_id, mode):
            calls.append((model, package, run_id, mode))
            self.audit = type("Audit", (), {"document": {"model": "mock"}})()
            self.evidence = {
                "classification": "npu-strict",
                "providers": ["WaveNativeBackend"],
                "native_calls": 1,
                "cpu_sessions": 0,
            }

        def run(self, inputs):
            assert set(inputs) == {"input"}
            return {"output": np.array([[1, 2, 3, 4]], dtype=np.uint8)}

    monkeypatch.setattr(native_backend, "NativeSession", Session)
    args = _run_args(tmp_path)
    assert cli.main(args) == 0
    assert len(calls) == 1
    with np.load(tmp_path / "out.npz") as archive:
        np.testing.assert_array_equal(archive["output"], [[1, 2, 3, 4]])
    evidence = json.loads((tmp_path / "evidence.json").read_text())
    assert evidence["execution"]["providers"] == ["WaveNativeBackend"]
    assert evidence["execution"]["native_calls"] == 1


@pytest.mark.parametrize(
    "change",
    [
        lambda args: args.__setitem__(
            args.index("--outputs") + 1, args[args.index("--inputs") + 1]
        ),
        lambda args: args.__setitem__(
            args.index("--evidence") + 1, args[args.index("--outputs") + 1]
        ),
        lambda args: args.__setitem__(args.index("--run-id") + 1, "../invalid"),
    ],
)
def test_native_run_rejects_path_or_id_before_session(monkeypatch, tmp_path, change):
    monkeypatch.setattr(
        native_backend,
        "NativeSession",
        lambda *args, **kwargs: pytest.fail("session constructed after bad path"),
    )
    args = _run_args(tmp_path)
    change(args)
    assert cli.main(args) == 2
    assert not (tmp_path / "out.npz").exists()


def test_default_run_rejects_native_only_arguments(tmp_path):
    args = _run_args(tmp_path)
    args[args.index("--backend") + 1] = "default"
    assert cli.main(args) == 2


def test_native_run_rejects_legacy_policy_override_before_session(
    monkeypatch, tmp_path
):
    monkeypatch.setattr(
        native_backend,
        "NativeSession",
        lambda *args, **kwargs: pytest.fail("session constructed with legacy policy"),
    )
    args = _run_args(tmp_path)
    args.extend(["--policy", str(tmp_path / "other-policy.json")])
    assert cli.main(args) == 2
