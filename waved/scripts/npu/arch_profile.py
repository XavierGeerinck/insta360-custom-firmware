#!/usr/bin/env python3
"""Recover and validate host-side architecture-profile evidence.

This module is deliberately host-only.  It inventories architecture-model
libraries, drives the existing bounded Ghidra exporter, and validates the
machine-readable Wave architecture profile.  It never loads a target library
or communicates with a Wave device.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    from . import ghidra_abi
except ImportError:  # pragma: no cover - direct script execution
    import ghidra_abi  # type: ignore


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_TARGET_CONFIG = Path(__file__).with_name("arch_profile_targets.json")
DEFAULT_GHIDRA_EVIDENCE = (
    REPOSITORY_ROOT / "docs/npu/profile/arch-profile-ghidra-evidence.json"
)
DEFAULT_PROFILE = REPOSITORY_ROOT / "docs/npu/profile/wave-profile-evidence.json"
DEFAULT_SCHEMA = REPOSITORY_ROOT / "docs/npu/profile/wave-profile.schema.json"

LIVE_CAPABILITIES_NOTEBOOK = (
    REPOSITORY_ROOT / "docs/npu/lab-notebook/2026-07-28-poc5-custom-compiler.md"
)
LIVE_IDENTITY_NOTEBOOK = (
    REPOSITORY_ROOT / "docs/npu/lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md"
)
LIVE_BASELINE_MANIFEST = REPOSITORY_ROOT / "docs/npu/manifests/wave-live-v1.1.6.6.json"

EXPECTED_TARGET_IDS = frozenset(
    {
        "wave-archmodel-aarch64",
        "wave-nnarchperf-aarch64",
        "x86-sim-archmodel",
        "x86-sim-nnarchperf",
    }
)

EXPECTED_ROLE_BY_TARGET = {
    "wave-archmodel-aarch64": (
        "architecture-model",
        "exact-wave",
        "live-matched",
    ),
    "wave-nnarchperf-aarch64": (
        "architecture-performance",
        "exact-wave",
        "live-matched",
    ),
    "x86-sim-archmodel": (
        "architecture-model",
        "differential-reference",
        "x86-simulator",
    ),
    "x86-sim-nnarchperf": (
        "architecture-performance",
        "differential-reference",
        "x86-simulator",
    ),
}

EXPECTED_STATIC_ABSENCES = {
    "x86-sim-nnarchperf": frozenset(
        {"amInitialize", "amSetFeaturesSetting", "amSetState"}
    )
}

PROFILE_ROOT_KEYS = frozenset(
    {
        "schema_version",
        "profile_id",
        "target",
        "inputs",
        "identity",
        "features",
        "resources",
        "versions",
        "unknowns",
        "non_claims",
    }
)
TARGET_KEYS = frozenset({"value", "hex"})
INPUT_KEYS = frozenset({"id", "path", "sha256", "bytes", "roles"})
FACT_KEYS = frozenset(
    {
        "value",
        "representation",
        "evidence_class",
        "confidence",
        "sources",
        "validation_state",
        "notes",
    }
)
SOURCE_KEYS = frozenset({"artifact_id", "location"})

EVIDENCE_CLASSES = frozenset(
    {
        "observed-live",
        "static-verified",
        "differentially-verified",
        "inferred",
        "unknown",
    }
)
CONFIDENCE_LEVELS = frozenset({"high", "medium", "low", "none"})
VALIDATION_STATES = frozenset(
    {"corroborated", "single-source", "candidate", "unresolved"}
)

_IDENTIFIER_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_HEX32_RE = re.compile(r"^0x[0-9a-f]{8}$")


@dataclass(frozen=True)
class ArchitectureTableSpec:
    """Static table geometry recovered from a named CreateAPModel body."""

    evidence_id: str
    virtual_address: int
    record_size: int
    record_count: int
    expected_address_token: str
    expected_stride_token: str
    expected_count_token: str


@dataclass(frozen=True)
class ArchitectureTableRecovery:
    """Auditable recovery of the unique Wave identity row."""

    evidence_id: str
    virtual_address: int
    file_offset: int
    record_size: int
    record_count: int
    matched_row_index: int
    identity_words: tuple[int, int, int, int, int]
    generic_revision_marker: int
    row_sha256: str
    row_words: tuple[int, ...]


ARCHITECTURE_TABLE_SPECS = (
    ArchitectureTableSpec(
        evidence_id="wave-nnarchperf-aarch64",
        # Ghidra rebases this ET_DYN image at 0x00100000 and reports
        # DAT_00161450.  The corresponding ELF virtual address is 0x61450.
        virtual_address=0x00061450,
        record_size=0x240,
        record_count=0x1C9,
        expected_address_token="DAT_00161450",
        expected_stride_token="0x90",
        expected_count_token="0x1c9",
    ),
    ArchitectureTableSpec(
        evidence_id="x86-sim-nnarchperf",
        virtual_address=0x00241120,
        record_size=0x228,
        record_count=0x183,
        expected_address_token="gChipInfo",
        expected_stride_token="0x8a",
        expected_count_token="0x183",
    ),
)

WAVE_CUSTOMER_ID = 0x1000000E
EXPECTED_DATABASE_IDENTITY = (
    0x00009000,
    0x00009001,
    0x05090001,
    0x04000000,
    WAVE_CUSTOMER_ID,
)


class ArchProfileError(RuntimeError):
    """Base error for architecture-profile tooling."""


class ArchProfileConfigError(ArchProfileError):
    """The static-analysis target configuration is invalid."""


class ArchProfileEvidenceError(ArchProfileError):
    """Static evidence could not be generated or verified."""


class ProfileValidationError(ArchProfileError):
    """The recovered Wave profile violates its strict contract."""


def canonical_json(value: Any) -> bytes:
    """Return deterministic JSON using the shared evidence serializer."""

    return ghidra_abi.canonical_json(value)


def _reject_duplicate_keys(pairs: Sequence[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_strict_json(path: Path, error_type: type[ArchProfileError]) -> Any:
    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(handle, object_pairs_hook=_reject_duplicate_keys)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise error_type(f"cannot load {path}: {exc}") from exc


def _require_mapping(value: Any, location: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ProfileValidationError(f"{location} must be an object")
    return value


def _require_exact_keys(
    value: Mapping[str, Any], expected: frozenset[str], location: str
) -> None:
    actual = frozenset(value)
    if actual == expected:
        return
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    details: list[str] = []
    if missing:
        details.append(f"missing keys {missing}")
    if extra:
        details.append(f"unknown keys {extra}")
    raise ProfileValidationError(f"{location}: {', '.join(details)}")


def _require_string(value: Any, location: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value):
        qualifier = "a string" if allow_empty else "a non-empty string"
        raise ProfileValidationError(f"{location} must be {qualifier}")
    return value


def _require_identifier(value: Any, location: str) -> str:
    identifier = _require_string(value, location)
    if not _IDENTIFIER_RE.fullmatch(identifier):
        raise ProfileValidationError(
            f"{location} must match {_IDENTIFIER_RE.pattern!r}"
        )
    return identifier


def _require_string_list(
    value: Any, location: str, *, allow_empty: bool = False
) -> list[str]:
    if not isinstance(value, list):
        raise ProfileValidationError(f"{location} must be an array")
    if not allow_empty and not value:
        raise ProfileValidationError(f"{location} must not be empty")
    result: list[str] = []
    for index, item in enumerate(value):
        result.append(_require_string(item, f"{location}[{index}]"))
    if len(result) != len(set(result)):
        raise ProfileValidationError(f"{location} contains duplicates")
    return result


def _validate_target(target: Any) -> None:
    target_object = _require_mapping(target, "target")
    _require_exact_keys(target_object, TARGET_KEYS, "target")
    value = target_object["value"]
    if not isinstance(value, int) or isinstance(value, bool):
        raise ProfileValidationError("target.value must be an integer")
    if value != 0x1000000E:
        raise ProfileValidationError(
            "target.value must be the observed Wave target 0x1000000e"
        )
    target_hex = _require_string(target_object["hex"], "target.hex")
    if not _HEX32_RE.fullmatch(target_hex):
        raise ProfileValidationError("target.hex must be lowercase 32-bit hex")
    if target_hex != f"0x{value:08x}":
        raise ProfileValidationError("target.hex does not match target.value")


def _validate_inputs(inputs: Any) -> set[str]:
    if not isinstance(inputs, list) or not inputs:
        raise ProfileValidationError("inputs must be a non-empty array")

    artifact_ids: set[str] = set()
    for index, raw_input in enumerate(inputs):
        location = f"inputs[{index}]"
        input_object = _require_mapping(raw_input, location)
        _require_exact_keys(input_object, INPUT_KEYS, location)

        artifact_id = _require_identifier(input_object["id"], f"{location}.id")
        if artifact_id in artifact_ids:
            raise ProfileValidationError(f"duplicate input id: {artifact_id}")
        artifact_ids.add(artifact_id)

        relative_path = _require_string(input_object["path"], f"{location}.path")
        candidate_path = Path(relative_path)
        if candidate_path.is_absolute() or ".." in candidate_path.parts:
            raise ProfileValidationError(
                f"{location}.path must be repository-relative and contained"
            )

        digest = _require_string(input_object["sha256"], f"{location}.sha256")
        if not _SHA256_RE.fullmatch(digest):
            raise ProfileValidationError(f"{location}.sha256 must be lowercase SHA-256")

        byte_count = input_object["bytes"]
        if (
            not isinstance(byte_count, int)
            or isinstance(byte_count, bool)
            or byte_count <= 0
        ):
            raise ProfileValidationError(f"{location}.bytes must be positive")

        _require_string_list(input_object["roles"], f"{location}.roles")

    return artifact_ids


def _validate_source(
    source: Any, location: str, artifact_ids: set[str]
) -> tuple[str, str]:
    source_object = _require_mapping(source, location)
    _require_exact_keys(source_object, SOURCE_KEYS, location)
    artifact_id = _require_identifier(
        source_object["artifact_id"], f"{location}.artifact_id"
    )
    if artifact_id not in artifact_ids:
        raise ProfileValidationError(
            f"{location}.artifact_id references unknown artifact {artifact_id!r}"
        )
    evidence_location = _require_string(
        source_object["location"], f"{location}.location"
    )
    return artifact_id, evidence_location


def _validate_fact(
    raw_fact: Any,
    fact_path: str,
    artifact_ids: set[str],
) -> bool:
    fact = _require_mapping(raw_fact, fact_path)
    _require_exact_keys(fact, FACT_KEYS, fact_path)

    _require_string(fact["representation"], f"{fact_path}.representation")
    evidence_class = _require_string(
        fact["evidence_class"], f"{fact_path}.evidence_class"
    )
    if evidence_class not in EVIDENCE_CLASSES:
        raise ProfileValidationError(
            f"{fact_path}.evidence_class must be one of {sorted(EVIDENCE_CLASSES)}"
        )

    confidence = _require_string(fact["confidence"], f"{fact_path}.confidence")
    if confidence not in CONFIDENCE_LEVELS:
        raise ProfileValidationError(
            f"{fact_path}.confidence must be one of {sorted(CONFIDENCE_LEVELS)}"
        )

    validation_state = _require_string(
        fact["validation_state"], f"{fact_path}.validation_state"
    )
    if validation_state not in VALIDATION_STATES:
        raise ProfileValidationError(
            f"{fact_path}.validation_state must be one of {sorted(VALIDATION_STATES)}"
        )

    _require_string(fact["notes"], f"{fact_path}.notes", allow_empty=True)

    sources = fact["sources"]
    if not isinstance(sources, list):
        raise ProfileValidationError(f"{fact_path}.sources must be an array")
    source_keys: set[tuple[str, str]] = set()
    for index, source in enumerate(sources):
        source_key = _validate_source(
            source, f"{fact_path}.sources[{index}]", artifact_ids
        )
        if source_key in source_keys:
            raise ProfileValidationError(
                f"{fact_path}.sources contains duplicate source {source_key}"
            )
        source_keys.add(source_key)

    is_unknown = evidence_class == "unknown"
    if is_unknown:
        if fact["value"] is not None:
            raise ProfileValidationError(
                f"{fact_path}: unknown evidence requires a null value"
            )
        if confidence != "none":
            raise ProfileValidationError(f"{fact_path}.confidence must be none")
        if validation_state != "unresolved":
            raise ProfileValidationError(
                f"{fact_path}.validation_state must be unresolved"
            )
        if sources:
            raise ProfileValidationError(f"{fact_path}.sources must be empty")
    else:
        if fact["value"] is None:
            raise ProfileValidationError(
                f"{fact_path}.value may be null only for unknown facts"
            )
        if confidence == "none":
            raise ProfileValidationError(
                f"{fact_path}.confidence may be none only for unknown facts"
            )
        if validation_state == "unresolved":
            raise ProfileValidationError(
                f"{fact_path}.validation_state may be unresolved only for unknown facts"
            )
        if not sources:
            raise ProfileValidationError(
                f"{fact_path}.sources must contain at least one source"
            )

    return is_unknown


def validate_profile(document: Any) -> Mapping[str, Any]:
    """Validate a Wave profile and raise ``ProfileValidationError`` on failure."""

    root = _require_mapping(document, "profile")
    _require_exact_keys(root, PROFILE_ROOT_KEYS, "profile")

    if root["schema_version"] != 1:
        raise ProfileValidationError("schema_version must be 1")
    _require_identifier(root["profile_id"], "profile_id")
    _validate_target(root["target"])
    artifact_ids = _validate_inputs(root["inputs"])

    unknown_fact_paths: set[str] = set()
    for section_name in ("identity", "features", "resources", "versions"):
        section = _require_mapping(root[section_name], section_name)
        if not section:
            raise ProfileValidationError(f"{section_name} must not be empty")
        for fact_name, raw_fact in section.items():
            _require_identifier(fact_name, f"{section_name} fact name")
            fact_path = f"{section_name}.{fact_name}"
            if _validate_fact(raw_fact, fact_path, artifact_ids):
                unknown_fact_paths.add(fact_path)

    declared_unknowns = _require_string_list(
        root["unknowns"], "unknowns", allow_empty=True
    )
    if declared_unknowns != sorted(declared_unknowns):
        raise ProfileValidationError("unknowns must be sorted")
    if set(declared_unknowns) != unknown_fact_paths:
        missing = sorted(unknown_fact_paths - set(declared_unknowns))
        extra = sorted(set(declared_unknowns) - unknown_fact_paths)
        details: list[str] = []
        if missing:
            details.append(f"missing unknown facts {missing}")
        if extra:
            details.append(f"not unknown facts {extra}")
        raise ProfileValidationError(
            "unknowns must exactly match unknown facts: " + ", ".join(details)
        )

    non_claims = _require_string_list(
        root["non_claims"], "non_claims", allow_empty=True
    )
    if non_claims != sorted(non_claims):
        raise ProfileValidationError("non_claims must be sorted")
    return root


def load_profile(path: Path | str) -> dict[str, Any]:
    """Load a profile with duplicate-key rejection and strict validation."""

    profile_path = Path(path)
    document = _load_strict_json(profile_path, ProfileValidationError)
    validate_profile(document)
    return dict(document)


def load_target_config(
    path: Path | str = DEFAULT_TARGET_CONFIG,
    *,
    repository_root: Path | str = REPOSITORY_ROOT,
) -> ghidra_abi.GhidraConfig:
    """Load and strengthen validation of the dedicated Ghidra target config."""

    config_path = Path(path)
    root = Path(repository_root).resolve()
    try:
        config = ghidra_abi.load_config(config_path, repository_root=root)
    except ghidra_abi.GhidraConfigError as exc:
        raise ArchProfileConfigError(str(exc)) from exc

    target_by_id = config.targets
    actual_ids = frozenset(target_by_id)
    if actual_ids != EXPECTED_TARGET_IDS:
        missing = sorted(EXPECTED_TARGET_IDS - actual_ids)
        extra = sorted(actual_ids - EXPECTED_TARGET_IDS)
        details: list[str] = []
        if missing:
            details.append(f"missing target IDs {missing}")
        if extra:
            details.append(f"unexpected target IDs {extra}")
        raise ArchProfileConfigError("; ".join(details))

    for target_id, required_roles in EXPECTED_ROLE_BY_TARGET.items():
        target = target_by_id[target_id]
        roles = target.roles
        missing_roles = [role for role in required_roles if role not in roles]
        if missing_roles:
            raise ArchProfileConfigError(f"{target_id}.roles missing {missing_roles}")

    return config


def build_ghidra_evidence(
    config: ghidra_abi.GhidraConfig,
    executor: Any | None = None,
) -> dict[str, Any]:
    """Generate bounded static evidence through the existing Ghidra adapter."""

    try:
        return ghidra_abi.build_evidence(config, executor=executor)
    except (
        ghidra_abi.GhidraConfigError,
        ghidra_abi.GhidraExecutionError,
    ) as exc:
        raise ArchProfileEvidenceError(str(exc)) from exc


def load_ghidra_evidence(
    path: Path | str,
    config: ghidra_abi.GhidraConfig,
) -> dict[str, Any]:
    """Load generated Ghidra evidence and bind it to the current manifest."""

    evidence_path = Path(path)
    raw_document = _load_strict_json(evidence_path, ArchProfileEvidenceError)
    if not isinstance(raw_document, dict):
        raise ArchProfileEvidenceError("Ghidra evidence root must be an object")
    if raw_document.get("schema_version") != 1:
        raise ArchProfileEvidenceError("Ghidra evidence schema_version must be 1")
    if raw_document.get("config_sha256") != config.source_sha256:
        raise ArchProfileEvidenceError(
            "Ghidra evidence was not generated from the current target config"
        )
    if raw_document.get("generator") != "waved/scripts/npu/ghidra_abi.py":
        raise ArchProfileEvidenceError("unexpected Ghidra evidence generator")

    raw_targets = raw_document.get("targets")
    if not isinstance(raw_targets, list):
        raise ArchProfileEvidenceError("Ghidra evidence targets must be an array")
    target_by_id: dict[str, Mapping[str, Any]] = {}
    for index, raw_target in enumerate(raw_targets):
        if not isinstance(raw_target, Mapping):
            raise ArchProfileEvidenceError(
                f"Ghidra evidence target {index} must be an object"
            )
        evidence_id = raw_target.get("evidence_id")
        if not isinstance(evidence_id, str):
            raise ArchProfileEvidenceError(
                f"Ghidra evidence target {index} has no evidence_id"
            )
        if evidence_id in target_by_id:
            raise ArchProfileEvidenceError(
                f"duplicate Ghidra evidence target {evidence_id}"
            )
        target_by_id[evidence_id] = raw_target

    if frozenset(target_by_id) != EXPECTED_TARGET_IDS:
        raise ArchProfileEvidenceError(
            "Ghidra evidence target IDs do not match the exact four roles"
        )

    for evidence_id, configured in config.targets.items():
        recovered = target_by_id[evidence_id]
        if recovered.get("sha256") != configured.sha256:
            raise ArchProfileEvidenceError(
                f"{evidence_id}: Ghidra evidence SHA-256 mismatch"
            )
        analysis = recovered.get("analysis")
        if not isinstance(analysis, Mapping):
            raise ArchProfileEvidenceError(f"{evidence_id}: missing analysis object")
        program = analysis.get("program")
        if (
            not isinstance(program, Mapping)
            or program.get("sha256") != configured.sha256
            or "ELF" not in str(program.get("format", ""))
        ):
            raise ArchProfileEvidenceError(
                f"{evidence_id}: invalid Ghidra program identity"
            )
        tool = analysis.get("tool")
        if (
            not isinstance(tool, Mapping)
            or tool.get("name") != "Ghidra"
            or tool.get("script") != "ExportNpuAbi.java"
            or not isinstance(tool.get("version"), str)
            or not tool.get("version")
        ):
            raise ArchProfileEvidenceError(
                f"{evidence_id}: invalid Ghidra tool identity"
            )
        warnings = analysis.get("warnings")
        if warnings != []:
            raise ArchProfileEvidenceError(
                f"{evidence_id}: Ghidra warnings must be reviewed: {warnings!r}"
            )
        function_records = analysis.get("targets")
        if not isinstance(function_records, list):
            raise ArchProfileEvidenceError(
                f"{evidence_id}: missing focused-function records"
            )
        records_by_name: dict[str, Mapping[str, Any]] = {}
        for function_record in function_records:
            if not isinstance(function_record, Mapping):
                raise ArchProfileEvidenceError(
                    f"{evidence_id}: malformed focused-function record"
                )
            function_name = function_record.get("target")
            if not isinstance(function_name, str):
                raise ArchProfileEvidenceError(
                    f"{evidence_id}: focused-function record has no name"
                )
            if function_name in records_by_name:
                raise ArchProfileEvidenceError(
                    f"{evidence_id}: duplicate function record {function_name}"
                )
            records_by_name[function_name] = function_record
        if frozenset(records_by_name) != frozenset(configured.functions):
            raise ArchProfileEvidenceError(
                f"{evidence_id}: focused-function set differs from manifest"
            )
        expected_absent = EXPECTED_STATIC_ABSENCES.get(evidence_id, frozenset())
        for function_name, function_record in records_by_name.items():
            observed_found = function_record.get("found")
            expected_found = function_name not in expected_absent
            if observed_found is not expected_found:
                state = "present" if expected_found else "absent"
                raise ArchProfileEvidenceError(
                    f"{evidence_id}: {function_name} must be {state}"
                )

    return raw_document


def _decompiled_function(
    evidence: Mapping[str, Any], evidence_id: str, function_name: str
) -> str:
    raw_targets = evidence["targets"]
    for raw_target in raw_targets:
        if raw_target.get("evidence_id") != evidence_id:
            continue
        analysis = raw_target.get("analysis")
        if not isinstance(analysis, Mapping):
            break
        target_records = analysis.get("targets")
        if not isinstance(target_records, list):
            break
        for target_record in target_records:
            if (
                isinstance(target_record, Mapping)
                and target_record.get("target") == function_name
            ):
                functions = target_record.get("functions")
                if not isinstance(functions, list):
                    break
                for function in functions:
                    if (
                        isinstance(function, Mapping)
                        and function.get("name") == function_name
                        and function.get("decompile_success") is True
                        and function.get("decompile_truncated") is False
                    ):
                        decompiled = function.get("decompiled_c")
                        if isinstance(decompiled, str) and decompiled:
                            return decompiled
    raise ArchProfileEvidenceError(
        f"{evidence_id}: missing complete decompilation for {function_name}"
    )


def _elf_virtual_to_file_offset(payload: bytes, virtual_address: int, span: int) -> int:
    """Map an ELF64 little-endian virtual range to immutable file bytes."""

    if len(payload) < 64 or payload[:4] != b"\x7fELF":
        raise ArchProfileEvidenceError("architecture database target is not ELF")
    if payload[4] != 2:
        raise ArchProfileEvidenceError("architecture database target is not ELF64")
    if payload[5] != 1:
        raise ArchProfileEvidenceError(
            "architecture database target is not little-endian"
        )
    try:
        header = struct.unpack_from("<16sHHIQQQIHHHHHH", payload, 0)
    except struct.error as exc:
        raise ArchProfileEvidenceError("truncated ELF64 header") from exc
    program_header_offset = header[5]
    program_header_entry_size = header[9]
    program_header_count = header[10]
    if program_header_entry_size < 56:
        raise ArchProfileEvidenceError("invalid ELF64 program-header size")
    if program_header_count == 0:
        raise ArchProfileEvidenceError("ELF has no program headers")

    requested_end = virtual_address + span
    for index in range(program_header_count):
        entry_offset = program_header_offset + index * program_header_entry_size
        try:
            (
                segment_type,
                _segment_flags,
                segment_offset,
                segment_virtual_address,
                _segment_physical_address,
                segment_file_size,
                _segment_memory_size,
                _segment_alignment,
            ) = struct.unpack_from("<IIQQQQQQ", payload, entry_offset)
        except struct.error as exc:
            raise ArchProfileEvidenceError(
                "truncated ELF64 program-header table"
            ) from exc
        if segment_type != 1:
            continue
        segment_file_end = segment_virtual_address + segment_file_size
        if (
            virtual_address >= segment_virtual_address
            and requested_end <= segment_file_end
        ):
            file_offset = segment_offset + virtual_address - segment_virtual_address
            if file_offset + span > len(payload):
                raise ArchProfileEvidenceError(
                    "mapped architecture table exceeds the ELF file"
                )
            return file_offset
    raise ArchProfileEvidenceError(
        f"ELF virtual range 0x{virtual_address:x}+0x{span:x} is not file-backed"
    )


def recover_architecture_tables(
    config: ghidra_abi.GhidraConfig,
    evidence: Mapping[str, Any],
) -> dict[str, ArchitectureTableRecovery]:
    """Recover the unique Wave row without loading either analyzed library."""

    recoveries: dict[str, ArchitectureTableRecovery] = {}
    for spec in ARCHITECTURE_TABLE_SPECS:
        target = config.targets[spec.evidence_id]
        binary_path = config.repository_root / target.path
        try:
            payload = binary_path.read_bytes()
        except OSError as exc:
            raise ArchProfileEvidenceError(
                f"cannot read architecture table target {binary_path}: {exc}"
            ) from exc
        observed_hash = hashlib.sha256(payload).hexdigest()
        if observed_hash != target.sha256:
            raise ArchProfileEvidenceError(
                f"{spec.evidence_id}: configured SHA-256 {target.sha256}, "
                f"observed {observed_hash}"
            )

        create_ap_model = _decompiled_function(
            evidence, spec.evidence_id, "CreateAPModel"
        )
        required_tokens = (
            spec.expected_address_token,
            spec.expected_stride_token,
            spec.expected_count_token,
            "0xfff0",
            "CustomerID:0x%x",
        )
        missing_tokens = [
            token for token in required_tokens if token not in create_ap_model
        ]
        if missing_tokens:
            raise ArchProfileEvidenceError(
                f"{spec.evidence_id}: CreateAPModel no longer demonstrates "
                f"table geometry or lookup contract: {missing_tokens}"
            )

        span = spec.record_size * spec.record_count
        file_offset = _elf_virtual_to_file_offset(payload, spec.virtual_address, span)
        matches: list[tuple[int, tuple[int, ...], bytes]] = []
        for row_index in range(spec.record_count):
            row_offset = file_offset + row_index * spec.record_size
            row_bytes = payload[row_offset : row_offset + spec.record_size]
            row_words = struct.unpack(f"<{spec.record_size // 4}I", row_bytes)
            if row_words[4] == WAVE_CUSTOMER_ID:
                matches.append((row_index, row_words, row_bytes))
        if len(matches) != 1:
            raise ArchProfileEvidenceError(
                f"{spec.evidence_id}: expected one customer "
                f"0x{WAVE_CUSTOMER_ID:08x} row, found {len(matches)}"
            )

        row_index, row_words, row_bytes = matches[0]
        identity_words = tuple(row_words[:5])
        if identity_words != EXPECTED_DATABASE_IDENTITY:
            rendered = ", ".join(f"0x{value:08x}" for value in identity_words)
            raise ArchProfileEvidenceError(
                f"{spec.evidence_id}: unexpected Wave identity tuple {rendered}"
            )
        recoveries[spec.evidence_id] = ArchitectureTableRecovery(
            evidence_id=spec.evidence_id,
            virtual_address=spec.virtual_address,
            file_offset=file_offset,
            record_size=spec.record_size,
            record_count=spec.record_count,
            matched_row_index=row_index,
            identity_words=identity_words,  # type: ignore[arg-type]
            generic_revision_marker=row_words[8],
            row_sha256=hashlib.sha256(row_bytes).hexdigest(),
            row_words=tuple(row_words),
        )

    wave = recoveries["wave-nnarchperf-aarch64"]
    simulator = recoveries["x86-sim-nnarchperf"]
    if wave.identity_words != simulator.identity_words:
        raise ArchProfileEvidenceError(
            "Wave and simulator architecture tables disagree on identity"
        )
    if wave.generic_revision_marker != 0 or simulator.generic_revision_marker != 0:
        raise ArchProfileEvidenceError(
            "Wave database rows no longer use the demonstrated generic marker"
        )
    return recoveries


def architecture_table_summary(
    recoveries: Mapping[str, ArchitectureTableRecovery],
) -> dict[str, Any]:
    """Return a JSON-safe, bounded summary rather than the entire private row."""

    result: dict[str, Any] = {}
    for evidence_id, recovery in sorted(recoveries.items()):
        result[evidence_id] = {
            "virtual_address": f"0x{recovery.virtual_address:08x}",
            "file_offset": f"0x{recovery.file_offset:08x}",
            "record_size": recovery.record_size,
            "record_count": recovery.record_count,
            "matched_row_index": recovery.matched_row_index,
            "identity_words": [f"0x{word:08x}" for word in recovery.identity_words],
            "generic_revision_marker": recovery.generic_revision_marker,
            "row_sha256": recovery.row_sha256,
        }
    return result


def _profile_input(
    artifact_id: str,
    path: Path,
    roles: Sequence[str],
    *,
    repository_root: Path = REPOSITORY_ROOT,
) -> dict[str, Any]:
    root = repository_root.resolve()
    resolved = path.resolve()
    try:
        relative_path = resolved.relative_to(root)
    except ValueError as exc:
        raise ArchProfileEvidenceError(
            f"profile input {path} is outside the repository"
        ) from exc
    try:
        payload = resolved.read_bytes()
    except OSError as exc:
        raise ArchProfileEvidenceError(
            f"cannot read profile input {path}: {exc}"
        ) from exc
    return {
        "id": artifact_id,
        "path": relative_path.as_posix(),
        "sha256": hashlib.sha256(payload).hexdigest(),
        "bytes": len(payload),
        "roles": sorted(roles),
    }


def _source(artifact_id: str, location: str) -> dict[str, str]:
    return {"artifact_id": artifact_id, "location": location}


def _fact(
    value: Any,
    representation: str,
    evidence_class: str,
    confidence: str,
    sources: Sequence[Mapping[str, str]],
    validation_state: str,
    notes: str,
) -> dict[str, Any]:
    return {
        "value": value,
        "representation": representation,
        "evidence_class": evidence_class,
        "confidence": confidence,
        "sources": [dict(source) for source in sources],
        "validation_state": validation_state,
        "notes": notes,
    }


def _unknown_fact(representation: str, notes: str) -> dict[str, Any]:
    return _fact(
        None,
        representation,
        "unknown",
        "none",
        [],
        "unresolved",
        notes,
    )


def build_profile_document(
    config: ghidra_abi.GhidraConfig,
    evidence: Mapping[str, Any],
    *,
    evidence_path: Path = DEFAULT_GHIDRA_EVIDENCE,
) -> dict[str, Any]:
    """Build the initial Wave profile from pinned static and prior live facts."""

    recoveries = recover_architecture_tables(config, evidence)
    wave_table = recoveries["wave-nnarchperf-aarch64"]
    simulator_table = recoveries["x86-sim-nnarchperf"]

    shared_prefix_words = 0
    for index, (wave_word, simulator_word) in enumerate(
        zip(wave_table.row_words, simulator_table.row_words)
    ):
        if wave_word != simulator_word and index != 6:
            break
        shared_prefix_words = index + 1
    if shared_prefix_words != 114:
        raise ArchProfileEvidenceError(
            "unexpected differential architecture-table prefix length "
            f"{shared_prefix_words}; expected 114"
        )
    equal_non_build_specific_words = sum(
        wave_table.row_words[index] == simulator_table.row_words[index]
        for index in range(shared_prefix_words)
        if index != 6
    )
    if equal_non_build_specific_words != 113:
        raise ArchProfileEvidenceError(
            "architecture-table common prefix changed unexpectedly"
        )

    input_documents: list[dict[str, Any]] = []
    for evidence_id, target in config.targets.items():
        input_documents.append(
            _profile_input(
                evidence_id,
                config.repository_root / target.path,
                target.roles,
                repository_root=config.repository_root,
            )
        )
    input_documents.extend(
        [
            _profile_input(
                "arch-profile-ghidra-evidence",
                evidence_path,
                ["decompilation", "host-only", "static-evidence"],
                repository_root=config.repository_root,
            ),
            _profile_input(
                "poc5-custom-compiler-notebook",
                LIVE_CAPABILITIES_NOTEBOOK,
                ["observed-live", "prior-poc", "read-only-capabilities"],
                repository_root=config.repository_root,
            ),
            _profile_input(
                "poc6-7-custom-nbg-notebook",
                LIVE_IDENTITY_NOTEBOOK,
                ["observed-live", "prior-poc", "runtime-identity"],
                repository_root=config.repository_root,
            ),
            _profile_input(
                "wave-live-baseline-manifest",
                LIVE_BASELINE_MANIFEST,
                ["artifact-provenance", "observed-live", "prior-poc"],
                repository_root=config.repository_root,
            ),
        ]
    )
    input_documents.sort(key=lambda artifact: artifact["id"])

    wave_binary_source = _source(
        "wave-nnarchperf-aarch64",
        "CreateAPModel@00116ad8 and ELF table "
        f"file+0x{wave_table.file_offset:08x}, row "
        f"{wave_table.matched_row_index}",
    )
    simulator_binary_source = _source(
        "x86-sim-nnarchperf",
        "CreateAPModel@0011cdc0 and gChipInfo ELF table "
        f"file+0x{simulator_table.file_offset:08x}, row "
        f"{simulator_table.matched_row_index}",
    )
    capability_source = _source(
        "poc5-custom-compiler-notebook",
        "PoC 5D hardware capabilities returned by the single read-only query",
    )
    runtime_source = _source(
        "poc5-custom-compiler-notebook",
        "Fixed device and runtime identity: galcore exact version",
    )
    identity_source = _source(
        "poc6-7-custom-nbg-notebook",
        "PoC 6 exact runtime IDENTITY record before the reviewed Erf run",
    )
    baseline_source = _source(
        "wave-live-baseline-manifest",
        "npu.nbg_product_target and exact runtime artifact inventory",
    )
    archmodel_source = _source(
        "wave-archmodel-aarch64",
        "archSwLibInit@00117298 and archShowConfigurations@00107ca8",
    )
    x86_archmodel_source = _source(
        "x86-sim-archmodel",
        "archSwLibInit@0011aa00",
    )
    ghidra_source = _source(
        "arch-profile-ghidra-evidence",
        "tool.version and four hash-bound focused analyses",
    )

    identity = {
        "chip_model": _fact(
            0x00009000,
            "uint32; 0x00009000",
            "observed-live",
            "high",
            [identity_source, wave_binary_source, simulator_binary_source],
            "corroborated",
            "The live identity and both static database variants agree.",
        ),
        "chip_revision_observed": _fact(
            0x00009002,
            "uint32; 0x00009002",
            "observed-live",
            "high",
            [identity_source],
            "single-source",
            "This is the live silicon revision, not the database row revision.",
        ),
        "chip_revision_database": _fact(
            0x00009001,
            "uint32; 0x00009001",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "Both static tables use the generic 0x9001 family row.",
        ),
        "product_id": _fact(
            0x05090001,
            "uint32; 0x05090001",
            "observed-live",
            "high",
            [identity_source, wave_binary_source, simulator_binary_source],
            "corroborated",
            "The live identity and both database variants agree.",
        ),
        "eco_id": _fact(
            0x04000000,
            "uint32; 0x04000000",
            "observed-live",
            "high",
            [capability_source, identity_source, wave_binary_source],
            "corroborated",
            "Read-only capability and identity probes agree with the table.",
        ),
        "customer_id": _fact(
            WAVE_CUSTOMER_ID,
            "uint32; 0x1000000e",
            "observed-live",
            "high",
            [
                capability_source,
                identity_source,
                baseline_source,
                wave_binary_source,
                simulator_binary_source,
            ],
            "corroborated",
            "This is also the product target in all 138 stock VPMN v2 NBGs.",
        ),
        "lookup_key_order": _fact(
            [
                "chip_id",
                "chip_version",
                "product_id",
                "eco_id",
                "customer_id",
            ],
            "five consecutive little-endian uint32 keys",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "CreateAPModel compares words 0 through 4 in this order; the "
            "failure diagnostic independently names the order.",
        ),
        "lookup_key_offsets_bytes": _fact(
            [0, 4, 8, 12, 16],
            "byte offsets in APM_IN_PARAM_T and each architecture table row",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "Both decompilations compare five consecutive 32-bit words.",
        ),
        "revision_fallback_mask": _fact(
            0x0000FFF0,
            "uint32; (input_revision XOR row_revision) AND 0x0000fff0",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "The second lookup loop ignores only the low revision nibble.",
        ),
        "database_match_mode": _fact(
            "masked-revision-family-fallback",
            "live 0x9002 matches generic table 0x9001 under mask 0x0000fff0",
            "inferred",
            "medium",
            [identity_source, wave_binary_source, simulator_binary_source],
            "candidate",
            "Static control flow plus the live tuple demonstrates the only "
            "matching row, but PoC 9A does not call CreateAPModel to observe "
            "the branch dynamically.",
        ),
        "lookup_failure_behavior": _fact(
            "diagnostic-then-assert",
            "CreateAPModel prints all five keys and calls __assert_fail",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "No permissive unknown-chip fallback was found.",
        ),
        "database_row_summary": _fact(
            architecture_table_summary(recoveries),
            "host-read ELF table geometry and SHA-256 of each complete row",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "The complete rows are hash-bound; only bounded identity metadata "
            "is serialized into the profile.",
        ),
        "database_common_prefix": _fact(
            {
                "words_compared": shared_prefix_words,
                "equal_non_build_specific_words": (equal_non_build_specific_words),
                "build_specific_word_index": 6,
            },
            "uint32 word comparison from the start of both matched rows",
            "differentially-verified",
            "high",
            [wave_binary_source, simulator_binary_source],
            "corroborated",
            "Words 0..113 agree except one build-specific word at index 6; "
            "its semantics are deliberately not inferred here. Layouts "
            "diverge at word 114.",
        ),
    }

    features = {
        "evis1": _fact(
            False,
            "boolean capability",
            "observed-live",
            "high",
            [capability_source],
            "single-source",
            "Returned by the reviewed PoC 5D read-only capability query.",
        ),
        "evis2": _fact(
            True,
            "boolean capability",
            "observed-live",
            "high",
            [capability_source],
            "single-source",
            "Returned by the reviewed PoC 5D read-only capability query.",
        ),
        "subgroup_size": _fact(
            4,
            "elements",
            "observed-live",
            "high",
            [capability_source],
            "single-source",
            "Returned by the reviewed PoC 5D read-only capability query.",
        ),
        "va40": _fact(
            False,
            "boolean capability",
            "observed-live",
            "high",
            [capability_source],
            "single-source",
            "Returned by the reviewed PoC 5D read-only capability query.",
        ),
        "stream_processor": _fact(
            False,
            "boolean capability",
            "observed-live",
            "high",
            [capability_source],
            "single-source",
            "Both stream-processor availability and execution count were zero.",
        ),
        "complete_bug_feature_vector": _unknown_fact(
            "ordered named boolean vector",
            "The row tail contains bit vectors, but PoC 9A has not yet "
            "recovered a complete name-to-bit mapping without ambiguity.",
        ),
    }

    resources = {
        "wave_structure_sizes_bytes": _fact(
            {
                "arch_nn_config": 0x1A0,
                "arch_drv_option": 0x2088,
                "arch_hal_chipidentity": 0x20,
                "arch_nn_database_feature": 0x160,
                "apm_input": 0x128,
                "apm_hw_info": 0xE8,
            },
            "bytes copied or allocated by exact AArch64 initializers",
            "static-verified",
            "high",
            [archmodel_source, wave_binary_source],
            "single-source",
            "These are exact-build ABI bounds, not public or stable ABI sizes.",
        ),
        "x86_simulator_structure_sizes_bytes": _fact(
            {
                "arch_nn_config": 0x198,
                "arch_drv_option": 0x268,
                "arch_hal_chipidentity": 0x20,
                "arch_nn_database_feature": 0x148,
                "apm_input": 0xF0,
                "apm_hw_info": 0xB0,
            },
            "bytes copied or allocated by x86 simulator initializers",
            "static-verified",
            "high",
            [x86_archmodel_source, simulator_binary_source],
            "single-source",
            "The size differences prohibit treating the simulator ABI as the "
            "exact Wave ABI.",
        ),
        "chip_identity_print_offsets_bytes": _fact(
            {
                "chip_model": 0x00,
                "chip_revision": 0x04,
                "product_id": 0x08,
                "customer_id": 0x0C,
                "eco_id": 0x10,
            },
            "archHAL_CHIPIDENTITY byte offsets in archShowConfigurations",
            "static-verified",
            "high",
            [archmodel_source],
            "single-source",
            "This print-structure order differs from the APM lookup tuple's "
            "eco/customer ordering.",
        ),
        "arch_nn_config_mapped_offsets_bytes": _fact(
            {
                "num_nn_cores": 0x68,
                "num_nn_cores_int8": 0x6C,
                "num_nn_cores_int16": 0x70,
                "num_nn_cores_uint16": 0x74,
                "num_nn_cores_float16": 0x78,
                "axi_sram_size": 0x84,
                "vip_sram_base_size": 0x80,
                "sw_tiling": 0xAC,
                "ddr_latency": 0xB0,
                "vector_prune": 0xC8,
                "axi_sram_latency": 0xD0,
                "usc_cache_size": 0xD4,
            },
            "selected high-confidence offsets from archShowConfigurations",
            "static-verified",
            "high",
            [archmodel_source],
            "single-source",
            "Only fields with unambiguous decompiled argument mapping are "
            "included; the notebook inventories the remaining labels.",
        ),
        "vip_sram_size_expression": _fact(
            "config[0x20] + (config[0x1a] + 8) * 0x400",
            "archShowConfigurations expression; unit not established",
            "static-verified",
            "medium",
            [archmodel_source],
            "single-source",
            "The expression is recovered, but PoC 9A does not assign its "
            "printed integer a byte unit.",
        ),
        "physical_npu_core_count": _unknown_fact(
            "physical accelerator cores",
            "The live capability field was zero but is a graph-batch field; "
            "table values have not been mapped strongly enough to physical "
            "cores.",
        ),
        "per_precision_usable_core_count": _unknown_fact(
            "mapping from precision to usable cores",
            "Configuration labels are mapped, but no reviewed live values "
            "exist for the per-precision fields.",
        ),
        "vip_sram_partitioning": _unknown_fact(
            "bytes per bank/core and reserved regions",
            "The print expression is known; exact partition semantics and "
            "units are not.",
        ),
        "axi_sram_compiler_budget_bytes": _unknown_fact(
            "bytes available to generated graphs",
            "A configuration field exists, but its live value and compiler "
            "reservation policy are unresolved.",
        ),
        "cache_line_bytes": _unknown_fact(
            "bytes",
            "Several cache fields are printed, but no single compile-time "
            "tensor cache-line constraint is yet demonstrated.",
        ),
        "tensor_alignment_bytes": _unknown_fact(
            "bytes by tensor/data type",
            "No complete alignment table has been recovered in PoC 9A.",
        ),
    }

    ghidra_versions: set[str] = set()
    for target in evidence["targets"]:
        tool = target.get("analysis", {}).get("tool", {})
        if isinstance(tool, Mapping) and isinstance(tool.get("version"), str):
            ghidra_versions.add(tool["version"])
    if len(ghidra_versions) != 1:
        raise ArchProfileEvidenceError(
            f"Ghidra evidence versions disagree: {sorted(ghidra_versions)}"
        )
    ghidra_version = next(iter(ghidra_versions))

    versions = {
        "galcore_runtime": _fact(
            "6.4.19.5.946407",
            "exact live runtime version string",
            "observed-live",
            "high",
            [runtime_source],
            "single-source",
            "Previously captured from the exact live Wave runtime notebook.",
        ),
        "firmware_baseline": _fact(
            "v1.1.6.6-compatible",
            "rootfs compatibility label",
            "inferred",
            "medium",
            [baseline_source],
            "candidate",
            "Runtime library hashes match the extraction; the live release "
            "file did not itself report v1.1.6.6.",
        ),
        "ghidra": _fact(
            ghidra_version,
            "host static-analysis tool version",
            "static-verified",
            "high",
            [ghidra_source],
            "single-source",
            "Recorded by every focused Ghidra analysis.",
        ),
        "arch_model_version": _unknown_fact(
            "vendor architecture-model semantic version",
            "The label is present, but no immutable static value has been "
            "mapped to it.",
        ),
        "sw_tiling_version": _unknown_fact(
            "vendor software-tiling semantic version",
            "The label is present, but no immutable static value has been "
            "mapped to it.",
        ),
        "vip7_version": _unknown_fact(
            "numeric architecture-generation value",
            "The configuration offset is present, but its exact Wave value "
            "has not been observed or safely recovered.",
        ),
    }

    fact_sections = {
        "identity": identity,
        "features": features,
        "resources": resources,
        "versions": versions,
    }
    unknowns = sorted(
        f"{section_name}.{fact_name}"
        for section_name, section in fact_sections.items()
        for fact_name, fact_document in section.items()
        if fact_document["evidence_class"] == "unknown"
    )
    document = {
        "schema_version": 1,
        "profile_id": "wave-fw-v1.1.6.6-0x1000000e",
        "target": {
            "value": WAVE_CUSTOMER_ID,
            "hex": "0x1000000e",
        },
        "inputs": input_documents,
        **fact_sections,
        "unknowns": unknowns,
        "non_claims": sorted(
            [
                (
                    "PoC 9A does not call any compiler, architecture-model, "
                    "or performance-model entrypoint."
                ),
                (
                    "PoC 9A does not prove a VIV_GPU_FILE grammar or emit a "
                    "candidate compiler profile."
                ),
                (
                    "PoC 9A does not prove physical core count, SRAM "
                    "partitioning, or tensor alignment."
                ),
                "PoC 9A does not touch ADB, /dev/galcore, or the Wave device.",
                (
                    "The x86 simulator ABI is a differential reference and "
                    "is not binary-compatible with the exact Wave ABI."
                ),
                (
                    "The static 0x00009001 database revision is not relabeled "
                    "as the live 0x00009002 silicon revision."
                ),
            ]
        ),
    }
    validate_profile(document)
    return document


def _atomic_write_json(path: Path, document: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    serialized = canonical_json(document)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary_path = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_path, path)
    except BaseException:
        temporary_path.unlink(missing_ok=True)
        raise


def _command_validate_targets(args: argparse.Namespace) -> int:
    load_target_config(args.config, repository_root=args.repository_root)
    print(f"validated {args.config}")
    return 0


def _command_generate_ghidra(args: argparse.Namespace) -> int:
    config = load_target_config(args.config, repository_root=args.repository_root)
    evidence = build_ghidra_evidence(config)
    _atomic_write_json(Path(args.output), evidence)
    print(f"wrote {args.output}")
    return 0


def _command_generate_profile(args: argparse.Namespace) -> int:
    config = load_target_config(args.config, repository_root=args.repository_root)
    evidence = load_ghidra_evidence(args.ghidra, config)
    profile = build_profile_document(config, evidence, evidence_path=Path(args.ghidra))
    _atomic_write_json(Path(args.output), profile)
    print(f"wrote {args.output}")
    return 0


def _command_validate_profile(args: argparse.Namespace) -> int:
    load_profile(args.profile)
    print(f"validated {args.profile}")
    return 0


def _command_verify_generated(args: argparse.Namespace) -> int:
    config = load_target_config(args.config, repository_root=args.repository_root)
    if args.ghidra_only:
        generated = canonical_json(build_ghidra_evidence(config))
        try:
            tracked = Path(args.ghidra).read_bytes()
        except OSError as exc:
            raise ArchProfileEvidenceError(
                f"cannot read tracked Ghidra evidence: {exc}"
            ) from exc
        if tracked != generated:
            raise ArchProfileEvidenceError(
                "tracked Ghidra evidence differs from deterministic generation"
            )
        print("generated Ghidra evidence is current")
        return 0

    evidence = load_ghidra_evidence(args.ghidra, config)
    generated_profile = canonical_json(
        build_profile_document(config, evidence, evidence_path=Path(args.ghidra))
    )
    try:
        tracked_profile = Path(args.profile).read_bytes()
    except OSError as exc:
        raise ArchProfileEvidenceError(
            f"cannot read tracked Wave profile: {exc}"
        ) from exc
    if tracked_profile != generated_profile:
        raise ArchProfileEvidenceError(
            "tracked Wave profile differs from deterministic generation"
        )
    load_profile(args.profile)
    print("generated Wave profile is current")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Host-only Wave architecture-profile recovery tooling"
    )
    parser.set_defaults(handler=None)
    subparsers = parser.add_subparsers(dest="command")

    validate_targets = subparsers.add_parser(
        "validate-targets", help="validate the four-library analysis manifest"
    )
    validate_targets.add_argument("--config", type=Path, default=DEFAULT_TARGET_CONFIG)
    validate_targets.add_argument(
        "--repository-root", type=Path, default=REPOSITORY_ROOT
    )
    validate_targets.set_defaults(handler=_command_validate_targets)

    generate_ghidra = subparsers.add_parser(
        "generate-ghidra", help="run bounded host-only Ghidra analysis"
    )
    generate_ghidra.add_argument("--config", type=Path, default=DEFAULT_TARGET_CONFIG)
    generate_ghidra.add_argument(
        "--repository-root", type=Path, default=REPOSITORY_ROOT
    )
    generate_ghidra.add_argument("--output", type=Path, default=DEFAULT_GHIDRA_EVIDENCE)
    generate_ghidra.set_defaults(handler=_command_generate_ghidra)

    generate_profile = subparsers.add_parser(
        "generate-profile",
        help="recover the strict Wave evidence profile from host artifacts",
    )
    generate_profile.add_argument("--config", type=Path, default=DEFAULT_TARGET_CONFIG)
    generate_profile.add_argument(
        "--repository-root", type=Path, default=REPOSITORY_ROOT
    )
    generate_profile.add_argument(
        "--ghidra", type=Path, default=DEFAULT_GHIDRA_EVIDENCE
    )
    generate_profile.add_argument("--output", type=Path, default=DEFAULT_PROFILE)
    generate_profile.set_defaults(handler=_command_generate_profile)

    validate_profile_parser = subparsers.add_parser(
        "validate-profile", help="validate a generated Wave profile"
    )
    validate_profile_parser.add_argument(
        "--profile", type=Path, default=DEFAULT_PROFILE
    )
    validate_profile_parser.set_defaults(handler=_command_validate_profile)

    verify_generated = subparsers.add_parser(
        "verify-generated",
        help="verify tracked profile or rerun Ghidra for byte identity",
    )
    verify_generated.add_argument("--config", type=Path, default=DEFAULT_TARGET_CONFIG)
    verify_generated.add_argument(
        "--repository-root", type=Path, default=REPOSITORY_ROOT
    )
    verify_generated.add_argument(
        "--ghidra", type=Path, default=DEFAULT_GHIDRA_EVIDENCE
    )
    verify_generated.add_argument("--profile", type=Path, default=DEFAULT_PROFILE)
    verify_generated.add_argument(
        "--ghidra-only",
        action="store_true",
        help="rerun only the expensive four-target Ghidra extraction",
    )
    verify_generated.set_defaults(handler=_command_verify_generated)

    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    if args.handler is None:
        parser.print_help()
        return 2
    try:
        return int(args.handler(args))
    except ArchProfileError as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
