import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_onnx_native_lab as lab


class NativeBridgeTests(unittest.TestCase):
    def test_package_hash_mismatch_refuses_before_transport(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "package.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "model_sha256": "0" * 64,
                        "lowering_sha256": "1" * 64,
                        "files": {"model.onnx": "0" * 64},
                    }
                )
            )
            (root / "model.onnx").write_bytes(b"changed")
            with mock.patch.object(lab.npu_lab, "AdbTransport") as transport:
                with self.assertRaises(lab.NativeLabError):
                    lab.load_package(root)
                transport.assert_not_called()

    def test_root_is_derived_from_lowering_digest(self):
        record = {"lowering_sha256": "ab" * 32}
        self.assertEqual(
            lab.remote_root(record, "compile"),
            "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile",
        )
        for kind in ("../execute", "compile;id"):
            with self.assertRaises(lab.NativeLabError):
                lab.remote_root(record, kind)

    def test_service_respawn_is_a_health_difference(self):
        manifest = mock.Mock(serial="wave", policy=mock.Mock(poc_id="17"))
        before = {"processes": ["2197 root /data/waved/waved"]}
        after = {"processes": ["2201 root /data/waved/waved"]}
        with mock.patch.object(
            lab.compile_lab, "compare_health", return_value=([], [])
        ):
            differences, alerts = lab.health(manifest, before, after)
        self.assertIn("service process identities changed", differences)
        self.assertEqual(alerts, [])

    def test_inventory_rejects_links_and_unexpected_files(self):
        payloads = {
            "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile/bin/probe": ("a", "0555")
        }
        root = "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile"
        expected = lab.expected_inventory(root, payloads)
        observed = dict(expected)
        observed[root + "/bin/probe"] = "L lrwxrwxrwx"
        with self.assertRaises(lab.NativeLabError):
            lab.validate_inventory(expected, observed)
        observed = dict(expected)
        observed[root + "/unknown"] = "F -r--r--r--"
        with self.assertRaises(lab.NativeLabError):
            lab.validate_inventory(expected, observed)

    def test_inventory_includes_empty_output_directory(self):
        root = "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile"
        expected = lab.expected_inventory(root, {root + "/bin/probe": ("a", "0555")})
        self.assertEqual(expected[root + "/output"], "D drwxr-xr-x")

    def test_shared_compile_health_detects_17_service_respawn(self):
        manifest = mock.Mock(serial="wave", policy=mock.Mock(poc_id="17"))
        before = {
            "serials": ["wave"],
            "dmesg_tail": ["marker"],
            "processes": ["2197 root /data/waved/waved"],
        }
        after = {**before, "processes": ["2201 root /data/waved/waved"]}
        differences, _ = lab.compile_lab.compare_health(manifest, before, after)
        self.assertIn("service process identities changed", differences)

    def test_postflight_fallback_retains_fault_detection(self):
        manifest = mock.Mock(policy=mock.Mock(poc_id="17"))
        before = {"dmesg_tail": ["original marker"]}
        after = {"dmesg_tail": ["rotated window"], "errors": []}
        with mock.patch.object(
            lab.compile_lab,
            "_shell_value",
            return_value="original marker\ngalcore kernel panic\n",
        ):
            lab.compile_lab.align_native_diagnostics(
                mock.Mock(), manifest, before, after
            )
        self.assertEqual(
            after["dmesg_tail"], ["original marker", "galcore kernel panic"]
        )

    def test_elf_policy_rejects_clc_and_wrong_runpath(self):
        report = """Class: ELF64
Data: 2's complement, little endian
Machine: AArch64
Type: DYN
GNU_RELRO
GNU_STACK 0 0 0 0 0 RW 0
BIND_NOW
(NEEDED) Shared library: [libtim-vx.so]
(NEEDED) Shared library: [libGAL.so]
(NEEDED) Shared library: [libc.so.6]
(RUNPATH) Library runpath: [ROOT/lib:/usr/lib]
gcoHAL_QueryChipIdentityEx
GLIBC_2.17 GLIBCXX_3.4 CXXABI_1.3
"""
        root = "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile"
        good = report.replace("ROOT", root)
        lab.validate_elf(good, root)
        for changed in (
            good + "(NEEDED) Shared library: [libCLC.so]\n",
            good.replace(root + "/lib", "/another/lib"),
            good.replace("AArch64", "X86-64"),
        ):
            with self.assertRaises(lab.NativeLabError):
                lab.validate_elf(changed, root)

    def test_cleanup_refuses_health_change_during_inventory_before_deletion(self):
        root = "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile"
        remote = root + "/bin/probe"
        payloads = {remote: ("local", "0555")}
        candidate = lab.compile_lab.Candidate(
            "candidate-1",
            "--candidate-1",
            root + "/output/model.nbg",
            root + "/output/.model.nbg.partial",
        )
        policy = mock.Mock(poc_id="17", absent_artifacts=())
        manifest = lab.compile_lab.CompileManifest(
            policy=policy,
            serial=lab.DEVICE["serial"],
            kernel=lab.DEVICE["kernel"],
            galcore_major_minor="199:0",
            npu_parameters=lab.DEVICE["npu_parameters"],
            environment={},
            remote_executable=remote,
            timeout_ms=20000,
            required_artifacts={**lab.SYSTEM_HASHES, remote: "a" * 64},
            candidates={"candidate-1": candidate},
            expected_nbg={},
            sha256="b" * 64,
        )
        before = {
            "serials": [manifest.serial],
            "kernel": manifest.kernel,
            "boot_id": "12345678-1234-1234-1234-123456789abc",
            "galcore_device": "crw 199, 0",
            "npu_parameters": dict(manifest.npu_parameters),
            "galcore_holders": [],
            "artifact_hashes": dict(manifest.required_artifacts),
            "absent_artifact_states": {},
            "output_states": {
                candidate.output: {"status": "absent"},
                candidate.partial_output: {"status": "absent"},
            },
            "dmesg_tail": ["marker"],
            "errors": [],
            "processes": ["2197 root /data/waved/waved"],
        }
        for changed in (
            {"processes": ["2201 root /data/waved/waved"]},
            {"dmesg_tail": ["marker", "galcore kernel panic"]},
        ):
            with (
                self.subTest(changed=changed),
                tempfile.TemporaryDirectory() as temporary,
            ):
                run_dir = Path(temporary)
                (run_dir / "postflight.json").write_text(json.dumps(before))
                system = {
                    **before,
                    "artifact_hashes": dict(lab.SYSTEM_HASHES),
                    **changed,
                }

                def adb(_manifest, *args):
                    if args[0] == "shell" and args[1].startswith("if "):
                        return "ABSENT"
                    return "a" * 64 + " " + remote

                with (
                    mock.patch.object(lab.npu_lab, "AdbTransport"),
                    mock.patch.object(lab, "_snapshot", return_value=before),
                    mock.patch.object(
                        lab.compile_lab, "collect_snapshot", return_value=system
                    ),
                    mock.patch.object(
                        lab,
                        "_inventory",
                        return_value=lab.expected_inventory(root, payloads),
                    ),
                    mock.patch.object(lab, "_adb", side_effect=adb) as adb_call,
                ):
                    with self.assertRaises(lab.NativeLabError):
                        lab._cleanup(
                            manifest,
                            payloads,
                            root,
                            {"status": "PASS", "transport": {"remote_exit": 0}},
                            run_dir,
                        )
                    self.assertFalse(
                        any(
                            call.args[2].startswith(("rm ", "rmdir "))
                            for call in adb_call.call_args_list
                        )
                    )

    def test_stage_checks_local_hash_before_any_device_call(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "probe"
            path.write_bytes(b"changed")
            remote = "/tmp/wave-npu-onnx17-" + "ab" * 8 + "-compile/bin/probe"
            manifest = mock.Mock(required_artifacts={remote: "0" * 64})
            with (
                mock.patch.object(lab, "_verify_runtime"),
                mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
            ):
                with self.assertRaises(lab.NativeLabError):
                    lab._stage(
                        manifest,
                        {remote: (path, "0555")},
                        remote.rsplit("/bin/", 1)[0],
                        Path(temporary),
                    )
                transport.assert_not_called()

    def test_qualification_invalid_run_id_does_not_write_attempt(self):
        with tempfile.TemporaryDirectory() as temporary:
            with mock.patch.object(lab, "load_package") as loader:
                with self.assertRaises(lab.NativeLabError):
                    lab.qualify_package(Path(temporary), "input.npz", "../bad")
                loader.assert_not_called()
            self.assertFalse((Path(temporary) / "qualification-attempt.json").exists())

    def test_cleanup_uncertain_exit_refuses_before_device_contact(self):
        for state in (
            {"remote_exit": None},
            {"remote_exit": 0, "timed_out": True},
            {"remote_exit": 124},
        ):
            with (
                self.subTest(state=state),
                mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
            ):
                with self.assertRaises(lab.NativeLabError):
                    lab._cleanup(
                        mock.Mock(),
                        {},
                        "/tmp/fake",
                        {"transport": state},
                        Path("/unused"),
                    )
                transport.assert_not_called()

    def test_runtime_oracle_validation_does_not_create_cpu_session(self):
        import numpy as np

        importer, _ = lab._host()
        fixture = (
            lab.REPOSITORY_ROOT
            / "waved/tools/onnx-hybrid/fixtures/native-poc17/17a1.onnx"
        )
        record = importer.import_native_model(fixture)
        values = np.array([1, -1] * 8, dtype=np.float16).reshape(1, 1, 16)
        with mock.patch(
            "wave_onnx_hybrid.runtime.execute_cpu_model",
            side_effect=AssertionError("CPU session"),
        ) as cpu:
            _, output = lab._cpu_expected(
                {"record": record},
                {record["onnx_input"]["name"]: values},
                verify_onnx_runtime=False,
            )
            self.assertEqual(len(output), 32)
            cpu.assert_not_called()

    def test_runtime_oracle_nonfinite_output_refuses_without_cpu(self):
        import numpy as np

        importer, _ = lab._host()
        record = importer.import_native_model(
            lab.REPOSITORY_ROOT
            / "waved/tools/onnx-hybrid/fixtures/native-poc17/17a2.onnx"
        )
        values = np.zeros((1, 1, 1, 16), dtype=np.float16)
        with (
            mock.patch.object(
                importer,
                "independent_oracle",
                return_value={
                    record["onnx_output"]["name"]: np.full(
                        values.shape, np.inf, dtype=np.float16
                    )
                },
            ),
            mock.patch("wave_onnx_hybrid.runtime.execute_cpu_model") as cpu,
        ):
            with self.assertRaises(lab.NativeLabError):
                lab._cpu_expected(
                    {"record": record},
                    {record["onnx_input"]["name"]: values},
                    verify_onnx_runtime=False,
                )
            cpu.assert_not_called()


if __name__ == "__main__":
    unittest.main()
