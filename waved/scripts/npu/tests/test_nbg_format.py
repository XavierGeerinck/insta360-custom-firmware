#!/usr/bin/env python3
"""Contract and malformed-input tests for the host-only Wave NBG parser."""

from __future__ import annotations

import copy
import hashlib
import json
import random
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = SCRIPT_DIR.parents[2]
sys.path.insert(0, str(SCRIPT_DIR))

import nbg_format  # noqa: E402


def float_word(value: float) -> int:
    return struct.unpack("<I", struct.pack("<f", value))[0]


def padded_name(value: str) -> bytes:
    encoded = value.encode("utf-8") + b"\0"
    if len(encoded) > 64:
        raise ValueError("test name is too long")
    return encoded.ljust(64, b"\0")


def v2_io(
    name: str,
    dimensions: tuple[int, ...],
    *,
    data_format: int = 1,
    data_type: int = 0,
    quantization_format: int = 0,
    word_0: int | None = None,
    word_1: int = 0,
) -> bytes:
    if len(dimensions) > 6:
        raise ValueError("too many test dimensions")
    dims = dimensions + (1,) * (6 - len(dimensions))
    if word_0 is None:
        word_0 = float_word(1.0)
    return struct.pack(
        "<I6I5I64s",
        len(dimensions),
        *dims,
        data_format,
        data_type,
        quantization_format,
        word_0,
        word_1,
        padded_name(name),
    )


def v1_io(
    name: str,
    dimensions: tuple[int, ...],
    *,
    data_format: int = 2,
    data_type: int = 0,
    quantization_format: int = 0,
) -> bytes:
    if len(dimensions) > 6:
        raise ValueError("too many test dimensions")
    dims = dimensions + (1,) * (6 - len(dimensions))
    return struct.pack(
        "<I6I6I64s",
        len(dimensions),
        *dims,
        data_format,
        data_type,
        quantization_format,
        0,
        float_word(1.0),
        0,
        padded_name(name),
    )


def v2_network_info(
    name: str = "synthetic-network",
    *,
    input_count: int = 1,
    output_count: int = 1,
    address_width: int = 32,
    attribute: int = 0,
) -> bytes:
    return struct.pack(
        "<64s8I31I",
        padded_name(name),
        input_count,
        output_count,
        3,
        5,
        7,
        1,
        address_width,
        attribute,
        *(0,) * 31,
    )


def set_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def v2_serializer_file_checksum(
    section_payloads: list[tuple[int, bytes, int]],
) -> int:
    checksum = 0
    for section_type, section_data, _stride in section_payloads:
        if section_type == 40:
            normalized = bytearray(section_data)
            if len(normalized) % nbg_format.V2_CHECKSUM_RECORD_BYTES:
                raise ValueError("checksum section must contain whole records")
            for offset in range(4, len(normalized), 8):
                normalized[offset : offset + 4] = b"\0" * 4
            section_data = bytes(normalized)
        checksum = zlib.crc32(section_data, checksum)
    return checksum


def make_v2(
    *,
    input_record: bytes | None = None,
    output_record: bytes | None = None,
    opaque: bytes | None = b"opaque-data",
    extra_sections: list[tuple[int, bytes, int]] | None = None,
) -> bytes:
    input_record = input_record or v2_io("input", (4, 2))
    output_record = output_record or v2_io(
        "output", (4, 2), data_format=0
    )
    section_payloads: list[tuple[int, bytes, int]] = [
        (1, input_record, nbg_format.V2_IO_RECORD_BYTES),
        (2, output_record, nbg_format.V2_IO_RECORD_BYTES),
    ]
    section_payloads.extend(extra_sections or [])
    if opaque is not None:
        section_payloads.append((24, opaque, 1))

    directory_locator = nbg_format.V2_HEADER_BYTES
    records_offset = directory_locator + 12
    directory_end = (
        records_offset
        + len(section_payloads) * nbg_format.V2_DIRECTORY_RECORD_BYTES
    )
    cursor = directory_end
    records: list[bytes] = []
    payload = bytearray()
    for section_type, section_data, stride in section_payloads:
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
                0,
                stride,
                0,
                0,
            )
        )
        payload.extend(section_data)
        cursor += len(section_data)

    data = bytearray(nbg_format.V2_HEADER_BYTES)
    data[:4] = nbg_format.MAGIC
    set_u32(data, 4, nbg_format.V2_VERSION)
    set_u32(data, 8, 0x1000000E)
    set_u32(data, 24, directory_locator)
    set_u32(data, 28, nbg_format.V2_SUB_HEADER_BYTES)
    set_u32(data, 32, 1)
    data.extend(
        struct.pack(
            "<III",
            records_offset,
            nbg_format.V2_DIRECTORY_RECORD_BYTES,
            len(records),
        )
    )
    data.extend(b"".join(records))
    data.extend(payload)
    data.extend(bytes(range(256)) * 2)
    set_u32(data, 16, len(data))
    set_u32(data, 12, v2_serializer_file_checksum(section_payloads))
    return bytes(data)


