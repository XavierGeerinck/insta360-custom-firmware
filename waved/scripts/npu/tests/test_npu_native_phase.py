"""Persistent encompassing continuity gate; all transport is fake/replayed."""

import copy
import hashlib
import json
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_compile_lab as compile_lab
import npu_erf_execution_lab as execution
import npu_native_phase as phase
import npu_onnx_native_lab as lab


def manifest():
    return SimpleNamespace(
        **{k: lab.DEVICE[k] for k in ("serial", "kernel", "npu_parameters")},
        policy=SimpleNamespace(poc_id="18"),
    )


def snapshot():
    return {
        "serials": [lab.DEVICE["serial"]],
        "kernel": lab.DEVICE["kernel"],
        "boot_id": "11111111-2222-3333-4444-555555555555",
        "npu_parameters": lab.DEVICE["npu_parameters"],
        "galcore_device": "crw-rw---- 1 root root 199, 0 /dev/galcore",
        "galcore_holders": [],
        "artifact_hashes": lab.SYSTEM_HASHES,
        "absent_artifact_states": {
            p: {"status": "absent"} for p in phase.SYSTEM_ABSENT
        },
        "errors": [],
        "processes": ["123 root /data/waved/waved"],
        "dmesg_tail": ["[1] accepted marker"],
    }


def seeded(tmp_path, anchor=None):
    source = tmp_path / "reviewed.json"
    source.write_text(json.dumps(anchor or snapshot()))
    path = tmp_path / "native-phase.json"
    phase.seed_phase(
        path,
        source,
        hashlib.sha256(source.read_bytes()).hexdigest(),
        "reviewed-test",
        "Explicit test-only authorization",
        manifest(),
        lab.SYSTEM_HASHES,
    )
    return phase.NativePhase(path, manifest(), lab.SYSTEM_HASHES)


@pytest.mark.parametrize(
    "fault", ["warning", "gap", "service", "state", "unit", "hash", "errors"]
)
def test_gap_or_fault_latches_halted_without_reseed(tmp_path, fault):
    guard = seeded(tmp_path)
    current = copy.deepcopy(snapshot())
    if fault == "warning":
        current["dmesg_tail"].append(
            "[2] WARNING transfer event TRB DMA ptr not part of current TD"
        )
    elif fault == "gap":
        current["dmesg_tail"] = ["rotated history"]
    elif fault == "service":
        current["processes"] = ["124 root /data/waved/waved"]
    elif fault == "state":
        current["npu_parameters"]["recovery"] = "0"
    elif fault == "unit":
        current["serials"] = ["another-unit"]
    elif fault == "hash":
        current["artifact_hashes"][next(iter(current["artifact_hashes"]))] = "0" * 64
    else:
        current["errors"] = ["USB device absent"]
    with pytest.raises(phase.PhaseError):
        guard.check(current, "before-stage")
    assert json.loads(guard.path.read_bytes())["state"] == "HALTED"
    with pytest.raises(phase.PhaseError):
        phase.NativePhase(guard.path, manifest(), lab.SYSTEM_HASHES)


def test_missing_or_invalid_anchor_cannot_authorize_launch(tmp_path):
    path = tmp_path / "native-phase.json"
    with pytest.raises(phase.PhaseError):
        phase.NativePhase(path, manifest(), lab.SYSTEM_HASHES)
    assert json.loads(path.read_bytes())["state"] == "HALTED"
    path.write_text("{}")
    with pytest.raises(phase.PhaseError):
        phase.NativePhase(path, manifest(), lab.SYSTEM_HASHES)


def test_only_clean_cleanup_advances_and_ort_extra_hash_is_separate(tmp_path):
    guard = seeded(tmp_path)
    before = copy.deepcopy(snapshot())
    before["artifact_hashes"]["/usr/lib/libonnxruntime.so.1.15.1"] = "a" * 64
    guard.check(before, "immediate-preflight")
    after = copy.deepcopy(snapshot())
    after["dmesg_tail"].append("[2] clean teardown")
    guard.advance(
        {
            "status": "CLEAN",
            "health_differences": [],
            "kernel_alerts": [],
            "after": after,
        }
    )
    document = json.loads(guard.path.read_bytes())
    assert (
        document["state"] == "READY"
        and document["accepted_anchor"]["dmesg_tail"] == after["dmesg_tail"]
    )
    phase.NativePhase(guard.path, manifest(), lab.SYSTEM_HASHES)


@pytest.mark.parametrize(
    "cleanup",
    [
        None,
        {"status": "FAIL"},
        {"status": "CLEAN", "health_differences": ["changed"], "kernel_alerts": []},
    ],
)
def test_failed_cleanup_never_advances(tmp_path, cleanup):
    guard = seeded(tmp_path)
    original = json.loads(guard.path.read_bytes())["accepted_anchor_sha256"]
    guard.check(snapshot(), "stage")
    with pytest.raises(phase.PhaseError):
        guard.advance(cleanup)
    halted = json.loads(guard.path.read_bytes())
    assert halted["state"] == "HALTED" and halted["accepted_anchor_sha256"] == original


