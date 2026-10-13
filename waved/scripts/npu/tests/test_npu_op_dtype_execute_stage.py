from __future__ import annotations

import copy
import json
import re
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_erf_execution_lab as execution
import npu_op_dtype_execute_stage as stage


def manifest_for(model="fp16-conv"):
    original = execution.load_manifest(policy=execution.POC7_POLICY)
    fixed = stage.MODELS[model]
    root = fixed["root"]
    executable = f"{root}/bin/wave-npu-execute-{model}-v1"
    wrapper, nbg = f"{root}/lib/libtim-vx.so", f"{root}/model/wave-{model}.nbg"
    hashes = {**stage.SYSTEM_HASHES, executable: "1" * 64,
              wrapper: stage.WRAPPER_SHA256, nbg: fixed["sha256"]}
    absent = ("/lib/libCLC.so", "/usr/lib/libCLC.so", "/usr/lib64/libCLC.so",
              f"{root}/lib/libCLC.so")
    policy = replace(original.policy, poc_id=fixed["poc_id"], experiment=model + "-execute-once",
                     remote_root=root, remote_executable=executable, remote_tim_vx=wrapper,
                     remote_nbg=nbg, fixed_artifacts=hashes, absent_artifacts=absent,
                     staged_modes={executable: "-r-xr-xr-x", wrapper: "-r--r--r--", nbg: "-r--r--r--"},
                     nbg_bytes=fixed["bytes"], execution_re=re.compile("fixed"))
    return replace(original, policy=policy, remote_executable=executable,
                   environment={"LD_LIBRARY_PATH": root + "/lib:/usr/lib",
                                "VIVANTE_SDK_DIR": "/usr", "VSI_NN_LOG_LEVEL": "5"},
                   required_artifacts=hashes, absent_artifacts=absent, sha256="a" * 64)


def successful_envelope(manifest, run_id):
    return {"manifest_sha256": manifest.sha256, "experiment": manifest.policy.experiment,
            "run_id": run_id, "status": "PASS", "execution_attempts": 1,
            "errors": [], "health_differences": [], "kernel_alerts": [],
            "transport": {"timed_out": False, "remote_exit": 0, "adb_returncode": 0},
            "result": {"iterations": 1, "bytes": len(manifest.policy.output_values),
                       "output": list(manifest.policy.output_values)}}


def snapshot_for(manifest, system_only=False, model_present=True):
    hashes = stage.SYSTEM_HASHES if system_only else manifest.required_artifacts
    return {"serials": [manifest.serial], "kernel": manifest.kernel,
            "boot_id": "434af77c-6a2a-4ecd-98e9-66ab99d677c7",
            "galcore_device": "crw-rw---- 1 0 0 199, 0 /dev/galcore",
            "npu_parameters": dict(manifest.npu_parameters), "galcore_holders": [],
            "artifact_hashes": dict(hashes), "errors": [], "dmesg_tail": ["baseline"],
            "processes": ["2197 root /data/waved/waved"],
            "absent_artifact_states": {path: {"status": "absent"} for path in manifest.absent_artifacts},
            "output_states": {manifest.policy.remote_nbg: {"status": "present" if model_present else "absent"},
                              manifest.policy.remote_nbg + ".partial": {"status": "absent"}},
            "staged_artifact_states": {path: {"status": "present", "permissions": mode,
                                       "bytes": manifest.policy.nbg_bytes if path == manifest.policy.remote_nbg else 1}
                                       for path, mode in manifest.policy.staged_modes.items()}}