def make_v1() -> bytes:
    version = 0x0001001E
    input_records = v1_io("input[0]", (2,)) + v1_io(
        "input[1]", (2,)
    )
    output_records = v1_io("output", (2,))
    fixed_prefix_end = 0x1F8
    first_section = 0x200
    output_offset = first_section + len(input_records)

    data = bytearray(fixed_prefix_end)
    data[:4] = nbg_format.MAGIC
    set_u32(data, 4, version)
    set_u32(data, 8, 0x10000020)
    data[12:76] = padded_name("synthetic-add")
    set_u32(data, 76, 1)
    set_u32(data, 80, 2)
    set_u32(data, 84, 2)
    set_u32(data, 88, 1)
    cursor = 92 + 256 + 12 + 8 + 8
    sections = [
        (first_section, len(input_records)),
        (output_offset, len(output_records)),
    ] + [(0, 0)] * 14
    for offset, size in sections:
        set_u32(data, cursor, offset)
        set_u32(data, cursor + 4, size)
        cursor += 8
    if cursor != fixed_prefix_end:
        raise AssertionError((cursor, fixed_prefix_end))
    data.extend(b"\x00" * (first_section - len(data)))
    data.extend(input_records)
    data.extend(output_records)
    data.extend(bytes(range(256)) * 2)
    return bytes(data)


def valid_config(
    reference_digest: str,
    *,
    expected_count: int = 1,
) -> dict:
    return {
        "schema_version": 1,
        "collections": [
            {
                "id": "stock",
                "root": "models",
                "glob": "**/*.nb",
                "role": "stock",
                "required": True,
                "expected_count": expected_count,
            }
        ],
        "files": [
            {
                "id": "reference",
                "path": "fixtures/reference.nb",
                "role": "reference",
                "required": True,
                "expected_sha256": reference_digest,
            }
        ],
    }


