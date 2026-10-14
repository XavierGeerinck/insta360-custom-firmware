"""The mean repair uses a fresh phase/namespace and refuses missing NBG locally."""

import sys
from pathlib import Path
from unittest import mock

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_onnx_native_lab as lab
import npu_compile_lab as compile_lab


def record():
    return {
        "profile_id": "fp16-squeezenet-mean-v1",
        "lowering_sha256": "ab" * 32,
        "native_input": {
            "dtype": "FLOAT16",
            "shape": [13, 13, 512, 1],
            "byte_size": 173056,
        },
        "native_output": {"dtype": "FLOAT16", "shape": [1000, 1], "byte_size": 2000},
        "onnx_input": {"byte_size": 173056},
        "control_id": "20d",
    }


def test_mean_has_distinct_root_and_unchanged_old_namespace(tmp_path):
    r = record()
    expected = "/tmp/wave-npu-onnx21-" + "ab" * 8 + "-compile"
    assert lab.remote_root(r, "compile") == expected
    assert lab.ROOT_RE.fullmatch(expected)
    p = lab._compile_policy({"record": r, "directory": str(tmp_path)})
    assert p.poc_id == "21" and p.inputs[0].dimensions == (13, 13, 512, 1)
    assert lab._poc_id({**r, "profile_id": "fp16-squeezenet-v1"}) == "20"


def test_mean_requires_own_phase_before_transport(tmp_path):
    token = lab._ACTIVE_PHASE.set(None)
    try:
        with (
            mock.patch.object(lab, "_compile_manifest", return_value="compile"),
            mock.patch.object(lab, "_system_manifest", return_value="system"),
            mock.patch.object(lab.npu_native_phase, "NativePhase") as phase,
            mock.patch.object(lab.npu_lab, "AdbTransport") as adb,
        ):
            lab._activate_phase(
                {"record": record(), "directory": str(tmp_path / "21b")}
            )
            phase.assert_called_once_with(
                tmp_path / "native-phase.json", "system", lab.SYSTEM_HASHES
            )
            adb.assert_not_called()
    finally:
        lab._ACTIVE_PHASE.reset(token)


def test_mean_tensor_bound_does_not_expand_old_contract():
    assert lab._tensor_bytes(record(), "onnx_input") == 173056
    with pytest.raises(lab.NativeLabError):
        lab._tensor_bytes({**record(), "profile_id": "fp16-conv-v1"}, "onnx_input")


def test_mean_compile_health_checks_service_identity():
    from types import SimpleNamespace

    m = SimpleNamespace(serial="wave", policy=SimpleNamespace(poc_id="21"))
    before = {
        "serials": ["wave"],
        "dmesg_tail": ["marker"],
        "processes": ["123 root /data/waved/waved"],
    }
    after = {**before, "processes": ["124 root /data/waved/waved"]}
    differences, alerts = compile_lab.compare_health(m, before, after)
    assert "service process identities changed" in differences and not alerts


def test_uncompiled_mean_refuses_before_phase(tmp_path):
    p = {
        "record": record(),
        "directory": str(tmp_path),
        "qualification": None,
        "compiled": None,
    }
    with (
        mock.patch.object(lab, "load_package", return_value=p),
        mock.patch.object(
            lab, "_activate_phase", side_effect=AssertionError("phase activation")
        ),
        mock.patch.object(
            lab.npu_lab, "AdbTransport", side_effect=AssertionError("device call")
        ),
    ):
        with pytest.raises(
            lab.NativeLabError, match="no qualified native compile artifact"
        ):
            lab.qualify_package(
                tmp_path, tmp_path / "nonexistent.npz", "host-mean-refusal"
            )
