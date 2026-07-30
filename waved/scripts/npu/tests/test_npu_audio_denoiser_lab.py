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

import npu_audio_denoiser_lab  # noqa: E402


class AudioManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_audio_denoiser_lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(
            npu_audio_denoiser_lab.DEFAULT_MANIFEST.read_text(
                encoding="utf-8"
            )
        )

    def _load_document(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_audio_denoiser_lab.load_manifest(path)

    def test_manifest_is_fixed_direct_graph_execution(self):
        self.assertEqual(
            self.manifest.remote_executable,
            npu_audio_denoiser_lab.REMOTE_EXECUTABLE,
        )
        self.assertEqual(self.manifest.remote_argument, "--execute-once")
        self.assertEqual(self.manifest.timeout_ms, 12000)
        self.assertEqual(self.manifest.candidates, {})
        self.assertEqual(
            self.manifest.expected_result["backend"], "OPENVX"
        )
        self.assertEqual(
            self.manifest.expected_result["output_energy"], "0.125000"
        )
        self.assertRegex(self.manifest.sha256, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_invocation_or_energy_change(self):
        document = self._document()
        document["remote_argument"] = "--benchmark"
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioManifestError,
            "invocation",
        ):
            self._load_document(document)

        document = self._document()
        document["expected_result"]["output_energy"] = "1.000000"
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioManifestError,
            "expected result",
        ):
            self._load_document(document)

    def test_manifest_requires_runner_hash_and_clc_absence(self):
        document = self._document()
        document["required_artifacts"][
            npu_audio_denoiser_lab.REMOTE_EXECUTABLE
        ] = "0" * 64
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioManifestError,
            "fixed artifact",
        ):
            self._load_document(document)

        document = self._document()
        document["absent_artifacts"].remove("/usr/lib/libCLC.so")
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioManifestError,
            "CLC isolation",
        ):
            self._load_document(document)


class AudioResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_audio_denoiser_lab.load_manifest()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.selector = (
            'D [selector]Instance OPENVX node with kernel "matrixmul"'
        )
        self.result = (
            "AUDIO status=PASS mode=direct-graph iterations=1 "
            "input_energy=16.000000 output_energy=0.125000 "
            "max_error=0.000000"
        )

    def test_exact_result_and_selector_pass(self):
        parsed = npu_audio_denoiser_lab.parse_result(
            f"{self.identity}\n{self.selector}\n{self.result}",
            self.manifest,
        )
        self.assertEqual(parsed["selector"]["backend"], "OPENVX")
        self.assertEqual(parsed["energy_ratio"], 0.0078125)

    def test_wrong_backend_or_energy_is_rejected(self):
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioResultError,
            "selected EVIS",
        ):
            npu_audio_denoiser_lab.parse_result(
                f"{self.identity}\n"
                f"{self.selector.replace('OPENVX', 'EVIS')}\n"
                f"{self.result}",
                self.manifest,
            )
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioResultError,
            "malformed",
        ):
            npu_audio_denoiser_lab.parse_result(
                f"{self.identity}\n{self.selector}\n"
                f"{self.result.replace('0.125000', '1.000000')}",
                self.manifest,
            )

    def test_remote_command_is_one_shot_without_export_or_retry(self):
        sentinel = (
            "__WAVE_AUDIO_EXIT_0123456789abcdef0123456789abcdef__"
        )
        command = npu_audio_denoiser_lab.build_remote_command(
            self.manifest, sentinel
        )
        self.assertEqual(command.count("--execute-once"), 1)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("candidate", command)
        self.assertNotIn("/tmp/wave-npu-compile", command)

    def test_remote_exit_sentinel_is_unique(self):
        sentinel = (
            "__WAVE_AUDIO_EXIT_0123456789abcdef0123456789abcdef__"
        )
        cleaned, status = npu_audio_denoiser_lab.parse_remote_exit(
            f"hello\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "hello")
        self.assertEqual(status, 0)
        with self.assertRaises(
            npu_audio_denoiser_lab.AudioResultError
        ):
            npu_audio_denoiser_lab.parse_remote_exit(
                f"{sentinel}:0\n{sentinel}:0\n", sentinel
            )


class AudioHealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_audio_denoiser_lab.load_manifest()
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
            "absent_artifact_states": {
                path: {"status": "absent"}
                for path in self.manifest.absent_artifacts
            },
            "staged_artifact_states": {
                path: {
                    "status": "present",
                    "permissions": mode,
                    "bytes": 100,
                    "sha256": self.manifest.required_artifacts[path],
                }
                for path, mode in (
                    npu_audio_denoiser_lab.EXPECTED_STAGED_MODES.items()
                )
            },
            "output_states": {},
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self):
        npu_audio_denoiser_lab.verify_preflight(
            self.manifest, self.snapshot
        )

    def test_preflight_rejects_clc_holder_or_runner(self):
        changed = copy.deepcopy(self.snapshot)
        changed["absent_artifact_states"][
            "/tmp/wave-npu-audio8b/lib/libCLC.so"
        ] = {"status": "present"}
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioPreflightError,
            "forbidden compiler",
        ):
            npu_audio_denoiser_lab.verify_preflight(
                self.manifest, changed
            )

        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 7, "fd": 3}]
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioPreflightError,
            "already held",
        ):
            npu_audio_denoiser_lab.verify_preflight(
                self.manifest, changed
            )

        changed = copy.deepcopy(self.snapshot)
        changed["processes"] = [
            "7 root wave-npu-run-audio-denoiser-v1 --execute-once"
        ]
        with self.assertRaisesRegex(
            npu_audio_denoiser_lab.AudioPreflightError,
            "already present",
        ):
            npu_audio_denoiser_lab.verify_preflight(
                self.manifest, changed
            )

    def test_health_gate_detects_new_fault(self):
        changed = copy.deepcopy(self.snapshot)
        changed["uptime"] = "101.0 91.0"
        changed["dmesg_tail"].append("[2.0] galcore MMU fault")
        differences, alerts = npu_audio_denoiser_lab.compare_health(
            self.manifest, self.snapshot, changed
        )
        self.assertTrue(alerts)
        self.assertIn(
            "new kernel warning/error diagnostics were observed",
            differences,
        )


if __name__ == "__main__":
    unittest.main()
