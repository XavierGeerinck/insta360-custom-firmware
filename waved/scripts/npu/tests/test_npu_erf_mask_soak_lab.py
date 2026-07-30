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

import npu_erf_mask_soak_lab as lab  # noqa: E402


class SoakManifestAndOracleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load_document(self, document: dict) -> lab.SoakManifest:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path)

    def test_manifest_and_independent_final_digest_are_fixed(self) -> None:
        self.assertEqual(self.manifest.remote_argument, "--soak-10000")
        checkpoints = lab.expected_checkpoints()
        self.assertEqual(len(checkpoints), 10)
        self.assertEqual(
            checkpoints[-1],
            {
                "frames": 10000,
                "input_fnv1a64": "0x1417bf4ceca8d26e",
                "output_fnv1a64": "0xcbd4ae7bafd1fccd",
            },
        )

    def test_manifest_rejects_seed_or_invocation_change(self) -> None:
        document = self._document()
        document["expected_result"]["seed"] = "0x00000000"
        with self.assertRaisesRegex(lab.ManifestError, "expected soak"):
            self._load_document(document)

        document = self._document()
        document["remote_argument"] = "--soak-forever"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)


class SoakResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.expected = lab.expected_checkpoints()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )

    def _stdout(self) -> str:
        lines = [self.identity]
        for checkpoint in self.expected:
            lines.append(
                f"CHECKPOINT frames={checkpoint['frames']} "
                f"input_fnv1a64={checkpoint['input_fnv1a64']} "
                f"output_fnv1a64={checkpoint['output_fnv1a64']}"
            )
        final = self.expected[-1]
        lines.append(
            "ERF_SOAK status=PASS mode=soak-10000 frames=10000 "
            "bytes_per_frame=481 compile_count=1 run_count=10000 "
            "checkpoints=10 canaries=PASS total_run_us=900000 "
            "min_run_us=80 max_run_us=1600 "
            f"input_fnv1a64={final['input_fnv1a64']} "
            f"output_fnv1a64={final['output_fnv1a64']}"
        )
        return "\n".join(lines)

    def test_exact_soak_result_passes(self) -> None:
        result = lab.parse_result(
            self._stdout(), self.manifest, self.expected
        )
        self.assertTrue(result["numeric_digest_match"])
        self.assertEqual(result["frames"], 10000)
        self.assertEqual(result["compared_bytes"], 4_810_000)
        self.assertLess(
            result["latency_us"]["graph_run_realtime_fraction"], 1.0
        )

    def test_changed_checkpoint_or_duplicate_summary_is_rejected(self) -> None:
        changed = self._stdout().replace(
            self.expected[3]["output_fnv1a64"],
            "0x0000000000000000",
            1,
        )
        with self.assertRaisesRegex(lab.ResultError, "digest"):
            lab.parse_result(changed, self.manifest, self.expected)
        with self.assertRaisesRegex(lab.ResultError, "expected one"):
            lab.parse_result(
                f"{self._stdout()}\n{self._stdout().splitlines()[-1]}",
                self.manifest,
                self.expected,
            )


class SoakHealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.snapshot = {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "0b8091ee-557d-4857-8019-af1aaab9d665",
            "uptime": "100.0 90.0",
            "galcore_device": (
                "crw-rw---- 1 0 0 199, 0 Jul 29 12:00 /dev/galcore"
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
                    "bytes": 5840 if path == lab.REMOTE_NBG else 100,
                    "sha256": self.manifest.required_artifacts[path],
                }
                for path, mode in lab.EXPECTED_STAGED_MODES.items()
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self) -> None:
        lab.verify_preflight(self.manifest, self.snapshot)

    def test_holder_or_kernel_alert_fails_health_gates(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 4, "fd": 8}]
        with self.assertRaisesRegex(lab.PreflightError, "already held"):
            lab.verify_preflight(self.manifest, changed)

        after = copy.deepcopy(self.snapshot)
        after["uptime"] = "101.0 91.0"
        after["dmesg_tail"].append("[2.0] galcore MMU fault")
        differences, alerts = lab.compare_health(
            self.manifest, self.snapshot, after
        )
        self.assertTrue(alerts)
        self.assertIn(
            "new kernel warning/error diagnostics were observed",
            differences,
        )


if __name__ == "__main__":
    unittest.main()
