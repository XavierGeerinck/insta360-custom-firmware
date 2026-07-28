#!/usr/bin/env python3
"""Contract tests for deterministic Wave NPU static ABI inventory."""

from __future__ import annotations

import copy
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import abi_inventory  # noqa: E402


def valid_document() -> dict:
    return {
        "schema_version": 1,
        "reference_firmware": "fw-live",
        "firmwares": {
            "fw-old": {"root": "firmwares/old/rootfs", "live_match": False},
            "fw-live": {"root": "firmwares/live/rootfs", "live_match": True},
        },
        "binaries": {
            "liboptional.so": {
                "path": "usr/lib/liboptional.so",
                "required": False,
                "roles": ["optional", "provider"],
            },
            "libruntime.so": {
                "path": "usr/lib/libruntime.so",
                "required": True,
                "roles": ["provider", "runtime"],
            },
        },
        "symbol_filters": {
            "exact": ["aml_module_create", "aml_module_destroy"],
            "prefixes": ["aml_module_", "read_nbg_"],
            "contains": ["Adla"],
        },
        "string_filters": ["nbg", "product list"],
        "caller_scan": {
            "directories": ["usr/bin", "usr/lib"],
            "needles": ["aml_module_"],
            "maximum_file_bytes": 4096,
        },
        "project_ffi": ["aml_module_create", "aml_module_destroy"],
    }


def info_response() -> dict:
    return {
        "info": {
            "arch": "arm",
            "bits": 64,
            "endian": "LE",
            "bintype": "elf",
            "class": "ELF64",
            "machine": "ARM aarch64",
            "PIE": True,
            "stripped": True,
            "baddr": 0,
        }
    }


def tool_responses(
    *,
    create_bytes: tuple[int, int] = (8, 8),
    create_size: int = 8,
    include_destroy: bool = True,
) -> dict[str, dict]:
    create_paddr, create_vaddr = create_bytes
    symbols = [
        {
            "name": "aml_module_create",
            "realname": "aml_module_create",
            "bind": "GLOBAL",
            "type": "FUNC",
            "vaddr": create_vaddr,
            "paddr": create_paddr,
            "size": create_size,
            "is_imported": False,
        },
        {
            "name": "read_nbg_from_memory",
            "realname": "read_nbg_from_memory",
            "bind": "GLOBAL",
            "type": "FUNC",
            "vaddr": 24,
            "paddr": 24,
            "size": 20,
            "is_imported": False,
        },
        {
            "name": "totally_unrelated",
            "realname": "totally_unrelated",
            "bind": "GLOBAL",
            "type": "FUNC",
            "vaddr": 48,
            "paddr": 48,
            "size": 4,
            "is_imported": False,
        },
        {
            "name": "vipunify_core.c",
            "realname": "vipunify_core.c",
            "bind": "LOCAL",
            "size": 0,
            "type": "FILE",
            "vaddr": 0,
            "paddr": 0,
            "is_imported": False,
        },
    ]
    if include_destroy:
        symbols.append(
            {
                "name": "aml_module_destroy",
                "realname": "aml_module_destroy",
                "bind": "GLOBAL",
                "type": "FUNC",
                "vaddr": 52,
                "paddr": 52,
                "size": 20,
                "is_imported": False,
            }
        )
    return {
        "info": info_response(),
        "libs": {"libs": ["libc.so", "libovxlib.so"]},
        "imports": {
            "imports": [
                {
                    "name": "aml_module_input_set",
                    "bind": "GLOBAL",
                    "type": "FUNC",
                    "plt": 100,
                },
                {
                    "name": "printf",
                    "bind": "GLOBAL",
                    "type": "FUNC",
                    "plt": 104,
                },
            ]
        },
        "symbols": {"symbols": symbols},
        "relocs": {
            "relocs": [
                {
                    "name": "aml_module_input_set",
                    "type": "R_AARCH64_JUMP_SLOT",
                    "vaddr": 100,
                    "paddr": 100,
                },
                {
                    "type": "R_AARCH64_RELATIVE",
                    "vaddr": 200,
                    "paddr": 200,
                },
            ]
        },
        "strings": {
            "strings": [
                {
                    "string": "hardware version is not in product list",
                    "vaddr": 300,
                    "paddr": 300,
                    "length": 39,
                    "section": ".rodata",
                },
                {
                    "string": "ordinary unrelated message",
                    "vaddr": 400,
                    "paddr": 400,
                    "length": 26,
                    "section": ".rodata",
                },
            ]
        },
    }


