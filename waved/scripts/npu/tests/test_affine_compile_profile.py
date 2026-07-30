from __future__ import annotations

import sys
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import affine_compile_profile as profile  # noqa: E402


class SourceProfileTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = profile.ELTWISE_SOURCE.read_text(encoding="utf-8")

    def test_add_table_is_finite_and_has_no_per_channel_pair(self) -> None:
        table = profile.extract_add_type_table(self.source)
        self.assertEqual(len(table), 59)
        self.assertIn(("D_U8|Q_ASYM", "D_U8|Q_ASYM"), table)
        self.assertIn(("D_U8|Q_ASYM", "D_I32"), table)
        self.assertNotIn(("D_U8|Q_ASYM", "D_I8|Q_SYM_PC"), table)
        self.assertFalse(any("PC" in value for pair in table for value in pair))

    def test_multiply_delegates_to_add_checker(self) -> None:
        self.assertTrue(profile.multiply_delegates_to_add(self.source))

    def test_exact_capture_maps_to_unsupported_signature(self) -> None:
        document = profile.build_profile()
        self.assertEqual(document["status"], "clean-negative")
        self.assertEqual(
            document["capture"]["rejection"]["input_descriptions"],
            ["ASYM UINT8", "SYMM PC INT8"],
        )
        self.assertEqual(document["checker"]["accepted_pair_count"], 59)
        self.assertEqual(document["checker"]["per_channel_pair_count"], 0)
        self.assertFalse(document["checker"]["rejected_signature_present"])
        self.assertFalse(
            document["interpretation"][
                "current_target_a_eligible_for_semantic_emission"
            ]
        )


if __name__ == "__main__":
    unittest.main()