def test_seed_requires_exact_review_hash_authorization_and_new_path(tmp_path):
    source = tmp_path / "reviewed.json"
    source.write_text(json.dumps(snapshot()))
    path = tmp_path / "native-phase.json"
    with pytest.raises(phase.PhaseError):
        phase.seed_phase(
            path, source, "0" * 64, "test", "approved", manifest(), lab.SYSTEM_HASHES
        )
    with pytest.raises(phase.PhaseError):
        phase.seed_phase(
            path,
            source,
            hashlib.sha256(source.read_bytes()).hexdigest(),
            "test",
            "",
            manifest(),
            lab.SYSTEM_HASHES,
        )
    seeded(tmp_path)
    with pytest.raises(phase.PhaseError):
        phase.seed_phase(
            path,
            source,
            hashlib.sha256(source.read_bytes()).hexdigest(),
            "test",
            "approved again",
            manifest(),
            lab.SYSTEM_HASHES,
        )


def test_exact_retained_usb_warning_blocks_staging_before_mutation(
    tmp_path, monkeypatch
):
    root = Path(__file__).resolve().parents[4]
    health_path = root / "artifacts/npu/poc19/health/snapshot-0014.json"
    cleanup_path = (
        root / "artifacts/npu/poc19/packages/19d/evidence/qualification/cleanup.json"
    )
    stage_path = root / "artifacts/npu/poc19/packages/19e/evidence"
    if not health_path.exists():
        pytest.skip("ignored replay evidence unavailable")
    baseline = json.loads(health_path.read_bytes())["snapshot"]
    clean = json.loads(cleanup_path.read_bytes())
    paths = list(stage_path.glob("*-stage/stage-before.json"))
    assert len(paths) == 1
    warning = json.loads(paths[0].read_bytes())
    guard = seeded(tmp_path, baseline)
    guard.check(baseline, "start")
    guard.advance(clean)
    # The exact warning already present in the local baseline is the regression.
    assert any(
        "4992.636" in line and "WARNING" in line for line in warning["dmesg_tail"]
    )
    run_dir = tmp_path / "stage"
    run_dir.mkdir()
    with (
        mock.patch.object(lab, "_verify_runtime"),
        mock.patch.object(lab, "_system_manifest", return_value=manifest()),
        mock.patch.object(lab.compile_lab, "collect_snapshot", return_value=warning),
        mock.patch.object(lab.compile_lab, "verify_preflight"),
        mock.patch.object(lab.npu_lab, "AdbTransport"),
        mock.patch.object(lab, "_adb") as transport,
    ):
        with pytest.raises(phase.PhaseError):
            lab._stage(manifest(), {}, "/tmp/test", run_dir, phase=guard)
        transport.assert_not_called()
    assert json.loads(guard.path.read_bytes())["state"] == "HALTED"
    assert json.loads((run_dir / "stage-before.json").read_bytes()) == warning


@pytest.mark.parametrize("which", ["compile", "execute"])
def test_immediate_preflight_callback_blocks_vendor_command(
    tmp_path, monkeypatch, which
):
    guard = seeded(tmp_path)
    current = copy.deepcopy(snapshot())
    current["dmesg_tail"].append("[2] WARNING immediate preflight fault")
    transport = mock.Mock()
    if which == "compile":
        m = compile_lab.load_manifest()
        monkeypatch.setattr(compile_lab, "DEFAULT_RUN_ROOT", tmp_path / "runs")
        monkeypatch.setattr(compile_lab, "collect_snapshot", lambda *a: current)
        monkeypatch.setattr(compile_lab, "verify_preflight", lambda *a: None)
        result, _ = compile_lab.run_candidate(
            transport,
            m,
            "candidate-1",
            "preflight-refuse",
            preflight_validator=guard.callback(
                compile_lab.PreflightError, "compile-preflight"
            ),
        )
    else:
        m = execution.load_manifest()
        monkeypatch.setattr(execution, "DEFAULT_RUN_ROOT", tmp_path / "runs")
        monkeypatch.setattr(execution, "collect_snapshot", lambda *a: current)
        monkeypatch.setattr(execution, "verify_preflight", lambda *a: None)
        result, _ = execution.run_once(
            transport,
            m,
            "preflight-refuse",
            preflight_validator=guard.callback(
                execution.ExecutionPreflightError, "execute-preflight"
            ),
        )
        assert result["execution_attempts"] == 0
    assert result["status"] == "PREFLIGHT_FAILED"
    transport.run.assert_not_called()
    assert json.loads(guard.path.read_bytes())["state"] == "HALTED"


