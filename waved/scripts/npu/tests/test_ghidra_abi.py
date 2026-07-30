#!/usr/bin/env python3
"""Contract tests for focused Wave NPU Ghidra evidence orchestration."""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import ghidra_abi  # noqa: E402


def document(digest: str) -> dict:
    return {
        "schema_version": 1,
        "targets": {
            "live-runtime": {
                "firmware": "fw-live",
                "path": "firmwares/live/lib.so",
                "sha256": digest,
                "roles": ["provider", "runtime"],
                "functions": ["aml_module_create", "aml_module_destroy"],
                "string_filters": ["nbg", "product list"],
            }
        },
    }


class FakeExecutor:
    def __init__(self, digest: str):
        self.digest = digest
        self.calls = []

    def analyze(self, target, binary_path):
        self.calls.append((target.evidence_id, binary_path.name))
        return {
            "schema_version": 1,
            "tool": {"name": "Ghidra", "version": "test"},
            "program": {
                "name": binary_path.name,
                "sha256": self.digest,
                "language_id": "AARCH64:LE:64:v8A",
            },
            "targets": [
                {
                    "target": name,
                    "found": True,
                    "symbol_addresses": [],
                    "functions": [],
                }
                for name in reversed(target.functions)
            ],
            "warnings": [],
        }


class ConfigTests(unittest.TestCase):
    def prepare(self, root: Path):
        binary = root / "firmwares/live/lib.so"
        binary.parent.mkdir(parents=True)
        payload = b"\x7fELFsynthetic-wave-npu"
        binary.write_bytes(payload)
        digest = hashlib.sha256(payload).hexdigest()
        config_path = root / "targets.json"
        config_path.write_text(json.dumps(document(digest)), encoding="utf-8")
        return config_path, digest

    def test_valid_config_loads(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            config_path, digest = self.prepare(root)
            config = ghidra_abi.load_config(
                config_path, repository_root=root
            )
            target = config.targets["live-runtime"]
            self.assertEqual(target.sha256, digest)
            self.assertEqual(
                target.functions,
                ("aml_module_create", "aml_module_destroy"),
            )

    def test_schema_unknown_key_path_escape_and_bad_hash_fail(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            config_path, digest = self.prepare(root)

            bad = document(digest)
            bad["schema_version"] = 2
            config_path.write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaisesRegex(
                ghidra_abi.GhidraConfigError, "schema"
            ):
                ghidra_abi.load_config(config_path, repository_root=root)

            bad = document(digest)
            bad["targets"]["live-runtime"]["shell"] = "id"
            config_path.write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaisesRegex(
                ghidra_abi.GhidraConfigError, "unknown"
            ):
                ghidra_abi.load_config(config_path, repository_root=root)

            bad = document(digest)
            bad["targets"]["live-runtime"]["path"] = "../escape"
            config_path.write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaisesRegex(
                ghidra_abi.GhidraConfigError, "escape"
            ):
                ghidra_abi.load_config(config_path, repository_root=root)

            bad = document("not-a-digest")
            config_path.write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaisesRegex(
                ghidra_abi.GhidraConfigError, "SHA-256"
            ):
                ghidra_abi.load_config(config_path, repository_root=root)

    def test_build_checks_target_hash_and_elf(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            config_path, digest = self.prepare(root)
            config = ghidra_abi.load_config(
                config_path, repository_root=root
            )
            target_path = root / "firmwares/live/lib.so"
            target_path.write_bytes(b"\x7fELFchanged")
            with self.assertRaisesRegex(
                ghidra_abi.GhidraExecutionError, "configured SHA-256"
            ):
                ghidra_abi.build_evidence(
                    config, FakeExecutor(digest)
                )

    def test_build_is_deterministic_relative_and_validates_program_hash(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            config_path, digest = self.prepare(root)
            config = ghidra_abi.load_config(
                config_path, repository_root=root
            )
            first = ghidra_abi.build_evidence(
                config, FakeExecutor(digest)
            )
            second = ghidra_abi.build_evidence(
                config, FakeExecutor(digest)
            )
            first_bytes = ghidra_abi.canonical_json(first)
            self.assertEqual(first_bytes, ghidra_abi.canonical_json(second))
            self.assertNotIn(str(root).encode(), first_bytes)
            self.assertEqual(
                first["targets"][0]["path"], "firmwares/live/lib.so"
            )

            executor = FakeExecutor("0" * 64)
            with self.assertRaisesRegex(
                ghidra_abi.GhidraExecutionError, "SHA-256 mismatch"
            ):
                ghidra_abi.build_evidence(config, executor)

    def test_missing_target_record_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            config_path, digest = self.prepare(root)
            config = ghidra_abi.load_config(
                config_path, repository_root=root
            )

            class MissingExecutor(FakeExecutor):
                def analyze(self, target, binary_path):
                    value = super().analyze(target, binary_path)
                    value["targets"] = value["targets"][:1]
                    return value

            with self.assertRaisesRegex(
                ghidra_abi.GhidraExecutionError, "target records missing"
            ):
                ghidra_abi.build_evidence(
                    config, MissingExecutor(digest)
                )


if __name__ == "__main__":
    unittest.main()