class ConfigTests(unittest.TestCase):
    def prepare(self, root: Path) -> tuple[Path, bytes]:
        model = make_v2()
        model_path = root / "models/graph.nb"
        model_path.parent.mkdir(parents=True)
        model_path.write_bytes(model)
        reference_path = root / "fixtures/reference.nb"
        reference_path.parent.mkdir(parents=True)
        reference_path.write_bytes(model)
        digest = hashlib.sha256(model).hexdigest()
        config_path = root / "corpus.json"
        config_path.write_text(
            json.dumps(valid_config(digest)), encoding="utf-8"
        )
        return config_path, model

    def test_valid_config_discovers_deterministically(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config_path, _model = self.prepare(root)
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            discovery = nbg_format.discover_corpus(corpus)
            self.assertEqual(len(discovery.files), 2)
            self.assertEqual(
                [item.display_path for item in discovery.files],
                ["models/graph.nb", "fixtures/reference.nb"],
            )
            self.assertEqual(discovery.collections[0]["observed_count"], 1)

    def test_duplicate_json_key_and_schema_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "corpus.json"
            path.write_text(
                '{"schema_version":1,"schema_version":1,'
                '"collections":[],"files":[]}',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "duplicate JSON key"
            ):
                nbg_format.load_corpus(path, repository_root=root)

            path.write_text(
                json.dumps(
                    {
                        "schema_version": 2,
                        "collections": [],
                        "files": [],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "schema_version"
            ):
                nbg_format.load_corpus(path, repository_root=root)

    def test_unknown_keys_ids_roles_paths_globs_and_hashes_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config_path, model = self.prepare(root)
            digest = hashlib.sha256(model).hexdigest()
            cases: list[tuple[dict, str]] = []

            unknown = valid_config(digest)
            unknown["shell"] = "id"
            cases.append((unknown, "unknown"))

            duplicate_id = valid_config(digest)
            duplicate_id["files"][0]["id"] = "stock"
            cases.append((duplicate_id, "duplicate corpus ID"))

            bad_role = valid_config(digest)
            bad_role["collections"][0]["role"] = "execute"
            cases.append((bad_role, "role"))

            escape = valid_config(digest)
            escape["files"][0]["path"] = "../escape.nb"
            cases.append((escape, "escape"))

            absolute = valid_config(digest)
            absolute["collections"][0]["root"] = "/tmp/models"
            cases.append((absolute, "escape"))

            bad_glob = valid_config(digest)
            bad_glob["collections"][0]["glob"] = "**/*"
            cases.append((bad_glob, "glob"))

            bad_extension = valid_config(digest)
            bad_extension["files"][0]["path"] = "fixtures/model.bin"
            cases.append((bad_extension, ".nb"))

            bad_hash = valid_config("not-a-hash")
            cases.append((bad_hash, "SHA-256"))

            for document, pattern in cases:
                with self.subTest(pattern=pattern):
                    config_path.write_text(
                        json.dumps(document), encoding="utf-8"
                    )
                    with self.assertRaisesRegex(
                        nbg_format.NbgCorpusError, pattern
                    ):
                        nbg_format.load_corpus(
                            config_path, repository_root=root
                        )

    def test_missing_count_mismatch_symlink_and_duplicate_path_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config_path, model = self.prepare(root)
            digest = hashlib.sha256(model).hexdigest()

            missing = valid_config(digest)
            missing["collections"][0]["root"] = "missing"
            config_path.write_text(json.dumps(missing), encoding="utf-8")
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "missing"
            ):
                nbg_format.discover_corpus(corpus)

            mismatch = valid_config(digest, expected_count=2)
            config_path.write_text(
                json.dumps(mismatch), encoding="utf-8"
            )
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "count mismatch"
            ):
                nbg_format.discover_corpus(corpus)

            symlink = root / "models/link.nb"
            symlink.symlink_to(root / "models/graph.nb")
            mismatch["collections"][0]["expected_count"] = 2
            config_path.write_text(
                json.dumps(mismatch), encoding="utf-8"
            )
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "symlink"
            ):
                nbg_format.discover_corpus(corpus)
            symlink.unlink()

            duplicate = valid_config(digest)
            duplicate["files"][0]["path"] = "models/graph.nb"
            config_path.write_text(
                json.dumps(duplicate), encoding="utf-8"
            )
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            with self.assertRaisesRegex(
                nbg_format.NbgCorpusError, "duplicate physical"
            ):
                nbg_format.discover_corpus(corpus)

    def test_reference_hash_mismatch_fails_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config_path, model = self.prepare(root)
            document = valid_config("0" * 64)
            config_path.write_text(
                json.dumps(document), encoding="utf-8"
            )
            corpus = nbg_format.load_corpus(
                config_path, repository_root=root
            )
            with self.assertRaisesRegex(
                nbg_format.NbgInventoryError, "hash mismatch"
            ):
                nbg_format.build_inventory(corpus)
            self.assertNotEqual(hashlib.sha256(model).hexdigest(), "0" * 64)


class EnvelopeTests(unittest.TestCase):
    def test_short_inputs_and_bad_magic_fail_structurally(self):
        for size in (0, 1, 4, 8, 12, 512):
            with self.subTest(size=size):
                with self.assertRaises(nbg_format.NbgParseError) as caught:
                    nbg_format.parse_bytes(b"\0" * size)
                self.assertEqual(caught.exception.code, "file-too-small")

        data = bytearray(make_v2())
        data[:4] = b"NOPE"
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(data))
        self.assertEqual(caught.exception.code, "bad-magic")

    def test_unsupported_versions_fail(self):
        for version in (0, 0x0000FFFF, 0x00020001, 0x01000000):
            data = bytearray(make_v2())
            set_u32(data, 4, version)
            with self.subTest(version=version):
                with self.assertRaises(nbg_format.NbgParseError) as caught:
                    nbg_format.parse_bytes(bytes(data))
                self.assertEqual(
                    caught.exception.code, "unsupported-version-family"
                )

    def test_error_json_is_bounded_and_deterministic(self):
        error = nbg_format.NbgParseError(
            "bad-magic",
            "envelope",
            "invalid",
            offset=0,
            context="magic",
        )
        first = nbg_format.canonical_json(error.as_dict())
        second = nbg_format.canonical_json(error.as_dict())
        self.assertEqual(first, second)
        self.assertLess(len(first), 1024)
        self.assertNotIn(b"Traceback", first)


