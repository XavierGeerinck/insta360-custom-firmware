"""File frontend checks use fake sessions and never contact the device."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest
import soundfile as sf

from wave_onnx_hybrid import marblenet_cli
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def fixture(tmp_path, monkeypatch):
    checkpoint = tmp_path / "pinned.nemo"
    checkpoint.write_bytes(b"checkpoint")
    package = tmp_path / "package"
    package.mkdir()
    model = package / "model.onnx"
    model.write_bytes(b"full model")
    qualification = {"status": "PASS", "nbg_sha256": "c" * 64}
    (package / "qualification.json").write_text(json.dumps(qualification))
    audio = tmp_path / "audio.wav"
    sf.write(
        audio, np.linspace(-0.1, 0.1, 64000, dtype=np.float32), 16000, subtype="FLOAT"
    )
    checkpoint_sha = hashlib.sha256(checkpoint.read_bytes()).hexdigest()
    model_sha = hashlib.sha256(model.read_bytes()).hexdigest()
    monkeypatch.setattr(marblenet_cli, "CHECKPOINT_SHA256", checkpoint_sha)
    monkeypatch.setattr(marblenet_cli, "FULL_MODEL_SHA256", model_sha)
    record = {
        "profile_id": "fp16-marblenet-v1",
        "source_model_sha256": model_sha,
        "lowering_sha256": "b" * 64,
        "graph_signature": "e" * 64,
        "onnx_input": {
            "name": "input",
            "dtype": "FLOAT16",
            "shape": [1, 80, 400],
            "byte_size": 64000,
        },
        "onnx_output": {
            "name": "output",
            "dtype": "FLOAT16",
            "shape": [1, 2, 200],
            "byte_size": 800,
        },
    }
    loaded = {
        "directory": str(package),
        "record": record,
        "package_sha256": "a" * 64,
        "compiled": {"status": "PASS", "nbg_sha256": "c" * 64},
        "qualification": qualification,
    }
    calls = []

    def loader(path, require_qualification=False):
        assert Path(path) == package
        assert require_qualification == (args["mode"] == "strict")
        return loaded

    def features(wave, state):
        calls.append(("features", wave.copy()))
        return np.zeros((1, 80, 400), np.float32)

    helper = SimpleNamespace(
        torch=SimpleNamespace(__version__="2.5.1"),
        load_checkpoint=lambda path: {"state": {}, "checkpoint_sha256": checkpoint_sha},
        preprocess_audio=features,
        probabilities=lambda logits: np.full(200, 0.5, np.float32),
    )

    class Session:
        def __init__(self, model, package, *, run_id, mode):
            calls.append(("session", run_id, mode))
            provider = (
                "WaveNativeBackend" if str(mode) == "strict" else "CPUExecutionProvider"
            )
            self.providers = [provider]
            self.evidence = {
                "providers": self.providers,
                "classification": "npu-strict"
                if str(mode) == "strict"
                else "cpu-native-execution-fallback",
                "native_attempts": 1,
                "native_calls": 1 if str(mode) == "strict" else None,
                "cpu_sessions": 0 if str(mode) == "strict" else 1,
                "model_sha256": model_sha,
                "package_sha256": "a" * 64,
                "nbg_sha256": "c" * 64,
            }

        def run(self, inputs):
            calls.append(("run", inputs))
            return {"output": np.zeros((1, 2, 200), np.float16)}

    args = {
        "checkpoint": checkpoint,
        "package": package,
        "audio": audio,
        "offset_samples": 20,
        "run_id": "frontend-test-1",
        "mode": "strict",
        "outputs": tmp_path / "out.npz",
        "evidence": tmp_path / "evidence.json",
        "package_loader": loader,
        "session_factory": Session,
        "model_helper": helper,
    }
    return args, calls, loaded


def test_help_never_loads_torch(monkeypatch):
    monkeypatch.setattr(
        marblenet_cli, "_model_helper", lambda: pytest.fail("Torch imported for help")
    )
    with pytest.raises(SystemExit) as exit_info:
        marblenet_cli.main(["--help"])
    assert exit_info.value.code == 0


@pytest.mark.parametrize(
    "mode,provider",
    [("strict", "WaveNativeBackend"), ("hybrid", "CPUExecutionProvider")],
)
def test_actual_provider_selection_and_output_payload(
    tmp_path, monkeypatch, mode, provider
):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    args["mode"] = mode
    result = marblenet_cli.run_marblenet(**args)
    assert sum(call[0] == "run" for call in calls) == 1
    assert result["execution"]["providers"] == [provider]
    with np.load(args["outputs"], allow_pickle=False) as data:
        assert set(data.files) == {"logits", "speech_probability", "speech"}
        assert (
            data["logits"].shape == (1, 2, 200) and data["logits"].dtype == np.float16
        )
        assert data["speech_probability"].shape == (200,) and data["speech"].all()
    evidence = json.loads(args["evidence"].read_text())
    assert evidence["audio"]["selection"]["offset_samples"] == 20
    assert (
        evidence["audio"]["file_sha256"]
        == hashlib.sha256(args["audio"].read_bytes()).hexdigest()
    )
    assert evidence["context"]["causal_latency_qualified"] is False


def test_strict_failure_writes_no_outputs_and_does_not_retry(tmp_path, monkeypatch):
    args, calls, _ = fixture(tmp_path, monkeypatch)

    class Failed:
        def __init__(self, *a, **kw):
            pass

        def run(self, inputs):
            calls.append(("run", inputs))
            raise HybridError(ErrorCode.PARTITION_COMPILE_FAILURE, "frozen failure")

    args["session_factory"] = Failed
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert sum(call[0] == "run" for call in calls) == 1
    assert not args["outputs"].exists() and not args["evidence"].exists()


def test_self_consistent_pad_profile_does_not_bypass_trained_model_gate(
    tmp_path, monkeypatch
):
    args, calls, loaded = fixture(tmp_path, monkeypatch)
    loaded["record"]["profile_id"] = "fp16-marblenet-pad-v1"
    # Even a matching loader/model digest and PASS qualification cannot
    # authorize bytes that do not reverse to the pinned trained graph.
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert calls == []
    assert not args["outputs"].exists() and not args["evidence"].exists()


def test_dangling_output_symlink_refuses_before_dispatch(tmp_path, monkeypatch):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    args["outputs"].symlink_to(tmp_path / "new-target.npz")
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert not any(call[0] == "run" for call in calls)
    assert not (tmp_path / "new-target.npz").exists()


def test_qualification_file_changed_during_dispatch_refuses_output(
    tmp_path, monkeypatch
):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    original = args["session_factory"]

    class Changed(original):
        def run(self, inputs):
            result = super().run(inputs)
            (args["package"] / "qualification.json").write_text('{"status":"FAIL"}')
            return result

    args["session_factory"] = Changed
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert sum(call[0] == "run" for call in calls) == 1
    assert not args["outputs"].exists() and not args["evidence"].exists()


@pytest.mark.parametrize(
    "change",
    [
        "sample-rate",
        "stereo",
        "short",
        "nonfinite",
        "format",
        "bad-offset",
        "invalid-wave",
    ],
)
def test_bad_audio_refuses_before_feature_or_session(tmp_path, monkeypatch, change):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    if change == "sample-rate":
        sf.write(args["audio"], np.zeros(64000, np.float32), 48000, subtype="FLOAT")
    elif change == "stereo":
        sf.write(
            args["audio"], np.zeros((64000, 2), np.float32), 16000, subtype="FLOAT"
        )
    elif change == "short":
        sf.write(args["audio"], np.zeros(63840, np.float32), 16000, subtype="FLOAT")
    elif change == "nonfinite":
        sf.write(
            args["audio"], np.full(64000, np.nan, np.float32), 16000, subtype="FLOAT"
        )
    elif change == "format":
        sf.write(
            args["audio"],
            np.zeros(64000, np.float32),
            16000,
            format="FLAC",
            subtype="PCM_16",
        )
    elif change == "bad-offset":
        args["offset_samples"] = -1
    else:
        args["audio"].write_bytes(b"not a wave")
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert (
        calls == [] and not args["outputs"].exists() and not args["evidence"].exists()
    )


@pytest.mark.parametrize(
    "change",
    [
        "checkpoint",
        "audio",
        "same-output",
        "package",
        "inside-package",
        "existing",
        "linked-parent",
    ],
)
def test_output_path_alias_refuses_before_work(tmp_path, monkeypatch, change):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    if change in {"checkpoint", "audio", "package"}:
        args["outputs"] = args[change]
    elif change == "same-output":
        args["evidence"] = args["outputs"]
    elif change == "inside-package":
        args["outputs"] = args["package"] / "out.npz"
    elif change == "existing":
        args["outputs"].write_bytes(b"preserve")
    else:
        parent = tmp_path / "linked-parent"
        parent.symlink_to(args["package"], target_is_directory=True)
        args["outputs"] = parent / "out.npz"
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert calls == []
    if change == "existing":
        assert args["outputs"].read_bytes() == b"preserve"


@pytest.mark.parametrize(
    "change",
    [
        "profile",
        "partial-input",
        "partial-output",
        "input-name",
        "output-name",
        "alternate-full-model",
        "unqualified",
    ],
)
def test_partial_or_wrong_package_refuses_before_features(
    tmp_path, monkeypatch, change
):
    args, calls, loaded = fixture(tmp_path, monkeypatch)
    record = loaded["record"]
    if change == "profile":
        record["profile_id"] = "fp16-conv-v1"
    elif change == "partial-input":
        record["onnx_input"]["shape"] = [1, 80, 200]
    elif change == "partial-output":
        record["onnx_output"]["shape"] = [1, 128, 200]
    elif change == "input-name":
        record["onnx_input"]["name"] = "other"
    elif change == "output-name":
        record["onnx_output"]["name"] = "other"
    elif change == "alternate-full-model":
        record["source_model_sha256"] = "0" * 64
    else:
        loaded["qualification"] = None
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert calls == []


@pytest.mark.parametrize("change", ["checkpoint", "onnx"])
def test_changed_source_identity_refuses_before_session(tmp_path, monkeypatch, change):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    (
        args["checkpoint"] if change == "checkpoint" else args["package"] / "model.onnx"
    ).write_bytes(b"changed")
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert calls == []


@pytest.mark.parametrize(
    "change",
    [
        "logit-shape",
        "logit-dtype",
        "nonfinite-logits",
        "probability-shape",
        "strict-cpu",
        "wrong-provider-evidence",
    ],
)
def test_bad_backend_outputs_or_provider_refuse_writes(tmp_path, monkeypatch, change):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    original = args["session_factory"]

    class Bad(original):
        def run(self, inputs):
            result = super().run(inputs)
            if change == "logit-shape":
                result["output"] = np.zeros((1, 200, 2), np.float16)
            elif change == "logit-dtype":
                result["output"] = np.zeros((1, 2, 200), np.float32)
            elif change == "nonfinite-logits":
                result["output"] = np.full((1, 2, 200), np.inf, np.float16)
            elif change == "strict-cpu":
                self.providers = ["CPUExecutionProvider"]
                self.evidence["providers"] = self.providers
            elif change == "wrong-provider-evidence":
                self.evidence["providers"] = ["CPUExecutionProvider"]
            return result

    args["session_factory"] = Bad
    if change == "probability-shape":
        args["model_helper"].probabilities = lambda logits: np.zeros(
            (1, 200), np.float32
        )
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert sum(call[0] == "run" for call in calls) == 1
    assert not args["outputs"].exists() and not args["evidence"].exists()


def test_evidence_write_failure_removes_only_own_output(tmp_path, monkeypatch):
    args, calls, _ = fixture(tmp_path, monkeypatch)
    writer = marblenet_cli._write_exclusive

    def failed(path, payload):
        if path == args["evidence"]:
            raise OSError("fake filesystem failure")
        writer(path, payload)

    monkeypatch.setattr(marblenet_cli, "_write_exclusive", failed)
    with pytest.raises(OSError):
        marblenet_cli.run_marblenet(**args)
    assert sum(call[0] == "run" for call in calls) == 1
    assert not args["outputs"].exists() and not args["evidence"].exists()
    assert args["checkpoint"].read_bytes() == b"checkpoint"


def test_main_refusal_is_stable_json_and_no_helper(tmp_path, monkeypatch, capsys):
    args, _, _ = fixture(tmp_path, monkeypatch)
    monkeypatch.setattr(
        marblenet_cli, "_model_helper", lambda: pytest.fail("helper loaded for alias")
    )
    status = marblenet_cli.main(
        [
            "run",
            "--checkpoint",
            str(args["checkpoint"]),
            "--package",
            str(args["package"]),
            "--audio",
            str(args["audio"]),
            "--run-id",
            "bad-id",
            "--outputs",
            str(args["audio"]),
            "--evidence",
            str(args["evidence"]),
        ]
    )
    assert status == 2
    assert json.loads(capsys.readouterr().err)["code"] == "POLICY_INVALID"


def test_unqualified_full_model_hybrid_cpu_delivery_and_strict_refusal(
    tmp_path, monkeypatch
):
    args, calls, loaded = fixture(tmp_path, monkeypatch)
    loaded["compiled"] = None
    loaded["qualification"] = None
    (args["package"] / "qualification.json").unlink()

    def loader(path, require_qualification=False):
        assert Path(path) == args["package"]
        return loaded

    args["package_loader"] = loader
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert calls == []
    original = args["session_factory"]

    class CpuOnly(original):
        def __init__(self, *a, **kw):
            super().__init__(*a, **kw)
            self.evidence.update(
                classification="cpu-native-preflight-fallback",
                native_calls=0,
                native_attempts=0,
                cpu_sessions=1,
                package_sha256=None,
                nbg_sha256=None,
                npu_error="full graph is unqualified",
            )

    args["mode"] = "hybrid"
    args["session_factory"] = CpuOnly
    result = marblenet_cli.run_marblenet(**args)
    assert result["execution"]["providers"] == ["CPUExecutionProvider"]
    assert result["execution"]["native_attempts"] == 0
    assert result["execution"]["cpu_sessions"] == 1
    assert result["package"]["qualification_sha256"] is None
    assert result["package"]["qualification_status"] == "UNQUALIFIED"
    assert args["outputs"].is_file()


def test_real_native_session_latches_unqualified_cpu_without_executor(
    tmp_path, monkeypatch
):
    from wave_onnx_hybrid import native_backend

    actual_model = (
        Path(__file__).resolve().parents[4] / "artifacts/npu/poc18/prepared/18d.onnx"
    )
    if not actual_model.is_file():
        pytest.skip("ignored full trained model is unavailable")
    args, calls, loaded = fixture(tmp_path, monkeypatch)
    payload = actual_model.read_bytes()
    digest = hashlib.sha256(payload).hexdigest()
    (args["package"] / "model.onnx").write_bytes(payload)
    (args["package"] / "qualification.json").unlink()
    monkeypatch.setattr(marblenet_cli, "FULL_MODEL_SHA256", digest)
    loaded["record"]["source_model_sha256"] = digest
    loaded["compiled"] = None
    loaded["qualification"] = None

    class Bridge:
        def load_package(self, path, require_qualification=False):
            if require_qualification:
                raise HybridError(
                    ErrorCode.MODEL_PACKAGE_INVALID, "no full-graph qualification"
                )
            return loaded

        def execute_package(self, *a, **kw):
            pytest.fail("unqualified graph reached native executor")

    cpu_constructions = []

    class Cpu:
        def __init__(self, model_payload, output_names):
            assert model_payload == payload and output_names == ["output"]
            cpu_constructions.append(1)

        def run(self, inputs):
            return {"output": np.zeros((1, 2, 200), np.float16)}

    bridge = Bridge()
    monkeypatch.setattr(native_backend, "native_bridge", lambda: bridge)
    monkeypatch.setattr(native_backend, "_OrtCpuSession", Cpu)
    args["package_loader"] = bridge.load_package
    args["session_factory"] = native_backend.NativeSession
    args["mode"] = "strict"
    with pytest.raises(HybridError):
        marblenet_cli.run_marblenet(**args)
    assert cpu_constructions == [] and calls == []
    args["mode"] = "hybrid"
    result = marblenet_cli.run_marblenet(**args)
    assert cpu_constructions == [1]
    assert result["execution"]["providers"] == ["CPUExecutionProvider"]
    assert (
        result["execution"]["native_calls"]
        == result["execution"]["native_attempts"]
        == 0
    )
    assert result["execution"]["cpu_sessions"] == 1
    assert result["execution"]["classification"] == "cpu-native-preflight-fallback"
