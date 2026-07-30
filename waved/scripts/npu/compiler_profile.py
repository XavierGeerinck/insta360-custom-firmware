#!/usr/bin/env python3
"""Recover the Wave compiler-profile boundary without loading a compiler.

PoC 9C is intentionally host-only.  This module validates hash-pinned source
and ELF artifacts, consumes bounded Ghidra evidence, recovers an inert
simulator configuration catalog, and implements a hardened five-field parser.
It never loads OpenVX, VSC, CLC, GAL, or a target architecture library.  The
only optional dynamic action is delegated to a separately isolated parser
probe; compiler and device execution are not implemented here.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import struct
import subprocess
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any

try:
    from . import arch_profile, ghidra_abi
except ImportError:  # pragma: no cover - direct script execution
    import arch_profile  # type: ignore
    import ghidra_abi  # type: ignore


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CORPUS = Path(__file__).with_name("compiler_profile_corpus.json")
DEFAULT_TARGET_CONFIG = Path(__file__).with_name("compiler_profile_targets.json")
DEFAULT_GHIDRA_EVIDENCE = (
    REPOSITORY_ROOT / "docs/npu/profile/compiler-profile-ghidra-evidence.json"
)
DEFAULT_WAVE_PROFILE = REPOSITORY_ROOT / "docs/npu/profile/wave-profile-evidence.json"
DEFAULT_GRAMMAR = REPOSITORY_ROOT / "docs/npu/profile/viv-gpu-file-grammar.json"
DEFAULT_CANDIDATE = (
    REPOSITORY_ROOT / "docs/npu/profile/wave-viv-gpu-file.candidate.config"
)
DEFAULT_CANDIDATE_MANIFEST = (
    REPOSITORY_ROOT / "docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json"
)
DEFAULT_PROBE_RESULTS = (
    REPOSITORY_ROOT / "docs/npu/profile/vsim-parser-probe-results.json"
)
PARSER_PROBE_SOURCE = (
    REPOSITORY_ROOT / "waved/tools/npu-profile/vsim_config_parser_probe.cc"
)

CORPUS_ROOT_KEYS = frozenset({"schema_version", "artifacts"})
CORPUS_ARTIFACT_KEYS = frozenset(
    {"path", "sha256", "kind", "roles", "execution_policy"}
)
CORPUS_KINDS = frozenset({"elf", "text"})
EXECUTION_POLICIES = frozenset(
    {
        "static-only",
        "not-executable",
        "isolated-parser-only",
        "quarantined-never-call",
    }
)
EXPECTED_TARGET_ID = "x86-simulator-emulator"
EXPECTED_TARGET_FUNCTIONS = frozenset(
    {
        "StateParser::QueryFeatureDatabase",
        "StateParser::StateParser",
        "StateParser::chooseTargetByConfigFile",
        "StateParser::chooseTargetByFeature",
        "StateParser::chooseTargetByID",
        "StateParser::chooseTargetByName",
        "StateParser::getConfig",
        "StateParser::~StateParser",
        "gcQueryFeatureDB",
    }
)
EXPECTED_TARGET_ROLES = frozenset(
    {
        "configuration-parser",
        "differential-reference",
        "host-x86",
        "static-only",
    }
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
CONFIG_LINE_RE_TEMPLATE = r"^{key}[ \t]*=[ \t]*(?:0[xX])?([0-9a-fA-F]{{1,8}});[ \t]*$"

MAX_CONFIG_BYTES = 512
MAX_CONFIG_LINE_BYTES = 128
VSIM_CONFIG_VIRTUAL_ADDRESS = 0x0047C760
VSIM_CONFIG_RECORD_SIZE = 100
VSIM_CONFIG_RECORD_COUNT = 358
VSIM_CONFIG_NAME_BYTES = 80
VSIM_CONFIG_TABLE_SHA256 = (
    "5f6bf1cf829c1b13878a8162950a0edc2e3b750aafb4dba0beeace5a6992b1ec"
)
WAVE_CONFIG_RECORD_SHA256 = (
    "4d533d3c898a3eb1e317148826bad0d16172cecf7add61f10ede6d45ef407e69"
)
WAVE_CUSTOMER_ID = 0x1000000E
WAVE_CONFIG_NAME = "VIP9000NANOS_PID0X1000000E"
DEFAULT_CONFIG_INDEX = 187
DEFAULT_CONFIG_NAME = "VIP9000NANODI_PID0X10000020"
PROBE_DOCKER_IMAGE = "wave-npu-compiler-toolchain:ubuntu22.04-amd64"
PROBE_DOCKER_IMAGE_ID = (
    "sha256:083ca628e0feb7ca682aec5180e5331d0c758bb4d73fcd878b9900cdd3d777ce"
)
PROBE_CASES = (
    "built-in-wave-name",
    "valid-database",
    "valid-live",
    "missing",
    "empty",
    "truncated",
    "wrong-order",
    "duplicate-key",
    "unknown-key",
    "malformed-number",
    "overlong-line",
    "wrong-customer",
    "trailing-content",
)
MAX_PROBE_LOG_CHARS = 8192


class CompilerProfileError(RuntimeError):
    """Base failure for host-only compiler-profile recovery."""


class CorpusConfigError(CompilerProfileError):
    """The hash-pinned input corpus is malformed or stale."""


class CompilerEvidenceError(CompilerProfileError):
    """Focused static evidence is missing, inconsistent, or incomplete."""


class ConfigGrammarError(CompilerProfileError):
    """A configuration does not satisfy the hardened five-field grammar."""


class CandidateError(CompilerProfileError):
    """A Wave candidate cannot pass the evidence and confidence gate."""


@dataclass(frozen=True)
class CorpusArtifact:
    """One inert, hash-pinned input artifact."""

    artifact_id: str
    path: Path
    resolved_path: Path
    sha256: str
    byte_count: int
    kind: str
    roles: tuple[str, ...]
    execution_policy: str


@dataclass(frozen=True)
class Corpus:
    """Validated input manifest."""

    repository_root: Path
    source_path: Path
    source_sha256: str
    artifacts: Mapping[str, CorpusArtifact]


@dataclass(frozen=True)
class HardwareIdentity:
    """The five uint32 values consumed by the recovered parser."""

    chip_model: int
    chip_revision: int
    product_id: int
    eco_id: int
    customer_id: int

    def __post_init__(self) -> None:
        for field_name, value in zip(
            (
                "chip_model",
                "chip_revision",
                "product_id",
                "eco_id",
                "customer_id",
            ),
            self.as_tuple(),
            strict=True,
        ):
            if (
                not isinstance(value, int)
                or isinstance(value, bool)
                or not 0 <= value <= 0xFFFFFFFF
            ):
                raise ConfigGrammarError(
                    f"{field_name} must be an unsigned 32-bit integer"
                )

    def as_tuple(self) -> tuple[int, int, int, int, int]:
        return (
            self.chip_model,
            self.chip_revision,
            self.product_id,
            self.eco_id,
            self.customer_id,
        )


@dataclass(frozen=True)
class ConfigField:
    """One field proven in the simulator parser."""

    key: str
    member: str
    offset: int
    scanf_format: str


CONFIG_FIELDS = (
    ConfigField(
        "chipModel",
        "chip_model",
        0x50,
        "chipModel              = %x;\\n",
    ),
    ConfigField(
        "chipRevision",
        "chip_revision",
        0x54,
        "chipRevision           = %x;\\n",
    ),
    ConfigField(
        "productID",
        "product_id",
        0x58,
        "productID              = %x;\\n",
    ),
    ConfigField(
        "ecoID",
        "eco_id",
        0x5C,
        "ecoID                  = %x;\\n",
    ),
    ConfigField(
        "customerID",
        "customer_id",
        0x60,
        "customerID             = %x;\\n",
    ),
)


@dataclass(frozen=True)
class ParserFacts:
    """Facts statically recovered from a complete selector body."""

    function: str
    fields: tuple[ConfigField, ...]
    record_size: int
    name_bytes: int
    file_mode: str
    vendor_success_test: str
    vendor_accepts_eof_bug: bool
    missing_file_falls_back: bool
    query_key_order: tuple[str, ...]


@dataclass(frozen=True)
class VsimConfigRecord:
    """One immutable 100-byte built-in simulator target."""

    index: int
    name: str
    identity: HardwareIdentity
    sha256: str


@dataclass(frozen=True)
class CatalogRecovery:
    """Strict recovery of ``vsimConfigs`` from inert ELF bytes."""

    virtual_address: int
    file_offset: int
    record_size: int
    record_count: int
    table_sha256: str
    records: tuple[VsimConfigRecord, ...]
    wave_record: VsimConfigRecord
    default_record: VsimConfigRecord


def canonical_json(value: Any) -> bytes:
    """Serialize evidence deterministically."""

    return ghidra_abi.canonical_json(value)


def _reject_duplicate_keys(pairs: Sequence[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_strict_json(path: Path, error_type: type[CompilerProfileError]) -> Any:
    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_reject_duplicate_keys,
        )
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise error_type(f"cannot load {path}: {exc}") from exc


def _require_mapping(
    value: Any, location: str, error_type: type[CompilerProfileError]
) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise error_type(f"{location} must be an object")
    return value


def _require_exact_keys(
    value: Mapping[str, Any],
    expected: frozenset[str],
    location: str,
    error_type: type[CompilerProfileError],
) -> None:
    actual = frozenset(value)
    if actual == expected:
        return
    details: list[str] = []
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    if missing:
        details.append(f"missing keys {missing}")
    if unknown:
        details.append(f"unknown keys {unknown}")
    raise error_type(f"{location}: {', '.join(details)}")


def _require_string(
    value: Any, location: str, error_type: type[CompilerProfileError]
) -> str:
    if not isinstance(value, str) or not value:
        raise error_type(f"{location} must be a non-empty string")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in value):
        raise error_type(f"{location} contains a control character")
    return value


def _contained_path(root: Path, relative: Path, location: str) -> Path:
    root_resolved = root.resolve()
    resolved = (root_resolved / relative).resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise CorpusConfigError(f"{location} resolves outside the repository") from exc
    return resolved


def load_corpus(
    path: Path | str = DEFAULT_CORPUS,
    *,
    repository_root: Path | str = REPOSITORY_ROOT,
) -> Corpus:
    """Load, hash, and type-check the inert compiler-profile corpus."""

    source_path = Path(path)
    root = Path(repository_root).resolve()
    try:
        source_payload = source_path.read_bytes()
    except OSError as exc:
        raise CorpusConfigError(f"cannot read corpus {source_path}: {exc}") from exc
    document = _load_strict_json(source_path, CorpusConfigError)
    root_object = _require_mapping(document, "corpus", CorpusConfigError)
    _require_exact_keys(root_object, CORPUS_ROOT_KEYS, "corpus", CorpusConfigError)
    if root_object["schema_version"] != 1:
        raise CorpusConfigError("corpus.schema_version must be 1")
    artifacts_object = _require_mapping(
        root_object["artifacts"], "corpus.artifacts", CorpusConfigError
    )
    if not artifacts_object:
        raise CorpusConfigError("corpus.artifacts must not be empty")

    artifacts: dict[str, CorpusArtifact] = {}
    for artifact_id, raw_artifact in sorted(artifacts_object.items()):
        if not isinstance(artifact_id, str) or ID_RE.fullmatch(artifact_id) is None:
            raise CorpusConfigError(f"invalid artifact ID: {artifact_id!r}")
        location = f"corpus.artifacts.{artifact_id}"
        artifact = _require_mapping(raw_artifact, location, CorpusConfigError)
        _require_exact_keys(
            artifact,
            CORPUS_ARTIFACT_KEYS,
            location,
            CorpusConfigError,
        )

        path_text = _require_string(
            artifact["path"], f"{location}.path", CorpusConfigError
        )
        relative_path = Path(path_text)
        if relative_path.is_absolute() or ".." in relative_path.parts:
            raise CorpusConfigError(
                f"{location}.path must be repository-relative and contained"
            )
        resolved_path = _contained_path(root, relative_path, f"{location}.path")
        try:
            payload = resolved_path.read_bytes()
        except OSError as exc:
            raise CorpusConfigError(
                f"cannot read {artifact_id} at {resolved_path}: {exc}"
            ) from exc

        expected_hash = _require_string(
            artifact["sha256"], f"{location}.sha256", CorpusConfigError
        )
        if SHA256_RE.fullmatch(expected_hash) is None:
            raise CorpusConfigError(f"{location}.sha256 must be a lowercase SHA-256")
        observed_hash = hashlib.sha256(payload).hexdigest()
        if observed_hash != expected_hash:
            raise CorpusConfigError(
                f"{artifact_id}: SHA-256 mismatch; expected {expected_hash}, "
                f"observed {observed_hash}"
            )

        kind = _require_string(artifact["kind"], f"{location}.kind", CorpusConfigError)
        if kind not in CORPUS_KINDS:
            raise CorpusConfigError(
                f"{location}.kind must be one of {sorted(CORPUS_KINDS)}"
            )
        if kind == "elf" and not payload.startswith(b"\x7fELF"):
            raise CorpusConfigError(f"{artifact_id}: configured ELF is not ELF")
        if kind == "text":
            try:
                payload.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise CorpusConfigError(
                    f"{artifact_id}: text artifact is not valid UTF-8"
                ) from exc

        raw_roles = artifact["roles"]
        if not isinstance(raw_roles, list) or not raw_roles:
            raise CorpusConfigError(f"{location}.roles must be a non-empty array")
        roles = tuple(
            _require_string(role, f"{location}.roles[{index}]", CorpusConfigError)
            for index, role in enumerate(raw_roles)
        )
        if len(roles) != len(set(roles)):
            raise CorpusConfigError(f"{location}.roles contains duplicate values")

        execution_policy = _require_string(
            artifact["execution_policy"],
            f"{location}.execution_policy",
            CorpusConfigError,
        )
        if execution_policy not in EXECUTION_POLICIES:
            raise CorpusConfigError(
                f"{location}.execution_policy must be one of "
                f"{sorted(EXECUTION_POLICIES)}"
            )
        if "quarantined" in roles and execution_policy != "quarantined-never-call":
            raise CorpusConfigError(
                f"{artifact_id}: quarantined artifacts require quarantined-never-call"
            )
        if execution_policy == "isolated-parser-only" and (
            kind != "elf"
            or not {
                "configuration-parser",
                "configuration-parser-dependency",
            }.intersection(roles)
        ):
            raise CorpusConfigError(
                f"{artifact_id}: isolated-parser-only is limited to a "
                "configuration-parser ELF or its reviewed dependency"
            )

        artifacts[artifact_id] = CorpusArtifact(
            artifact_id=artifact_id,
            path=relative_path,
            resolved_path=resolved_path,
            sha256=expected_hash,
            byte_count=len(payload),
            kind=kind,
            roles=roles,
            execution_policy=execution_policy,
        )

    return Corpus(
        repository_root=root,
        source_path=source_path.resolve(),
        source_sha256=hashlib.sha256(source_payload).hexdigest(),
        artifacts=MappingProxyType(artifacts),
    )


def load_compiler_target_config(
    path: Path | str = DEFAULT_TARGET_CONFIG,
    *,
    repository_root: Path | str = REPOSITORY_ROOT,
) -> ghidra_abi.GhidraConfig:
    """Load and strengthen the one-target compiler-profile Ghidra manifest."""

    try:
        config = ghidra_abi.load_config(
            Path(path), repository_root=Path(repository_root)
        )
    except ghidra_abi.GhidraConfigError as exc:
        raise CompilerEvidenceError(str(exc)) from exc
    if frozenset(config.targets) != frozenset({EXPECTED_TARGET_ID}):
        raise CompilerEvidenceError(
            f"Ghidra target set must be exactly {EXPECTED_TARGET_ID!r}"
        )
    target = config.targets[EXPECTED_TARGET_ID]
    if frozenset(target.roles) != EXPECTED_TARGET_ROLES:
        raise CompilerEvidenceError(
            f"{EXPECTED_TARGET_ID}: roles differ from the reviewed boundary"
        )
    if frozenset(target.functions) != EXPECTED_TARGET_FUNCTIONS:
        raise CompilerEvidenceError(
            f"{EXPECTED_TARGET_ID}: function set differs from the reviewed boundary"
        )
    try:
        payload = (config.repository_root / target.path).read_bytes()
    except OSError as exc:
        raise CompilerEvidenceError(
            f"cannot read configured parser target: {exc}"
        ) from exc
    observed = hashlib.sha256(payload).hexdigest()
    if observed != target.sha256:
        raise CompilerEvidenceError(
            f"{EXPECTED_TARGET_ID}: configured SHA-256 {target.sha256}, "
            f"observed {observed}"
        )
    if not payload.startswith(b"\x7fELF"):
        raise CompilerEvidenceError(
            f"{EXPECTED_TARGET_ID}: configured target is not ELF"
        )
    return config


def _complete_decompilations(
    target_record: Mapping[str, Any],
) -> tuple[str, ...]:
    functions = target_record.get("functions")
    if not isinstance(functions, list):
        return ()
    bodies: list[str] = []
    for function in functions:
        if (
            isinstance(function, Mapping)
            and function.get("decompile_success") is True
            and function.get("decompile_truncated") is False
            and isinstance(function.get("decompiled_c"), str)
            and function["decompiled_c"]
        ):
            bodies.append(function["decompiled_c"])
    return tuple(bodies)


def load_compiler_ghidra_evidence(
    path: Path | str,
    config: ghidra_abi.GhidraConfig,
) -> dict[str, Any]:
    """Load evidence and require a complete primary body for every focus."""

    evidence_path = Path(path)
    document = _load_strict_json(evidence_path, CompilerEvidenceError)
    root = _require_mapping(document, "Ghidra evidence", CompilerEvidenceError)
    _require_exact_keys(
        root,
        frozenset({"schema_version", "config_sha256", "generator", "targets"}),
        "Ghidra evidence",
        CompilerEvidenceError,
    )
    if root["schema_version"] != 1:
        raise CompilerEvidenceError("Ghidra evidence schema_version must be 1")
    if root["config_sha256"] != config.source_sha256:
        raise CompilerEvidenceError(
            "Ghidra evidence was not generated from the current target config"
        )
    if root["generator"] != "waved/scripts/npu/ghidra_abi.py":
        raise CompilerEvidenceError("unexpected Ghidra evidence generator")
    raw_targets = root["targets"]
    if not isinstance(raw_targets, list) or len(raw_targets) != 1:
        raise CompilerEvidenceError(
            "Ghidra evidence must contain exactly one parser target"
        )
    target_document = _require_mapping(
        raw_targets[0], "Ghidra evidence target", CompilerEvidenceError
    )
    configured = config.targets[EXPECTED_TARGET_ID]
    if target_document.get("evidence_id") != EXPECTED_TARGET_ID:
        raise CompilerEvidenceError("unexpected Ghidra evidence target ID")
    if target_document.get("sha256") != configured.sha256:
        raise CompilerEvidenceError("Ghidra evidence target SHA-256 mismatch")
    analysis = _require_mapping(
        target_document.get("analysis"),
        "Ghidra analysis",
        CompilerEvidenceError,
    )
    if analysis.get("warnings") != []:
        raise CompilerEvidenceError(
            f"Ghidra warnings require review: {analysis.get('warnings')!r}"
        )
    program = _require_mapping(
        analysis.get("program"), "Ghidra program", CompilerEvidenceError
    )
    if (
        program.get("sha256") != configured.sha256
        or "ELF" not in str(program.get("format", ""))
        or program.get("language_id") != "x86:LE:64:default"
    ):
        raise CompilerEvidenceError("invalid Ghidra program identity")
    tool = _require_mapping(analysis.get("tool"), "Ghidra tool", CompilerEvidenceError)
    if (
        tool.get("name") != "Ghidra"
        or tool.get("script") != "ExportNpuAbi.java"
        or not isinstance(tool.get("version"), str)
        or not tool["version"]
    ):
        raise CompilerEvidenceError("invalid Ghidra tool identity")

    focus_records = analysis.get("targets")
    if not isinstance(focus_records, list):
        raise CompilerEvidenceError("Ghidra focused targets must be an array")
    by_name: dict[str, Mapping[str, Any]] = {}
    for index, raw_record in enumerate(focus_records):
        record = _require_mapping(
            raw_record, f"Ghidra focus[{index}]", CompilerEvidenceError
        )
        name = record.get("target")
        if not isinstance(name, str) or not name:
            raise CompilerEvidenceError(f"Ghidra focus[{index}] has no target")
        if name in by_name:
            raise CompilerEvidenceError(f"duplicate Ghidra focus target: {name}")
        by_name[name] = record
    if frozenset(by_name) != EXPECTED_TARGET_FUNCTIONS:
        raise CompilerEvidenceError(
            "Ghidra focused target set differs from the current manifest"
        )
    for name, record in by_name.items():
        if record.get("found") is not True:
            raise CompilerEvidenceError(f"Ghidra did not find {name}")
        if not _complete_decompilations(record):
            raise CompilerEvidenceError(
                f"Ghidra has no complete decompilation for {name}"
            )
    return dict(root)


def _focus_record(evidence: Mapping[str, Any], target_name: str) -> Mapping[str, Any]:
    analysis = evidence["targets"][0]["analysis"]
    for record in analysis["targets"]:
        if record.get("target") == target_name:
            return record
    raise CompilerEvidenceError(f"missing focus record {target_name}")


def _body_with_tokens(
    evidence: Mapping[str, Any],
    target_name: str,
    required_tokens: Sequence[str],
) -> str:
    record = _focus_record(evidence, target_name)
    for body in _complete_decompilations(record):
        if all(token in body for token in required_tokens):
            return body
    raise CompilerEvidenceError(
        f"{target_name}: no complete body contains reviewed tokens "
        f"{list(required_tokens)}"
    )


def recover_parser_facts(evidence: Mapping[str, Any]) -> ParserFacts:
    """Recover the exact five sequential ``fscanf`` operations."""

    formats = tuple(field.scanf_format for field in CONFIG_FIELDS)
    selector_tokens = (
        'fopen(param_1,"rb")',
        *formats,
        "QueryFeatureDatabase();",
        "this + 0xf0",
        "fclose(__stream)",
        "iVar15 != 0",
    )
    _body_with_tokens(
        evidence,
        "StateParser::chooseTargetByConfigFile",
        selector_tokens,
    )
    query_tokens = (
        "PTR_m_Config_004b1b70 + 0x60",
        "PTR_m_Config_004b1b70 + 0x5c",
        "PTR_m_Config_004b1b70 + 0x58",
        "PTR_m_Config_004b1b70 + 0x54",
        "PTR_m_Config_004b1b70 + 0x50",
        "gcQueryFeatureDB(uVar5,uVar4,uVar3,uVar2,uVar1)",
    )
    _body_with_tokens(
        evidence,
        "StateParser::QueryFeatureDatabase",
        query_tokens,
    )
    get_config_tokens = (
        "param_1 + 0x50",
        "param_1 + 0x58",
        "param_1 + 0x60",
    )
    _body_with_tokens(
        evidence,
        "StateParser::getConfig",
        get_config_tokens,
    )
    return ParserFacts(
        function="StateParser::chooseTargetByConfigFile(char const*)",
        fields=CONFIG_FIELDS,
        record_size=VSIM_CONFIG_RECORD_SIZE,
        name_bytes=VSIM_CONFIG_NAME_BYTES,
        file_mode="rb",
        vendor_success_test="fscanf(...) != 0",
        vendor_accepts_eof_bug=True,
        missing_file_falls_back=True,
        query_key_order=tuple(field.key for field in CONFIG_FIELDS),
    )


def _elf_virtual_to_file_offset(payload: bytes, virtual_address: int, span: int) -> int:
    """Map an ELF64 little-endian virtual range to file-backed bytes."""

    if len(payload) < 64 or payload[:4] != b"\x7fELF":
        raise CompilerEvidenceError("simulator catalog target is not ELF")
    if payload[4] != 2 or payload[5] != 1:
        raise CompilerEvidenceError(
            "simulator catalog target must be little-endian ELF64"
        )
    try:
        header = struct.unpack_from("<16sHHIQQQIHHHHHH", payload, 0)
    except struct.error as exc:
        raise CompilerEvidenceError("truncated ELF64 header") from exc
    program_offset = header[5]
    entry_size = header[9]
    entry_count = header[10]
    if entry_size < 56 or entry_count == 0:
        raise CompilerEvidenceError("invalid ELF64 program-header table")
    requested_end = virtual_address + span
    for index in range(entry_count):
        offset = program_offset + index * entry_size
        try:
            (
                segment_type,
                _flags,
                segment_offset,
                segment_virtual,
                _physical,
                file_size,
                _memory_size,
                _alignment,
            ) = struct.unpack_from("<IIQQQQQQ", payload, offset)
        except struct.error as exc:
            raise CompilerEvidenceError("truncated ELF64 program-header table") from exc
        if (
            segment_type == 1
            and virtual_address >= segment_virtual
            and requested_end <= segment_virtual + file_size
        ):
            file_offset = segment_offset + virtual_address - segment_virtual
            if file_offset + span > len(payload):
                raise CompilerEvidenceError("simulator catalog exceeds the ELF file")
            return file_offset
    raise CompilerEvidenceError(
        f"ELF virtual range 0x{virtual_address:x}+0x{span:x} is not file-backed"
    )


def recover_vsim_catalog(artifact: CorpusArtifact) -> CatalogRecovery:
    """Recover all 358 immutable simulator target records."""

    if (
        artifact.artifact_id != "x86-emulator-6.4.6"
        or artifact.kind != "elf"
        or artifact.execution_policy != "isolated-parser-only"
    ):
        raise CompilerEvidenceError(
            "catalog recovery requires the reviewed x86 emulator artifact"
        )
    try:
        payload = artifact.resolved_path.read_bytes()
    except OSError as exc:
        raise CompilerEvidenceError(f"cannot read simulator ELF: {exc}") from exc
    if hashlib.sha256(payload).hexdigest() != artifact.sha256:
        raise CompilerEvidenceError("simulator ELF changed after corpus validation")
    span = VSIM_CONFIG_RECORD_SIZE * VSIM_CONFIG_RECORD_COUNT
    file_offset = _elf_virtual_to_file_offset(
        payload, VSIM_CONFIG_VIRTUAL_ADDRESS, span
    )
    table = payload[file_offset : file_offset + span]
    table_hash = hashlib.sha256(table).hexdigest()
    if table_hash != VSIM_CONFIG_TABLE_SHA256:
        raise CompilerEvidenceError(f"unexpected vsimConfigs digest: {table_hash}")

    records: list[VsimConfigRecord] = []
    for index in range(VSIM_CONFIG_RECORD_COUNT):
        record_bytes = table[
            index * VSIM_CONFIG_RECORD_SIZE : (index + 1) * VSIM_CONFIG_RECORD_SIZE
        ]
        name_bytes = record_bytes[:VSIM_CONFIG_NAME_BYTES]
        terminator = name_bytes.find(b"\0")
        if terminator <= 0:
            raise CompilerEvidenceError(
                f"vsimConfigs[{index}] has no bounded non-empty name"
            )
        if any(name_bytes[terminator + 1 :]):
            raise CompilerEvidenceError(
                f"vsimConfigs[{index}] has nonzero name padding"
            )
        try:
            name = name_bytes[:terminator].decode("ascii")
        except UnicodeDecodeError as exc:
            raise CompilerEvidenceError(
                f"vsimConfigs[{index}] name is not ASCII"
            ) from exc
        identity = HardwareIdentity(
            *struct.unpack_from("<5I", record_bytes, VSIM_CONFIG_NAME_BYTES)
        )
        records.append(
            VsimConfigRecord(
                index=index,
                name=name,
                identity=identity,
                sha256=hashlib.sha256(record_bytes).hexdigest(),
            )
        )

    wave_matches = [
        record for record in records if record.identity.customer_id == WAVE_CUSTOMER_ID
    ]
    if len(wave_matches) != 1:
        raise CompilerEvidenceError(
            f"expected one customer 0x{WAVE_CUSTOMER_ID:08x} row, "
            f"found {len(wave_matches)}"
        )
    wave_record = wave_matches[0]
    if (
        wave_record.index != 157
        or wave_record.name != WAVE_CONFIG_NAME
        or wave_record.identity
        != HardwareIdentity(
            0x00009000,
            0x00009001,
            0x05090001,
            0x04000000,
            WAVE_CUSTOMER_ID,
        )
        or wave_record.sha256 != WAVE_CONFIG_RECORD_SHA256
    ):
        raise CompilerEvidenceError("the unique Wave simulator record changed")
    default_record = records[DEFAULT_CONFIG_INDEX]
    if default_record.name != DEFAULT_CONFIG_NAME:
        raise CompilerEvidenceError("the simulator default record changed")

    return CatalogRecovery(
        virtual_address=VSIM_CONFIG_VIRTUAL_ADDRESS,
        file_offset=file_offset,
        record_size=VSIM_CONFIG_RECORD_SIZE,
        record_count=VSIM_CONFIG_RECORD_COUNT,
        table_sha256=table_hash,
        records=tuple(records),
        wave_record=wave_record,
        default_record=default_record,
    )


def _bounded_probe_log(value: str) -> str:
    if len(value) <= MAX_PROBE_LOG_CHARS:
        return value
    half = MAX_PROBE_LOG_CHARS // 2
    return value[:half] + "\n... [probe log truncated] ...\n" + value[-half:]


def _run_bounded_process(
    command: Sequence[str],
    *,
    timeout_seconds: int,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(command),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as exc:
        raise CompilerEvidenceError(
            f"isolated command exceeded {timeout_seconds} seconds"
        ) from exc
    except OSError as exc:
        raise CompilerEvidenceError(f"cannot execute isolated command: {exc}") from exc


def _docker_envelope(
    *,
    repository_root: Path,
    output_directory: Path,
    output_read_only: bool,
    memory: str,
) -> list[str]:
    output_mode = ",readonly" if output_read_only else ""
    return [
        "docker",
        "run",
        "--platform",
        "linux/amd64",
        "--rm",
        "--network",
        "none",
        "--read-only",
        "--pids-limit",
        "64",
        "--memory",
        memory,
        "--cpus",
        "1",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "--ulimit",
        "core=0",
        "--ulimit",
        "nofile=64:64",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=16m,mode=1777",
        "--mount",
        (f"type=bind,source={repository_root},target=/workspace,readonly"),
        "--mount",
        (f"type=bind,source={output_directory},target=/out{output_mode}"),
        PROBE_DOCKER_IMAGE,
    ]


def _classify_probe_record(record: Mapping[str, Any]) -> str:
    identity_keys = (
        "chip_model",
        "chip_revision",
        "product_id",
        "eco_id",
        "customer_id",
    )
    if record.get("name") == WAVE_CONFIG_NAME and tuple(
        record.get(key) for key in identity_keys
    ) == (
        "0x00009000",
        "0x00009001",
        "0x05090001",
        "0x04000000",
        "0x1000000e",
    ):
        return "selected-wave-database-record"
    if record.get("name") == DEFAULT_CONFIG_NAME:
        return "fell-back-to-constructor-default"
    if record.get("name") == "" and tuple(record.get(key) for key in identity_keys) == (
        "0x00009000",
        "0x00009001",
        "0x05090001",
        "0x04000000",
        "0x1000000e",
    ):
        return "accepted-database-identity-file"
    if record.get("name") == "" and tuple(record.get(key) for key in identity_keys) == (
        "0x00009000",
        "0x00009002",
        "0x05090001",
        "0x04000000",
        "0x1000000e",
    ):
        return "accepted-live-identity-file"
    if record.get("name") == "" and all(
        record.get(key) == "0x00000000" for key in identity_keys
    ):
        return "retained-zero-initial-state"
    if record.get("name") == "":
        return "retained-other-file-identity"
    return "other-observed-state"


def run_isolated_parser_probe(corpus: Corpus) -> dict[str, Any]:
    """Run fixed parser-only cases in fresh networkless amd64 containers.

    The reviewed library is never loaded by the macOS host process.  A probe
    binary is compiled once in the pinned local image, then each fixed case is
    run in a new read-only container with an empty process environment.
    """

    emulator = corpus.artifacts.get("x86-emulator-6.4.6")
    vdtproxy = corpus.artifacts.get("x86-vdtproxy-6.4.6")
    if emulator is None:
        raise CompilerEvidenceError("corpus lacks x86-emulator-6.4.6")
    if vdtproxy is None:
        raise CompilerEvidenceError("corpus lacks x86-vdtproxy-6.4.6")
    if emulator.execution_policy != "isolated-parser-only":
        raise CompilerEvidenceError(
            "emulator is not authorized for isolated parser calls"
        )
    if vdtproxy.execution_policy != "isolated-parser-only":
        raise CompilerEvidenceError(
            "vdtproxy is not authorized as an isolated parser dependency"
        )
    if not PARSER_PROBE_SOURCE.is_file():
        raise CompilerEvidenceError(
            f"missing parser probe source: {PARSER_PROBE_SOURCE}"
        )
    try:
        source_payload = PARSER_PROBE_SOURCE.read_bytes()
    except OSError as exc:
        raise CompilerEvidenceError(f"cannot read parser probe: {exc}") from exc

    inspect = _run_bounded_process(
        [
            "docker",
            "image",
            "inspect",
            "--format",
            "{{.Id}}",
            PROBE_DOCKER_IMAGE,
        ],
        timeout_seconds=15,
    )
    observed_image_id = inspect.stdout.strip()
    if inspect.returncode != 0:
        raise CompilerEvidenceError(
            "cannot inspect the isolated parser image: "
            + _bounded_probe_log(inspect.stderr)
        )
    if observed_image_id != PROBE_DOCKER_IMAGE_ID:
        raise CompilerEvidenceError(
            f"parser image ID changed; expected {PROBE_DOCKER_IMAGE_ID}, "
            f"observed {observed_image_id}"
        )

    with tempfile.TemporaryDirectory(
        prefix=".wave-poc9c-probe-", dir=corpus.repository_root
    ) as temporary_name:
        output_directory = Path(temporary_name).resolve()
        os.chmod(output_directory, 0o777)
        compile_command = _docker_envelope(
            repository_root=corpus.repository_root,
            output_directory=output_directory,
            output_read_only=False,
            memory="512m",
        )
        compile_command.extend(
            [
                "env",
                "-i",
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "g++",
                "-std=c++17",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fno-omit-frame-pointer",
                "-o",
                "/out/vsim_config_parser_probe",
                ("/workspace/waved/tools/npu-profile/vsim_config_parser_probe.cc"),
                "-ldl",
            ]
        )
        compiled = _run_bounded_process(compile_command, timeout_seconds=120)
        if compiled.returncode != 0:
            raise CompilerEvidenceError(
                "isolated parser harness compilation failed:\n"
                + _bounded_probe_log(compiled.stdout + compiled.stderr)
            )
        binary_path = output_directory / "vsim_config_parser_probe"
        try:
            binary_payload = binary_path.read_bytes()
        except OSError as exc:
            raise CompilerEvidenceError(
                f"cannot read compiled parser harness: {exc}"
            ) from exc
        dependency_path = output_directory / "libvdtproxy.so"
        try:
            shutil.copyfile(vdtproxy.resolved_path, dependency_path)
            os.chmod(dependency_path, 0o444)
            copied_dependency = dependency_path.read_bytes()
        except OSError as exc:
            raise CompilerEvidenceError(
                f"cannot stage the isolated parser dependency: {exc}"
            ) from exc
        if hashlib.sha256(copied_dependency).hexdigest() != vdtproxy.sha256:
            raise CompilerEvidenceError(
                "staged parser dependency differs from the corpus hash"
            )

        version_command = _docker_envelope(
            repository_root=corpus.repository_root,
            output_directory=output_directory,
            output_read_only=True,
            memory="256m",
        )
        version_command.extend(
            [
                "env",
                "-i",
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "g++",
                "-dumpfullversion",
            ]
        )
        version = _run_bounded_process(version_command, timeout_seconds=15)
        if version.returncode != 0:
            raise CompilerEvidenceError("cannot identify the isolated C++ compiler")

        case_documents: list[dict[str, Any]] = []
        for case_name in PROBE_CASES:
            run_command = _docker_envelope(
                repository_root=corpus.repository_root,
                output_directory=output_directory,
                output_read_only=True,
                memory="256m",
            )
            run_command.extend(
                [
                    "env",
                    "-i",
                    (
                        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:"
                        "/usr/bin:/sbin:/bin"
                    ),
                    ("LD_LIBRARY_PATH=/out"),
                    "/out/vsim_config_parser_probe",
                    (
                        "/workspace/waved/third_party/TIM-VX/prebuilt-sdk/"
                        "x86_64_linux/lib/libEmulator.so"
                    ),
                    case_name,
                ]
            )
            try:
                process = subprocess.run(
                    run_command,
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    check=False,
                    timeout=15,
                )
                timed_out = False
            except subprocess.TimeoutExpired as exc:
                process = None
                timed_out = True
                stdout = (
                    exc.stdout.decode("utf-8", errors="replace")
                    if isinstance(exc.stdout, bytes)
                    else (exc.stdout or "")
                )
                stderr = (
                    exc.stderr.decode("utf-8", errors="replace")
                    if isinstance(exc.stderr, bytes)
                    else (exc.stderr or "")
                )
            if timed_out:
                case_documents.append(
                    {
                        "case": case_name,
                        "status": "timeout",
                        "container_exit_code": None,
                        "record": None,
                        "classification": "no-result",
                        "stdout": _bounded_probe_log(stdout),
                        "stderr": _bounded_probe_log(stderr),
                    }
                )
                continue

            assert process is not None
            marker_lines = [
                line[len("PROBE_RESULT ") :]
                for line in process.stdout.splitlines()
                if line.startswith("PROBE_RESULT ")
            ]
            record: Mapping[str, Any] | None = None
            if len(marker_lines) == 1:
                try:
                    parsed = json.loads(
                        marker_lines[0],
                        object_pairs_hook=_reject_duplicate_keys,
                    )
                except (json.JSONDecodeError, ValueError):
                    parsed = None
                if isinstance(parsed, Mapping):
                    record = parsed
            status = (
                "completed"
                if process.returncode == 0 and record is not None
                else "failed"
            )
            vendor_stdout = "\n".join(
                line
                for line in process.stdout.splitlines()
                if not line.startswith("PROBE_RESULT ")
            )
            case_documents.append(
                {
                    "case": case_name,
                    "status": status,
                    "container_exit_code": process.returncode,
                    "record": dict(record) if record is not None else None,
                    "classification": (
                        _classify_probe_record(record)
                        if record is not None
                        else "no-result"
                    ),
                    "stdout": _bounded_probe_log(vendor_stdout),
                    "stderr": _bounded_probe_log(process.stderr),
                }
            )

        completed_count = sum(case["status"] == "completed" for case in case_documents)
        document = {
            "schema_version": 1,
            "probe": "StateParser fixed config/name selector matrix",
            "artifact_path": emulator.path.as_posix(),
            "artifact_sha256": emulator.sha256,
            "dependency": {
                "path": vdtproxy.path.as_posix(),
                "sha256": vdtproxy.sha256,
                "bytes": vdtproxy.byte_count,
                "needed_by": "libEmulator.so DT_NEEDED",
                "own_dependencies": ["libc.so.6"],
            },
            "harness_path": PARSER_PROBE_SOURCE.relative_to(
                corpus.repository_root
            ).as_posix(),
            "harness_sha256": hashlib.sha256(source_payload).hexdigest(),
            "harness_binary_sha256": hashlib.sha256(binary_payload).hexdigest(),
            "host_architecture": platform.machine(),
            "container": {
                "image": PROBE_DOCKER_IMAGE,
                "image_id": observed_image_id,
                "platform": "linux/amd64",
                "compiler": f"g++ {version.stdout.strip()}",
                "network": "none",
                "root_filesystem": "read-only",
                "repository_mount": "read-only",
                "fresh_container_per_case": True,
                "process_environment": (
                    "empty except fixed PATH and LD_LIBRARY_PATH=/out, which "
                    "contains only the harness and copied hash-pinned "
                    "libvdtproxy.so"
                ),
                "limits": {
                    "cpus": 1,
                    "memory": "256m",
                    "pids": 64,
                    "nofile": 64,
                    "core_bytes": 0,
                    "wall_seconds_per_case": 15,
                },
            },
            "abi_basis": {
                "fake_receiver_bytes": 0x200,
                "only_initialized_receiver_offset": "0x00f0",
                "initialized_value": DEFAULT_CONFIG_INDEX,
                "output_record_bytes": VSIM_CONFIG_RECORD_SIZE,
                "called_symbols": [
                    ("_ZN11StateParser18chooseTargetByNameEPKc"),
                    ("_ZN11StateParser24chooseTargetByConfigFileEPKc"),
                    ("_ZN11StateParser9getConfigEP17_VSimulatorConfig"),
                ],
                "forbidden_symbols": [
                    "OpenVX entrypoints",
                    "VSC entrypoints",
                    "CLC entrypoints",
                    "graph construction",
                    "compiler entrypoints",
                ],
            },
            "cases": case_documents,
            "summary": {
                "requested_cases": len(PROBE_CASES),
                "completed_cases": completed_count,
                "failed_cases": sum(
                    case["status"] == "failed" for case in case_documents
                ),
                "timed_out_cases": sum(
                    case["status"] == "timeout" for case in case_documents
                ),
                "all_cases_completed": completed_count == len(PROBE_CASES),
            },
            "non_claims": [
                "No constructor was called explicitly.",
                "No compiler, graph, OpenVX, VSC, or CLC symbol was called.",
                "No Wave device or driver was accessed.",
                (
                    "This old simulator behavior does not prove the absent "
                    "VIV_GPU_FILE consumer behavior."
                ),
            ],
        }
    return document


def parse_config_bytes(payload: bytes) -> HardwareIdentity:
    """Parse the hardened subset of the recovered vendor grammar."""

    if len(payload) > MAX_CONFIG_BYTES:
        raise ConfigGrammarError(
            f"configuration is too large ({len(payload)} > {MAX_CONFIG_BYTES})"
        )
    if b"\0" in payload:
        raise ConfigGrammarError("configuration contains NUL")
    try:
        text = payload.decode("ascii")
    except UnicodeDecodeError as exc:
        raise ConfigGrammarError("configuration must be ASCII") from exc
    for index, line in enumerate(payload.splitlines(), start=1):
        if len(line) > MAX_CONFIG_LINE_BYTES:
            raise ConfigGrammarError(
                f"configuration line {index} exceeds {MAX_CONFIG_LINE_BYTES} bytes"
            )
    return parse_config_text(text)


def parse_config_text(text: str) -> HardwareIdentity:
    """Parse exactly five ordered assignments and no trailing content."""

    if not isinstance(text, str):
        raise ConfigGrammarError("configuration text must be a string")
    try:
        payload = text.encode("ascii")
    except UnicodeEncodeError as exc:
        raise ConfigGrammarError("configuration must be ASCII") from exc
    if len(payload) > MAX_CONFIG_BYTES:
        raise ConfigGrammarError(
            f"configuration is too large ({len(payload)} > {MAX_CONFIG_BYTES})"
        )
    if "\0" in text:
        raise ConfigGrammarError("configuration contains NUL")
    if "\r" in text:
        raise ConfigGrammarError("configuration must use LF line endings")
    lines = text.splitlines()
    if len(lines) != len(CONFIG_FIELDS):
        raise ConfigGrammarError(
            f"configuration must contain exactly {len(CONFIG_FIELDS)} lines"
        )
    if not text.endswith("\n"):
        raise ConfigGrammarError("configuration must end with LF")
    values: list[int] = []
    for index, (field, line) in enumerate(
        zip(CONFIG_FIELDS, lines, strict=True), start=1
    ):
        if len(line.encode("ascii")) > MAX_CONFIG_LINE_BYTES:
            raise ConfigGrammarError(
                f"configuration line {index} exceeds {MAX_CONFIG_LINE_BYTES} bytes"
            )
        pattern = re.compile(CONFIG_LINE_RE_TEMPLATE.format(key=re.escape(field.key)))
        match = pattern.fullmatch(line)
        if match is None:
            raise ConfigGrammarError(
                f"line {index} must be the ordered {field.key!r} assignment"
            )
        value = int(match.group(1), 16)
        if value > 0xFFFFFFFF:
            raise ConfigGrammarError(
                f"line {index} value exceeds unsigned 32-bit range"
            )
        values.append(value)
    return HardwareIdentity(*values)


def parse_config_file(path: Path | str) -> HardwareIdentity:
    """Read and parse a bounded config without following vendor fallbacks."""

    config_path = Path(path)
    try:
        with config_path.open("rb") as handle:
            payload = handle.read(MAX_CONFIG_BYTES + 1)
    except OSError as exc:
        raise ConfigGrammarError(f"cannot read configuration {path}: {exc}") from exc
    return parse_config_bytes(payload)


def render_config(identity: HardwareIdentity) -> str:
    """Render the unique canonical spelling accepted by this PoC."""

    values = identity.as_tuple()
    return "".join(
        f"{field.key:<23}= 0x{value:08x};\n"
        for field, value in zip(CONFIG_FIELDS, values, strict=True)
    )


def load_wave_profile(path: Path | str = DEFAULT_WAVE_PROFILE) -> dict[str, Any]:
    """Load the PoC 9A profile through its strict schema implementation."""

    try:
        return arch_profile.load_profile(Path(path))
    except arch_profile.ProfileValidationError as exc:
        raise CandidateError(f"invalid PoC 9A profile: {exc}") from exc


def _artifact_reference(artifact: CorpusArtifact) -> dict[str, Any]:
    return {
        "artifact_id": artifact.artifact_id,
        "path": artifact.path.as_posix(),
        "sha256": artifact.sha256,
        "bytes": artifact.byte_count,
        "roles": list(artifact.roles),
        "execution_policy": artifact.execution_policy,
    }


def _identity_document(identity: HardwareIdentity) -> dict[str, str]:
    return {
        field.member: f"0x{value:08x}"
        for field, value in zip(CONFIG_FIELDS, identity.as_tuple(), strict=True)
    }


def _record_document(record: VsimConfigRecord) -> dict[str, Any]:
    return {
        "index": record.index,
        "name": record.name,
        "identity": _identity_document(record.identity),
        "record_sha256": record.sha256,
    }


def validate_probe_results(
    probe_results: Mapping[str, Any],
    emulator: CorpusArtifact,
    vdtproxy: CorpusArtifact,
) -> Mapping[str, Any]:
    """Bind isolated observations to the reviewed artifacts and case matrix."""

    if probe_results.get("schema_version") != 1:
        raise CompilerEvidenceError("probe results schema_version must be 1")
    if probe_results.get("artifact_sha256") != emulator.sha256:
        raise CompilerEvidenceError(
            "probe results do not identify the reviewed emulator"
        )
    dependency = _require_mapping(
        probe_results.get("dependency"),
        "probe results dependency",
        CompilerEvidenceError,
    )
    if (
        dependency.get("sha256") != vdtproxy.sha256
        or dependency.get("path") != vdtproxy.path.as_posix()
    ):
        raise CompilerEvidenceError(
            "probe results do not identify the reviewed vdtproxy dependency"
        )
    raw_cases = probe_results.get("cases")
    if not isinstance(raw_cases, list):
        raise CompilerEvidenceError("probe results cases must be an array")
    observed_names: list[str] = []
    for index, raw_case in enumerate(raw_cases):
        case = _require_mapping(
            raw_case, f"probe results cases[{index}]", CompilerEvidenceError
        )
        case_name = case.get("case")
        if not isinstance(case_name, str):
            raise CompilerEvidenceError(
                f"probe results cases[{index}] has no case name"
            )
        observed_names.append(case_name)
        if case.get("status") not in {"completed", "failed", "timeout"}:
            raise CompilerEvidenceError(
                f"probe results case {case_name} has invalid status"
            )
        if not isinstance(case.get("classification"), str):
            raise CompilerEvidenceError(
                f"probe results case {case_name} has no classification"
            )
    if tuple(observed_names) != PROBE_CASES:
        raise CompilerEvidenceError(
            "probe result cases differ from the reviewed fixed matrix"
        )
    summary = _require_mapping(
        probe_results.get("summary"),
        "probe results summary",
        CompilerEvidenceError,
    )
    completed = sum(case.get("status") == "completed" for case in raw_cases)
    failed = sum(case.get("status") == "failed" for case in raw_cases)
    timed_out = sum(case.get("status") == "timeout" for case in raw_cases)
    expected_summary = {
        "requested_cases": len(PROBE_CASES),
        "completed_cases": completed,
        "failed_cases": failed,
        "timed_out_cases": timed_out,
        "all_cases_completed": completed == len(PROBE_CASES),
    }
    for key, expected in expected_summary.items():
        if summary.get(key) != expected:
            raise CompilerEvidenceError(f"probe results summary.{key} is inconsistent")
    return probe_results


def build_grammar_document(
    corpus: Corpus,
    target_config: ghidra_abi.GhidraConfig,
    evidence: Mapping[str, Any],
    catalog: CatalogRecovery,
    *,
    probe_results: Mapping[str, Any] | None,
) -> dict[str, Any]:
    """Build the deterministic grammar and discovery evidence document."""

    facts = recover_parser_facts(evidence)
    emulator = corpus.artifacts.get("x86-emulator-6.4.6")
    vdtproxy = corpus.artifacts.get("x86-vdtproxy-6.4.6")
    platform_readme = corpus.artifacts.get("timvx-platform-readme")
    multi_readme = corpus.artifacts.get("timvx-multi-device-readme")
    if (
        emulator is None
        or vdtproxy is None
        or platform_readme is None
        or multi_readme is None
    ):
        raise CompilerEvidenceError(
            "corpus lacks emulator, its parser dependency, or TIM-VX "
            "VIV_GPU_FILE documentation"
        )
    if emulator.sha256 != target_config.targets[EXPECTED_TARGET_ID].sha256:
        raise CompilerEvidenceError("corpus and Ghidra target disagree on the emulator")
    evidence_hash = hashlib.sha256(canonical_json(evidence)).hexdigest()

    field_documents = []
    for field in facts.fields:
        field_documents.append(
            {
                "key": field.key,
                "member": field.member,
                "type": "uint32",
                "record_offset": f"0x{field.offset:02x}",
                "vendor_scanf_format": field.scanf_format,
                "required": True,
                "order": len(field_documents),
                "evidence_class": "static-verified",
            }
        )

    isolated_probe: dict[str, Any]
    if probe_results is None:
        isolated_probe = {
            "status": "not-run",
            "evidence_class": "unknown",
            "reason": (
                "No isolated probe result was supplied; the static parser "
                "contract remains sufficient for strict host rendering."
            ),
        }
    else:
        probe_root = _require_mapping(
            probe_results, "probe results", CompilerEvidenceError
        )
        validate_probe_results(probe_root, emulator, vdtproxy)
        raw_cases = probe_root["cases"]
        isolated_probe = {
            "status": (
                "completed"
                if probe_root["summary"]["all_cases_completed"]
                else "partial"
            ),
            "evidence_class": "isolated-observed",
            "results_sha256": hashlib.sha256(
                canonical_json(dict(probe_root))
            ).hexdigest(),
            "case_count": len(raw_cases),
            "summary": probe_root.get("summary"),
            "observations": [
                {
                    "case": case["case"],
                    "status": case["status"],
                    "classification": case["classification"],
                }
                for case in raw_cases
            ],
        }

    document = {
        "schema_version": 1,
        "grammar_id": "vsimulator-five-identity-fields-v1",
        "scope": {
            "proven_parser": "x86 TIM-VX simulator 6.4.6 libEmulator.so",
            "consumer_status": "unverified-missing-no-kernel-sdk",
            "candidate_status": "parser-shaped-not-compiler-qualified",
            "evidence_boundary": (
                "The local SDK proves VSIMULATOR_CONFIG file parsing. It "
                "does not contain the no-kernel >=6.4.22 VIV_GPU_FILE "
                "consumer named by the TIM-VX platform sample."
            ),
        },
        "inputs": {
            "corpus_manifest": {
                "path": corpus.source_path.relative_to(
                    corpus.repository_root
                ).as_posix(),
                "sha256": corpus.source_sha256,
            },
            "emulator": _artifact_reference(emulator),
            "emulator_dependency": _artifact_reference(vdtproxy),
            "platform_sample": _artifact_reference(platform_readme),
            "multi_device_sample": _artifact_reference(multi_readme),
            "ghidra_target_config": {
                "path": target_config.source_path.relative_to(
                    target_config.repository_root
                ).as_posix(),
                "sha256": target_config.source_sha256,
            },
            "ghidra_evidence": {
                "path": "docs/npu/profile/compiler-profile-ghidra-evidence.json",
                "sha256": evidence_hash,
                "tool": evidence["targets"][0]["analysis"]["tool"],
            },
        },
        "discovery": {
            "viv_gpu_file": {
                "variable": "VIV_GPU_FILE",
                "status": "source-documented-consumer-absent",
                "sample_value": ("/path/to/VIP9000NANOQ_PLUS_PID0X100000XX.config"),
                "minimum_sdk": "Vivante SDK 6.4.22",
                "required_sdk_flavor": "dedicated no-kernel SDK",
                "evidence_class": "source-documented",
            },
            "vsimulator_config": {
                "variable": "VSIMULATOR_CONFIG",
                "owner": "libEmulator.so DebugServer constructor",
                "decimal_prefix": "select built-in ID",
                "name_prefixes": ["VIP", "GC", "CC"],
                "other_value": "configuration file path",
                "evidence_class": "static-verified",
            },
        },
        "vendor_parser": {
            "function": facts.function,
            "file_mode": facts.file_mode,
            "fields": field_documents,
            "record_layout": {
                "record_bytes": facts.record_size,
                "name_offset": "0x00",
                "name_bytes": facts.name_bytes,
                "identity_offset": "0x50",
                "identity_bytes": 20,
            },
            "query_key_order": list(facts.query_key_order),
            "success_test": facts.vendor_success_test,
            "fallbacks": {
                "missing_file": "retain current/default configuration",
                "unknown_name": "select constructor default",
                "unknown_identity": (
                    "gcQueryFeatureDB logs failure; selector has no status"
                ),
            },
            "defects": [
                {
                    "id": "nonzero-fscanf-result-treated-as-success",
                    "effect": (
                        "EOF (-1) is accepted, so empty or truncated input "
                        "can retain prior/default field values."
                    ),
                    "evidence_class": "static-verified",
                },
                {
                    "id": "trailing-content-unchecked",
                    "effect": ("The parser does not test EOF after the fifth field."),
                    "evidence_class": "static-verified",
                },
                {
                    "id": "void-selector-no-failure-status",
                    "effect": (
                        "Callers cannot distinguish fallback from a selected "
                        "configuration using the return value."
                    ),
                    "evidence_class": "static-verified",
                },
            ],
        },
        "strict_host_contract": {
            "encoding": "ASCII",
            "line_endings": "LF",
            "required_final_lf": True,
            "max_file_bytes": MAX_CONFIG_BYTES,
            "max_line_bytes": MAX_CONFIG_LINE_BYTES,
            "field_order": [field.key for field in CONFIG_FIELDS],
            "integer_lexeme": "(?:0[xX])?[0-9a-fA-F]{1,8}",
            "integer_semantics": "unsigned hexadecimal uint32",
            "unknown_fields": "reject",
            "duplicate_fields": "reject",
            "comments": "reject",
            "blank_lines": "reject",
            "trailing_content": "reject",
            "canonical_rendering": (
                "<key padded to 23 columns>= 0x<eight lowercase hex digits>;"
            ),
        },
        "catalog": {
            "symbol": "vsimConfigs",
            "virtual_address": f"0x{catalog.virtual_address:08x}",
            "file_offset": f"0x{catalog.file_offset:08x}",
            "record_size": catalog.record_size,
            "record_count": catalog.record_count,
            "table_sha256": catalog.table_sha256,
            "wave_record": _record_document(catalog.wave_record),
            "default_record": _record_document(catalog.default_record),
        },
        "isolated_probe": isolated_probe,
        "non_claims": sorted(
            [
                "No compiler entrypoint was called.",
                "No graph was built, compiled, or executed.",
                "No Wave device, ADB endpoint, or /dev/galcore node was touched.",
                (
                    "The built-in simulator name does not prove the filename "
                    "required by the absent no-kernel SDK."
                ),
                (
                    "The simulator parser grammar is not asserted to be the "
                    "VIV_GPU_FILE grammar of SDK 6.4.22 or newer."
                ),
            ]
        ),
    }
    return document


def _require_emittable_fact(
    profile: Mapping[str, Any], fact_name: str
) -> tuple[int, Mapping[str, Any]]:
    identity = _require_mapping(
        profile.get("identity"), "profile.identity", CandidateError
    )
    fact = _require_mapping(
        identity.get(fact_name),
        f"profile.identity.{fact_name}",
        CandidateError,
    )
    value = fact.get("value")
    if (
        not isinstance(value, int)
        or isinstance(value, bool)
        or not 0 <= value <= 0xFFFFFFFF
    ):
        raise CandidateError(f"profile identity fact {fact_name} is not a known uint32")
    if fact.get("evidence_class") != "observed-live":
        raise CandidateError(f"profile identity fact {fact_name} is not observed-live")
    if fact.get("confidence") != "high":
        raise CandidateError(
            f"profile identity fact {fact_name} is not high confidence"
        )
    if fact.get("validation_state") not in {"single-source", "corroborated"}:
        raise CandidateError(f"profile identity fact {fact_name} is not validated")
    sources = fact.get("sources")
    if not isinstance(sources, list) or not sources:
        raise CandidateError(f"profile identity fact {fact_name} has no provenance")
    return value, fact


def build_candidate_bundle(
    profile: Mapping[str, Any],
    profile_path: Path | str,
    grammar: Mapping[str, Any],
    catalog: CatalogRecovery,
    corpus: Corpus,
) -> tuple[str, dict[str, Any]]:
    """Render a Wave candidate while preserving the compiler refusal gate."""

    scope = _require_mapping(grammar.get("scope"), "grammar.scope", CandidateError)
    if scope.get("consumer_status") != "unverified-missing-no-kernel-sdk":
        raise CandidateError("grammar consumer status changed without review")
    if scope.get("candidate_status") != "parser-shaped-not-compiler-qualified":
        raise CandidateError("grammar candidate status changed without review")

    profile_artifact = corpus.artifacts.get("wave-profile-poc9a")
    if profile_artifact is None:
        raise CandidateError("corpus does not contain the PoC 9A profile")
    resolved_profile = Path(profile_path).resolve()
    if resolved_profile != profile_artifact.resolved_path:
        raise CandidateError("candidate profile path is not the hash-pinned input")
    try:
        profile_payload = resolved_profile.read_bytes()
    except OSError as exc:
        raise CandidateError(f"cannot hash the PoC 9A profile: {exc}") from exc
    profile_hash = hashlib.sha256(profile_payload).hexdigest()
    if profile_hash != profile_artifact.sha256:
        raise CandidateError("PoC 9A profile differs from the corpus hash")

    field_to_profile_fact = {
        "chip_model": "chip_model",
        "chip_revision": "chip_revision_observed",
        "product_id": "product_id",
        "eco_id": "eco_id",
        "customer_id": "customer_id",
    }
    values: list[int] = []
    emitted_fields: list[dict[str, Any]] = []
    for field in CONFIG_FIELDS:
        profile_fact = field_to_profile_fact[field.member]
        value, fact = _require_emittable_fact(profile, profile_fact)
        values.append(value)
        emitted_fields.append(
            {
                "config_key": field.key,
                "profile_fact": f"identity.{profile_fact}",
                "value": value,
                "hex": f"0x{value:08x}",
                "evidence_class": fact["evidence_class"],
                "confidence": fact["confidence"],
                "validation_state": fact["validation_state"],
                "sources": fact["sources"],
            }
        )
    identity = HardwareIdentity(*values)
    candidate = render_config(identity)
    if parse_config_text(candidate) != identity:
        raise CandidateError("internal candidate round trip failed")
    if identity.customer_id != catalog.wave_record.identity.customer_id:
        raise CandidateError("live customer ID differs from the simulator row")
    if (
        identity.chip_model,
        identity.product_id,
        identity.eco_id,
        identity.customer_id,
    ) != (
        catalog.wave_record.identity.chip_model,
        catalog.wave_record.identity.product_id,
        catalog.wave_record.identity.eco_id,
        catalog.wave_record.identity.customer_id,
    ):
        raise CandidateError(
            "live identity differs from the unique simulator row outside revision"
        )

    candidate_payload = candidate.encode("ascii")
    grammar_hash = hashlib.sha256(canonical_json(dict(grammar))).hexdigest()
    manifest = {
        "schema_version": 1,
        "decision": "parser-shaped-candidate-not-compiler-qualified",
        "consumer_status": "unverified-missing-no-kernel-sdk",
        "candidate": {
            "path": ("docs/npu/profile/wave-viv-gpu-file.candidate.config"),
            "sha256": hashlib.sha256(candidate_payload).hexdigest(),
            "bytes": len(candidate_payload),
            "identity": _identity_document(identity),
        },
        "grammar": {
            "path": "docs/npu/profile/viv-gpu-file-grammar.json",
            "sha256": grammar_hash,
            "scope": "x86 simulator parser; not qualified for VIV_GPU_FILE",
        },
        "wave_profile": {
            "path": profile_artifact.path.as_posix(),
            "sha256": profile_hash,
            "profile_id": profile.get("profile_id"),
        },
        "emitted_fields": emitted_fields,
        "simulator_catalog": {
            "built_in_wave_record": _record_document(catalog.wave_record),
            "live_revision": f"0x{identity.chip_revision:08x}",
            "database_revision": (
                f"0x{catalog.wave_record.identity.chip_revision:08x}"
            ),
            "revision_relation": (
                "PoC 9A proves masked revision-family fallback; the candidate "
                "uses the observed live revision and does not overwrite it "
                "with the older database row."
            ),
        },
        "authorization": {
            "strict_host_parse_and_render": True,
            "isolated_x86_parser_probe": True,
            "compiler_execution": False,
            "graph_construction": False,
            "device_execution": False,
        },
        "qualification_blockers": [
            (
                "Acquire and hash-pin the dedicated no-kernel Vivante SDK "
                "6.4.22 or newer named by the TIM-VX platform sample."
            ),
            (
                "Recover and test the actual VIV_GPU_FILE consumer rather "
                "than transferring simulator behavior by assumption."
            ),
            (
                "Review a separate PoC 9D plan before any fixed-graph "
                "compiler entrypoint is called."
            ),
        ],
        "non_claims": sorted(
            [
                "This file is not a compiler-qualified Acuity profile.",
                "This manifest does not authorize loading libCLC.so.",
                "This manifest does not authorize NPU or Wave execution.",
                (
                    "The filename is intentionally marked .candidate and is "
                    "not a proven vendor target filename."
                ),
            ]
        ),
    }
    return candidate, manifest


def _atomic_write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def _load_optional_probe(path: Path | None) -> Mapping[str, Any] | None:
    if path is None or not path.is_file():
        return None
    document = _load_strict_json(path, CompilerEvidenceError)
    return _require_mapping(document, "probe results", CompilerEvidenceError)


def _recover_all(
    *,
    corpus_path: Path,
    target_path: Path,
    ghidra_path: Path,
    repository_root: Path,
    probe_path: Path | None,
) -> tuple[
    Corpus,
    ghidra_abi.GhidraConfig,
    dict[str, Any],
    CatalogRecovery,
    dict[str, Any],
]:
    corpus = load_corpus(corpus_path, repository_root=repository_root)
    targets = load_compiler_target_config(target_path, repository_root=repository_root)
    evidence = load_compiler_ghidra_evidence(ghidra_path, targets)
    catalog = recover_vsim_catalog(corpus.artifacts["x86-emulator-6.4.6"])
    grammar = build_grammar_document(
        corpus,
        targets,
        evidence,
        catalog,
        probe_results=_load_optional_probe(probe_path),
    )
    return corpus, targets, evidence, catalog, grammar


def _command_validate_targets(args: argparse.Namespace) -> int:
    corpus = load_corpus(args.corpus, repository_root=args.repository_root)
    load_compiler_target_config(args.targets, repository_root=args.repository_root)
    print(f"validated {len(corpus.artifacts)} inert artifacts and one Ghidra target")
    return 0


def _command_generate_ghidra(args: argparse.Namespace) -> int:
    config = load_compiler_target_config(
        args.targets, repository_root=args.repository_root
    )
    try:
        evidence = ghidra_abi.build_evidence(config)
    except ghidra_abi.GhidraAbiError as exc:
        raise CompilerEvidenceError(str(exc)) from exc
    _atomic_write(args.output, canonical_json(evidence))
    print(f"wrote {args.output}")
    return 0


def _command_probe_vendor_parser(args: argparse.Namespace) -> int:
    corpus = load_corpus(args.corpus, repository_root=args.repository_root)
    results = run_isolated_parser_probe(corpus)
    _atomic_write(args.output, canonical_json(results))
    summary = results["summary"]
    print(
        f"wrote {args.output}: {summary['completed_cases']}/"
        f"{summary['requested_cases']} cases completed"
    )
    return 0


def _command_recover_grammar(args: argparse.Namespace) -> int:
    _, _, _, _, grammar = _recover_all(
        corpus_path=args.corpus,
        target_path=args.targets,
        ghidra_path=args.ghidra,
        repository_root=args.repository_root,
        probe_path=args.probe_results,
    )
    _atomic_write(args.output, canonical_json(grammar))
    print(f"wrote {args.output}")
    return 0


def _command_generate_candidate(args: argparse.Namespace) -> int:
    corpus, _, _, catalog, grammar = _recover_all(
        corpus_path=args.corpus,
        target_path=args.targets,
        ghidra_path=args.ghidra,
        repository_root=args.repository_root,
        probe_path=args.probe_results,
    )
    profile = load_wave_profile(args.profile)
    candidate, manifest = build_candidate_bundle(
        profile, args.profile, grammar, catalog, corpus
    )
    _atomic_write(args.candidate, candidate.encode("ascii"))
    _atomic_write(args.manifest, canonical_json(manifest))
    print(f"wrote {args.candidate}")
    print(f"wrote {args.manifest}")
    return 0


def _command_verify_generated(args: argparse.Namespace) -> int:
    corpus, _, _, catalog, grammar = _recover_all(
        corpus_path=args.corpus,
        target_path=args.targets,
        ghidra_path=args.ghidra,
        repository_root=args.repository_root,
        probe_path=args.probe_results,
    )
    try:
        tracked_grammar = args.grammar.read_bytes()
    except OSError as exc:
        raise CompilerEvidenceError(f"cannot read tracked grammar: {exc}") from exc
    if tracked_grammar != canonical_json(grammar):
        raise CompilerEvidenceError(
            "tracked grammar differs from deterministic recovery"
        )
    profile = load_wave_profile(args.profile)
    candidate, manifest = build_candidate_bundle(
        profile, args.profile, grammar, catalog, corpus
    )
    try:
        tracked_candidate = args.candidate.read_bytes()
        tracked_manifest = args.manifest.read_bytes()
    except OSError as exc:
        raise CandidateError(f"cannot read tracked candidate outputs: {exc}") from exc
    if tracked_candidate != candidate.encode("ascii"):
        raise CandidateError("tracked candidate differs from deterministic rendering")
    if tracked_manifest != canonical_json(manifest):
        raise CandidateError(
            "tracked candidate manifest differs from deterministic rendering"
        )
    parse_config_bytes(tracked_candidate)
    print("generated compiler-profile evidence is current")
    return 0


def _add_common_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--targets", type=Path, default=DEFAULT_TARGET_CONFIG)
    parser.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)


def _add_recovery_arguments(parser: argparse.ArgumentParser) -> None:
    _add_common_arguments(parser)
    parser.add_argument("--ghidra", type=Path, default=DEFAULT_GHIDRA_EVIDENCE)
    parser.add_argument("--probe-results", type=Path, default=DEFAULT_PROBE_RESULTS)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Host-only Wave compiler-profile grammar recovery"
    )
    parser.set_defaults(handler=None)
    subparsers = parser.add_subparsers(dest="command")

    validate = subparsers.add_parser(
        "validate-targets", help="validate the inert corpus and Ghidra target"
    )
    _add_common_arguments(validate)
    validate.set_defaults(handler=_command_validate_targets)

    generate_ghidra = subparsers.add_parser(
        "generate-ghidra", help="rerun bounded static Ghidra analysis"
    )
    generate_ghidra.add_argument("--targets", type=Path, default=DEFAULT_TARGET_CONFIG)
    generate_ghidra.add_argument(
        "--repository-root", type=Path, default=REPOSITORY_ROOT
    )
    generate_ghidra.add_argument("--output", type=Path, default=DEFAULT_GHIDRA_EVIDENCE)
    generate_ghidra.set_defaults(handler=_command_generate_ghidra)

    probe = subparsers.add_parser(
        "probe-vendor-parser",
        help="run fixed parser-only cases in fresh networkless containers",
    )
    probe.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    probe.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)
    probe.add_argument("--output", type=Path, default=DEFAULT_PROBE_RESULTS)
    probe.set_defaults(handler=_command_probe_vendor_parser)

    grammar = subparsers.add_parser(
        "recover-grammar", help="recover deterministic parser evidence"
    )
    _add_recovery_arguments(grammar)
    grammar.add_argument("--output", type=Path, default=DEFAULT_GRAMMAR)
    grammar.set_defaults(handler=_command_recover_grammar)

    candidate = subparsers.add_parser(
        "generate-candidate",
        help="render a non-qualified Wave candidate and refusal manifest",
    )
    _add_recovery_arguments(candidate)
    candidate.add_argument("--profile", type=Path, default=DEFAULT_WAVE_PROFILE)
    candidate.add_argument("--candidate", type=Path, default=DEFAULT_CANDIDATE)
    candidate.add_argument("--manifest", type=Path, default=DEFAULT_CANDIDATE_MANIFEST)
    candidate.set_defaults(handler=_command_generate_candidate)

    verify = subparsers.add_parser(
        "verify-generated", help="verify all tracked PoC 9C outputs"
    )
    _add_recovery_arguments(verify)
    verify.add_argument("--profile", type=Path, default=DEFAULT_WAVE_PROFILE)
    verify.add_argument("--grammar", type=Path, default=DEFAULT_GRAMMAR)
    verify.add_argument("--candidate", type=Path, default=DEFAULT_CANDIDATE)
    verify.add_argument("--manifest", type=Path, default=DEFAULT_CANDIDATE_MANIFEST)
    verify.set_defaults(handler=_command_verify_generated)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    if args.handler is None:
        parser.print_help()
        return 2
    try:
        return int(args.handler(args))
    except CompilerProfileError as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
