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

import npu_caps_lab  # noqa: E402


class CapsManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_caps_lab.load_manifest()

    def test_manifest_is_fixed_and_query_only(self):
        self.assertEqual(
            self.manifest.remote_executable,
            "/tmp/wave-npu-compile/bin/wave-npu-query-caps-v1",
        )
        self.assertEqual(self.manifest.timeout_ms, 12000)
        self.assertEqual(self.manifest.candidates, {})
        self.assertRegex(self.manifest.sha256, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_arbitrary_executable(self):
        document = json.loads(
            npu_caps_lab.DEFAULT_MANIFEST.read_text(encoding="utf-8")
        )
        document["remote_executable"] = "/tmp/other"
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(
                npu_caps_lab.CapsManifestError, "remote executable"
            ):
                npu_caps_lab.load_manifest(path)


class CapsResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_caps_lab.load_manifest()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        self.caps = (
            "CAPS status=PASS mode=query-only ext_bytes=32 ext2_bytes=40 "
            "eco_id=0x04000000 customer_id=0x1000000e evis1=0 evis2=0 "
            "core_count=1 subgroup_size=0 support_va40=0 "
            "support_stream_processor=0 stream_processor_exec_count=0 "
            "stream_processor_vector_size=0"
        )

    def test_parse_exact_caps(self):
        parsed = npu_caps_lab.parse_result(
            f"{self.identity}\n{self.caps}\n", self.manifest
        )
        self.assertEqual(parsed["capabilities"]["subgroup_size"], 0)
        self.assertFalse(parsed["capabilities"]["shader_selector_eligible"])

    def test_rejects_capability_identity_mismatch(self):
        changed = self.caps.replace("0x04000000", "0x00000000")
        with self.assertRaisesRegex(
            npu_caps_lab.CapsResultError, "ECO ID"
        ):
            npu_caps_lab.parse_result(
                f"{self.identity}\n{changed}\n", self.manifest
            )

    def test_remote_command_is_fixed_query(self):
        sentinel = "__WAVE_CAPS_EXIT_0123456789abcdef0123456789abcdef__"
        command = npu_caps_lab.build_remote_command(self.manifest, sentinel)
        self.assertIn("env -i", command)
        self.assertIn("wave-npu-query-caps-v1 --query", command)
        self.assertNotIn("graph", command.lower())
        self.assertNotIn("retry", command.lower())


class CapsPreflightTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_caps_lab.load_manifest()
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
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self):
        npu_caps_lab.verify_preflight(self.manifest, self.snapshot)

    def test_preflight_rejects_changed_probe(self):
        changed = copy.deepcopy(self.snapshot)
        changed["artifact_hashes"][self.manifest.remote_executable] = "0" * 64
        with self.assertRaisesRegex(
            npu_caps_lab.CapsPreflightError, "artifact mismatch"
        ):
            npu_caps_lab.verify_preflight(self.manifest, changed)


if __name__ == "__main__":
    unittest.main()
