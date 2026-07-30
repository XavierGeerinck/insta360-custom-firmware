from __future__ import annotations

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_compile_lab  # noqa: E402


class CompileManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(npu_compile_lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load_document(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_compile_lab.load_manifest(path)

    def test_manifest_is_compile_only_and_fixed(self):
        self.assertEqual(
            self.manifest.remote_executable,
            "/tmp/wave-npu-compile/bin/wave-npu-compile-add-v2",
        )
        self.assertEqual(set(self.manifest.candidates), {"candidate-1", "candidate-2"})
        self.assertEqual(self.manifest.timeout_ms, 20000)
        self.assertRegex(self.manifest.sha256, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_arbitrary_executable(self):
        document = self._document()
        document["remote_executable"] = "/tmp/other"
        with self.assertRaisesRegex(npu_compile_lab.ManifestError, "remote executable"):
            self._load_document(document)

    def test_manifest_rejects_arbitrary_output(self):
        document = self._document()
        document["candidates"]["candidate-1"]["output"] = "/tmp/other.nbg"
        with self.assertRaisesRegex(npu_compile_lab.ManifestError, "fixed policy"):
            self._load_document(document)

    def test_manifest_rejects_loader_interposition_key(self):
        document = self._document()
        document["environment"]["LD_PRELOAD"] = "/tmp/hook.so"
        with self.assertRaises(npu_compile_lab.ManifestError):
            self._load_document(document)


class ErfCompilePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest(policy=npu_compile_lab.ERF_POLICY)
        self.candidate = self.manifest.candidates["candidate-1"]

    def _document(self) -> dict:
        return json.loads(
            npu_compile_lab.ERF_POLICY.manifest_path.read_text(encoding="utf-8")
        )

    def _load_document(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_compile_lab.load_manifest(
                path, policy=npu_compile_lab.ERF_POLICY
            )

    def test_erf_manifest_is_separate_and_fixed(self):
        self.assertEqual(self.manifest.policy.operation, "Erf")
        self.assertEqual(
            self.manifest.remote_executable,
            "/tmp/wave-npu-compile/bin/wave-npu-compile-erf-v1",
        )
        self.assertEqual(self.manifest.policy.input_count, 1)
        self.assertEqual(self.manifest.policy.output_count, 1)
        self.assertEqual(self.manifest.policy.inputs[0].dimensions, (4,))
        self.assertEqual(self.manifest.policy.inputs[0].byte_size, 4)

    def test_erf_manifest_rejects_add_executable(self):
        document = self._document()
        document["remote_executable"] = npu_compile_lab.ADD_POLICY.remote_executable
        with self.assertRaisesRegex(npu_compile_lab.ManifestError, "remote executable"):
            self._load_document(document)

    def test_erf_manifest_rejects_add_output(self):
        document = self._document()
        document["candidates"]["candidate-1"]["output"] = (
            "/tmp/wave-npu-compile/output/wave-add-candidate-1.nbg"
        )
        with self.assertRaisesRegex(npu_compile_lab.ManifestError, "fixed policy"):
            self._load_document(document)

    def test_erf_result_and_shape_policy(self):
        identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=4096 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile/output/"
            "wave-erf-candidate-1.nbg"
        )
        parsed = npu_compile_lab.parse_native_result(
            f'{identity}\nD [selector]Instance OPENVX node with kernel "erf"\n{result}',
            self.manifest,
            self.candidate,
        )
        self.assertEqual(parsed["result"]["bytes"], 4096)
        self.assertEqual(parsed["selector"]["backend"], "OPENVX")
        report = {
            "magic": "VPMN",
            "version_hex": "0x00020000",
            "product_hex": "0x1000000e",
            "size": 4096,
            "inputs": [
                {
                    "dimensions": [4],
                    "data_format_name": "uint8",
                    "byte_size": 4,
                    "quantization": {"format_name": "affine-asymmetric"},
                }
            ],
            "outputs": [
                {
                    "dimensions": [4],
                    "data_format_name": "uint8",
                    "byte_size": 4,
                    "quantization": {"format_name": "affine-asymmetric"},
                }
            ],
        }
        npu_compile_lab.validate_nbg_report(report, self.manifest)
        report["inputs"][0]["dimensions"] = [2]
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "shape mismatch"):
            npu_compile_lab.validate_nbg_report(report, self.manifest)


class SignalFrameCompilePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest(
            policy=npu_compile_lab.SIGNAL_FRAME_POLICY
        )
        self.candidate = self.manifest.candidates["candidate-1"]
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=8192 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile/output/"
            "wave-signal-frame-candidate-1.nbg"
        )

    def test_signal_frame_manifest_is_separate_and_fixed(self):
        policy = self.manifest.policy
        self.assertEqual(policy.operation, "SignalFrame")
        self.assertEqual(policy.selector_backend, "EVIS")
        self.assertEqual(policy.selector_kernel, "signal_frame")
        self.assertEqual(policy.inputs[0].dimensions, (32,))
        self.assertEqual(policy.inputs[0].data_format_name, "fp16")
        self.assertEqual(policy.inputs[0].byte_size, 64)
        self.assertEqual(policy.outputs[0].dimensions, (8, 7))
        self.assertEqual(policy.outputs[0].byte_size, 112)

    def test_signal_frame_requires_exact_evis_selection(self):
        stdout = (
            f"{self.identity}\n"
            'D [selector]Instance EVIS node with kernel "signal_frame"\n'
            f"{self.result}"
        )
        parsed = npu_compile_lab.parse_native_result(
            stdout, self.manifest, self.candidate
        )
        self.assertEqual(
            parsed["selector"],
            {"backend": "EVIS", "kernel": "signal_frame"},
        )

        wrong_backend = stdout.replace("Instance EVIS", "Instance OPENVX")
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "selected OPENVX"):
            npu_compile_lab.parse_native_result(
                wrong_backend, self.manifest, self.candidate
            )

        duplicate = stdout.replace(
            f"{self.result}",
            f'D [selector]Instance EVIS node with kernel "signal_frame"\n{self.result}',
        )
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "observed 2"):
            npu_compile_lab.parse_native_result(
                duplicate, self.manifest, self.candidate
            )

    def test_signal_frame_asymmetric_tensor_policy(self):
        report = {
            "magic": "VPMN",
            "version_hex": "0x00020000",
            "product_hex": "0x1000000e",
            "size": 8192,
            "inputs": [
                {
                    "dimensions": [32],
                    "data_format_name": "fp16",
                    "byte_size": 64,
                    "quantization": {"format_name": "none"},
                }
            ],
            "outputs": [
                {
                    "dimensions": [8, 7],
                    "data_format_name": "fp16",
                    "byte_size": 112,
                    "quantization": {"format_name": "none"},
                }
            ],
        }
        npu_compile_lab.validate_nbg_report(report, self.manifest)
        report["outputs"][0]["dimensions"] = [7, 8]
        with self.assertRaisesRegex(
            npu_compile_lab.ResultError, "output 0 shape mismatch"
        ):
            npu_compile_lab.validate_nbg_report(report, self.manifest)


class SignalFrameClcCompilePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest(
            policy=npu_compile_lab.SIGNAL_FRAME_CLC_POLICY
        )
        self.candidate = self.manifest.candidates["candidate-1"]

    def test_clc_policy_has_new_executable_and_output_namespace(self):
        self.assertEqual(self.manifest.policy.poc_id, "5F-B")
        self.assertEqual(
            self.manifest.remote_executable,
            "/tmp/wave-npu-compile/bin/wave-npu-compile-signal-frame-clc-v1",
        )
        self.assertEqual(
            self.candidate.output,
            "/tmp/wave-npu-compile/output/wave-signal-frame-clc-candidate-1.nbg",
        )
        self.assertIn(
            "/tmp/wave-npu-compile/lib/libCLC.so",
            self.manifest.required_artifacts,
        )
        self.assertEqual(self.manifest.policy.selector_backend, "EVIS")

    def test_clc_result_requires_evis_in_new_namespace(self):
        identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=8192 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile/output/"
            "wave-signal-frame-clc-candidate-1.nbg"
        )
        parsed = npu_compile_lab.parse_native_result(
            f"{identity}\n"
            'D [selector]Instance EVIS node with kernel "signal_frame"\n'
            f"{result}",
            self.manifest,
            self.candidate,
        )
        self.assertEqual(parsed["selector"]["backend"], "EVIS")


class AudioDenoiserCompilePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = npu_compile_lab.AUDIO_DENOISER_POLICY
        self.manifest = npu_compile_lab.load_manifest(policy=self.policy)
        self.candidate = self.manifest.candidates["candidate-1"]
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=8192 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile8/"
            "output/wave-audio-denoiser-candidate-1.nbg"
        )

    def test_audio_policy_is_single_candidate_and_clc_isolated(self):
        self.assertEqual(self.policy.poc_id, "8A")
        self.assertEqual(set(self.manifest.candidates), {"candidate-1"})
        self.assertEqual(
            self.manifest.environment["LD_LIBRARY_PATH"],
            "/tmp/wave-npu-compile8/lib:/usr/lib",
        )
        self.assertEqual(self.policy.inputs[0].dimensions, (16, 1))
        self.assertEqual(self.policy.inputs[0].data_format_name, "fp32")
        self.assertEqual(self.policy.inputs[0].byte_size, 64)
        self.assertEqual(self.policy.selector_backend, "OPENVX")
        self.assertEqual(self.policy.selector_kernel, "matrixmul")
        self.assertIn(
            "/tmp/wave-npu-compile8/lib/libCLC.so",
            self.policy.absent_artifacts,
        )

    def test_audio_result_requires_openvx_matrixmul(self):
        stdout = (
            f"{self.identity}\n"
            'D [selector]Instance OPENVX node with kernel "matrixmul"\n'
            f"{self.result}"
        )
        parsed = npu_compile_lab.parse_native_result(
            stdout, self.manifest, self.candidate
        )
        self.assertEqual(
            parsed["selector"],
            {"backend": "OPENVX", "kernel": "matrixmul"},
        )
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "selected EVIS"):
            npu_compile_lab.parse_native_result(
                stdout.replace("Instance OPENVX", "Instance EVIS"),
                self.manifest,
                self.candidate,
            )

    def test_audio_nbg_shape_and_type_policy(self):
        report = {
            "magic": "VPMN",
            "version_hex": "0x00020000",
            "product_hex": "0x1000000e",
            "size": 8192,
            "inputs": [
                {
                    "dimensions": [16, 1],
                    "data_format_name": "fp32",
                    "byte_size": 64,
                    "quantization": {"format_name": "none"},
                }
            ],
            "outputs": [
                {
                    "dimensions": [16, 1],
                    "data_format_name": "fp32",
                    "byte_size": 64,
                    "quantization": {"format_name": "none"},
                }
            ],
        }
        npu_compile_lab.validate_nbg_report(report, self.manifest)
        report["inputs"].append(copy.deepcopy(report["inputs"][0]))
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "input count"):
            npu_compile_lab.validate_nbg_report(report, self.manifest)

    def test_audio_preflight_rejects_clc_visibility(self):
        snapshot = {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "0b8091ee-557d-4857-8019-af1aaab9d665",
            "uptime": "100.0 90.0",
            "galcore_device": ("crw-rw---- 1 0 0 199, 0 Jul 28 12:00 /dev/galcore"),
            "npu_parameters": dict(self.manifest.npu_parameters),
            "galcore_holders": [],
            "processes": [],
            "artifact_hashes": dict(self.manifest.required_artifacts),
            "absent_artifact_states": {
                path: {"status": "absent"} for path in self.policy.absent_artifacts
            },
            "output_states": {
                self.candidate.output: {"status": "absent"},
                self.candidate.partial_output: {"status": "absent"},
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }
        npu_compile_lab.verify_preflight(self.manifest, self.candidate, snapshot)
        snapshot["absent_artifact_states"]["/tmp/wave-npu-compile8/lib/libCLC.so"] = {
            "status": "present"
        }
        with self.assertRaisesRegex(
            npu_compile_lab.PreflightError,
            "forbidden compiler",
        ):
            npu_compile_lab.verify_preflight(self.manifest, self.candidate, snapshot)


class ConvAudioDenoiserCompilePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = npu_compile_lab.CONV_AUDIO_DENOISER_POLICY
        self.manifest = npu_compile_lab.load_manifest(policy=self.policy)
        self.candidate = self.manifest.candidates["candidate-1"]
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=12288 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile8c/"
            "output/wave-conv-audio-denoiser-candidate-1.nbg"
        )

    def test_conv_policy_is_single_candidate_and_clc_isolated(self):
        self.assertEqual(self.policy.poc_id, "8C")
        self.assertEqual(set(self.manifest.candidates), {"candidate-1"})
        self.assertEqual(
            self.manifest.environment["LD_LIBRARY_PATH"],
            "/tmp/wave-npu-compile8c/lib:/usr/lib",
        )
        self.assertEqual(self.policy.inputs[0].dimensions, (16, 1, 1, 1))
        self.assertEqual(self.policy.inputs[0].data_format_name, "fp32")
        self.assertEqual(self.policy.selector_backend, "OPENVX")
        self.assertEqual(self.policy.selector_kernel, "conv2d")
        self.assertIn(
            "/tmp/wave-npu-compile8c/lib/libCLC.so",
            self.policy.absent_artifacts,
        )

    def test_conv_result_requires_openvx_conv2d(self):
        stdout = (
            f"{self.identity}\n"
            'D [selector]Instance OPENVX node with kernel "conv2d"\n'
            f"{self.result}"
        )
        parsed = npu_compile_lab.parse_native_result(
            stdout, self.manifest, self.candidate
        )
        self.assertEqual(
            parsed["selector"],
            {"backend": "OPENVX", "kernel": "conv2d"},
        )
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "selected EVIS"):
            npu_compile_lab.parse_native_result(
                stdout.replace("Instance OPENVX", "Instance EVIS"),
                self.manifest,
                self.candidate,
            )

    def test_conv_nbg_shape_and_type_policy(self):
        report = {
            "magic": "VPMN",
            "version_hex": "0x00020000",
            "product_hex": "0x1000000e",
            "size": 12288,
            "inputs": [
                {
                    "dimensions": [16, 1, 1, 1],
                    "data_format_name": "fp32",
                    "byte_size": 64,
                    "quantization": {"format_name": "none"},
                }
            ],
            "outputs": [
                {
                    "dimensions": [16, 1, 1, 1],
                    "data_format_name": "fp32",
                    "byte_size": 64,
                    "quantization": {"format_name": "none"},
                }
            ],
        }
        npu_compile_lab.validate_nbg_report(report, self.manifest)
        report["outputs"][0]["dimensions"] = [16, 1]
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "shape mismatch"):
            npu_compile_lab.validate_nbg_report(report, self.manifest)


class NativeResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest()
        self.candidate = self.manifest.candidates["candidate-1"]
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009001 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "RESULT status=PASS mode=compile-only target=wave "
            "candidate=candidate-1 bytes=4096 version=0x00020000 "
            "product=0x1000000e output=/tmp/wave-npu-compile/output/"
            "wave-add-candidate-1.nbg"
        )

    def test_parse_exact_native_result(self):
        parsed = npu_compile_lab.parse_native_result(
            f"prefix\n{self.identity}\n{self.result}\n",
            self.manifest,
            self.candidate,
        )
        self.assertEqual(parsed["identity"]["customer_id"], "0x1000000e")
        self.assertEqual(parsed["result"]["bytes"], 4096)

    def test_rejects_foreign_target(self):
        foreign = self.identity.replace("0x1000000e", "0x10000020")
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "customer/target"):
            npu_compile_lab.parse_native_result(
                f"{foreign}\n{self.result}",
                self.manifest,
                self.candidate,
            )

    def test_rejects_duplicate_result(self):
        with self.assertRaisesRegex(npu_compile_lab.ResultError, "observed"):
            npu_compile_lab.parse_native_result(
                f"{self.identity}\n{self.result}\n{self.result}",
                self.manifest,
                self.candidate,
            )

    def test_remote_exit_sentinel_is_unique(self):
        sentinel = "__WAVE_COMPILE_EXIT_0123456789abcdef0123456789abcdef__"
        cleaned, status = npu_compile_lab.parse_remote_exit(
            f"hello\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "hello")
        self.assertEqual(status, 0)
        with self.assertRaises(npu_compile_lab.ResultError):
            npu_compile_lab.parse_remote_exit(f"{sentinel}:0\n{sentinel}:0\n", sentinel)

    def test_legacy_adb_status_uses_internal_remote_exit(self):
        npu_compile_lab.verify_transport_status(0, 0, "native compiler")
        with self.assertRaisesRegex(
            npu_compile_lab.ResultError, "native compiler exited 5"
        ):
            npu_compile_lab.verify_transport_status(0, 5, "native compiler")
        with self.assertRaisesRegex(
            npu_compile_lab.ResultError, "ADB transport exited 1"
        ):
            npu_compile_lab.verify_transport_status(1, 0, "native compiler")


class HealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_compile_lab.load_manifest()
        self.candidate = self.manifest.candidates["candidate-1"]
        self.snapshot = {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "0b8091ee-557d-4857-8019-af1aaab9d665",
            "uptime": "100.0 90.0",
            "galcore_device": ("crw-rw---- 1 0 0 199, 0 Jul 28 12:00 /dev/galcore"),
            "npu_parameters": dict(self.manifest.npu_parameters),
            "galcore_holders": [],
            "processes": [],
            "artifact_hashes": dict(self.manifest.required_artifacts),
            "output_states": {
                candidate.output: {"status": "absent"}
                for candidate in self.manifest.candidates.values()
            }
            | {
                candidate.partial_output: {"status": "absent"}
                for candidate in self.manifest.candidates.values()
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self):
        npu_compile_lab.verify_preflight(self.manifest, self.candidate, self.snapshot)

    def test_preflight_rejects_existing_output(self):
        changed = copy.deepcopy(self.snapshot)
        changed["output_states"][self.candidate.output] = {
            "status": "present",
            "bytes": 1,
        }
        with self.assertRaisesRegex(npu_compile_lab.PreflightError, "not absent"):
            npu_compile_lab.verify_preflight(self.manifest, self.candidate, changed)

    def test_preflight_rejects_driver_holder(self):
        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 42, "fd": 3}]
        with self.assertRaisesRegex(npu_compile_lab.PreflightError, "already held"):
            npu_compile_lab.verify_preflight(self.manifest, self.candidate, changed)

    def test_health_gate_detects_new_kernel_fault(self):
        changed = copy.deepcopy(self.snapshot)
        changed["uptime"] = "101.0 91.0"
        changed["dmesg_tail"].append("[2.0] galcore MMU fault")
        differences, alerts = npu_compile_lab.compare_health(
            self.manifest, self.snapshot, changed
        )
        self.assertTrue(alerts)
        self.assertIn("new kernel warning/error diagnostics were observed", differences)

    def test_uptime_change_is_not_a_health_failure(self):
        changed = copy.deepcopy(self.snapshot)
        changed["uptime"] = "101.0 91.0"
        differences, alerts = npu_compile_lab.compare_health(
            self.manifest, self.snapshot, changed
        )
        self.assertEqual(differences, [])
        self.assertEqual(alerts, [])

    def test_remote_command_has_no_execution_or_retry_argument(self):
        sentinel = "__WAVE_COMPILE_EXIT_0123456789abcdef0123456789abcdef__"
        command = npu_compile_lab.build_remote_command(
            self.manifest, self.candidate, sentinel
        )
        self.assertIn("env -i", command)
        self.assertIn("--candidate-1", command)
        self.assertNotIn("--run", command)
        self.assertNotIn("retry", command.lower())


if __name__ == "__main__":
    unittest.main()
