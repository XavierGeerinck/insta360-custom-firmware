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

import npu_clc_loader_lab  # noqa: E402


class ClcManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_clc_loader_lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(
            npu_clc_loader_lab.DEFAULT_MANIFEST.read_text(encoding="utf-8")
        )

    def _load_document(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_clc_loader_lab.load_manifest(path)

    def test_manifest_is_fixed_and_relocation_only(self):
        self.assertEqual(
            self.manifest.remote_executable,
            npu_clc_loader_lab.REMOTE_EXECUTABLE,
        )
        self.assertIn(
            npu_clc_loader_lab.REMOTE_CLC,
            self.manifest.required_artifacts,
        )
        self.assertEqual(self.manifest.timeout_ms, 12000)
        self.assertEqual(self.manifest.candidates, {})
        self.assertRegex(self.manifest.sha256, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_arbitrary_executable(self):
        document = self._document()
        document["remote_executable"] = "/tmp/other"
        with self.assertRaisesRegex(
            npu_clc_loader_lab.ClcManifestError, "remote executable"
        ):
            self._load_document(document)

    def test_manifest_requires_clc_hash_pin(self):
        document = self._document()
        del document["required_artifacts"][npu_clc_loader_lab.REMOTE_CLC]
        with self.assertRaisesRegex(
            npu_clc_loader_lab.ClcManifestError, "not hash-pinned"
        ):
            self._load_document(document)

    def test_manifest_rejects_loader_interposition(self):
        document = self._document()
        document["environment"]["LD_PRELOAD"] = "/tmp/hook.so"
        with self.assertRaises(npu_clc_loader_lab.ClcManifestError):
            self._load_document(document)


class ClcResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_clc_loader_lab.load_manifest()

    def test_exact_result_passes(self):
        parsed = npu_clc_loader_lab.parse_result(
            "CLC status=PASS mode=relocation-only "
            "library=/tmp/wave-npu-compile/lib/libCLC.so "
            "exports=3 calls=0"
        )
        self.assertEqual(parsed["exports_resolved"], 3)
        self.assertEqual(parsed["symbols_called"], 0)

    def test_result_rejects_symbol_calls(self):
        with self.assertRaisesRegex(
            npu_clc_loader_lab.ClcResultError, "exactly one"
        ):
            npu_clc_loader_lab.parse_result(
                "CLC status=PASS mode=relocation-only "
                "library=/tmp/wave-npu-compile/lib/libCLC.so "
                "exports=3 calls=1"
            )

    def test_remote_command_is_fixed(self):
        sentinel = "__WAVE_CLC_EXIT_0123456789abcdef0123456789abcdef__"
        command = npu_clc_loader_lab.build_remote_command(
            self.manifest, sentinel
        )
        self.assertIn("env -i", command)
        self.assertIn("--probe", command)
        self.assertNotIn("--compile", command)
        self.assertNotIn("--run", command)

    def test_remote_exit_sentinel_is_unique(self):
        sentinel = "__WAVE_CLC_EXIT_0123456789abcdef0123456789abcdef__"
        cleaned, status = npu_clc_loader_lab.parse_remote_exit(
            f"hello\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "hello")
        self.assertEqual(status, 0)
        with self.assertRaises(npu_clc_loader_lab.ClcResultError):
            npu_clc_loader_lab.parse_remote_exit(
                f"{sentinel}:0\n{sentinel}:0\n", sentinel
            )


class ClcHealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = npu_clc_loader_lab.load_manifest()
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
        npu_clc_loader_lab.verify_preflight(
            self.manifest, self.snapshot
        )

    def test_preflight_rejects_clc_mismatch(self):
        changed = copy.deepcopy(self.snapshot)
        changed["artifact_hashes"][
            npu_clc_loader_lab.REMOTE_CLC
        ] = "0" * 64
        with self.assertRaisesRegex(
            npu_clc_loader_lab.ClcPreflightError,
            "artifact mismatch",
        ):
            npu_clc_loader_lab.verify_preflight(
                self.manifest, changed
            )

    def test_preflight_rejects_driver_holder(self):
        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 9, "fd": 4}]
        with self.assertRaisesRegex(
            npu_clc_loader_lab.ClcPreflightError, "already held"
        ):
            npu_clc_loader_lab.verify_preflight(
                self.manifest, changed
            )


if __name__ == "__main__":
    unittest.main()
