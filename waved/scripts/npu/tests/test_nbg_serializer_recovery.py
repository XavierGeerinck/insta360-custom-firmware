#!/usr/bin/env python3
"""Contract tests for the host-only NBG serializer evidence orchestrator."""

from __future__ import annotations

import copy
import hashlib
import json
import struct
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import nbg_serializer_recovery as recovery  # noqa: E402


SERIALIZER_FUNCTIONS = {
    "vsi_nn_GenerateNBG",
    "vxGenerateNBG",
    "vxoBinaryGraph_AllocateBinaryLoadBuffer",
    "vxoBinaryGraph_GenerateSectionMemory",
    "vxoBinaryGraph_LoadNBG",
    "vxoBinaryGraph_NetworkBinaryGraph",
    "vxoBinaryGraph_Patch32bitVa",
    "vxoBinaryGraph_Patch40bitVa",
    "vxoBinaryGraph_ReleaseNBG",
    "vxoBinaryGraph_RelocateBinaryEntrance",
    "vxoBinaryGraph_RelocateNetworkBinaryGraph",
    "vxoBinaryGraph_SaveBinaryEntrance",
    "vxoBinaryGraph_SaveBinaryEntranceBefore",
    "vxoBinaryGraph_SaveNNTPCommand",
    "vxoBinaryGraph_SaveNNTPInstruction",
    "vxoBinaryGraph_SaveNNTPOperation",
    "vxoBinaryGraph_SaveNNTPStates",
    "vxoBinaryGraph_SaveNetworkBinaryGraph",
    "vxoBinaryGraph_SaveWeightAndBias",
    "vxoBinaryGraph_SetPatchDataInfo",
    "vxoBinaryGraph_UpdateCoefHeaderCache",
}


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def minimal_aarch64_elf() -> bytes:
    ident = bytearray(16)
    ident[:4] = b"\x7fELF"
    ident[4] = 2
    ident[5] = 1
    ident[6] = 1
    file_size = 64 + 56
    header = struct.pack(
        "<16sHHIQQQIHHHHHH",
        bytes(ident),
        3,
        183,
        1,
        0,
        64,
        0,
        0,
        64,
        56,
        1,
        64,
        0,
        0,
    )
    program_header = struct.pack(
        "<IIQQQQQQ",
        1,
        5,
        0,
        0x10000,
        0x10000,
        file_size,
        file_size,
        0x1000,
    )
    return header + program_header


def minimal_v2_nbg(tag: int) -> bytes:
    payload = bytearray(513)
    payload[:4] = b"VPMN"
    struct.pack_into("<I", payload, 4, 0x00020000)
    struct.pack_into("<I", payload, 8, 0x1000000E)
    payload[12:] = bytes([tag]) * (len(payload) - 12)
    return bytes(payload)


