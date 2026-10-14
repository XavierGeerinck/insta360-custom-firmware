"""New spatial profile gets the existing lifecycle gates; old bounds stay fixed."""

import sys
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_compile_lab as compile_lab
import npu_onnx_native_lab as lab


def record():
    return {
        "profile_id": "fp16-squeezenet-v1",
        "lowering_sha256": "ab" * 32,
        "native_input": {
            "dtype": "FLOAT16",
            "shape": [224, 224, 3, 1],
            "byte_size": 301056,
        },
        "native_output": {"dtype": "FLOAT16", "shape": [1000, 1], "byte_size": 2000},
        "onnx_input": {"byte_size": 301056},
    }


def test_vision_namespace_and_compile_policy_are_distinct(tmp_path):
    source = record()
    expected = "/tmp/wave-npu-onnx20-" + "ab" * 8 + "-compile"
    assert lab.remote_root(source, "compile") == expected
    assert lab.ROOT_RE.fullmatch(expected)
    policy = lab._compile_policy({"record": source, "directory": str(tmp_path)})
    assert policy.poc_id == "20"
    assert policy.inputs[0].dimensions == (224, 224, 3, 1)
    assert policy.remote_executable.startswith(expected + "/")


def test_vision_requires_phase_before_device_contact(tmp_path):
    package = {"record": record(), "directory": str(tmp_path / "20a")}
    token = lab._ACTIVE_PHASE.set(None)
    try:
        with (
            mock.patch.object(lab, "_compile_manifest", return_value="compile"),
            mock.patch.object(lab, "_system_manifest", return_value="system"),
            mock.patch.object(lab.npu_native_phase, "NativePhase") as guard,
            mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
        ):
            lab._activate_phase(package)
            guard.assert_called_once_with(
                tmp_path / "native-phase.json", "system", lab.SYSTEM_HASHES
            )
            transport.assert_not_called()
    finally:
        lab._ACTIVE_PHASE.reset(token)


def test_spatial_tensor_bounds_do_not_widen_old_single_operator_profile():
    assert lab._tensor_bytes(record(), "onnx_input") == 301056
    old = {"profile_id": "fp16-conv-v1", "onnx_input": {"byte_size": 34}}
    with pytest.raises(lab.NativeLabError):
        lab._tensor_bytes(old, "onnx_input")


def test_vision_compile_health_detects_service_respawn():
    manifest = SimpleNamespace(serial="wave", policy=SimpleNamespace(poc_id="20"))
    before = {
        "serials": ["wave"],
        "dmesg_tail": ["marker"],
        "processes": ["123 root /data/waved/waved"],
    }
    after = {**before, "processes": ["124 root /data/waved/waved"]}
    differences, alerts = compile_lab.compare_health(manifest, before, after)
    assert "service process identities changed" in differences
    assert not alerts


def test_only_pool_control_can_qualify_tp_without_nn():
    tp = "operation target VXNNE_OPERATION_TARGET_TP, type=pool"
    source = {**record(), "control_id": "20b"}
    assert lab._qualified_target(source, tp) == "VXNNE_OPERATION_TARGET_TP"
    for control in ("20a", "20c", "20d", "20e"):
        assert lab._qualified_target({**source, "control_id": control}, tp) is None
    for forbidden in ("SH", "SW", "CPU"):
        assert (
            lab._qualified_target(
                source,
                tp
                + "\noperation target VXNNE_OPERATION_TARGET_"
                + forbidden
                + ", type=bad",
            )
            is None
        )
    assert lab._qualified_target(source, "") is None
    assert not lab._nn_target(tp, allow_tp=True)
