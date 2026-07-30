#!/usr/bin/env python3
"""Contract tests for host-only compiler-profile recovery."""

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

import compiler_profile  # noqa: E402

REPOSITORY_ROOT = Path(__file__).resolve().parents[4]


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


class CorpusFixture:
    """Create a minimal hash-pinned corpus below a temporary repository."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.elf_payload = b"\x7fELF" + b"\x00" * 124
        self.text_payload = b"VIV_GPU_FILE=/vendor/example.config\n"
        (root / "artifacts").mkdir(exist_ok=True)
        (root / "artifacts/parser.so").write_bytes(self.elf_payload)
        (root / "artifacts/README").write_bytes(self.text_payload)
        self.path = root / "corpus.json"
        self.document = {
            "schema_version": 1,
            "artifacts": {
                "parser": {
                    "path": "artifacts/parser.so",
                    "sha256": sha256(self.elf_payload),
                    "kind": "elf",
                    "roles": [
                        "configuration-parser",
                        "host-x86",
                    ],
                    "execution_policy": "isolated-parser-only",
                },
                "readme": {
                    "path": "artifacts/README",
                    "sha256": sha256(self.text_payload),
                    "kind": "text",
                    "roles": [
                        "source-documentation",
                        "viv-gpu-file-example",
                    ],
                    "execution_policy": "not-executable",
                },
            },
        }
        self.write()

    def write(self) -> None:
        self.path.write_text(
            json.dumps(self.document, indent=2) + "\n", encoding="utf-8"
        )


class CorpusTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.fixture = CorpusFixture(self.root)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def load(self):
        return compiler_profile.load_corpus(
            self.fixture.path, repository_root=self.root
        )

    def test_loads_hash_pinned_inert_artifacts(self) -> None:
        corpus = self.load()
        self.assertEqual(tuple(corpus.artifacts), ("parser", "readme"))
        self.assertEqual(corpus.artifacts["parser"].kind, "elf")
        self.assertEqual(
            corpus.artifacts["parser"].execution_policy,
            "isolated-parser-only",
        )

    def test_rejects_duplicate_json_key(self) -> None:
        self.fixture.path.write_text(
            '{"schema_version":1,"schema_version":1,"artifacts":{}}\n',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(
            compiler_profile.CorpusConfigError, "duplicate JSON key"
        ):
            self.load()

    def test_rejects_unknown_root_and_artifact_keys(self) -> None:
        for location in ("root", "artifact"):
            with self.subTest(location=location):
                fixture = CorpusFixture(self.root)
                if location == "root":
                    fixture.document["unexpected"] = True
                else:
                    fixture.document["artifacts"]["parser"]["unexpected"] = True
                fixture.write()
                with self.assertRaisesRegex(
                    compiler_profile.CorpusConfigError, "unknown keys"
                ):
                    self.load()

    def test_rejects_path_escape_and_symlink_escape(self) -> None:
        outside = self.root.parent / f"{self.root.name}-outside.so"
        outside.write_bytes(self.fixture.elf_payload)
        self.addCleanup(outside.unlink, missing_ok=True)

        self.fixture.document["artifacts"]["parser"]["path"] = "../outside.so"
        self.fixture.write()
        with self.assertRaisesRegex(
            compiler_profile.CorpusConfigError, "repository-relative"
        ):
            self.load()

        self.fixture = CorpusFixture(self.root)
        link = self.root / "artifacts/link.so"
        link.symlink_to(outside)
        self.fixture.document["artifacts"]["parser"]["path"] = "artifacts/link.so"
        self.fixture.document["artifacts"]["parser"]["sha256"] = sha256(
            self.fixture.elf_payload
        )
        self.fixture.write()
        with self.assertRaisesRegex(
            compiler_profile.CorpusConfigError, "resolves outside"
        ):
            self.load()

    def test_rejects_hash_kind_and_utf8_mismatch(self) -> None:
        mutations = (
            (
                "hash",
                lambda: self.fixture.document["artifacts"]["parser"].update(
                    sha256="0" * 64
                ),
                "SHA-256 mismatch",
            ),
            (
                "elf",
                lambda: (self.root / "artifacts/parser.so").write_bytes(b"not an ELF"),
                "not ELF",
            ),
            (
                "text",
                lambda: (self.root / "artifacts/README").write_bytes(b"\xff"),
                "UTF-8",
            ),
        )
        for name, mutate, message in mutations:
            with self.subTest(name=name):
                self.fixture = CorpusFixture(self.root)
                mutate()
                if name in {"elf", "text"}:
                    artifact = "parser" if name == "elf" else "readme"
                    payload = (
                        self.root / self.fixture.document["artifacts"][artifact]["path"]
                    ).read_bytes()
                    self.fixture.document["artifacts"][artifact]["sha256"] = sha256(
                        payload
                    )
                self.fixture.write()
                with self.assertRaisesRegex(
                    compiler_profile.CorpusConfigError, message
                ):
                    self.load()

    def test_rejects_duplicate_roles_and_unsafe_quarantine_policy(self) -> None:
        self.fixture.document["artifacts"]["parser"]["roles"].append("host-x86")
        self.fixture.write()
        with self.assertRaisesRegex(compiler_profile.CorpusConfigError, "duplicate"):
            self.load()

        self.fixture = CorpusFixture(self.root)
        artifact = self.fixture.document["artifacts"]["parser"]
        artifact["roles"] = ["compiler", "quarantined"]
        artifact["execution_policy"] = "isolated-parser-only"
        self.fixture.write()
        with self.assertRaisesRegex(
            compiler_profile.CorpusConfigError, "quarantined-never-call"
        ):
            self.load()


class StrictConfigGrammarTests(unittest.TestCase):
    def setUp(self) -> None:
        self.identity = compiler_profile.HardwareIdentity(
            chip_model=0x00009000,
            chip_revision=0x00009002,
            product_id=0x05090001,
            eco_id=0x04000000,
            customer_id=0x1000000E,
        )

    def test_canonical_round_trip(self) -> None:
        rendered = compiler_profile.render_config(self.identity)
        self.assertEqual(
            rendered,
            (
                "chipModel              = 0x00009000;\n"
                "chipRevision           = 0x00009002;\n"
                "productID              = 0x05090001;\n"
                "ecoID                  = 0x04000000;\n"
                "customerID             = 0x1000000e;\n"
            ),
        )
        self.assertEqual(compiler_profile.parse_config_text(rendered), self.identity)

    def test_accepts_vendor_hex_lexical_forms_but_normalizes(self) -> None:
        text = (
            "chipModel=9000;\n"
            "chipRevision\t=\t0X9002;\n"
            "productID = 05090001;\n"
            "ecoID = 04000000;\n"
            "customerID = 1000000E;\n"
        )
        parsed = compiler_profile.parse_config_text(text)
        self.assertEqual(parsed, self.identity)
        self.assertEqual(
            compiler_profile.parse_config_text(compiler_profile.render_config(parsed)),
            self.identity,
        )

    def test_rejects_malformed_or_dangerously_ambiguous_input(self) -> None:
        valid_lines = compiler_profile.render_config(self.identity).splitlines()
        cases = {
            "empty": "",
            "truncated": "\n".join(valid_lines[:4]) + "\n",
            "wrong-order": "\n".join([valid_lines[1], valid_lines[0], *valid_lines[2:]])
            + "\n",
            "duplicate": "\n".join([valid_lines[0], valid_lines[0], *valid_lines[2:]])
            + "\n",
            "unknown": "\n".join([*valid_lines[:4], "unknownID = 0x1000000e;"]) + "\n",
            "comment": compiler_profile.render_config(self.identity)
            + "# ignored by nobody\n",
            "trailing": compiler_profile.render_config(self.identity) + "junk\n",
            "sign": compiler_profile.render_config(self.identity).replace(
                "0x00009000", "-0x00009000", 1
            ),
            "overflow": compiler_profile.render_config(self.identity).replace(
                "0x00009000", "0x100000000", 1
            ),
            "missing-semicolon": compiler_profile.render_config(self.identity).replace(
                "0x00009000;", "0x00009000", 1
            ),
            "leading-whitespace": " " + compiler_profile.render_config(self.identity),
            "blank-line": compiler_profile.render_config(self.identity).replace(
                "\n", "\n\n", 1
            ),
        }
        for name, text in cases.items():
            with (
                self.subTest(name=name),
                self.assertRaises(compiler_profile.ConfigGrammarError),
            ):
                compiler_profile.parse_config_text(text)

    def test_rejects_non_ascii_nul_and_size_limits(self) -> None:
        with self.assertRaisesRegex(compiler_profile.ConfigGrammarError, "ASCII"):
            compiler_profile.parse_config_bytes("é".encode())
        with self.assertRaisesRegex(compiler_profile.ConfigGrammarError, "NUL"):
            compiler_profile.parse_config_bytes(b"\0")
        with self.assertRaisesRegex(compiler_profile.ConfigGrammarError, "too large"):
            compiler_profile.parse_config_bytes(
                b"x" * (compiler_profile.MAX_CONFIG_BYTES + 1)
            )
        prefix = b"chipModel" + b" " * compiler_profile.MAX_CONFIG_LINE_BYTES
        with self.assertRaisesRegex(compiler_profile.ConfigGrammarError, "line"):
            compiler_profile.parse_config_bytes(prefix + b"\n")


class RealEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.corpus = compiler_profile.load_corpus(
            compiler_profile.DEFAULT_CORPUS,
            repository_root=REPOSITORY_ROOT,
        )
        cls.targets = compiler_profile.load_compiler_target_config(
            compiler_profile.DEFAULT_TARGET_CONFIG,
            repository_root=REPOSITORY_ROOT,
        )
        cls.evidence = compiler_profile.load_compiler_ghidra_evidence(
            compiler_profile.DEFAULT_GHIDRA_EVIDENCE,
            cls.targets,
        )

    def test_recovers_exact_simulator_catalog_and_unique_wave_row(self) -> None:
        catalog = compiler_profile.recover_vsim_catalog(
            self.corpus.artifacts["x86-emulator-6.4.6"]
        )
        self.assertEqual(catalog.record_count, 358)
        self.assertEqual(catalog.record_size, 100)
        self.assertEqual(catalog.file_offset, 0x27C760)
        self.assertEqual(
            catalog.table_sha256,
            "5f6bf1cf829c1b13878a8162950a0edc2e3b750aafb4dba0beeace5a6992b1ec",
        )
        self.assertEqual(catalog.wave_record.index, 157)
        self.assertEqual(
            catalog.wave_record.name,
            "VIP9000NANOS_PID0X1000000E",
        )
        self.assertEqual(
            catalog.wave_record.identity,
            compiler_profile.HardwareIdentity(
                0x9000,
                0x9001,
                0x05090001,
                0x04000000,
                0x1000000E,
            ),
        )
        self.assertEqual(catalog.default_record.index, 187)
        self.assertEqual(
            catalog.default_record.name,
            "VIP9000NANODI_PID0X10000020",
        )

    def test_evidence_proves_complete_selector_and_five_formats(self) -> None:
        facts = compiler_profile.recover_parser_facts(self.evidence)
        self.assertEqual(
            tuple(field.key for field in facts.fields),
            (
                "chipModel",
                "chipRevision",
                "productID",
                "ecoID",
                "customerID",
            ),
        )
        self.assertEqual(
            tuple(field.offset for field in facts.fields),
            (
                0x50,
                0x54,
                0x58,
                0x5C,
                0x60,
            ),
        )
        self.assertEqual(facts.record_size, 100)
        self.assertTrue(facts.vendor_accepts_eof_bug)

    def test_generates_deterministic_candidate_with_explicit_refusal_gate(
        self,
    ) -> None:
        catalog = compiler_profile.recover_vsim_catalog(
            self.corpus.artifacts["x86-emulator-6.4.6"]
        )
        grammar = compiler_profile.build_grammar_document(
            self.corpus,
            self.targets,
            self.evidence,
            catalog,
            probe_results=None,
        )
        profile = compiler_profile.load_wave_profile(
            compiler_profile.DEFAULT_WAVE_PROFILE
        )
        first = compiler_profile.build_candidate_bundle(
            profile,
            compiler_profile.DEFAULT_WAVE_PROFILE,
            grammar,
            catalog,
            self.corpus,
        )
        second = compiler_profile.build_candidate_bundle(
            profile,
            compiler_profile.DEFAULT_WAVE_PROFILE,
            grammar,
            catalog,
            self.corpus,
        )
        self.assertEqual(first, second)
        candidate, manifest = first
        self.assertIn("chipRevision           = 0x00009002;", candidate)
        self.assertEqual(
            manifest["decision"],
            "parser-shaped-candidate-not-compiler-qualified",
        )
        self.assertEqual(
            manifest["consumer_status"],
            "unverified-missing-no-kernel-sdk",
        )
        self.assertFalse(manifest["authorization"]["compiler_execution"])
        self.assertFalse(manifest["authorization"]["device_execution"])

    def test_refuses_candidate_when_emitted_profile_fact_is_not_high_confidence(
        self,
    ) -> None:
        catalog = compiler_profile.recover_vsim_catalog(
            self.corpus.artifacts["x86-emulator-6.4.6"]
        )
        grammar = compiler_profile.build_grammar_document(
            self.corpus,
            self.targets,
            self.evidence,
            catalog,
            probe_results=None,
        )
        profile = compiler_profile.load_wave_profile(
            compiler_profile.DEFAULT_WAVE_PROFILE
        )
        profile = copy.deepcopy(profile)
        profile["identity"]["chip_model"]["confidence"] = "medium"
        with self.assertRaisesRegex(compiler_profile.CandidateError, "chip_model"):
            compiler_profile.build_candidate_bundle(
                profile,
                compiler_profile.DEFAULT_WAVE_PROFILE,
                grammar,
                catalog,
                self.corpus,
            )


if __name__ == "__main__":
    unittest.main()
