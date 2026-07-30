#!/usr/bin/env python3
"""Binary-differential tests for the PoC 12B Erf constant controls."""

from __future__ import annotations

import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import nbg_constant_diff as diff  # noqa: E402
import nbg_format  # noqa: E402


def _name(value: str) -> bytes:
    return (value.encode() + b"\0").ljust(64, b"\0")


def _io(value: str) -> bytes:
    return struct.pack(
        "<I6I5I64s",
        1,
        4,
        1,
        1,
        1,
        1,
        1,
        2,
        0,
        0,
        0x3F800000,
        0,
        _name(value),
    )


def make_graph(
    constant: bytes,
    *,
    operation_tag: int = 7,
    include_input: bool = True,
) -> bytes:
    coefficient = constant + bytes(60)
    checksum = struct.pack("<II", 24, zlib.crc32(coefficient) & 0xFFFFFFFF)
    sections = [
        (2, _io("output"), 112, 0),
        (4, struct.pack("<8I", operation_tag, 0, 0, 0, 0, 0, 0, 0), 32, 0),
        (23, struct.pack("<II", 0, len(coefficient)), 8, 0),
        (24, coefficient, 1, 64),
        (40, checksum, 8, 0),
    ]
    if include_input:
        sections.insert(0, (1, _io("input"), 112, 0))
    directory_offset = 48
    records_offset = 60
    directory_end = records_offset + 44 * len(sections)
    cursor = directory_end
    records: list[bytes] = []
    payload = bytearray()
    for section_type, section_data, stride, alignment in sections:
        if alignment:
            gap = (-cursor) % alignment
            payload.extend(bytes(gap))
            cursor += gap
        records.append(
            struct.pack(
                "<11I",
                section_type,
                cursor,
                0,
                len(section_data),
                0,
                0,
                0,
                alignment,
                stride,
                0,
                0,
            )
        )
        payload.extend(section_data)
        cursor += len(section_data)
    # Model the parser's independent connection tail without assigning those
    # opaque bytes to the decoded checksum section.
    payload.extend(bytes(512))
    cursor += 512

    result = bytearray(48)
    result[:4] = b"VPMN"
    struct.pack_into("<I", result, 4, nbg_format.V2_VERSION)
    struct.pack_into("<I", result, 8, 0x1000000E)
    struct.pack_into("<I", result, 16, cursor)
    struct.pack_into("<I", result, 24, directory_offset)
    struct.pack_into("<I", result, 28, 12)
    struct.pack_into("<I", result, 32, 1)
    result.extend(struct.pack("<III", records_offset, 44, len(records)))
    result.extend(b"".join(records))
    result.extend(payload)
    file_checksum = 0
    for section_type, section_data, _stride, _alignment in sections:
        if section_type == 40:
            normalized = bytearray(section_data)
            for offset in range(4, len(normalized), 8):
                normalized[offset : offset + 4] = b"\0" * 4
            section_data = bytes(normalized)
        file_checksum = zlib.crc32(section_data, file_checksum)
    struct.pack_into("<I", result, 12, file_checksum)
    return bytes(result)


class DifferentialTests(unittest.TestCase):
    def setUp(self) -> None:
        self.a = make_graph(bytes([0, 1, 2, 3]))
        self.b = make_graph(bytes([0, 1, 2, 4]))

    def test_source_mapping_and_crc_collateral_stay_poc12b_limited(
        self,
    ) -> None:
        result = diff.analyze_payloads(self.a, self.a, self.b)
        self.assertEqual(result["classification"], "multi-region-unresolved")
        self.assertEqual(result["poc12c"], "not-ready")
        self.assertTrue(result["determinism"]["exact_match"])
        source_runs = [
            item
            for item in result["difference_runs"]
            if item["classification"] == "source-constant"
        ]
        self.assertEqual(len(source_runs), 1)
        self.assertEqual(source_runs[0]["old_hex"], "03")
        self.assertEqual(source_runs[0]["new_hex"], "04")
        header_runs = [
            item
            for item in result["difference_runs"]
            if item["start"] == 12
        ]
        self.assertEqual(len(header_runs), 1)
        self.assertEqual(
            header_runs[0]["classification"],
            "producer-integrity-collateral",
        )
        self.assertFalse(result["header_crc_sum"]["algorithm_proved"])
        collateral = result["integrity_collateral"]
        self.assertEqual(len(collateral), 1)
        self.assertTrue(collateral[0]["explained"])
        self.assertFalse(result["observed_b_reconstruction"]["attempted"])
        self.assertFalse(result["observed_b_reconstruction"]["exact_match"])

    def test_identical_a_b_is_folded_or_elided(self) -> None:
        result = diff.analyze_payloads(self.a, self.a, self.a)
        self.assertEqual(result["classification"], "constant-folded-or-elided")
        self.assertEqual(result["poc12c"], "not-ready")

    def test_zero_external_input_v2_is_admitted(self) -> None:
        a = make_graph(bytes([0, 1, 2, 3]), include_input=False)
        b = make_graph(bytes([0, 1, 2, 4]), include_input=False)
        result = diff.analyze_payloads(a, a, b)
        self.assertEqual(result["classification"], "multi-region-unresolved")

    def test_nondeterminism_blocks_b_interpretation(self) -> None:
        changed_a2 = bytearray(self.a)
        changed_a2[-1] ^= 1
        result = diff.analyze_payloads(self.a, bytes(changed_a2), self.b)
        self.assertEqual(result["classification"], "serializer-nondeterministic")
        self.assertEqual(result["difference_runs"], [])

    def test_structural_change_is_unresolved(self) -> None:
        changed = make_graph(bytes([0, 1, 2, 4]), operation_tag=8)
        result = diff.analyze_payloads(self.a, self.a, changed)
        self.assertEqual(result["classification"], "multi-region-unresolved")
        self.assertFalse(result["structural_comparison"]["compatible"])

    def test_evidence_is_deterministic_and_payload_free(self) -> None:
        left = diff.canonical_json(diff.analyze_payloads(self.a, self.a, self.b))
        right = diff.canonical_json(diff.analyze_payloads(self.a, self.a, self.b))
        self.assertEqual(left, right)
        self.assertNotIn(bytes(range(32)), left)
        document = json.loads(left)
        self.assertEqual(document["schema_version"], 1)

    def test_tracked_evidence_verifier_rejects_stale_copy(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            a1 = root / "a1.nbg"
            a2 = root / "a2.nbg"
            b = root / "b.nbg"
            a1.write_bytes(self.a)
            a2.write_bytes(self.a)
            b.write_bytes(self.b)
            evidence = root / "evidence.json"
            manifest = diff.fixture_manifest(a1, a2, b, evidence)
            diff.generate_evidence(manifest)
            diff.verify_generated(manifest)
            document = json.loads(evidence.read_text())
            document["classification"] = "direct-constant-mapping"
            evidence.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(diff.DifferentialError, "stale"):
                diff.verify_generated(manifest)


if __name__ == "__main__":
    unittest.main()
