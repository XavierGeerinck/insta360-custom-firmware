#!/usr/bin/env python3
"""Lossless ownership and unchanged-emission tests for VPMN v2 images."""

from __future__ import annotations

import dataclasses
import hashlib
import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import nbg_format  # noqa: E402
import nbg_image  # noqa: E402
import nbg_serializer_recovery as recovery  # noqa: E402


def padded_name(value: str) -> bytes:
    encoded = value.encode("utf-8") + b"\0"
    return encoded.ljust(64, b"\0")


def v2_io(name: str) -> bytes:
    return struct.pack(
        "<I6I5I64s",
        2,
        4,
        2,
        1,
        1,
        1,
        1,
        1,
        0,
        0,
        0x3F800000,
        0,
        padded_name(name),
    )


def make_v2(
    *,
    sections: list[tuple[int, bytes, int, int, bytes]] | None = None,
    independent_tail: bool = True,
) -> bytes:
    """Build a strict v2 fixture.

    Section tuples are (type, payload, stride, alignment, prefix_gap).
    """

    configured = [
        (1, v2_io("input"), nbg_format.V2_IO_RECORD_BYTES, 0, b""),
        (2, v2_io("output"), nbg_format.V2_IO_RECORD_BYTES, 0, b""),
    ]
    configured.extend(
        sections
        or [
            (24, b"opaque-section", 1, 0, b""),
        ]
    )
    directory_offset = nbg_format.V2_HEADER_BYTES
    records_offset = directory_offset + nbg_format.V2_SUB_HEADER_BYTES
    directory_end = (
        records_offset
        + len(configured) * nbg_format.V2_DIRECTORY_RECORD_BYTES
    )

    cursor = directory_end
    records = []
    payload = bytearray()
    for section_type, section_data, stride, alignment, prefix_gap in configured:
        payload.extend(prefix_gap)
        cursor += len(prefix_gap)
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

    result = bytearray(nbg_format.V2_HEADER_BYTES)
    result[:4] = nbg_format.MAGIC
    struct.pack_into("<I", result, 4, nbg_format.V2_VERSION)
    struct.pack_into("<I", result, 8, 0x1000000E)
    struct.pack_into("<I", result, 24, directory_offset)
    struct.pack_into("<I", result, 28, nbg_format.V2_SUB_HEADER_BYTES)
    struct.pack_into("<I", result, 32, 1)
    result.extend(
        struct.pack(
            "<III",
            records_offset,
            nbg_format.V2_DIRECTORY_RECORD_BYTES,
            len(records),
        )
    )
    result.extend(b"".join(records))
    result.extend(payload)
    if independent_tail:
        result.extend(bytes(range(256)) * 2)
    elif len(result) < nbg_format.MINIMUM_RUNTIME_NBG_BYTES:
        result.extend(b"\xa5" * (nbg_format.MINIMUM_RUNTIME_NBG_BYTES - len(result)))
        last_record = records_offset + (
            len(records) - 1
        ) * nbg_format.V2_DIRECTORY_RECORD_BYTES
        original_size = struct.unpack_from("<I", result, last_record + 12)[0]
        struct.pack_into(
            "<I",
            result,
            last_record + 12,
            original_size
            + nbg_format.MINIMUM_RUNTIME_NBG_BYTES
            - cursor,
        )
    struct.pack_into("<I", result, 16, len(result))
    file_checksum = 0
    for index in range(len(records)):
        record_offset = (
            records_offset
            + index * nbg_format.V2_DIRECTORY_RECORD_BYTES
        )
        section_type, section_offset = struct.unpack_from(
            "<II", result, record_offset
        )
        section_size = struct.unpack_from(
            "<I", result, record_offset + 12
        )[0]
        section_data = bytes(
            result[section_offset : section_offset + section_size]
        )
        if section_type == 40:
            normalized = bytearray(section_data)
            for offset in range(4, len(normalized), 8):
                normalized[offset : offset + 4] = b"\0" * 4
            section_data = bytes(normalized)
        file_checksum = zlib.crc32(section_data, file_checksum)
    struct.pack_into("<I", result, 12, file_checksum)
    return bytes(result)


