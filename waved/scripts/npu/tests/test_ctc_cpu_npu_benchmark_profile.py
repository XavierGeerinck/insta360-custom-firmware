from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import ctc_cpu_npu_benchmark_profile as profile  # noqa: E402


class CtcCpuNpuBenchmarkProfileTests(unittest.TestCase):
    def test_fixed_profile_qualifies_exact_anchor(self) -> None:
        document = profile.build_profile()
        self.assertEqual(document["status"], "host-profile-qualified")
        self.assertEqual(
            document["relationship"],
            "source-binary-anchor-pending-live-numerical-proof",
        )
        self.assertEqual(document["onnx"]["nodes"], ["MatMul", "Add", "LogSoftmax"])
        self.assertEqual(document["onnx"]["matrix_macs_per_invocation"], 40_951_808)
        self.assertEqual(document["onnx"]["input"]["shape"], [1, 16, 512])
        self.assertEqual(document["nbg"]["input"]["shape"], [512, 16, 1])
        self.assertEqual(document["onnx"]["output"]["shape"], [1, 16, 4999])
        self.assertEqual(document["nbg"]["output"]["shape"], [4999, 16, 1])
        self.assertTrue(
            document["layout_mapping"]["input"]["linear_memory_order_equal"]
        )
        self.assertTrue(
            document["layout_mapping"]["output"]["linear_memory_order_equal"]
        )

    def test_profile_pins_ort_15_api_and_cpu_provider(self) -> None:
        document = profile.build_profile()
        self.assertEqual(document["api_header"]["ort_api_version"], 15)
        self.assertEqual(document["api_header"]["license"]["spdx"], "MIT")
        self.assertEqual(
            document["cpu_runtime"]["required_provider"],
            "CPUExecutionProvider",
        )

    def test_profile_rejects_artifact_hash_change(self) -> None:
        original = profile.sha256

        def changed(path: Path) -> str:
            if path == profile.NBG_PATH:
                return "0" * 64
            return original(path)

        with mock.patch.object(profile, "sha256", side_effect=changed):
            with self.assertRaisesRegex(profile.ProfileError, "hashes differ"):
                profile.build_profile()

    def test_cli_refuses_arguments(self) -> None:
        self.assertEqual(profile.main(["--output", "unexpected"]), 2)


if __name__ == "__main__":
    unittest.main()