class FakeRzBin:
    def __init__(self, responses_by_path=None, default=None):
        self.responses_by_path = responses_by_path or {}
        self.default = default or tool_responses()
        self.calls: list[tuple[str, str]] = []

    def version(self) -> str:
        return "rz-bin test 1.0"

    def query(self, path: Path, query: str) -> dict:
        self.calls.append((path.name, query))
        responses = self.responses_by_path.get(path.name, self.default)
        value = responses[query]
        if isinstance(value, Exception):
            raise value
        return copy.deepcopy(value)


def write_binary(path: Path, marker: bytes = b"aml_module_create") -> bytes:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (
        b"\x7fELF"
        + b"ABCD"
        + b"create00"
        + marker
        + b"\x00"
        + b"0123456789abcdefghijklmnopqrstuvwxyz"
        + b"destroy-function-bytes"
    )
    path.write_bytes(payload)
    return payload


class CorpusTests(unittest.TestCase):
    def load(self, document: dict, root: Path) -> abi_inventory.Corpus:
        path = root / "corpus.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        return abi_inventory.load_corpus(path, repository_root=root)

    def test_valid_corpus_loads_and_is_immutable_enough(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus = self.load(valid_document(), root)
            self.assertEqual(corpus.reference_firmware, "fw-live")
            self.assertEqual(corpus.firmwares["fw-live"].live_match, True)
            self.assertEqual(
                corpus.binaries["libruntime.so"].path,
                Path("usr/lib/libruntime.so"),
            )

    def test_unknown_schema_and_keys_fail(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            document = valid_document()
            document["schema_version"] = 9
            with self.assertRaisesRegex(abi_inventory.CorpusError, "schema"):
                self.load(document, root)

            document = valid_document()
            document["surprise"] = True
            with self.assertRaisesRegex(abi_inventory.CorpusError, "unknown"):
                self.load(document, root)

            document = valid_document()
            document["binaries"]["libruntime.so"]["shell"] = "id"
            with self.assertRaisesRegex(abi_inventory.CorpusError, "unknown"):
                self.load(document, root)

    def test_duplicate_key_fails(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            path = root / "corpus.json"
            path.write_text(
                '{"schema_version":1,"schema_version":1}', encoding="utf-8"
            )
            with self.assertRaisesRegex(abi_inventory.CorpusError, "duplicate"):
                abi_inventory.load_corpus(path, repository_root=root)

    def test_path_escape_and_reference_mismatch_fail(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            document = valid_document()
            document["firmwares"]["fw-live"]["root"] = "../outside"
            with self.assertRaisesRegex(abi_inventory.CorpusError, "escape"):
                self.load(document, root)

            document = valid_document()
            document["binaries"]["libruntime.so"]["path"] = "../../escape.so"
            with self.assertRaisesRegex(abi_inventory.CorpusError, "escape"):
                self.load(document, root)

            document = valid_document()
            document["reference_firmware"] = "missing"
            with self.assertRaisesRegex(abi_inventory.CorpusError, "reference"):
                self.load(document, root)

    def test_exactly_one_live_match_is_required(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            document = valid_document()
            document["firmwares"]["fw-old"]["live_match"] = True
            with self.assertRaisesRegex(abi_inventory.CorpusError, "live_match"):
                self.load(document, root)


class InventoryTests(unittest.TestCase):
    def prepare(self, root: Path, *, old_marker=b"aml_module_create"):
        document = valid_document()
        corpus_path = root / "corpus.json"
        corpus_path.write_text(json.dumps(document), encoding="utf-8")
        live_path = (
            root
            / "firmwares/live/rootfs/usr/lib/libruntime.so"
        )
        old_path = root / "firmwares/old/rootfs/usr/lib/libruntime.so"
        live_payload = write_binary(live_path)
        old_payload = write_binary(old_path, marker=old_marker)
        return (
            abi_inventory.load_corpus(corpus_path, repository_root=root),
            live_payload,
            old_payload,
        )

    def test_required_missing_fails_and_optional_is_represented(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            document = valid_document()
            corpus_path = root / "corpus.json"
            corpus_path.write_text(json.dumps(document), encoding="utf-8")
            corpus = abi_inventory.load_corpus(
                corpus_path, repository_root=root
            )
            with self.assertRaisesRegex(
                abi_inventory.InventoryError, "required binary"
            ):
                abi_inventory.build_inventory(corpus, FakeRzBin())

            write_binary(
                root / "firmwares/live/rootfs/usr/lib/libruntime.so"
            )
            write_binary(root / "firmwares/old/rootfs/usr/lib/libruntime.so")
            inventory = abi_inventory.build_inventory(corpus, FakeRzBin())
            optional = inventory["firmwares"]["fw-live"]["binaries"][
                "liboptional.so"
            ]
            self.assertFalse(optional["present"])
            self.assertEqual(optional["status"], "optional-absent")

    def test_binary_hash_size_info_and_dependencies_are_exact(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, live_payload, _ = self.prepare(root)
            inventory = abi_inventory.build_inventory(corpus, FakeRzBin())
            binary = inventory["firmwares"]["fw-live"]["binaries"][
                "libruntime.so"
            ]
            self.assertEqual(binary["bytes"], len(live_payload))
            self.assertEqual(
                binary["sha256"], hashlib.sha256(live_payload).hexdigest()
            )
            self.assertEqual(binary["elf"]["machine"], "ARM aarch64")
            self.assertEqual(binary["needed"], ["libc.so", "libovxlib.so"])
            self.assertFalse(Path(binary["path"]).is_absolute())

    def test_relevant_symbols_imports_relocs_and_strings_only(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, _, _ = self.prepare(root)
            inventory = abi_inventory.build_inventory(corpus, FakeRzBin())
            binary = inventory["firmwares"]["fw-live"]["binaries"][
                "libruntime.so"
            ]
            export_names = [item["name"] for item in binary["exports"]]
            import_names = [item["name"] for item in binary["imports"]]
            strings = [item["string"] for item in binary["strings"]]
            self.assertEqual(
                export_names,
                [
                    "aml_module_create",
                    "aml_module_destroy",
                    "read_nbg_from_memory",
                ],
            )
            self.assertEqual(import_names, ["aml_module_input_set"])
            self.assertEqual(
                [item["name"] for item in binary["relocations"]],
                ["aml_module_input_set"],
            )
            self.assertEqual(
                strings, ["hardware version is not in product list"]
            )
            self.assertNotIn("totally_unrelated", json.dumps(binary))
            self.assertNotIn("ordinary unrelated", json.dumps(binary))

    def test_function_bytes_use_physical_offset_and_flag_thunk(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, live_payload, _ = self.prepare(root)
            responses = tool_responses(create_bytes=(8, 999), create_size=8)
            inventory = abi_inventory.build_inventory(
                corpus, FakeRzBin(default=responses)
            )
            create = next(
                item
                for item in inventory["firmwares"]["fw-live"]["binaries"][
                    "libruntime.so"
                ]["exports"]
                if item["name"] == "aml_module_create"
            )
            self.assertEqual(
                create["function_sha256"],
                hashlib.sha256(live_payload[8:16]).hexdigest(),
            )
            self.assertEqual(create["byte_hash_status"], "hashed")
            self.assertTrue(create["probable_thunk"])

    def test_invalid_function_range_never_reads_out_of_bounds(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, _, _ = self.prepare(root)
            responses = tool_responses(
                create_bytes=(10_000, 8), create_size=500
            )
            inventory = abi_inventory.build_inventory(
                corpus, FakeRzBin(default=responses)
            )
            create = next(
                item
                for item in inventory["firmwares"]["fw-live"]["binaries"][
                    "libruntime.so"
                ]["exports"]
                if item["name"] == "aml_module_create"
            )
            self.assertIsNone(create["function_sha256"])
            self.assertEqual(create["byte_hash_status"], "invalid-range")

    def test_tool_failure_and_malformed_json_are_clear(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, _, _ = self.prepare(root)
            responses = tool_responses()
            responses["info"] = abi_inventory.ToolError("synthetic failure")
            with self.assertRaisesRegex(
                abi_inventory.InventoryError, "synthetic failure"
            ):
                abi_inventory.build_inventory(
                    corpus, FakeRzBin(default=responses)
                )

            responses = tool_responses()
            responses["symbols"] = {"wrong": []}
            with self.assertRaisesRegex(
                abi_inventory.InventoryError, "symbols"
            ):
                abi_inventory.build_inventory(
                    corpus, FakeRzBin(default=responses)
                )

    def test_caller_discovery_is_bounded_and_filters_imports(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, _, _ = self.prepare(root)
            caller = root / "firmwares/live/rootfs/usr/bin/audio-app"
            write_binary(caller, marker=b"aml_module_create")
            unrelated = root / "firmwares/live/rootfs/usr/bin/unrelated"
            write_binary(unrelated, marker=b"nothing-relevant")

            caller_responses = tool_responses()
            caller_responses["imports"] = {
                "imports": [
                    {
                        "name": "aml_module_create",
                        "bind": "GLOBAL",
                        "type": "FUNC",
                        "plt": 88,
                    },
                    {
                        "name": "puts",
                        "bind": "GLOBAL",
                        "type": "FUNC",
                        "plt": 92,
                    },
                ]
            }
            tool = FakeRzBin(
                responses_by_path={"audio-app": caller_responses}
            )
            inventory = abi_inventory.build_inventory(corpus, tool)
            callers = inventory["firmwares"]["fw-live"][
                "discovered_callers"
            ]
            self.assertEqual(len(callers), 1)
            self.assertEqual(callers[0]["path"], "usr/bin/audio-app")
            self.assertEqual(
                [item["name"] for item in callers[0]["imports"]],
                ["aml_module_create"],
            )
            self.assertFalse(
                any(call[0] == "unrelated" for call in tool.calls)
            )

    def test_inventory_is_deterministic_and_has_no_absolute_root(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            corpus, _, _ = self.prepare(root)
            first = abi_inventory.build_inventory(corpus, FakeRzBin())
            second = abi_inventory.build_inventory(corpus, FakeRzBin())
            first_bytes = abi_inventory.canonical_json(first)
            second_bytes = abi_inventory.canonical_json(second)
            self.assertEqual(first_bytes, second_bytes)
            self.assertNotIn(str(root).encode(), first_bytes)


class MatrixTests(unittest.TestCase):
    def inventory(self):
        return {
            "reference_firmware": "fw-live",
            "firmwares": {
                "fw-live": {
                    "binaries": {
                        "lib.so": {
                            "present": True,
                            "exports": [
                                {
                                    "name": "same",
                                    "function_sha256": "a" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 20,
                                },
                                {
                                    "name": "changed",
                                    "function_sha256": "b" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 20,
                                },
                                {
                                    "name": "removed",
                                    "function_sha256": "c" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 20,
                                },
                                {
                                    "name": "unknown-bytes",
                                    "function_sha256": None,
                                    "byte_hash_status": "invalid-range",
                                    "size": 0,
                                },
                            ],
                        }
                    }
                },
                "fw-old": {
                    "binaries": {
                        "lib.so": {
                            "present": True,
                            "exports": [
                                {
                                    "name": "same",
                                    "function_sha256": "a" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 20,
                                },
                                {
                                    "name": "changed",
                                    "function_sha256": "d" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 24,
                                },
                                {
                                    "name": "added",
                                    "function_sha256": "e" * 64,
                                    "byte_hash_status": "hashed",
                                    "size": 16,
                                },
                                {
                                    "name": "unknown-bytes",
                                    "function_sha256": None,
                                    "byte_hash_status": "size-only",
                                    "size": 0,
                                },
                            ],
                        }
                    }
                },
                "fw-missing": {
                    "binaries": {
                        "lib.so": {
                            "present": False,
                            "exports": [],
                        }
                    }
                },
            },
        }

    def test_matrix_classifies_all_states(self):
        matrix = abi_inventory.build_matrix(self.inventory())
        states = {
            row["symbol"]: row["firmwares"]["fw-old"]["state"]
            for row in matrix["binaries"]["lib.so"]
        }
        self.assertEqual(states["same"], "identical")
        self.assertEqual(states["changed"], "changed")
        self.assertEqual(states["removed"], "removed")
        self.assertEqual(states["added"], "added")
        self.assertEqual(states["unknown-bytes"], "size-only")
        for row in matrix["binaries"]["lib.so"]:
            self.assertEqual(
                row["firmwares"]["fw-missing"]["state"], "absent"
            )

    def test_markdown_is_stable_and_contains_reference(self):
        matrix = abi_inventory.build_matrix(self.inventory())
        first = abi_inventory.render_matrix_markdown(matrix)
        second = abi_inventory.render_matrix_markdown(matrix)
        self.assertEqual(first, second)
        self.assertIn("Reference firmware: `fw-live`", first)
        self.assertIn("| `same` |", first)
        self.assertIn("identical", first)


if __name__ == "__main__":
    unittest.main()
