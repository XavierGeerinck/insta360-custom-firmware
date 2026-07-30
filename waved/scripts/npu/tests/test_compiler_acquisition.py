#!/usr/bin/env python3
"""Contract tests for inert Wave compiler acquisition and qualification."""

from __future__ import annotations

import hashlib
import io
import json
import struct
import sys
import tarfile
import tempfile
import unittest
import warnings
import zipfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import compiler_acquisition  # noqa: E402


def synthetic_elf64(*, segment_type: int = 1, machine: int = 62) -> bytes:
    """Return the smallest ELF accepted by the static program-header parser."""

    header = bytearray(compiler_acquisition.ELF64_HEADER_BYTES)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", header, 18, machine)
    struct.pack_into("<Q", header, 32, compiler_acquisition.ELF64_HEADER_BYTES)
    struct.pack_into(
        "<H",
        header,
        54,
        compiler_acquisition.ELF64_PROGRAM_HEADER_BYTES,
    )
    struct.pack_into("<H", header, 56, 1)
    program_header = bytearray(
        compiler_acquisition.ELF64_PROGRAM_HEADER_BYTES
    )
    struct.pack_into("<I", program_header, 0, segment_type)
    return bytes(header + program_header)


class StrictInputTests(unittest.TestCase):
    def test_rejects_duplicate_json_keys(self) -> None:
        with self.assertRaisesRegex(
            compiler_acquisition.TargetConfigError, "duplicate JSON key"
        ):
            compiler_acquisition._load_strict_json_bytes(
                b'{"a": 1, "a": 2}',
                "fixture",
                compiler_acquisition.TargetConfigError,
            )

    def test_parser_literals_allow_lf_but_reject_other_controls(self) -> None:
        self.assertEqual(
            compiler_acquisition._require_literal(
                "chipModel = %x;\n",
                "fixture",
                compiler_acquisition.TargetConfigError,
            ),
            "chipModel = %x;\n",
        )
        for payload in ("bad\x00literal", "bad\tliteral", "bad\rliteral"):
            with self.subTest(payload=repr(payload)):
                with self.assertRaisesRegex(
                    compiler_acquisition.TargetConfigError,
                    "control character",
                ):
                    compiler_acquisition._require_literal(
                        payload,
                        "fixture",
                        compiler_acquisition.TargetConfigError,
                    )

    def test_rejects_ambiguous_or_escaping_archive_names(self) -> None:
        unsafe = (
            "",
            "/absolute",
            "../escape",
            "nested/../escape",
            "nested/./file",
            "nested//file",
            r"nested\file",
            "C:/windows",
            "nul\x00name",
        )
        for name in unsafe:
            with self.subTest(name=repr(name)):
                with self.assertRaises(
                    compiler_acquisition.ArchiveSafetyError
                ):
                    compiler_acquisition._safe_archive_name(name, "fixture")

        self.assertEqual(
            compiler_acquisition._safe_archive_name(
                "safe/nested/file", "fixture"
            ).as_posix(),
            "safe/nested/file",
        )


class ArchiveInspectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_zip_rejects_traversal_duplicate_and_symlink_members(self) -> None:
        traversal = self.root / "traversal.zip"
        with zipfile.ZipFile(traversal, "w") as archive:
            archive.writestr("../escape", b"x")
        with self.assertRaisesRegex(
            compiler_acquisition.ArchiveSafetyError, "unsafe archive member"
        ):
            compiler_acquisition.inspect_zip(traversal)

        duplicate = self.root / "duplicate.zip"
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(duplicate, "w") as archive:
                archive.writestr("same", b"a")
                archive.writestr("same", b"b")
        with self.assertRaisesRegex(
            compiler_acquisition.ArchiveSafetyError, "duplicate member"
        ):
            compiler_acquisition.inspect_zip(duplicate)

        symlink = self.root / "symlink.zip"
        info = zipfile.ZipInfo("link")
        info.create_system = 3
        info.external_attr = (0o120777 << 16)
        with zipfile.ZipFile(symlink, "w") as archive:
            archive.writestr(info, b"target")
        with self.assertRaisesRegex(
            compiler_acquisition.ArchiveSafetyError, "symbolic-link member"
        ):
            compiler_acquisition.inspect_zip(symlink)

    def test_tar_rejects_traversal_special_files_and_escaping_links(self) -> None:
        fixtures = (
            ("traversal", "../escape", tarfile.REGTYPE, ""),
            ("fifo", "fifo", tarfile.FIFOTYPE, ""),
            ("link", "safe/link", tarfile.SYMTYPE, "../../escape"),
        )
        for fixture_name, member_name, member_type, link_name in fixtures:
            with self.subTest(fixture=fixture_name):
                path = self.root / f"{fixture_name}.tar"
                with tarfile.open(path, "w") as archive:
                    info = tarfile.TarInfo(member_name)
                    info.type = member_type
                    info.linkname = link_name
                    archive.addfile(info)
                with self.assertRaises(
                    compiler_acquisition.ArchiveSafetyError
                ):
                    compiler_acquisition.inspect_tar(path, mode="r:")

    def test_member_scan_finds_a_needle_across_read_boundaries(self) -> None:
        path = self.root / "scan.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("payload.bin", b"abcTARGETxyz")
        previous = compiler_acquisition.SCAN_CHUNK_BYTES
        compiler_acquisition.SCAN_CHUNK_BYTES = 5
        self.addCleanup(
            setattr,
            compiler_acquisition,
            "SCAN_CHUNK_BYTES",
            previous,
        )
        hits = compiler_acquisition.search_zip_members(path, (b"TARGET",))
        self.assertEqual(hits[b"TARGET"], ("payload.bin",))

    def test_hashes_selected_zip_member_and_rejects_a_missing_member(self) -> None:
        payload = b"nested acquisition bytes"
        path = self.root / "member.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("nested/member.tar", payload)
        self.assertEqual(
            compiler_acquisition.hash_zip_member(
                path, "nested/member.tar"
            ),
            hashlib.sha256(payload).hexdigest(),
        )
        with self.assertRaisesRegex(
            compiler_acquisition.StaticEvidenceError, "missing ZIP member"
        ):
            compiler_acquisition.hash_zip_member(path, "missing")

    def test_stream_scanner_handles_an_in_memory_boundary(self) -> None:
        previous = compiler_acquisition.SCAN_CHUNK_BYTES
        compiler_acquisition.SCAN_CHUNK_BYTES = 4
        self.addCleanup(
            setattr,
            compiler_acquisition,
            "SCAN_CHUNK_BYTES",
            previous,
        )
        hits = compiler_acquisition._archive_stream_contains(
            io.BytesIO(b"123VIV_GPU_FILE456"),
            (b"VIV_GPU_FILE", b"missing"),
        )
        self.assertEqual(hits, {b"VIV_GPU_FILE"})


class StaticElfTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_accepts_static_x86_64_and_rejects_dynamic_or_wrong_machine(self) -> None:
        static_path = self.root / "static"
        static_path.write_bytes(synthetic_elf64())
        evidence = compiler_acquisition.inspect_elf64_static(static_path)
        self.assertEqual(evidence["machine"], "x86-64")
        self.assertFalse(evidence["has_pt_dynamic"])
        self.assertFalse(evidence["has_pt_interp"])

        dynamic_path = self.root / "dynamic"
        dynamic_path.write_bytes(
            synthetic_elf64(segment_type=compiler_acquisition.PT_DYNAMIC)
        )
        with self.assertRaisesRegex(
            compiler_acquisition.StaticEvidenceError,
            "not statically linked",
        ):
            compiler_acquisition.inspect_elf64_static(dynamic_path)

        wrong_machine = self.root / "aarch64"
        wrong_machine.write_bytes(synthetic_elf64(machine=183))
        with self.assertRaisesRegex(
            compiler_acquisition.StaticEvidenceError, "expected x86-64"
        ):
            compiler_acquisition.inspect_elf64_static(wrong_machine)

    def test_decodes_relative_call_and_rejects_other_opcodes(self) -> None:
        call_address = compiler_acquisition.LOAD_BIAS + 0x20
        target_address = compiler_acquisition.LOAD_BIAS + 0x80
        payload = bytearray(0x100)
        payload[0x20] = 0xE8
        struct.pack_into(
            "<i",
            payload,
            0x21,
            target_address - (call_address + 5),
        )
        path = self.root / "call.bin"
        path.write_bytes(payload)
        self.assertEqual(
            compiler_acquisition._decode_relative_call_target(
                path, call_address
            ),
            target_address,
        )

        payload[0x20] = 0x90
        path.write_bytes(payload)
        with self.assertRaisesRegex(
            compiler_acquisition.StaticEvidenceError, "expected relative CALL"
        ):
            compiler_acquisition._decode_relative_call_target(
                path, call_address
            )


class QualificationPrimitiveTests(unittest.TestCase):
    def test_sdk_version_gate_ignores_build_but_not_patch(self) -> None:
        self.assertFalse(
            compiler_acquisition._version_meets_minimum(
                "6.4.18:967862", "6.4.22"
            )
        )
        self.assertTrue(
            compiler_acquisition._version_meets_minimum(
                "6.4.22:1", "6.4.22"
            )
        )
        self.assertTrue(
            compiler_acquisition._version_meets_minimum("6.5.0", "6.4.22")
        )
        with self.assertRaisesRegex(
            compiler_acquisition.StaticEvidenceError, "invalid SDK version"
        ):
            compiler_acquisition._version_meets_minimum("6.4", "6.4.22")

    def test_canonical_json_is_stable_and_newline_terminated(self) -> None:
        first = compiler_acquisition.canonical_json({"z": 1, "a": 2})
        second = compiler_acquisition.canonical_json({"a": 2, "z": 1})
        self.assertEqual(first, second)
        self.assertTrue(first.endswith(b"\n"))
        self.assertEqual(json.loads(first), {"a": 2, "z": 1})


if __name__ == "__main__":
    unittest.main()
