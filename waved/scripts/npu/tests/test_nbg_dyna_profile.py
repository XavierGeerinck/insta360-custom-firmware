#!/usr/bin/env python3
"""Contracts for the offline Wave DYNAIO and file-CRC profile."""

from __future__ import annotations

import json
import struct
import sys
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import nbg_dyna_profile as profile  # noqa: E402
import nbg_format  # noqa: E402
from tests.test_nbg_format import make_v2  # noqa: E402


def dyna_graph(constant: bytes, *, opaque: bytes = b"opaque-data") -> bytes:
    return make_v2(
        opaque=opaque,
        extra_sections=[
            (20, constant + bytes(64 - len(constant)), 1),
            (26, struct.pack("<II", 0, len(constant)), 8),
        ],
    )


class ControlledProfileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.a = dyna_graph(bytes((0, 1, 2, 3)))
        self.b = dyna_graph(bytes((0, 1, 2, 4)))

    def test_direct_mapping_and_exact_in_memory_reconstruction(self) -> None:
        result = profile.analyze_controlled_payloads(
            self.a,
            self.a,
            self.b,
            source_a=bytes((0, 1, 2, 3)),
            source_b=bytes((0, 1, 2, 4)),
        )
        self.assertEqual(
            result["classification"],
            "direct-dyna-io-entry-mapping",
        )
        self.assertTrue(result["determinism"]["exact_match"])
        self.assertEqual(result["dynamic_io"]["entry_index"], 0)
        self.assertEqual(result["dynamic_io"]["entry_offset"], 0)
        self.assertEqual(result["dynamic_io"]["entry_size"], 4)
        self.assertEqual(
            result["dynamic_io"]["changed_region_relative_offset"], 3
        )
        self.assertEqual(
            result["dynamic_io"]["changed_file_offset"],
            result["dynamic_io"]["data_section_offset"] + 3,
        )
        self.assertTrue(result["file_checksum"]["a_matches"])
        self.assertTrue(result["file_checksum"]["b_matches"])
        self.assertEqual(
            result["file_checksum"]["algorithm"], "crc32-ieee"
        )
        self.assertTrue(result["reconstruction"]["exact_match"])
        self.assertFalse(result["reconstruction"]["written_to_disk"])

    def test_serializer_crc_zeros_final_checksum_value_words(self) -> None:
        opaque = b"crc-normalization"
        checksum_record = struct.pack(
            "<II", 24, profile.crc32(opaque)
        )
        payload = make_v2(
            opaque=opaque,
            extra_sections=[(40, checksum_record, 8)],
        )
        parsed = nbg_format.parse_bytes(payload)
        self.assertEqual(
            profile.serializer_file_crc32(payload, parsed),
            parsed["header"]["crc32"],
        )
        changed = bytearray(payload)
        checksum_section = next(
            section
            for section in parsed["sections"]
            if section["type"] == 40
        )
        changed[checksum_section["offset"] + 4] ^= 0xFF
        self.assertEqual(
            profile.serializer_file_crc32(bytes(changed), parsed),
            parsed["header"]["crc32"],
        )

    def test_nondeterminism_and_non_dyna_changes_are_refused(self) -> None:
        changed_a2 = bytearray(self.a)
        changed_a2[-1] ^= 1
        with self.assertRaisesRegex(
            profile.ProfileError, "determinism"
        ):
            profile.analyze_controlled_payloads(
                self.a,
                bytes(changed_a2),
                self.b,
                source_a=bytes((0, 1, 2, 3)),
                source_b=bytes((0, 1, 2, 4)),
            )

        changed_opaque = dyna_graph(
            bytes((0, 1, 2, 4)), opaque=b"other-data!"
        )
        with self.assertRaisesRegex(
            profile.ProfileError, "outside DYNAIO"
        ):
            profile.analyze_controlled_payloads(
                self.a,
                self.a,
                changed_opaque,
                source_a=bytes((0, 1, 2, 3)),
                source_b=bytes((0, 1, 2, 4)),
            )

    def test_canonical_document_contains_no_raw_payload(self) -> None:
        result = profile.analyze_controlled_payloads(
            self.a,
            self.a,
            self.b,
            source_a=bytes((0, 1, 2, 3)),
            source_b=bytes((0, 1, 2, 4)),
        )
        first = profile.canonical_json(result)
        second = profile.canonical_json(
            profile.analyze_controlled_payloads(
                self.a,
                self.a,
                self.b,
                source_a=bytes((0, 1, 2, 3)),
                source_b=bytes((0, 1, 2, 4)),
            )
        )
        self.assertEqual(first, second)
        document = json.loads(first)
        self.assertNotIn("raw_payload", document)
        self.assertEqual(document["schema_version"], 1)


class CorpusAggregateTests(unittest.TestCase):
    def test_inventory_aggregate_counts_integrity_and_dyna_contracts(self) -> None:
        a = nbg_format.parse_bytes(
            self._graph(bytes((0, 1, 2, 3))), display_path="a.nb"
        )
        b = nbg_format.parse_bytes(make_v2(), display_path="b.nb")
        for index, item in enumerate((a, b)):
            item["corpus_id"] = f"item-{index}"
            item["role"] = "stock"
        inventory = {
            "corpus": {
                "configured_file_count": 2,
                "path": "corpus.json",
                "sha256": "1" * 64,
            },
            "files": [a, b],
            "generator": {
                "path": "nbg_format.py",
                "sha256": "2" * 64,
            },
            "schema_version": 1,
        }
        result = profile.aggregate_inventory(inventory)
        self.assertEqual(result["configured_files"], 2)
        self.assertEqual(result["v2_file_checksums_matched"], 2)
        self.assertEqual(result["v2_file_checksums_mismatched"], 0)
        self.assertEqual(result["dyna_io"]["files"], 1)
        self.assertEqual(result["dyna_io"]["entries"], 1)
        self.assertEqual(result["dyna_io"]["padding_nonzero_bytes"], 0)

    @staticmethod
    def _graph(constant: bytes) -> bytes:
        return dyna_graph(constant)


if __name__ == "__main__":
    unittest.main()