class ManifestFixture:
    def __init__(self, root: Path):
        self.root = root
        (root / "firmware").mkdir()
        (root / "models").mkdir()
        (root / "artifacts").mkdir()
        (root / "docs/npu/format").mkdir(parents=True)

        self.payloads = {
            "wave-openvx": minimal_aarch64_elf(),
            "wave-ovxlib": minimal_aarch64_elf(),
            "generated-erf": minimal_v2_nbg(1),
            "ctc-en": minimal_v2_nbg(2),
            "ctc-de": minimal_v2_nbg(3),
            "ctc-fr": minimal_v2_nbg(4),
            "ctc-ja": minimal_v2_nbg(5),
            "ctc-en-onnx": b"bounded-onnx-placeholder",
        }
        paths = {
            "wave-openvx": "firmware/libOpenVX.so",
            "wave-ovxlib": "firmware/libovxlib.so",
            "generated-erf": "artifacts/generated-erf.nbg",
            "ctc-en": "models/ctc-en.nb",
            "ctc-de": "models/ctc-de.nb",
            "ctc-fr": "models/ctc-fr.nb",
            "ctc-ja": "models/ctc-ja.nb",
            "ctc-en-onnx": "models/ctc-en.onnx",
        }
        roles = {
            "wave-openvx": ["exact-wave", "serializer-provider"],
            "wave-ovxlib": ["exact-wave", "serializer-caller"],
            "generated-erf": [
                "exact-wave",
                "generated-control",
                "roundtrip-control",
            ],
            "ctc-en": [
                "exact-wave",
                "roundtrip-control",
                "source-anchor-target",
            ],
            "ctc-de": [
                "differential-control",
                "exact-wave",
                "roundtrip-control",
            ],
            "ctc-fr": [
                "differential-control",
                "exact-wave",
                "roundtrip-control",
            ],
            "ctc-ja": [
                "differential-control",
                "exact-wave",
                "roundtrip-control",
            ],
            "ctc-en-onnx": ["source-anchor"],
        }
        kinds = {
            "wave-openvx": "elf",
            "wave-ovxlib": "elf",
            "generated-erf": "nbg",
            "ctc-en": "nbg",
            "ctc-de": "nbg",
            "ctc-fr": "nbg",
            "ctc-ja": "nbg",
            "ctc-en-onnx": "onnx",
        }
        artifacts = {}
        for artifact_id, payload in self.payloads.items():
            relative = paths[artifact_id]
            (root / relative).write_bytes(payload)
            artifacts[artifact_id] = {
                "bytes": len(payload),
                "kind": kinds[artifact_id],
                "path": relative,
                "roles": roles[artifact_id],
                "sha256": sha256(payload),
            }

        provider_functions = sorted(
            SERIALIZER_FUNCTIONS - {"vsi_nn_GenerateNBG"}
        )
        self.document = {
            "artifacts": artifacts,
            "correlation": {
                "anchor": {
                    "id": "ctc-en-anchor",
                    "nbg_artifact_id": "ctc-en",
                    "onnx_artifact_id": "ctc-en-onnx",
                    "relationship": "source-binary-anchor",
                },
                "control_nbg_ids": ["ctc-de", "ctc-fr", "ctc-ja"],
                "limits": {
                    "max_candidate_bytes": 67108864,
                    "max_candidates_per_tensor": 128,
                    "max_initializer_bytes": 67108864,
                    "max_search_bytes_per_tensor": 268435456,
                    "max_search_sections": 16,
                },
                "search_section_types": [
                    4,
                    5,
                    6,
                    7,
                    8,
                    9,
                    10,
                    21,
                    23,
                    24,
                    27,
                ],
                "transforms": [
                    "identity-fp32-le",
                    "cast-fp16-rne-le",
                    "cast-bf16-rne-le",
                    "transpose-2d",
                    "transpose-2d+cast-fp16-rne-le",
                    "transpose-2d+cast-bf16-rne-le",
                    "fixed-block-permute",
                    "byte-lane-permute",
                    "halfword-lane-permute",
                ],
            },
            "ghidra_batches": {
                "wave-openvx-all": {
                    "artifact_id": "wave-openvx",
                    "functions": provider_functions,
                    "roles": ["serializer-provider"],
                    "string_filters": [
                        "binary graph",
                        "coefficient",
                        "nbg",
                        "patch",
                        "section",
                    ],
                },
                "wave-ovxlib-entry": {
                    "artifact_id": "wave-ovxlib",
                    "functions": ["vsi_nn_GenerateNBG"],
                    "roles": ["serializer-caller"],
                    "string_filters": ["generate", "nbg"],
                },
            },
            "outputs": {
                "correlation": "docs/npu/format/onnx-nbg-correlation.json",
                "ghidra": "docs/npu/format/serializer-ghidra-evidence.json",
                "ownership": (
                    "docs/npu/format/serializer-byte-ownership.json"
                ),
                "roundtrip": (
                    "docs/npu/format/serializer-roundtrip-evidence.json"
                ),
            },
            "poc_id": "poc12a-open-nbg-serializer",
            "policy": {
                "allowed_operations": [
                    "compute-bounded-transforms",
                    "hash-files",
                    "parse-elf",
                    "parse-nbg",
                    "parse-onnx",
                    "run-ghidra-headless",
                    "write-identical-nbg-copy",
                    "write-json-evidence",
                    "write-markdown-evidence",
                ],
                "prohibited_actions": {
                    "adb_calls": 0,
                    "compiler_calls": 0,
                    "container_registrations": 0,
                    "container_runs": 0,
                    "device_calls": 0,
                    "generated_custom_nbg_files": 0,
                    "mutated_nbg_files": 0,
                    "native_extension_imports": 0,
                    "simulator_calls": 0,
                },
            },
            "roundtrip_nbg_ids": [
                "generated-erf",
                "ctc-en",
                "ctc-de",
                "ctc-fr",
                "ctc-ja",
            ],
            "schema_version": 1,
        }
        self.path = root / "manifest.json"
        self.write()

    def write(self) -> None:
        self.path.write_text(
            json.dumps(self.document, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )


class ManifestTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], ManifestFixture]:
        temporary = tempfile.TemporaryDirectory()
        return temporary, ManifestFixture(Path(temporary.name))

    def test_valid_manifest_is_immutable_and_deterministic(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            first = recovery.load_manifest(
                fixture.path, repository_root=fixture.root
            )
            second = recovery.load_manifest(
                fixture.path, repository_root=fixture.root
            )
            recovery.validate_artifacts(first)
            self.assertEqual(first, second)
            self.assertEqual(
                recovery.canonical_json(first.to_document()),
                recovery.canonical_json(second.to_document()),
            )
            self.assertEqual(len(first.artifacts), 8)
            self.assertEqual(
                {
                    function
                    for batch in first.ghidra_batches.values()
                    for function in batch.functions
                },
                SERIALIZER_FUNCTIONS,
            )

    def test_rejects_duplicate_json_key(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.path.write_text(
                '{"schema_version":1,"schema_version":1}',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                recovery.SerializerError, "duplicate JSON key"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_rejects_unknown_and_missing_root_keys(self) -> None:
        for mutation, expected in (
            (lambda value: value.update({"unknown": True}), "unknown keys"),
            (lambda value: value.pop("outputs"), "missing keys"),
        ):
            with self.subTest(expected=expected):
                temporary, fixture = self.fixture()
                with temporary:
                    mutation(fixture.document)
                    fixture.write()
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.load_manifest(
                            fixture.path, repository_root=fixture.root
                        )

    def test_rejects_bad_schema_and_poc(self) -> None:
        for key, value, expected in (
            ("schema_version", 2, "schema_version"),
            ("poc_id", "another-poc", "poc_id"),
        ):
            with self.subTest(key=key):
                temporary, fixture = self.fixture()
                with temporary:
                    fixture.document[key] = value
                    fixture.write()
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.load_manifest(
                            fixture.path, repository_root=fixture.root
                        )

    def test_rejects_invalid_artifact_fields(self) -> None:
        mutations = (
            ("artifact ID", lambda value: value["artifacts"].update(
                {"BAD ID": value["artifacts"].pop("ctc-ja")}
            )),
            ("kind", lambda value: value["artifacts"]["ctc-en"].update(
                {"kind": "blob"}
            )),
            ("roles", lambda value: value["artifacts"]["ctc-en"].update(
                {"roles": ["exact-wave", "exact-wave"]}
            )),
            ("bytes", lambda value: value["artifacts"]["ctc-en"].update(
                {"bytes": True}
            )),
            ("SHA-256", lambda value: value["artifacts"]["ctc-en"].update(
                {"sha256": "ABC"}
            )),
        )
        for expected, mutation in mutations:
            with self.subTest(expected=expected):
                temporary, fixture = self.fixture()
                with temporary:
                    mutation(fixture.document)
                    fixture.write()
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.load_manifest(
                            fixture.path, repository_root=fixture.root
                        )

    def test_rejects_path_escape_and_symlink(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["artifacts"]["ctc-en"]["path"] = "../escape.nb"
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "escape"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

        temporary, fixture = self.fixture()
        with temporary:
            target = fixture.root / "models/ctc-en.nb"
            link = fixture.root / "models/ctc-en-link.nb"
            link.symlink_to(target)
            fixture.document["artifacts"]["ctc-en"]["path"] = (
                "models/ctc-en-link.nb"
            )
            fixture.document["artifacts"]["ctc-en"]["bytes"] = target.stat().st_size
            fixture.document["artifacts"]["ctc-en"]["sha256"] = sha256(
                target.read_bytes()
            )
            fixture.write()
            manifest = recovery.load_manifest(
                fixture.path, repository_root=fixture.root
            )
            with self.assertRaisesRegex(
                recovery.SerializerError, "symlink"
            ):
                recovery.validate_artifacts(manifest)

    def test_rejects_hash_and_size_drift(self) -> None:
        for key, value, expected in (
            ("bytes", 999, "byte count"),
            ("sha256", "0" * 64, "SHA-256"),
        ):
            with self.subTest(key=key):
                temporary, fixture = self.fixture()
                with temporary:
                    fixture.document["artifacts"]["ctc-en"][key] = value
                    fixture.write()
                    manifest = recovery.load_manifest(
                        fixture.path, repository_root=fixture.root
                    )
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.validate_artifacts(manifest)

    def test_rejects_wrong_elf_identity_and_load_geometry(self) -> None:
        for payload, expected in (
            (b"not-elf", "ELF"),
            (
                minimal_aarch64_elf()[:64],
                "program-header",
            ),
        ):
            with self.subTest(expected=expected):
                temporary, fixture = self.fixture()
                with temporary:
                    path = fixture.root / fixture.document["artifacts"][
                        "wave-openvx"
                    ]["path"]
                    path.write_bytes(payload)
                    fixture.document["artifacts"]["wave-openvx"].update(
                        {"bytes": len(payload), "sha256": sha256(payload)}
                    )
                    fixture.write()
                    manifest = recovery.load_manifest(
                        fixture.path, repository_root=fixture.root
                    )
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.validate_artifacts(manifest)

    def test_rejects_invalid_nbg_envelope(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            payload = bytearray(fixture.payloads["ctc-en"])
            payload[:4] = b"NOPE"
            path = fixture.root / fixture.document["artifacts"]["ctc-en"][
                "path"
            ]
            path.write_bytes(payload)
            fixture.document["artifacts"]["ctc-en"].update(
                {"bytes": len(payload), "sha256": sha256(payload)}
            )
            fixture.write()
            manifest = recovery.load_manifest(
                fixture.path, repository_root=fixture.root
            )
            with self.assertRaisesRegex(
                recovery.SerializerError, "VPMN"
            ):
                recovery.validate_artifacts(manifest)

    def test_rejects_nonzero_prohibited_action(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["policy"]["prohibited_actions"][
                "device_calls"
            ] = 1
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "device_calls"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_rejects_unknown_or_duplicate_function(self) -> None:
        for function, expected, batch_id in (
            (
                "vxoBinaryGraph_GuessedFunction",
                "unapproved function",
                "wave-ovxlib-entry",
            ),
            (
                "vxoBinaryGraph_LoadNBG",
                "duplicate function",
                "wave-openvx-all",
            ),
        ):
            with self.subTest(function=function):
                temporary, fixture = self.fixture()
                with temporary:
                    fixture.document["ghidra_batches"][batch_id][
                        "functions"
                    ].append(function)
                    fixture.write()
                    with self.assertRaisesRegex(
                        recovery.SerializerError, expected
                    ):
                        recovery.load_manifest(
                            fixture.path, repository_root=fixture.root
                        )

    def test_rejects_function_in_wrong_elf_role(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["ghidra_batches"]["wave-openvx-all"][
                "functions"
            ].append("vsi_nn_GenerateNBG")
            fixture.document["ghidra_batches"]["wave-ovxlib-entry"][
                "functions"
            ] = ["vxGenerateNBG"]
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "wrong ELF role"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_rejects_unknown_correlation_endpoint(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["correlation"]["anchor"][
                "onnx_artifact_id"
            ] = "missing"
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "unknown artifact"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_rejects_output_escape(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["outputs"]["ghidra"] = "../evidence.json"
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "output"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_rejects_mutation_operation(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.document["policy"]["allowed_operations"].append(
                "mutate-nbg"
            )
            fixture.write()
            with self.assertRaisesRegex(
                recovery.SerializerError, "allowed_operations"
            ):
                recovery.load_manifest(
                    fixture.path, repository_root=fixture.root
                )

    def test_manifest_dataclasses_are_frozen(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            manifest = recovery.load_manifest(
                fixture.path, repository_root=fixture.root
            )
            with self.assertRaises((AttributeError, TypeError)):
                manifest.poc_id = "changed"  # type: ignore[misc]
            with self.assertRaises(TypeError):
                manifest.artifacts["new"] = copy.copy(  # type: ignore[index]
                manifest.artifacts["ctc-en"]
                )


class FakeSerializerExecutor:
    def __init__(
        self,
        exports: dict[str, dict],
        *,
        mode: str = "valid",
    ):
        self.exports = exports
        self.mode = mode
        self.calls: list[tuple[str, str]] = []

    def analyze(self, target, binary_path):
        self.calls.append((target.evidence_id, binary_path.name))
        records = []
        for name in reversed(target.functions):
            export = self.exports[name]
            function = {
                "body_bytes": export["size"],
                "callees": [],
                "callers": [],
                "decompile_error": None,
                "decompile_success": True,
                "decompile_truncated": False,
                "decompiled_c": f"void {name}(void) {{}}\n",
                "diagnostics": [],
                "entry": f"{export['vaddr']:08x}",
                "external": False,
                "name": name,
                "qualified_name": name,
                "role": "target",
                "thunk": name
                == "vxoBinaryGraph_SaveNetworkBinaryGraph",
            }
            if self.mode == "truncated" and name == "vxGenerateNBG":
                function["decompile_truncated"] = True
            if self.mode == "address-drift" and name == "vxGenerateNBG":
                function["entry"] = f"{export['vaddr'] + 4:08x}"
            if self.mode == "unexpected-thunk" and name == "vxGenerateNBG":
                function["thunk"] = True
            if self.mode == "missing" and name == "vxGenerateNBG":
                functions = []
                found = False
            else:
                functions = [function]
                found = True
            records.append(
                {
                    "found": found,
                    "functions": functions,
                    "symbol_addresses": [function["entry"]] if found else [],
                    "target": name,
                }
            )
        return {
            "program": {
                "format": "Executable and Linking Format (ELF)",
                "language_id": "AARCH64:LE:64:v8A",
                "name": binary_path.name,
                "sha256": target.sha256,
            },
            "schema_version": 1,
            "targets": records,
            "tool": {"name": "Ghidra", "version": "test"},
            "warnings": [],
        }


class SerializerStaticEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = recovery.load_manifest()
        recovery.validate_artifacts(cls.manifest)
        cls.exports = recovery.load_serializer_exports(cls.manifest)

    def test_exact_real_exports_have_hashes_and_geometry(self) -> None:
        self.assertEqual(set(self.exports), SERIALIZER_FUNCTIONS)
        for name, item in self.exports.items():
            with self.subTest(name=name):
                self.assertGreater(item["size"], 0)
                self.assertGreaterEqual(item["vaddr"], 0)
                self.assertGreaterEqual(item["paddr"], 0)
                self.assertRegex(item["function_sha256"], r"^[0-9a-f]{64}$")
        alias = self.exports[
            "vxoBinaryGraph_SaveNetworkBinaryGraph"
        ]
        self.assertEqual(alias["size"], 4)
        self.assertTrue(alias["probable_thunk"])
        self.assertEqual(alias["branch_kind"], "aarch64-b")

    def test_fake_static_evidence_is_deterministic(self) -> None:
        first = recovery.build_serializer_ghidra_evidence(
            self.manifest,
            executor=FakeSerializerExecutor(self.exports),
            export_records=self.exports,
        )
        second = recovery.build_serializer_ghidra_evidence(
            self.manifest,
            executor=FakeSerializerExecutor(self.exports),
            export_records=self.exports,
        )
        encoded = recovery.canonical_json(first)
        self.assertEqual(encoded, recovery.canonical_json(second))
        self.assertNotIn(str(self.manifest.repository_root).encode(), encoded)
        self.assertEqual(len(first["batches"]), 5)
        functions = [
            item
            for batch in first["batches"]
            for item in batch["functions"]
        ]
        self.assertEqual(len(functions), 21)
        alias = next(
            item
            for item in functions
            if item["name"]
            == "vxoBinaryGraph_SaveNetworkBinaryGraph"
        )
        self.assertEqual(alias["function_kind"], "export-thunk-alias")
        self.assertTrue(all(value == 0 for value in first["safety"].values()))

    def test_missing_truncated_address_drift_and_unexpected_thunk_fail(self):
        for mode, expected in (
            ("missing", "GHIDRA_TARGET_MISSING"),
            ("truncated", "GHIDRA_FUNCTION_DRIFT"),
            ("address-drift", "GHIDRA_FUNCTION_DRIFT"),
            ("unexpected-thunk", "GHIDRA_FUNCTION_DRIFT"),
        ):
            with self.subTest(mode=mode):
                with self.assertRaisesRegex(
                    recovery.SerializerError, expected
                ):
                    recovery.build_serializer_ghidra_evidence(
                        self.manifest,
                        executor=FakeSerializerExecutor(
                            self.exports, mode=mode
                        ),
                        export_records=self.exports,
                    )

    def test_duplicate_target_function_is_refused(self) -> None:
        class DuplicateExecutor(FakeSerializerExecutor):
            def analyze(self, target, binary_path):
                value = super().analyze(target, binary_path)
                value["targets"][0]["functions"].append(
                    copy.deepcopy(value["targets"][0]["functions"][0])
                )
                return value

        with self.assertRaisesRegex(
            recovery.SerializerError, "GHIDRA_FUNCTION_DRIFT"
        ):
            recovery.build_serializer_ghidra_evidence(
                self.manifest,
                executor=DuplicateExecutor(self.exports),
                export_records=self.exports,
            )


if __name__ == "__main__":
    unittest.main()