class V1ParserTests(unittest.TestCase):
    def test_synthetic_v1_metadata_and_opaque_extension(self):
        result = nbg_format.parse_bytes(make_v1(), display_path="v1.nb")
        self.assertEqual(result["format_family"], "v1-fixed")
        self.assertEqual(result["version"], 0x0001001E)
        self.assertEqual(result["product"], 0x10000020)
        self.assertEqual(result["header"]["network_name"], "synthetic-add")
        self.assertEqual(result["header"]["input_count"], 2)
        self.assertEqual(result["header"]["output_count"], 1)
        self.assertEqual(result["fixed_extension"]["size"], 8)
        self.assertEqual(
            [tensor["name"] for tensor in result["inputs"]],
            ["input[0]", "input[1]"],
        )
        self.assertEqual(result["outputs"][0]["byte_size"], 2)
        self.assertIn("exceeds pinned public", result["warnings"][0])

    def test_v1_table_range_stride_dimension_and_name_fail(self):
        base = bytearray(make_v1())
        input_pair_offset = 0x178

        bad_range = bytearray(base)
        set_u32(bad_range, input_pair_offset, len(bad_range) - 10)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_range))
        self.assertEqual(caught.exception.code, "section-range-overflow")

        bad_stride = bytearray(base)
        set_u32(bad_stride, input_pair_offset + 4, 230)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_stride))
        self.assertEqual(caught.exception.code, "invalid-io-record-size")

        bad_dimension_count = bytearray(base)
        set_u32(bad_dimension_count, 0x200, 7)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_dimension_count))
        self.assertEqual(caught.exception.code, "invalid-dimension-count")

        zero_dimension = bytearray(base)
        set_u32(zero_dimension, 0x204, 0)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(zero_dimension))
        self.assertEqual(caught.exception.code, "invalid-dimension")

        bad_name = bytearray(base)
        bad_name[0x200 + 52 : 0x200 + 116] = b"A" * 64
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_name))
        self.assertEqual(caught.exception.code, "invalid-name")

    @unittest.skipUnless(
        (
            REPOSITORY_ROOT
            / "waved/third_party/stock-npu-stack/sample-add.nb"
        ).is_file(),
        "ignored foreign add fixture is not present",
    )
    def test_real_foreign_add_fixture(self):
        path = (
            REPOSITORY_ROOT
            / "waved/third_party/stock-npu-stack/sample-add.nb"
        )
        result = nbg_format.parse_path(
            path,
            display_path=(
                "waved/third_party/stock-npu-stack/sample-add.nb"
            ),
        )
        self.assertEqual(
            result["sha256"],
            "9bef0c0f2271ad534cb975b386be8428e"
            "973dc7ee4c71c04854c95eeac1fea5c",
        )
        self.assertEqual(result["version"], 0x0001001E)
        self.assertEqual(result["product"], 0x10000020)
        self.assertEqual(
            [tensor["dimensions"] for tensor in result["inputs"]],
            [[2], [2]],
        )
        self.assertEqual(
            [tensor["dimensions"] for tensor in result["outputs"]],
            [[2]],
        )