@pytest.mark.parametrize("failure", [False, True])
def test_scoped_lifecycle_advances_only_after_successful_clean_result(
    tmp_path, monkeypatch, failure
):
    guard = seeded(tmp_path)
    original = json.loads(guard.path.read_bytes())["accepted_anchor_sha256"]
    monkeypatch.setattr(lab, "_compile_manifest", lambda *a: manifest())
    monkeypatch.setattr(lab, "_system_manifest", lambda m: m)
    after = copy.deepcopy(snapshot())
    after["dmesg_tail"].append("[2] clean teardown")
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(tmp_path / "19c"),
    }

    @lab._phase_guarded
    def operation(package_dir):
        lab._activate_phase(package)
        active = lab._ACTIVE_PHASE.get()
        active.check(snapshot(), "stage")
        active.pending_cleanup = {
            "status": "CLEAN",
            "health_differences": [],
            "kernel_alerts": [],
            "after": after,
        }
        if failure:
            raise lab.NativeLabError("numeric comparison failed after safe cleanup")
        return "complete"

    if failure:
        with pytest.raises(lab.NativeLabError):
            operation(tmp_path / "19c")
    else:
        assert operation(tmp_path / "19c") == "complete"
    document = json.loads(guard.path.read_bytes())
    assert document["state"] == ("HALTED" if failure else "READY")
    assert (document["accepted_anchor_sha256"] == original) == failure
    assert lab._ACTIVE_PHASE.get() is None


def test_previous_active_operation_is_not_automatically_resumed(tmp_path):
    guard = seeded(tmp_path)
    guard.check(snapshot(), "staged-but-unfinished")
    with pytest.raises(phase.PhaseError):
        phase.NativePhase(guard.path, manifest(), lab.SYSTEM_HASHES)
    assert json.loads(guard.path.read_bytes())["state"] == "HALTED"


@pytest.mark.parametrize("kind", ["compile", "qualify", "execute"])
def test_new_profile_requires_phase_before_any_live_transport(
    tmp_path, monkeypatch, kind
):
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(tmp_path / "19f"),
        "compiled": None if kind == "compile" else {},
        "qualification": None,
    }
    monkeypatch.setattr(lab, "_run_id", lambda *a: None)
    monkeypatch.setattr(lab, "load_package", lambda *a, **kw: package)
    monkeypatch.setattr(lab, "_compile_manifest", lambda *a: manifest())
    monkeypatch.setattr(lab, "_system_manifest", lambda m: m)
    with (
        mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
        mock.patch.object(lab, "_cpu_expected") as oracle,
    ):
        with pytest.raises(phase.PhaseError):
            if kind == "compile":
                lab.compile_package(tmp_path / "19f", "missing-phase")
            elif kind == "qualify":
                lab.qualify_package(
                    tmp_path / "19f", tmp_path / "input.npz", "missing-phase"
                )
            else:
                lab.execute_package(tmp_path / "19f", {}, "missing-phase")
        transport.assert_not_called()
        oracle.assert_not_called()
    assert (
        json.loads((tmp_path / "native-phase.json").read_bytes())["state"] == "HALTED"
    )


def test_old_profile_keeps_runner_arguments_and_has_no_phase_dependency(monkeypatch):
    package = {"record": {"profile_id": "fp16-marblenet-v1"}}
    with mock.patch.object(phase, "NativePhase") as constructor:

        @lab._phase_guarded
        def old_operation(package_dir):
            lab._activate_phase(package)
            assert lab._phase_runner_kwargs(compile_lab.PreflightError) == {}
            return "old-result"

        assert old_operation("unused-old-package") == "old-result"
        constructor.assert_not_called()


def test_clean_compile_negative_advances_verified_cleanup_and_stays_ready(
    tmp_path, monkeypatch
):
    guard = seeded(tmp_path)
    original = json.loads(guard.path.read_bytes())["accepted_anchor_sha256"]
    monkeypatch.setattr(lab, "_compile_manifest", lambda *a: manifest())
    monkeypatch.setattr(lab, "_system_manifest", lambda m: m)
    after = copy.deepcopy(snapshot())
    after["dmesg_tail"].append("[2] unsupported compile cleaned")
    cleanup = {
        "status": "CLEAN",
        "health_differences": [],
        "kernel_alerts": [],
        "after": after,
    }
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(tmp_path / "19c"),
    }

    @lab._phase_guarded
    def operation(package_dir):
        lab._activate_phase(package)
        lab._ACTIVE_PHASE.get().check(snapshot(), "compile-preflight")
        raise lab.CleanCompileNegative("clean unsupported compile", cleanup)

    with pytest.raises(lab.CleanCompileNegative):
        operation(tmp_path / "19c")
    document = json.loads(guard.path.read_bytes())
    assert (
        document["state"] == "READY" and document["accepted_anchor_sha256"] != original
    )
    phase.NativePhase(guard.path, manifest(), lab.SYSTEM_HASHES)


