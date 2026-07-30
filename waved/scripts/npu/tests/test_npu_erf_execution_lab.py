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

import npu_erf_execution_lab  # noqa: E402


class ExecutionManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_erf_execution_lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(
            npu_erf_execution_lab.DEFAULT_MANIFEST.read_text(
                encoding="utf-8"
            )
        )

    def _load_document(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_erf_execution_lab.load_manifest(path)

    def test_manifest_is_fixed_one_shot_execution(self):
        self.assertEqual(
            self.manifest.remote_executable,
            npu_erf_execution_lab.REMOTE_EXECUTABLE,
        )
        self.assertEqual(self.manifest.remote_argument, "--execute-once")
        self.assertEqual(self.manifest.timeout_ms, 12000)
        self.assertEqual(self.manifest.candidates, {})
        self.assertEqual(
            self.manifest.required_artifacts[
                npu_erf_execution_lab.REMOTE_NBG
            ],
            (
                "6b2ac998909fecfdf8243e0b1e2b283e"
                "5abb485d09d579ae818fbc59824de3ee"
            ),
        )
        self.assertRegex(self.manifest.sha256, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_arbitrary_executable_or_argument(self):
        document = self._document()
        document["remote_executable"] = "/tmp/other"
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "invocation",
        ):
            self._load_document(document)

        document = self._document()
        document["remote_argument"] = "--benchmark"
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "invocation",
        ):
            self._load_document(document)

    def test_manifest_rejects_iteration_or_output_change(self):
        document = self._document()
        document["expected_result"]["iterations"] = 2
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "expected result",
        ):
            self._load_document(document)

        document = self._document()
        document["expected_result"]["output"] = [1, 0, 0, 0]
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "expected result",
        ):
            self._load_document(document)

    def test_manifest_requires_exact_model_hash_and_clc_absence(self):
        document = self._document()
        document["required_artifacts"][
            npu_erf_execution_lab.REMOTE_NBG
        ] = "0" * 64
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "fixed artifact",
        ):
            self._load_document(document)

        document = self._document()
        document["absent_artifacts"].remove("/usr/lib/libCLC.so")
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionManifestError,
            "CLC isolation",
        ):
            self._load_document(document)

    def test_manifest_rejects_loader_interposition(self):
        document = self._document()
        document["environment"]["LD_PRELOAD"] = "/tmp/hook.so"
        with self.assertRaises(
            npu_erf_execution_lab.ExecutionManifestError
        ):
            self._load_document(document)


class ExecutionResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_erf_execution_lab.load_manifest()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "EXECUTION status=PASS mode=execute-once iterations=1 "
            "input=0,0,0,0 output=0,0,0,0 bytes=4"
        )

    def test_exact_result_passes(self):
        parsed = npu_erf_execution_lab.parse_result(
            f"{self.identity}\n{self.result}", self.manifest
        )
        self.assertEqual(parsed["iterations"], 1)
        self.assertEqual(parsed["output"], [0, 0, 0, 0])

    def test_result_rejects_wrong_output_or_duplicate(self):
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionResultError, "malformed"
        ):
            npu_erf_execution_lab.parse_result(
                f"{self.identity}\n"
                f"{self.result.replace('output=0,0,0,0', 'output=1,0,0,0')}",
                self.manifest,
            )
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionResultError, "exactly one"
        ):
            npu_erf_execution_lab.parse_result(
                f"{self.identity}\n{self.result}\n{self.result}",
                self.manifest,
            )

    def test_result_rejects_foreign_identity(self):
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionResultError, "identity"
        ):
            npu_erf_execution_lab.parse_result(
                f"{self.identity.replace('0x1000000e', '0x10000020')}\n"
                f"{self.result}",
                self.manifest,
            )

    def test_remote_command_is_one_shot_without_retry(self):
        sentinel = (
            "__WAVE_EXEC_EXIT_0123456789abcdef0123456789abcdef__"
        )
        command = npu_erf_execution_lab.build_remote_command(
            self.manifest, sentinel
        )
        self.assertIn("env -i", command)
        self.assertEqual(command.count("--execute-once"), 1)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("--benchmark", command)
        self.assertNotIn("/tmp/wave-npu-compile", command)

    def test_remote_exit_sentinel_is_unique(self):
        sentinel = (
            "__WAVE_EXEC_EXIT_0123456789abcdef0123456789abcdef__"
        )
        cleaned, status = npu_erf_execution_lab.parse_remote_exit(
            f"hello\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "hello")
        self.assertEqual(status, 0)
        with self.assertRaises(
            npu_erf_execution_lab.ExecutionResultError
        ):
            npu_erf_execution_lab.parse_remote_exit(
                f"{sentinel}:0\n{sentinel}:0\n", sentinel
            )


class NonzeroExecutionPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = npu_erf_execution_lab.POC7_POLICY
        self.manifest = npu_erf_execution_lab.load_manifest(
            policy=self.policy
        )
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.result = (
            "EXECUTION status=PASS mode=execute-once iterations=1 "
            "input=0,1,2,255 output=0,1,1,1 bytes=4"
        )

    def test_nonzero_policy_is_isolated_and_fixed(self):
        self.assertEqual(self.manifest.policy.poc_id, "7")
        self.assertEqual(
            self.manifest.remote_executable,
            "/tmp/wave-npu-exec7/bin/"
            "wave-npu-execute-erf-nonzero-v1",
        )
        self.assertEqual(
            self.manifest.expected_result["input"], [0, 1, 2, 255]
        )
        self.assertEqual(
            self.manifest.expected_result["output"], [0, 1, 1, 1]
        )
        self.assertNotIn(
            "/tmp/wave-npu-exec/lib/libCLC.so",
            self.manifest.absent_artifacts,
        )
        self.assertIn(
            "/tmp/wave-npu-exec7/lib/libCLC.so",
            self.manifest.absent_artifacts,
        )

    def test_nonzero_result_passes_and_zero_result_is_rejected(self):
        parsed = npu_erf_execution_lab.parse_result(
            f"{self.identity}\n{self.result}", self.manifest
        )
        self.assertEqual(parsed["output"], [0, 1, 1, 1])
        with self.assertRaises(
            npu_erf_execution_lab.ExecutionResultError
        ):
            npu_erf_execution_lab.parse_result(
                f"{self.identity}\n"
                f"{self.result.replace('output=0,1,1,1', 'output=0,0,0,0')}",
                self.manifest,
            )

    def test_nonzero_command_cannot_select_zero_poc(self):
        sentinel = (
            "__WAVE_EXEC_EXIT_0123456789abcdef0123456789abcdef__"
        )
        command = npu_erf_execution_lab.build_remote_command(
            self.manifest, sentinel
        )
        self.assertIn("/tmp/wave-npu-exec7/", command)
        self.assertNotIn("/tmp/wave-npu-exec/", command)
        self.assertEqual(command.count("--execute-once"), 1)


class ExecutionHealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_erf_execution_lab.load_manifest()
        self.snapshot = {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "0b8091ee-557d-4857-8019-af1aaab9d665",
            "uptime": "100.0 90.0",
            "galcore_device": (
                "crw-rw---- 1 0 0 199, 0 Jul 28 12:00 /dev/galcore"
            ),
            "npu_parameters": dict(self.manifest.npu_parameters),
            "galcore_holders": [],
            "processes": [],
            "artifact_hashes": dict(self.manifest.required_artifacts),
            "output_states": {},
            "absent_artifact_states": {
                path: {"status": "absent"}
                for path in self.manifest.absent_artifacts
            },
            "staged_artifact_states": {
                path: {
                    "status": "present",
                    "permissions": mode,
                    "bytes": (
                        5840
                        if path == npu_erf_execution_lab.REMOTE_NBG
                        else 100
                    ),
                    "sha256": self.manifest.required_artifacts[path],
                }
                for path, mode in (
                    npu_erf_execution_lab.EXPECTED_STAGED_MODES.items()
                )
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self):
        npu_erf_execution_lab.verify_preflight(
            self.manifest, self.snapshot
        )

    def test_preflight_rejects_clc_or_holder(self):
        changed = copy.deepcopy(self.snapshot)
        changed["absent_artifact_states"][
            "/tmp/wave-npu-exec/lib/libCLC.so"
        ] = {"status": "present"}
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionPreflightError,
            "forbidden compiler",
        ):
            npu_erf_execution_lab.verify_preflight(
                self.manifest, changed
            )

        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 9, "fd": 4}]
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionPreflightError,
            "already held",
        ):
            npu_erf_execution_lab.verify_preflight(
                self.manifest, changed
            )

    def test_preflight_rejects_competing_executor_or_wrong_mode(self):
        changed = copy.deepcopy(self.snapshot)
        changed["processes"] = [
            "123 root /tmp/wave-npu-exec/bin/"
            "wave-npu-execute-erf-zero-v1 --execute-once"
        ]
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionPreflightError,
            "already present",
        ):
            npu_erf_execution_lab.verify_preflight(
                self.manifest, changed
            )

        changed = copy.deepcopy(self.snapshot)
        changed["staged_artifact_states"][
            npu_erf_execution_lab.REMOTE_NBG
        ]["permissions"] = "-rw-r--r--"
        with self.assertRaisesRegex(
            npu_erf_execution_lab.ExecutionPreflightError,
            "metadata mismatch",
        ):
            npu_erf_execution_lab.verify_preflight(
                self.manifest, changed
            )

    def test_health_gate_detects_new_fault_and_clc_visibility(self):
        changed = copy.deepcopy(self.snapshot)
        changed["uptime"] = "101.0 91.0"
        changed["dmesg_tail"].append("[2.0] galcore MMU fault")
        changed["absent_artifact_states"]["/usr/lib/libCLC.so"] = {
            "status": "present"
        }
        differences, alerts = npu_erf_execution_lab.compare_health(
            self.manifest, self.snapshot, changed
        )
        self.assertTrue(alerts)
        self.assertIn("CLC isolation state changed", differences)
        self.assertIn(
            "a forbidden compiler artifact is visible", differences
        )


if __name__ == "__main__":
    unittest.main()
