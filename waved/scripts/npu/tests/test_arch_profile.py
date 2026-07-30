#!/usr/bin/env python3
"""Contract tests for host-only Wave architecture-profile recovery."""

from __future__ import annotations

import copy
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import arch_profile  # noqa: E402


def target_document(digest: str) -> dict:
    return {
        "schema_version": 1,
        "targets": {
            "wave-archmodel-aarch64": {
                "firmware": "fw-test",
                "path": "firmware/libArchModelSw.so",
                "sha256": digest,
                "roles": [
                    "architecture-model",
                    "exact-wave",
                    "live-matched",
                ],
                "functions": [
                    "archShowConfigurations",
                    "archSwLibInitFromDriver",
                ],
                "string_filters": ["ChipID", "featureDB"],
            },
            "wave-nnarchperf-aarch64": {
                "firmware": "fw-test",
                "path": "firmware/libNNArchPerf.so",
                "sha256": digest,
                "roles": [
                    "architecture-performance",
                    "exact-wave",
                    "live-matched",
                ],
                "functions": [
                    "APMDeriveHWFeatures",
                    "APMGetBugFeatures",
                ],
                "string_filters": ["NumNNCores", "VIP SRAM"],
            },
            "x86-sim-archmodel": {
                "firmware": "sim-test",
                "path": "sim/libArchModelSw.so",
                "sha256": digest,
                "roles": [
                    "architecture-model",
                    "differential-reference",
                    "x86-simulator",
                ],
                "functions": [
                    "archSwLibInitFromDriver",
                    "archIsFeatureAvailable",
                ],
                "string_filters": ["ChipID", "featureDB"],
            },
            "x86-sim-nnarchperf": {
                "firmware": "sim-test",
                "path": "sim/libNNArchPerf.so",
                "sha256": digest,
                "roles": [
                    "architecture-performance",
                    "differential-reference",
                    "x86-simulator",
                ],
                "functions": [
                    "APMDeriveHWFeatures",
                    "APMGetBugFeatures",
                ],
                "string_filters": ["NumNNCores", "VIP SRAM"],
            },
        },
    }


def source(artifact_id: str, location: str = "test location") -> dict:
    return {"artifact_id": artifact_id, "location": location}


def fact(
    value,
    *,
    evidence_class: str = "static-verified",
    validation_state: str = "single-source",
    sources: list[dict] | None = None,
) -> dict:
    return {
        "value": value,
        "representation": "test",
        "evidence_class": evidence_class,
        "confidence": "high" if value is not None else "none",
        "sources": (
            [source("wave-archmodel-aarch64")]
            if sources is None and value is not None
            else (sources or [])
        ),
        "validation_state": validation_state,
        "notes": "test fact",
    }


def profile_document(digest: str) -> dict:
    return {
        "schema_version": 1,
        "profile_id": "wave-fw-test-0x1000000e",
        "target": {
            "value": 0x1000000E,
            "hex": "0x1000000e",
        },
        "inputs": [
            {
                "id": "wave-archmodel-aarch64",
                "path": "firmware/libArchModelSw.so",
                "sha256": digest,
                "bytes": 23,
                "roles": ["architecture-model", "exact-wave"],
            },
            {
                "id": "poc5d-plan",
                "path": "docs/poc5d.md",
                "sha256": digest,
                "bytes": 17,
                "roles": ["observed-live", "prior-poc"],
            },
        ],
        "identity": {
            "customer_id": fact(
                0x1000000E,
                evidence_class="observed-live",
                validation_state="corroborated",
                sources=[source("poc5d-plan")],
            ),
            "lookup_key_order": fact(
                None,
                evidence_class="unknown",
                validation_state="unresolved",
            ),
        },
        "features": {
            "evis2": fact(
                True,
                evidence_class="observed-live",
                validation_state="corroborated",
                sources=[source("poc5d-plan")],
            )
        },
        "resources": {
            "physical_npu_core_count": fact(
                None,
                evidence_class="unknown",
                validation_state="unresolved",
            )
        },
        "versions": {
            "galcore_runtime": fact(
                "6.4.19.5.946407",
                evidence_class="observed-live",
                validation_state="corroborated",
                sources=[source("poc5d-plan")],
            )
        },
        "unknowns": [
            "identity.lookup_key_order",
            "resources.physical_npu_core_count",
        ],
        "non_claims": ["This profile does not prove physical core count."],
    }


class FakeExecutor:
    def analyze(self, target, binary_path):
        return {
            "schema_version": 1,
            "tool": {"name": "Ghidra", "version": "test"},
            "program": {
                "name": binary_path.name,
                "sha256": target.sha256,
                "language_id": (
                    "x86:LE:64:default"
                    if "x86" in target.evidence_id
                    else "AARCH64:LE:64:v8A"
                ),
                "compiler_spec_id": "gcc",
                "format": "ELF",
            },
            "targets": [
                {
                    "target": function,
                    "found": True,
                    "symbol_addresses": ["00000000"],
                    "functions": [],
                }
                for function in target.functions
            ],
            "warnings": [],
        }