class ExecutionStagingTests(unittest.TestCase):
    def test_wrong_local_hash_refuses_before_any_device_call(self):
        manifest = manifest_for()
        with tempfile.TemporaryDirectory() as temporary:
            wrong = Path(temporary) / "wrong-executor"
            wrong.write_bytes(b"wrong build")
            with mock.patch.object(stage, "local_files", return_value={manifest.remote_executable: (wrong, "0555")}), \
                 mock.patch.object(stage, "_adb") as adb, \
                 mock.patch.object(stage.npu_lab, "AdbTransport") as transport:
                with self.assertRaisesRegex(execution.ExecutionPreflightError, "hash mismatch"):
                    stage.stage(manifest, "fp16-conv")
                adb.assert_not_called()
                transport.assert_not_called()

    def test_fixed_paths_model_or_system_hash_cannot_expand_scope(self):
        for model in stage.MODELS:
            manifest = manifest_for(model)
            files = stage.local_files(manifest, model)
            self.assertEqual(len(files), 3)
            self.assertEqual(set(files), set(manifest.policy.staged_modes))
            self.assertNotIn("sdk", " ".join(files))
            wrong = replace(manifest, required_artifacts={**manifest.required_artifacts,
                                                         "/usr/lib/libOpenVX.so": "b" * 64})
            with self.assertRaises(execution.ExecutionPreflightError):
                stage.local_files(wrong, model)
            wrong = replace(manifest, policy=replace(manifest.policy, remote_root="/tmp/other"))
            with self.assertRaises(execution.ExecutionPreflightError):
                stage.local_files(wrong, model)
        with self.assertRaises(execution.ExecutionPreflightError):
            stage.local_files(manifest_for(), "arbitrary-model")

    def test_uncertain_cleanup_refuses_before_device_or_deletion(self):
        manifest = manifest_for()
        run_id = "fake-stage-cleanup"
        mutations = [
            lambda envelope: envelope.update(status="FAIL"),
            lambda envelope: envelope.update(execution_attempts=2),
            lambda envelope: envelope.update(manifest_sha256="b" * 64),
            lambda envelope: envelope.update(kernel_alerts=["warning"]),
            lambda envelope: envelope["transport"].update(timed_out=True),
            lambda envelope: envelope["transport"].update(remote_exit=5),
            lambda envelope: envelope["result"].update(output=[255]),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate), tempfile.TemporaryDirectory() as temporary:
                run = Path(temporary) / run_id
                run.mkdir()
                envelope = successful_envelope(manifest, run_id)
                mutate(envelope)
                (run / "run.json").write_text(json.dumps(envelope))
                with mock.patch.object(execution, "DEFAULT_RUN_ROOT", Path(temporary)), \
                     mock.patch.object(stage, "_adb") as adb, \
                     mock.patch.object(stage.npu_lab, "AdbTransport") as transport:
                    with self.assertRaises(execution.ExecutionPreflightError):
                        stage.cleanup(manifest, "fp16-conv", run_id)
                    adb.assert_not_called()
                    transport.assert_not_called()

    def test_cleanup_inventory_rejects_symlink_extra_file_or_directory(self):
        manifest = manifest_for()
        root = manifest.policy.remote_root
        correct = {path: "D drwxr-xr-x" for path in
                   [root, root + "/bin", root + "/lib", root + "/model"]}
        correct.update({path: "F " + mode for path, mode in manifest.policy.staged_modes.items()})
        for mutate in (
            lambda inventory: inventory.update({root: "L lrwxrwxrwx"}),
            lambda inventory: inventory.update({root + "/surprise": "F -r--r--r--"}),
            lambda inventory: inventory.update({root + "/surprise": "D drwxr-xr-x"}),
        ):
            wrong = copy.deepcopy(correct)
            mutate(wrong)
            with mock.patch.object(stage, "_inventory", return_value=wrong):
                with self.assertRaises(execution.ExecutionPreflightError):
                    stage._verify_inventory(manifest)

    def test_service_respawn_is_a_health_difference(self):
        manifest = manifest_for()
        before = {"processes": ["2092 root /bin/sh /etc/init.d/S60waved start",
                                "2197 root /data/waved/waved"]}
        after = {"processes": ["2092 root /bin/sh /etc/init.d/S60waved start",
                               "9999 root /data/waved/waved"]}
        with mock.patch.object(stage.compile_lab, "compare_health", return_value=([], [])):
            differences, alerts = stage._health(manifest, before, after)
        self.assertEqual(differences, ["service process identities changed"])
        self.assertEqual(alerts, [])

    def test_mocked_stage_and_cleanup_keep_exact_three_payload_boundary(self):
        manifest = manifest_for("u8-fc")
        remote_root = manifest.policy.remote_root
        inventory = {path: "D drwxr-xr-x" for path in
                     [remote_root, remote_root + "/bin", remote_root + "/lib", remote_root + "/model"]}
        inventory.update({path: "F " + mode for path, mode in manifest.policy.staged_modes.items()})
        calls = []

        def adb(_manifest, *arguments):
            calls.append(arguments)
            if arguments[0] == "push":
                return ""
            command = arguments[1]
            if command.startswith("find "):
                return "\n".join(value + " " + path for path, value in inventory.items())
            if "sha256sum" in command:
                import shlex
                remote = shlex.split(command)[-1].removesuffix(".incoming")
                return manifest.required_artifacts[remote] + "  " + remote
            if command.startswith("if [ -e "):
                return "ABSENT"
            return ""

        with tempfile.TemporaryDirectory() as temporary:
            local_root = Path(temporary)
            run_id = "fake-successful-cleanup"
            run = local_root / "runs" / run_id
            run.mkdir(parents=True)
            (run / "run.json").write_text(json.dumps(successful_envelope(manifest, run_id)))
            full = snapshot_for(manifest)
            (run / "postflight.json").write_text(json.dumps(full))
            files = {path: (local_root / Path(path).name, "0555" if "/bin/" in path else "0444")
                     for path in manifest.policy.staged_modes}
            with mock.patch.object(stage.compile_lab, "REPOSITORY_ROOT", local_root), \
                 mock.patch.object(execution, "DEFAULT_RUN_ROOT", local_root / "runs"), \
                 mock.patch.object(stage, "verify_local_files", return_value=files), \
                 mock.patch.object(stage, "_adb", side_effect=adb), \
                 mock.patch.object(stage.compile_lab, "collect_snapshot", side_effect=[
                     snapshot_for(manifest, True, False), snapshot_for(manifest, True),
                     snapshot_for(manifest, True), snapshot_for(manifest, True, False)]), \
                 mock.patch.object(execution, "collect_snapshot", return_value=full), \
                 mock.patch.object(stage.compiler_stage, "align_cleanup_diagnostics", create=True):
                self.assertEqual(stage.stage(manifest, "u8-fc")["status"], "STAGED")
                self.assertEqual(stage.cleanup(manifest, "u8-fc", run_id)["status"], "CLEAN")
            pushes = [call for call in calls if call[0] == "push"]
            self.assertEqual(len(pushes), 3)
            removals = [call[1] for call in calls if call[0] == "shell" and call[1].startswith("rm ")]
            self.assertEqual(len(removals), 1)
            for path in manifest.policy.staged_modes:
                self.assertIn(path, removals[0])
            self.assertNotIn(" -r", removals[0])
            self.assertNotIn("*", removals[0])
            self.assertTrue((run / "cleanup.json").is_file())


if __name__ == "__main__":
    unittest.main()