class V2ParserTests(unittest.TestCase):
    def test_synthetic_v2_metadata_quantization_and_fingerprints(self):
        result = nbg_format.parse_bytes(make_v2(), display_path="v2.nb")
        self.assertEqual(result["format_family"], "v2-directory")
        self.assertEqual(result["product"], 0x1000000E)
        self.assertEqual(result["target"], 0x1000000E)
        self.assertEqual(result["directory"]["section_header_count"], 3)
        self.assertEqual(
            result["header"]["crc32"],
            result["file_checksum"]["computed_crc32"],
        )
        self.assertTrue(result["file_checksum"]["matches"])
        self.assertEqual(result["header"]["sub_header_size"], 12)
        self.assertEqual(result["inputs"][0]["dimensions"], [4, 2])
        self.assertEqual(result["inputs"][0]["byte_size"], 16)
        self.assertEqual(result["outputs"][0]["byte_size"], 32)
        self.assertEqual(result["inputs"][0]["data_type_name"], "tensor")
        self.assertEqual(
            result["inputs"][0]["quantization"]["interpretation"], "none"
        )
        self.assertEqual(
            result["sections"][2]["type_name"],
            "NBG_SECTION_COEF_DATA",
        )
        self.assertEqual(
            result["connection_tail"]["fingerprint"]["chunk_count"], 1
        )
        self.assertEqual(
            len(result["sections"][2]["fingerprint"]["sha256"]), 64
        )

        fixed = make_v2(
            input_record=v2_io(
                "fixed",
                (4,),
                quantization_format=1,
                word_0=0xFFFFFFFD,
                word_1=0x1234,
            )
        )
        parsed_fixed = nbg_format.parse_bytes(fixed)
        quantization = parsed_fixed["inputs"][0]["quantization"]
        self.assertEqual(quantization["fixed_position"], -3)
        self.assertEqual(quantization["interpretation"], "fixed-point")

        affine = make_v2(
            input_record=v2_io(
                "affine",
                (4,),
                quantization_format=2,
                word_0=float_word(0.25),
                word_1=13,
            )
        )
        parsed_affine = nbg_format.parse_bytes(affine)
        quantization = parsed_affine["inputs"][0]["quantization"]
        self.assertEqual(quantization["format_name"], "affine-asymmetric")
        self.assertEqual(quantization["scale"], 0.25)
        self.assertEqual(quantization["zero_point"], 13)

    def test_public_v2_types_and_decoded_metadata(self):
        result = nbg_format.parse_bytes(
            make_v2(
                input_record=v2_io(
                    "state",
                    (3,),
                    data_format=8,
                    data_type=3,
                ),
                output_record=v2_io(
                    "packed",
                    (3,),
                    data_format=13,
                ),
                extra_sections=[
                    (
                        35,
                        v2_network_info(
                            "metadata-test",
                            address_width=40,
                            attribute=0b10_01_1,
                        ),
                        nbg_format.V2_NETWORK_INFO_BYTES,
                    ),
                    (37, struct.pack("<II", 0x1234, 1), 8),
                    (38, struct.pack("<I", 8192), 4),
                    (39, struct.pack("<I", 4096), 4),
                    (
                        40,
                        struct.pack(
                            "<II",
                            24,
                            zlib.crc32(b"opaque-data") & 0xFFFFFFFF,
                        ),
                        8,
                    ),
                ],
            )
        )
        self.assertEqual(result["inputs"][0]["data_format_name"], "int32")
        self.assertEqual(result["inputs"][0]["data_type_name"], "scalar")
        self.assertEqual(result["inputs"][0]["byte_size"], 12)
        self.assertEqual(result["outputs"][0]["data_format_name"], "int4")
        self.assertEqual(result["outputs"][0]["element_bits"], 4)
        self.assertIsNone(result["outputs"][0]["element_bytes"])
        self.assertEqual(result["outputs"][0]["byte_size"], 2)
        self.assertEqual(result["network_info"]["name"], "metadata-test")
        self.assertEqual(result["network_info"]["address_width"], 40)
        self.assertEqual(result["memory"]["memory_pool_bytes"], 0x100001234)
        self.assertEqual(result["memory"]["vip_sram_bytes"], 8192)
        self.assertEqual(result["memory"]["axi_sram_bytes"], 4096)
        self.assertEqual(result["checksums"][0]["section_type"], 24)
        self.assertTrue(result["checksums"][0]["matches"])
        self.assertEqual(
            result["checksums"][0]["section_type_name"],
            "NBG_SECTION_COEF_DATA",
        )
        self.assertEqual(
            result["file_checksum"]["normalized_checksum_value_words"], 1
        )

    def test_serializer_file_checksum_scope_and_mismatch(self):
        base = bytearray(make_v2())
        result = nbg_format.parse_bytes(bytes(base))
        integrity = result["file_checksum"]
        self.assertEqual(integrity["algorithm"], "crc32-ieee")
        self.assertEqual(integrity["initial_value"], 0)
        self.assertEqual(
            integrity["scope"], "section-payloads-in-directory-order"
        )
        self.assertEqual(integrity["excluded_alignment_gap_bytes"], 0)
        self.assertTrue(integrity["matches"])

        wrong_header = bytearray(base)
        set_u32(wrong_header, 12, result["header"]["crc32"] ^ 1)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(wrong_header))
        self.assertEqual(caught.exception.code, "file-checksum-mismatch")

        opaque_section = next(
            section for section in result["sections"] if section["type"] == 24
        )
        changed_payload = bytearray(base)
        changed_payload[opaque_section["offset"]] ^= 1
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(changed_payload))
        self.assertEqual(caught.exception.code, "file-checksum-mismatch")

        changed_tail = bytearray(base)
        changed_tail[-1] ^= 1
        tail_result = nbg_format.parse_bytes(bytes(changed_tail))
        self.assertTrue(tail_result["file_checksum"]["matches"])
        self.assertNotEqual(tail_result["sha256"], result["sha256"])

        changed_product = bytearray(base)
        set_u32(changed_product, 8, 0x1000000F)
        product_result = nbg_format.parse_bytes(bytes(changed_product))
        self.assertTrue(product_result["file_checksum"]["matches"])

    def test_checksum_values_are_zero_during_file_checksum(self):
        opaque = b"serializer-order"
        checksum_record = struct.pack(
            "<II", 24, zlib.crc32(opaque) & 0xFFFFFFFF
        )
        base = bytearray(
            make_v2(
                opaque=opaque,
                extra_sections=[(40, checksum_record, 8)],
            )
        )
        result = nbg_format.parse_bytes(bytes(base))
        self.assertTrue(result["file_checksum"]["matches"])
        self.assertEqual(
            result["file_checksum"]["normalized_checksum_value_words"], 1
        )

        checksum_section = next(
            section for section in result["sections"] if section["type"] == 40
        )
        changed_value = bytearray(base)
        changed_value[checksum_section["offset"] + 4] ^= 1
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(changed_value))
        self.assertEqual(caught.exception.code, "checksum-mismatch")

    def test_dyna_io_table_maps_aligned_payload_ranges(self):
        dyna_data = bytes((0, 1, 2, 3)) + bytes(60)
        result = nbg_format.parse_bytes(
            make_v2(
                extra_sections=[
                    (20, dyna_data, 1),
                    (26, struct.pack("<II", 0, 4), 8),
                ]
            )
        )
        profile = result["dynamic_io"]
        self.assertEqual(profile["entry_count"], 1)
        self.assertEqual(profile["region_bytes"], 64)
        self.assertEqual(profile["payload_bytes"], 4)
        self.assertEqual(profile["padding_bytes"], 60)
        self.assertTrue(profile["padding_all_zero"])
        self.assertEqual(
            profile["entries"],
            [
                {
                    "end": 4,
                    "index": 0,
                    "offset": 0,
                    "offset_alignment": 64,
                    "size": 4,
                }
            ],
        )

    def test_dyna_io_pair_shape_ranges_and_padding_fail(self):
        data = bytes((0, 1, 2, 3)) + bytes(60)
        cases = [
            (
                [(20, data, 1)],
                "dyna-io-pair-mismatch",
            ),
            (
                [(26, struct.pack("<II", 0, 4), 8)],
                "dyna-io-pair-mismatch",
            ),
            (
                [(20, data, 1), (26, struct.pack("<II", 0, 4), 4)],
                "invalid-section-record-size",
            ),
            (
                [(20, data, 1), (26, struct.pack("<II", 64, 1), 8)],
                "dyna-io-range",
            ),
            (
                [
                    (20, data, 1),
                    (26, struct.pack("<IIII", 0, 4, 0, 4), 8),
                ],
                "dyna-io-overlap",
            ),
            (
                [
                    (20, data, 1),
                    (26, struct.pack("<IIII", 0, 4, 4, 4), 8),
                ],
                "dyna-io-alignment",
            ),
            (
                [(20, data, 1), (26, struct.pack("<II", 0, 0), 8)],
                "dyna-io-empty-entry",
            ),
            (
                [
                    (20, bytes((0, 1, 2, 3, 9)) + bytes(59), 1),
                    (26, struct.pack("<II", 0, 4), 8),
                ],
                "dyna-io-padding-nonzero",
            ),
        ]
        for sections, code in cases:
            with self.subTest(code=code):
                with self.assertRaises(nbg_format.NbgParseError) as caught:
                    nbg_format.parse_bytes(make_v2(extra_sections=sections))
                self.assertEqual(caught.exception.code, code)

    def test_declared_size_locator_record_size_and_count_fail(self):
        base = bytearray(make_v2())
        cases: list[tuple[int, int, str]] = [
            (16, len(base) - 1, "invalid-header-field"),
            (20, 1, "invalid-header-field"),
            (24, 12, "invalid-directory-locator"),
            (28, 11, "invalid-header-field"),
            (32, 2, "invalid-header-field"),
            (36, 2, "invalid-header-field"),
            (40, 1, "invalid-header-field"),
            (44, 1, "invalid-header-field"),
            (52, 43, "invalid-directory-record-size"),
            (56, 0, "directory-count-limit"),
            (56, 257, "directory-count-limit"),
        ]
        for offset, value, code in cases:
            data = bytearray(base)
            set_u32(data, offset, value)
            with self.subTest(offset=offset, value=value):
                with self.assertRaises(nbg_format.NbgParseError) as caught:
                    nbg_format.parse_bytes(bytes(data))
                self.assertEqual(caught.exception.code, code)

    def test_unknown_section_and_bad_decoded_metadata_fail(self):
        unknown = bytearray(make_v2())
        third_record = 60 + 2 * nbg_format.V2_DIRECTORY_RECORD_BYTES
        set_u32(unknown, third_record, 41)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(unknown))
        self.assertEqual(caught.exception.code, "invalid-section-type")

        bad_stride = make_v2(
            extra_sections=[(35, v2_network_info(), 219)]
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bad_stride)
        self.assertEqual(caught.exception.code, "invalid-section-record-size")

        bad_counts = make_v2(
            extra_sections=[
                (
                    35,
                    v2_network_info(input_count=2),
                    nbg_format.V2_NETWORK_INFO_BYTES,
                )
            ]
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bad_counts)
        self.assertEqual(caught.exception.code, "metadata-count-mismatch")

        bad_checksum = make_v2(
            extra_sections=[(40, struct.pack("<II", 24, 0), 8)]
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bad_checksum)
        self.assertEqual(caught.exception.code, "checksum-mismatch")

        empty_checksums = make_v2(
            extra_sections=[(40, b"", 8)]
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(empty_checksums)
        self.assertEqual(caught.exception.code, "section-count-limit")

    def test_directory_range_and_section_overlap_fail(self):
        base = bytearray(make_v2())

        bad_directory = bytearray(base)
        set_u32(bad_directory, 48, len(bad_directory) - 8)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_directory))
        self.assertEqual(caught.exception.code, "directory-range-overflow")

        header_overlap = bytearray(base)
        first_record = 60
        set_u32(header_overlap, first_record + 4, 0)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(header_overlap))
        self.assertEqual(caught.exception.code, "section-overlap")

        section_overlap = bytearray(base)
        first_data_offset = struct.unpack_from(
            "<I", section_overlap, first_record + 4
        )[0]
        second_record = first_record + nbg_format.V2_DIRECTORY_RECORD_BYTES
        set_u32(section_overlap, second_record + 4, first_data_offset + 1)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(section_overlap))
        self.assertEqual(caught.exception.code, "section-overlap")

    def test_io_stride_size_count_dimensions_and_names_fail(self):
        base = bytearray(make_v2())
        first_record = 60
        first_data_offset = struct.unpack_from(
            "<I", base, first_record + 4
        )[0]

        bad_stride = bytearray(base)
        set_u32(bad_stride, first_record + 32, 111)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_stride))
        self.assertEqual(caught.exception.code, "invalid-io-record-size")

        bad_size = bytearray(base)
        set_u32(bad_size, first_record + 12, 111)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_size))
        self.assertEqual(caught.exception.code, "invalid-io-record-size")

        too_many = make_v2(
            input_record=v2_io("input", (1,)) * 257,
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(too_many)
        self.assertEqual(caught.exception.code, "io-count-limit")

        bad_dimension_count = bytearray(base)
        set_u32(bad_dimension_count, first_data_offset, 0)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_dimension_count))
        self.assertEqual(caught.exception.code, "invalid-dimension-count")

        zero_dimension = bytearray(base)
        set_u32(zero_dimension, first_data_offset + 4, 0)
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(zero_dimension))
        self.assertEqual(caught.exception.code, "invalid-dimension")

        unknown_format_zero_dimension = bytearray(base)
        set_u32(
            unknown_format_zero_dimension,
            first_data_offset + 28,
            0xFFFFFFFF,
        )
        set_u32(
            unknown_format_zero_dimension,
            first_data_offset + 4,
            0,
        )
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(unknown_format_zero_dimension))
        self.assertEqual(caught.exception.code, "invalid-dimension")

        bad_name = bytearray(base)
        bad_name[
            first_data_offset + 48 : first_data_offset + 112
        ] = b"A" * 64
        with self.assertRaises(nbg_format.NbgParseError) as caught:
            nbg_format.parse_bytes(bytes(bad_name))
        self.assertEqual(caught.exception.code, "invalid-name")

    def test_prefix_truncation_and_seeded_random_inputs_never_escape_error(self):
        base = make_v2()
        for size in range(0, len(base), 17):
            with self.subTest(prefix=size):
                with self.assertRaises(nbg_format.NbgParseError):
                    nbg_format.parse_bytes(base[:size])

        randomizer = random.Random(0x360)
        for index in range(64):
            size = randomizer.randint(513, 2048)
            data = bytes(randomizer.getrandbits(8) for _ in range(size))
            with self.subTest(random=index):
                with self.assertRaises(nbg_format.NbgParseError):
                    nbg_format.parse_bytes(data)

    def test_header_byte_flips_are_deterministic_and_bounded(self):
        base = bytearray(make_v2())
        for index in range(nbg_format.V2_HEADER_BYTES):
            changed = bytearray(base)
            changed[index] ^= 0x01
            outcomes = []
            for _repeat in range(2):
                try:
                    result = nbg_format.parse_bytes(bytes(changed))
                    outcomes.append(
                        ("success", result["structural_signature"])
                    )
                except nbg_format.NbgParseError as exc:
                    outcomes.append(("error", exc.code, exc.stage))
            with self.subTest(index=index):
                self.assertEqual(outcomes[0], outcomes[1])

    @unittest.skipUnless(
        (
            REPOSITORY_ROOT
            / "waved/third_party/stock-npu-stack/sample-ctc.nb"
        ).is_file(),
        "ignored stock CTC fixture is not present",
    )
    def test_real_stock_ctc_matches_live_metadata(self):
        path = (
            REPOSITORY_ROOT
            / "waved/third_party/stock-npu-stack/sample-ctc.nb"
        )
        result = nbg_format.parse_path(
            path,
            display_path=(
                "waved/third_party/stock-npu-stack/sample-ctc.nb"
            ),
        )
        self.assertEqual(
            result["sha256"],
            "5d2795be781af80593321cf0ae627f56f4"
            "f6f64e71e95d1d8b0a27f6192a9f66",
        )
        self.assertEqual(result["size"], 2_810_984)
        self.assertEqual(result["version"], 0x00020000)
        self.assertEqual(result["product"], 0x1000000E)
        self.assertEqual(len(result["sections"]), 18)
        self.assertEqual(result["inputs"][0]["dimensions"], [256, 16, 1])
        self.assertEqual(result["inputs"][0]["data_format_name"], "fp16")
        self.assertEqual(result["inputs"][0]["byte_size"], 8192)
        self.assertEqual(
            result["outputs"][0]["dimensions"], [5200, 16, 1]
        )
        self.assertEqual(result["outputs"][0]["data_format_name"], "fp32")
        self.assertEqual(result["outputs"][0]["byte_size"], 332_800)