@pytest.mark.parametrize("kind", ["compile", "qualify", "execute"])
def test_ineligible_local_command_does_not_activate_or_halt_phase(
    tmp_path, monkeypatch, kind
):
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(tmp_path / "19f"),
        "compiled": None if kind == "execute" else {"already": "compiled"},
        "qualification": {"already": "qualified"},
    }
    monkeypatch.setattr(lab, "_run_id", lambda *a: None)
    monkeypatch.setattr(lab, "load_package", lambda *a, **kw: package)
    with (
        mock.patch.object(phase, "NativePhase") as constructor,
        mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
    ):
        with pytest.raises(lab.NativeLabError):
            if kind == "compile":
                lab.compile_package(tmp_path / "19f", "no-retry")
            elif kind == "qualify":
                lab.qualify_package(
                    tmp_path / "19f", tmp_path / "input.npz", "no-retry"
                )
            else:
                lab.execute_package(tmp_path / "19f", {}, "no-nbg")
        constructor.assert_not_called()
        transport.assert_not_called()
    assert not (tmp_path / "native-phase.json").exists()


@pytest.mark.parametrize("canonical", [True, False])
def test_public_compile_classifies_only_verified_clean_negative_as_ready(
    tmp_path, monkeypatch, canonical
):
    guard = seeded(tmp_path)
    root = tmp_path / "19c"
    root.mkdir()
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(root),
        "compiled": None,
    }
    m = manifest()
    m.policy.remote_root = "/tmp/wave-npu-onnx18-" + "ab" * 8 + "-compile"
    m.remote_executable = m.policy.remote_root + "/bin/probe"
    m.sha256 = "a" * 64
    monkeypatch.setattr(lab, "_run_id", lambda *a: None)
    monkeypatch.setattr(lab, "load_package", lambda *a, **kw: package)
    monkeypatch.setattr(lab, "_compile_manifest", lambda *a: m)
    monkeypatch.setattr(lab, "_system_manifest", lambda x: x)
    monkeypatch.setattr(lab, "_identity", lambda *a: {})
    monkeypatch.setattr(
        lab,
        "_stage",
        lambda *a, **kw: lab._ACTIVE_PHASE.get().check(snapshot(), "stage"),
    )
    after = copy.deepcopy(snapshot())
    after["dmesg_tail"].append("[2] cleaned unsupported compile")
    clean = {
        "status": "CLEAN",
        "health_differences": [],
        "kernel_alerts": [],
        "after": after,
    }
    envelope = {
        "status": "FAIL",
        "health_differences": [],
        "kernel_alerts": [],
        "transport": {"remote_exit": 5, "timed_out": False, "adb_returncode": 0},
        "errors": ["native compiler exited 5"],
    }
    if not canonical:
        envelope["errors"].append("invalid native artifact")

    def run(*a, **kw):
        kw["preflight_validator"](snapshot())
        return envelope, tmp_path / "run"

    monkeypatch.setattr(lab.compile_lab, "run_candidate", run)
    with (
        mock.patch.object(lab, "_cleanup", return_value=clean) as cleanup,
        mock.patch.object(lab.npu_lab, "AdbTransport"),
    ):
        with pytest.raises(
            lab.CleanCompileNegative if canonical else lab.NativeLabError
        ):
            lab.compile_package(root, "unsupported")
        assert cleanup.call_count == int(canonical)
    assert json.loads(guard.path.read_bytes())["state"] == (
        "READY" if canonical else "HALTED"
    )


def test_uncompiled_pad_qualification_refuses_before_phase_cpu_or_attempt(
    tmp_path, monkeypatch
):
    package_root = tmp_path / "19f"
    package_root.mkdir()
    package = {
        "record": {"profile_id": "fp16-marblenet-pad-v1"},
        "directory": str(package_root),
        "compiled": None,
        "qualification": None,
    }
    monkeypatch.setattr(lab, "_run_id", lambda *a: None)
    monkeypatch.setattr(lab, "load_package", lambda *a, **kw: package)
    with (
        mock.patch.object(
            lab,
            "_activate_phase",
            side_effect=AssertionError("phase activated for uncompiled qualifier"),
        ) as activation,
        mock.patch.object(lab, "_cpu_expected") as oracle,
        mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
    ):
        with pytest.raises(lab.NativeLabError, match="compile artifact"):
            lab.qualify_package(
                package_root, tmp_path / "input.npz", "uncompiled-qualifier"
            )
        activation.assert_not_called()
        oracle.assert_not_called()
        transport.assert_not_called()
    assert not (tmp_path / "native-phase.json").exists()
    assert not (package_root / "qualification-attempt.json").exists()