class NbgImageTests(unittest.TestCase):
    def test_parse_owns_every_byte_and_roundtrips(self) -> None:
        payload = make_v2()
        image = nbg_image.NbgImage.parse(
            payload, display_path="basic-v2.nbg"
        )
        image.assert_total_ownership()
        self.assertEqual(image.to_bytes(), payload)
        self.assertEqual(image.source_sha256, hashlib.sha256(payload).hexdigest())
        self.assertEqual(image.ranges[0].start, 0)
        self.assertEqual(image.ranges[-1].end, len(payload))
        self.assertEqual(
            sum(item.length for item in image.ranges), len(payload)
        )

    def test_range_classes_are_exact(self) -> None:
        payload = make_v2()
        image = nbg_image.NbgImage.parse(payload)
        classes = {item.byte_class.value for item in image.ranges}
        self.assertEqual(
            classes,
            {
                "connection-tail",
                "decoded-record",
                "preserved-opaque",
            },
        )
        opaque = [
            item
            for item in image.ranges
            if item.byte_class is nbg_image.ByteClass.PRESERVED_OPAQUE
        ]
        self.assertEqual(len(opaque), 1)
        self.assertEqual(opaque[0].raw, b"opaque-section")

    def test_alignment_and_extra_nonzero_padding_are_distinct(self) -> None:
        payload = make_v2(
            sections=[
                (
                    3,
                    b"x",
                    1,
                    0,
                    b"",
                ),
                (
                    24,
                    b"coefficient",
                    1,
                    32,
                    b"\x7f" * 51,
                )
            ]
        )
        image = nbg_image.NbgImage.parse(payload)
        gaps = [
            item
            for item in image.ranges
            if item.byte_class
            in {nbg_image.ByteClass.ALIGNMENT, nbg_image.ByteClass.PADDING}
        ]
        self.assertEqual(
            b"".join(item.raw for item in gaps),
            b"\x7f" * 51,
        )
        self.assertEqual(
            [item.byte_class for item in gaps],
            [nbg_image.ByteClass.ALIGNMENT, nbg_image.ByteClass.PADDING],
        )
        self.assertEqual(image.to_bytes(), payload)

    def test_unknown_section_is_preserved_opaque(self) -> None:
        payload = make_v2(
            sections=[(36, b"\x01\x02\x03", 1, 0, b"")]
        )
        image = nbg_image.NbgImage.parse(payload)
        section = image.sections[-1]
        self.assertEqual(section.section_type, 36)
        self.assertEqual(image.section_bytes(section.index), b"\x01\x02\x03")
        owned = image.ranges_for_section(section.index)
        self.assertEqual(len(owned), 1)
        self.assertIs(
            owned[0].byte_class, nbg_image.ByteClass.PRESERVED_OPAQUE
        )

    def test_decoded_sections_are_split_by_record(self) -> None:
        checksums = struct.pack(
            "<II", 1, zlib.crc32(v2_io("input"))
        ) + struct.pack("<II", 2, zlib.crc32(v2_io("output")))
        payload = make_v2(
            sections=[
                (
                    40,
                    checksums,
                    nbg_format.V2_CHECKSUM_RECORD_BYTES,
                    0,
                    b"",
                )
            ]
        )
        image = nbg_image.NbgImage.parse(payload)
        checksum_section = image.sections[-1]
        records = image.ranges_for_section(checksum_section.index)
        self.assertEqual(len(records), 2)
        self.assertTrue(
            all(
                item.byte_class is nbg_image.ByteClass.DECODED_RECORD
                for item in records
            )
        )
        self.assertEqual(
            [item.schema for item in records],
            ["v2-checksum-record", "v2-checksum-record"],
        )

    def test_independent_connection_tail_is_owned(self) -> None:
        payload = make_v2(independent_tail=True)
        image = nbg_image.NbgImage.parse(payload)
        self.assertEqual(image.connection_tail_mode, "independent")
        tails = [
            item
            for item in image.ranges
            if item.byte_class is nbg_image.ByteClass.CONNECTION_TAIL
        ]
        self.assertEqual(len(tails), 1)
        self.assertEqual(tails[0].length, 512)

    def test_section_covered_last_512_bytes_are_not_double_owned(self) -> None:
        payload = make_v2(
            sections=[(24, b"\xa5" * 700, 1, 0, b"")],
            independent_tail=False,
        )
        image = nbg_image.NbgImage.parse(payload)
        self.assertEqual(image.connection_tail_mode, "section-covered-view")
        self.assertFalse(
            any(
                item.byte_class is nbg_image.ByteClass.CONNECTION_TAIL
                for item in image.ranges
            )
        )
        image.assert_total_ownership()
        self.assertEqual(image.to_bytes(), payload)

    def test_semantic_change_is_refused(self) -> None:
        image = nbg_image.NbgImage.parse(make_v2())
        changed = image.mark_changed("test mutation")
        with self.assertRaisesRegex(
            recovery.SerializerError, "NBG_SEMANTIC_CHANGE_REFUSED"
        ):
            changed.to_bytes()

    def test_gap_and_overlap_are_refused(self) -> None:
        image = nbg_image.NbgImage.parse(make_v2())
        first = image.ranges[0]
        for replacement, expected in (
            (
                dataclasses.replace(
                    first,
                    end=first.end - 1,
                    raw=first.raw[:-1],
                    raw_sha256=hashlib.sha256(first.raw[:-1]).hexdigest(),
                ),
                "NBG_RANGE_GAP",
            ),
            (
                dataclasses.replace(
                    first,
                    end=first.end + 1,
                    raw=first.raw + b"\0",
                    raw_sha256=hashlib.sha256(
                        first.raw + b"\0"
                    ).hexdigest(),
                ),
                "NBG_RANGE_OVERLAP",
            ),
        ):
            with self.subTest(expected=expected):
                corrupted = dataclasses.replace(
                    image, ranges=(replacement, *image.ranges[1:])
                )
                with self.assertRaisesRegex(
                    recovery.SerializerError, expected
                ):
                    corrupted.assert_total_ownership()

    def test_changed_raw_hash_is_refused(self) -> None:
        image = nbg_image.NbgImage.parse(make_v2())
        first = dataclasses.replace(image.ranges[0], raw_sha256="0" * 64)
        corrupted = dataclasses.replace(
            image, ranges=(first, *image.ranges[1:])
        )
        with self.assertRaisesRegex(
            recovery.SerializerError, "NBG_SECTION_GEOMETRY"
        ):
            corrupted.assert_total_ownership()

    def test_truncation_is_refused(self) -> None:
        payload = make_v2()
        for size in (0, 4, 12, 47, 48, 59, len(payload) - 1):
            with self.subTest(size=size):
                with self.assertRaisesRegex(
                    recovery.SerializerError, "NBG_PARSE_FAILURE"
                ):
                    nbg_image.NbgImage.parse(payload[:size])

    def test_v1_is_outside_lossless_image_scope(self) -> None:
        payload = bytearray(513)
        payload[:4] = b"VPMN"
        struct.pack_into("<I", payload, 4, 0x0001001E)
        struct.pack_into("<I", payload, 8, 0x10000020)
        with self.assertRaisesRegex(
            recovery.SerializerError, "VPMN v2"
        ):
            nbg_image.NbgImage.parse(bytes(payload))

    def test_ownership_document_contains_no_raw_payload(self) -> None:
        payload = make_v2()
        document = nbg_image.NbgImage.parse(payload).ownership_document()
        self.assertNotIn(payload.hex(), repr(document))
        for item in document["ranges"]:
            self.assertNotIn("raw", item)
            self.assertIn("raw_sha256", item)


class RoundtripEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = recovery.load_manifest()
        recovery.validate_artifacts(cls.manifest)

    def test_all_pinned_controls_roundtrip_exactly(self) -> None:
        for artifact_id in self.manifest.roundtrip_nbg_ids:
            artifact = self.manifest.artifacts[artifact_id]
            with self.subTest(artifact_id=artifact_id):
                result = nbg_image.roundtrip_path(
                    self.manifest.repository_root / artifact.path,
                    artifact_id=artifact_id,
                    display_path=artifact.path.as_posix(),
                    expected_size=artifact.byte_count,
                    expected_sha256=artifact.sha256,
                )
                self.assertTrue(result.exact_match)
                self.assertIsNone(result.first_difference)
                self.assertEqual(result.input_sha256, artifact.sha256)
                self.assertEqual(result.output_sha256, artifact.sha256)
                self.assertEqual(result.input_bytes, artifact.byte_count)
                self.assertEqual(result.output_bytes, artifact.byte_count)
                self.assertEqual(
                    sum(result.class_byte_totals.values()),
                    artifact.byte_count,
                )

    def test_evidence_is_deterministic_and_payload_free(self) -> None:
        first_ownership = nbg_image.build_ownership_evidence(self.manifest)
        second_ownership = nbg_image.build_ownership_evidence(self.manifest)
        first_roundtrip = nbg_image.build_roundtrip_evidence(self.manifest)
        second_roundtrip = nbg_image.build_roundtrip_evidence(self.manifest)
        self.assertEqual(
            recovery.canonical_json(first_ownership),
            recovery.canonical_json(second_ownership),
        )
        self.assertEqual(
            recovery.canonical_json(first_roundtrip),
            recovery.canonical_json(second_roundtrip),
        )
        self.assertEqual(len(first_ownership["artifacts"]), 5)
        self.assertEqual(len(first_roundtrip["artifacts"]), 5)
        self.assertTrue(
            all(
                item["exact_match"]
                for item in first_roundtrip["artifacts"]
            )
        )
        encoded = recovery.canonical_json(first_ownership)
        self.assertNotIn(b"raw_payload", encoded)
        self.assertNotIn(str(self.manifest.repository_root).encode(), encoded)
        parsed = json.loads(encoded)
        self.assertTrue(
            all(value == 0 for value in parsed["safety"].values())
        )

    def test_identical_copy_is_exclusive_and_verified(self) -> None:
        artifact_id = "generated-erf"
        artifact = self.manifest.artifacts[artifact_id]
        image = nbg_image.NbgImage.parse(
            (
                self.manifest.repository_root / artifact.path
            ).read_bytes(),
            artifact_id=artifact_id,
            display_path=artifact.path.as_posix(),
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            destination = root / "copy.nbg"
            document = nbg_image.write_identical_copy_exclusive(
                image, destination, repository_root=root
            )
            self.assertEqual(destination.read_bytes(), image.to_bytes())
            self.assertEqual(document["sha256"], artifact.sha256)
            with self.assertRaisesRegex(
                recovery.SerializerError, "OUTPUT_EXISTS"
            ):
                nbg_image.write_identical_copy_exclusive(
                    image, destination, repository_root=root
                )

    def test_copy_escape_is_refused(self) -> None:
        artifact = self.manifest.artifacts["generated-erf"]
        image = nbg_image.NbgImage.parse(
            (
                self.manifest.repository_root / artifact.path
            ).read_bytes(),
            artifact_id=artifact.artifact_id,
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(
                recovery.SerializerError, "OUTPUT_EXISTS"
            ):
                nbg_image.write_identical_copy_exclusive(
                    image,
                    root.parent / "escape-copy.nbg",
                    repository_root=root,
                )


if __name__ == "__main__":
    unittest.main()