class TargetConfigTests(unittest.TestCase):
    def prepare(self, root: Path) -> tuple[Path, str]:
        payload = b"\x7fELFsynthetic-profile"
        digest = hashlib.sha256(payload).hexdigest()
        for relative in (
            "firmware/libArchModelSw.so",
            "firmware/libNNArchPerf.so",
            "sim/libArchModelSw.so",
            "sim/libNNArchPerf.so",
        ):
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
        config_path = root / "targets.json"
        config_path.write_text(json.dumps(target_document(digest)), encoding="utf-8")
        return config_path, digest

    def test_valid_target_config_and_fake_analysis_are_deterministic(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path, _ = self.prepare(root)
            config = arch_profile.load_target_config(path, repository_root=root)
            first = arch_profile.build_ghidra_evidence(config, FakeExecutor())
            second = arch_profile.build_ghidra_evidence(config, FakeExecutor())
            self.assertEqual(
                arch_profile.canonical_json(first),
                arch_profile.canonical_json(second),
            )
            self.assertEqual(len(first["targets"]), 4)

    def test_target_config_requires_exact_four_roles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path, digest = self.prepare(root)
            document = target_document(digest)
            del document["targets"]["x86-sim-nnarchperf"]
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(
                arch_profile.ArchProfileConfigError,
                "target IDs",
            ):
                arch_profile.load_target_config(path, repository_root=root)

    def test_duplicate_key_escape_bad_hash_and_non_elf_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path, digest = self.prepare(root)

            path.write_text(
                '{"schema_version":1,"schema_version":1,"targets":{}}',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                arch_profile.ArchProfileConfigError, "duplicate"
            ):
                arch_profile.load_target_config(path, repository_root=root)

            document = target_document(digest)
            document["targets"]["wave-archmodel-aarch64"]["path"] = "../escape"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(arch_profile.ArchProfileConfigError, "escape"):
                arch_profile.load_target_config(path, repository_root=root)

            document = target_document("A" * 64)
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(arch_profile.ArchProfileConfigError, "SHA-256"):
                arch_profile.load_target_config(path, repository_root=root)

            path, _ = self.prepare(root)
            (root / "firmware/libArchModelSw.so").write_bytes(b"not-elf")
            config = arch_profile.load_target_config(path, repository_root=root)
            with self.assertRaisesRegex(
                arch_profile.ArchProfileEvidenceError,
                "configured SHA-256|not ELF",
            ):
                arch_profile.build_ghidra_evidence(config, FakeExecutor())


class ProfileValidationTests(unittest.TestCase):
    def setUp(self):
        self.digest = hashlib.sha256(b"source").hexdigest()
        self.profile = profile_document(self.digest)

    def test_valid_profile_is_canonical_and_deterministic(self):
        validated = arch_profile.validate_profile(self.profile)
        first = arch_profile.canonical_json(validated)
        second = arch_profile.canonical_json(
            arch_profile.validate_profile(copy.deepcopy(self.profile))
        )
        self.assertEqual(first, second)
        self.assertIn(b'"target"', first)

    def test_rejects_unknown_root_and_fact_keys(self):
        changed = copy.deepcopy(self.profile)
        changed["shell"] = "id"
        with self.assertRaisesRegex(arch_profile.ProfileValidationError, "unknown"):
            arch_profile.validate_profile(changed)

        changed = copy.deepcopy(self.profile)
        changed["features"]["evis2"]["guess"] = True
        with self.assertRaisesRegex(arch_profile.ProfileValidationError, "unknown"):
            arch_profile.validate_profile(changed)

    def test_rejects_wrong_target_and_bad_evidence_enums(self):
        changed = copy.deepcopy(self.profile)
        changed["target"]["hex"] = "0x10000020"
        with self.assertRaisesRegex(arch_profile.ProfileValidationError, "target"):
            arch_profile.validate_profile(changed)

        changed = copy.deepcopy(self.profile)
        changed["features"]["evis2"]["evidence_class"] = "guessed"
        with self.assertRaisesRegex(
            arch_profile.ProfileValidationError, "evidence_class"
        ):
            arch_profile.validate_profile(changed)

        changed = copy.deepcopy(self.profile)
        changed["features"]["evis2"]["validation_state"] = "probably"
        with self.assertRaisesRegex(
            arch_profile.ProfileValidationError, "validation_state"
        ):
            arch_profile.validate_profile(changed)

    def test_rejects_duplicate_or_missing_source_reference(self):
        changed = copy.deepcopy(self.profile)
        duplicate = copy.deepcopy(changed["features"]["evis2"]["sources"][0])
        changed["features"]["evis2"]["sources"].append(duplicate)
        with self.assertRaisesRegex(
            arch_profile.ProfileValidationError, "duplicate source"
        ):
            arch_profile.validate_profile(changed)

        changed = copy.deepcopy(self.profile)
        changed["features"]["evis2"]["sources"][0]["artifact_id"] = "missing"
        with self.assertRaisesRegex(
            arch_profile.ProfileValidationError, "unknown artifact"
        ):
            arch_profile.validate_profile(changed)

    def test_unknown_inventory_must_match_unknown_facts(self):
        changed = copy.deepcopy(self.profile)
        changed["unknowns"] = []
        with self.assertRaisesRegex(arch_profile.ProfileValidationError, "unknowns"):
            arch_profile.validate_profile(changed)

        changed = copy.deepcopy(self.profile)
        changed["resources"]["physical_npu_core_count"]["value"] = 16
        with self.assertRaisesRegex(
            arch_profile.ProfileValidationError,
            "unknown evidence",
        ):
            arch_profile.validate_profile(changed)

    def test_profile_loader_rejects_duplicate_json_key(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "profile.json"
            path.write_text(
                '{"schema_version":1,"schema_version":1}',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                arch_profile.ProfileValidationError, "duplicate"
            ):
                arch_profile.load_profile(path)

    def test_atomic_json_write_is_canonical_and_leaves_no_temp_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "nested/profile.json"
            document = {"z": 2, "a": 1}
            arch_profile._atomic_write_json(path, document)
            self.assertEqual(path.read_bytes(), arch_profile.canonical_json(document))
            self.assertEqual(
                sorted(item.name for item in path.parent.iterdir()),
                ["profile.json"],
            )

    def test_tracked_schema_has_the_same_closed_root_contract(self):
        schema = json.loads(arch_profile.DEFAULT_SCHEMA.read_text(encoding="utf-8"))
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(set(schema["required"]), arch_profile.PROFILE_ROOT_KEYS)
        self.assertEqual(
            schema["properties"]["target"]["properties"]["value"]["const"],
            0x1000000E,
        )


@unittest.skipUnless(
    arch_profile.DEFAULT_GHIDRA_EVIDENCE.is_file(),
    "real PoC 9A Ghidra evidence is not present",
)
class RealCorpusRecoveryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = arch_profile.load_target_config()
        cls.evidence = arch_profile.load_ghidra_evidence(
            arch_profile.DEFAULT_GHIDRA_EVIDENCE, cls.config
        )

    def test_recovers_unique_wave_rows_without_loading_elf(self):
        recovered = arch_profile.recover_architecture_tables(self.config, self.evidence)
        wave = recovered["wave-nnarchperf-aarch64"]
        simulator = recovered["x86-sim-nnarchperf"]
        self.assertEqual(
            wave.identity_words,
            (
                0x00009000,
                0x00009001,
                0x05090001,
                0x04000000,
                0x1000000E,
            ),
        )
        self.assertEqual(wave.matched_row_index, 320)
        self.assertEqual(wave.record_count, 457)
        self.assertEqual(wave.record_size, 576)
        self.assertEqual(simulator.matched_row_index, 290)
        self.assertEqual(simulator.record_count, 387)
        self.assertEqual(simulator.record_size, 552)
        self.assertEqual(wave.generic_revision_marker, 0)
        self.assertEqual(simulator.generic_revision_marker, 0)

    def test_real_profile_generation_is_deterministic(self):
        first = arch_profile.build_profile_document(
            self.config,
            self.evidence,
            evidence_path=arch_profile.DEFAULT_GHIDRA_EVIDENCE,
        )
        second = arch_profile.build_profile_document(
            self.config,
            self.evidence,
            evidence_path=arch_profile.DEFAULT_GHIDRA_EVIDENCE,
        )
        self.assertEqual(
            arch_profile.canonical_json(first),
            arch_profile.canonical_json(second),
        )
        self.assertEqual(first["identity"]["chip_revision_observed"]["value"], 0x9002)
        self.assertEqual(first["identity"]["chip_revision_database"]["value"], 0x9001)
        self.assertIn("resources.physical_npu_core_count", first["unknowns"])

    def test_rejects_evidence_from_a_different_manifest(self):
        changed = copy.deepcopy(self.evidence)
        changed["config_sha256"] = "0" * 64
        with tempfile.TemporaryDirectory() as temporary:
            evidence_path = Path(temporary) / "evidence.json"
            evidence_path.write_bytes(arch_profile.canonical_json(changed))
            with self.assertRaisesRegex(
                arch_profile.ArchProfileEvidenceError,
                "current target config",
            ):
                arch_profile.load_ghidra_evidence(evidence_path, self.config)


if __name__ == "__main__":
    unittest.main()