class InventoryTests(unittest.TestCase):
    def prepare(self, root: Path) -> nbg_format.Corpus:
        model = make_v2()
        model_path = root / "models/graph.nb"
        model_path.parent.mkdir(parents=True)
        model_path.write_bytes(model)
        reference_path = root / "fixtures/reference.nb"
        reference_path.parent.mkdir(parents=True)
        reference_path.write_bytes(model)
        digest = hashlib.sha256(model).hexdigest()
        config_path = root / "corpus.json"
        config_path.write_text(
            json.dumps(valid_config(digest)), encoding="utf-8"
        )
        return nbg_format.load_corpus(
            config_path, repository_root=root
        )

    def test_inventory_and_summary_are_deterministic_and_relative(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus = self.prepare(root)
            first = nbg_format.build_inventory(corpus)
            second = nbg_format.build_inventory(corpus)
            first_bytes = nbg_format.canonical_json(first)
            self.assertEqual(first_bytes, nbg_format.canonical_json(second))
            self.assertNotIn(str(root).encode(), first_bytes)
            self.assertEqual(first["schema_version"], 1)
            self.assertEqual(len(first["files"]), 2)
            summary = nbg_format.render_summary(first)
            self.assertIn("# Generated Wave NBG Corpus Summary", summary)
            self.assertIn("Unique full-file SHA-256 values: 1", summary)
            self.assertIn("Exact duplicate groups", summary)
            self.assertEqual(summary, nbg_format.render_summary(copy.deepcopy(first)))

    def test_structural_signature_changes_with_shape_not_payload_only(self):
        first = nbg_format.parse_bytes(make_v2(opaque=b"one"))
        second = nbg_format.parse_bytes(make_v2(opaque=b"two"))
        self.assertNotEqual(first["sha256"], second["sha256"])
        self.assertEqual(
            first["structural_signature"], second["structural_signature"]
        )
        third = nbg_format.parse_bytes(
            make_v2(input_record=v2_io("input", (8, 2)), opaque=b"one")
        )
        self.assertNotEqual(
            first["structural_signature"], third["structural_signature"]
        )

    def test_parse_path_rejects_symlink_and_non_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target.nb"
            target.write_bytes(make_v2())
            link = root / "link.nb"
            link.symlink_to(target)
            with self.assertRaises(nbg_format.NbgParseError) as caught:
                nbg_format.parse_path(link)
            self.assertEqual(caught.exception.code, "symlink-rejected")
            with self.assertRaises(nbg_format.NbgParseError) as caught:
                nbg_format.parse_path(root)
            self.assertEqual(caught.exception.code, "not-regular-file")


if __name__ == "__main__":
    unittest.main()
