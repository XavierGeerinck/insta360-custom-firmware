#!/usr/bin/env python3
"""Statically qualify acquired Wave compiler/profile artifacts.

PoC 9D's acquisition phase is deliberately inert.  This module hashes and
parses ZIP, TAR, OCI, text, JSON, and ELF inputs, but it never imports a
vendor extension, executes a compiler, registers a container image, touches
ADB, or opens a device.  A candidate may advance to a separately reviewed
compile-only harness only when every no-kernel NBG compiler gate passes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import struct
import tarfile
import tempfile
import zipfile
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from types import MappingProxyType
from typing import Any, BinaryIO

try:
    from . import compiler_profile
except ImportError:  # pragma: no cover - direct script execution
    import compiler_profile  # type: ignore


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_TARGETS = Path(__file__).with_name("compiler_acquisition_targets.json")
DEFAULT_EVIDENCE = (
    REPOSITORY_ROOT / "docs/npu/profile/compiler-acquisition-evidence.json"
)

ROOT_KEYS = frozenset(
    {
        "schema_version",
        "acquisition_id",
        "source",
        "policy",
        "archive_contract",
        "parser_contract",
        "artifacts",
    }
)
SOURCE_KEYS = frozenset(
    {
        "distribution_name",
        "documentation_url",
        "sharing_page_url",
        "direct_download_url",
        "retrieved_date",
        "http_content_length",
        "http_last_modified",
        "http_etag",
    }
)
POLICY_KEYS = frozenset(
    {
        "minimum_no_kernel_sdk",
        "wave_target_name",
        "wave_target_id",
        "required_profile_environment",
        "forbidden_actions",
    }
)
ARCHIVE_CONTRACT_KEYS = frozenset(
    {
        "outer_member",
        "outer_member_crc32",
        "inner_member",
        "inner_member_crc32",
        "image_reference",
        "image_created",
        "image_architecture",
        "image_os",
        "oci_manifest_digest",
        "oci_config_digest",
        "layer_count",
    }
)
PARSER_CONTRACT_KEYS = frozenset(
    {
        "artifact",
        "role",
        "sdk_version",
        "function_virtual_address",
        "function_file_offset",
        "function_byte_count",
        "function_sha256",
        "database_lookup_virtual_address",
        "call_sites",
        "fields",
        "required_strings",
    }
)
PARSER_FIELD_KEYS = frozenset(
    {
        "key",
        "file_offset",
        "format",
        "destination_virtual_address",
    }
)
ARTIFACT_KEYS = frozenset(
    {"path", "byte_count", "sha256", "kind", "roles", "execution_policy"}
)
ARTIFACT_KINDS = frozenset({"elf", "json", "tar", "text", "tgz", "zip"})
EXECUTION_POLICIES = frozenset(
    {
        "archive-only",
        "not-executable",
        "static-only-never-execute",
        "static-only-never-import",
    }
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
CRC32_RE = re.compile(r"^[0-9a-f]{8}$")
HEX_RE = re.compile(r"^0x[0-9a-fA-F]+$")
VERSION_RE = re.compile(r"^([0-9]+)\.([0-9]+)\.([0-9]+)(?::([0-9]+))?$")
DATE_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")

WAVE_DATABASE_IDENTITY = compiler_profile.HardwareIdentity(
    chip_model=0x00009000,
    chip_revision=0x00009001,
    product_id=0x05090001,
    eco_id=0x04000000,
    customer_id=0x1000000E,
)
WAVE_LIVE_REVISION = 0x00009002
ELF64_HEADER_BYTES = 64
ELF64_PROGRAM_HEADER_BYTES = 56
PT_DYNAMIC = 2
PT_INTERP = 3
EM_X86_64 = 62
LOAD_BIAS = 0x00400000
MAX_JSON_BYTES = 1 << 20
MAX_TEXT_BYTES = 1 << 20
MAX_ARCHIVE_NAME_BYTES = 4096
SCAN_CHUNK_BYTES = 1 << 20


class CompilerAcquisitionError(RuntimeError):
    """Base failure for inert compiler acquisition and qualification."""


class TargetConfigError(CompilerAcquisitionError):
    """The acquisition target manifest is malformed or stale."""


class ArchiveSafetyError(CompilerAcquisitionError):
    """An archive has an unsafe, ambiguous, or inconsistent structure."""


class StaticEvidenceError(CompilerAcquisitionError):
    """A pinned static artifact disagrees with the recovered contract."""


@dataclass(frozen=True)
class Artifact:
    """One hash-pinned, inert acquisition artifact."""

    artifact_id: str
    path: Path
    resolved_path: Path
    byte_count: int
    sha256: str
    kind: str
    roles: tuple[str, ...]
    execution_policy: str


@dataclass(frozen=True)
class Targets:
    """Validated acquisition configuration and its artifacts."""

    repository_root: Path
    source_path: Path
    source_sha256: str
    acquisition_id: str
    source: Mapping[str, Any]
    policy: Mapping[str, Any]
    archive_contract: Mapping[str, Any]
    parser_contract: Mapping[str, Any]
    artifacts: Mapping[str, Artifact]


def canonical_json(value: Any) -> bytes:
    """Serialize evidence deterministically."""

    return (
        json.dumps(
            value,
            indent=2,
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ": "),
        )
        + "\n"
    ).encode("utf-8")


def _reject_duplicate_keys(pairs: Sequence[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_strict_json_bytes(
    payload: bytes, location: str, error_type: type[CompilerAcquisitionError]
) -> Any:
    if len(payload) > MAX_JSON_BYTES:
        raise error_type(f"{location} exceeds the bounded JSON size")
    try:
        return json.loads(
            payload.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys
        )
    except (UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise error_type(f"cannot load {location}: {exc}") from exc


def _load_strict_json(
    path: Path, error_type: type[CompilerAcquisitionError]
) -> Any:
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise error_type(f"cannot read {path}: {exc}") from exc
    return _load_strict_json_bytes(payload, str(path), error_type)


def _require_mapping(
    value: Any, location: str, error_type: type[CompilerAcquisitionError]
) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise error_type(f"{location} must be an object")
    return value


def _require_exact_keys(
    value: Mapping[str, Any],
    expected: frozenset[str],
    location: str,
    error_type: type[CompilerAcquisitionError],
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
    value: Any, location: str, error_type: type[CompilerAcquisitionError]
) -> str:
    if not isinstance(value, str) or not value:
        raise error_type(f"{location} must be a non-empty string")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in value):
        raise error_type(f"{location} contains a control character")
    return value


def _require_literal(
    value: Any, location: str, error_type: type[CompilerAcquisitionError]
) -> str:
    """Validate an extracted string literal while allowing an embedded LF."""

    if not isinstance(value, str) or not value:
        raise error_type(f"{location} must be a non-empty string")
    for character in value:
        codepoint = ord(character)
        if character == "\n":
            continue
        if codepoint < 0x20 or codepoint == 0x7F:
            raise error_type(f"{location} contains a forbidden control character")
    return value


def _require_positive_int(
    value: Any, location: str, error_type: type[CompilerAcquisitionError]
) -> int:
    if (
        not isinstance(value, int)
        or isinstance(value, bool)
        or value <= 0
    ):
        raise error_type(f"{location} must be a positive integer")
    return value


def _require_string_array(
    value: Any,
    location: str,
    error_type: type[CompilerAcquisitionError],
) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise error_type(f"{location} must be a non-empty array")
    result = tuple(
        _require_string(item, f"{location}[{index}]", error_type)
        for index, item in enumerate(value)
    )
    if len(result) != len(set(result)):
        raise error_type(f"{location} contains duplicate values")
    return result


def _contained_path(root: Path, relative: Path, location: str) -> Path:
    root_resolved = root.resolve()
    resolved = (root_resolved / relative).resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise TargetConfigError(f"{location} resolves outside the repository") from exc
    return resolved


def sha256_stream(handle: BinaryIO) -> str:
    digest = hashlib.sha256()
    while True:
        chunk = handle.read(SCAN_CHUNK_BYTES)
        if not chunk:
            return digest.hexdigest()
        digest.update(chunk)


def sha256_file(path: Path) -> str:
    try:
        with path.open("rb") as handle:
            return sha256_stream(handle)
    except OSError as exc:
        raise TargetConfigError(f"cannot hash {path}: {exc}") from exc


def _validate_artifact_kind(artifact_id: str, path: Path, kind: str) -> None:
    try:
        if kind == "elf":
            with path.open("rb") as handle:
                if handle.read(4) != b"\x7fELF":
                    raise TargetConfigError(
                        f"{artifact_id}: configured ELF is not ELF"
                    )
        elif kind == "text":
            if path.stat().st_size > MAX_TEXT_BYTES:
                raise TargetConfigError(
                    f"{artifact_id}: text artifact exceeds the bounded size"
                )
            path.read_text(encoding="utf-8")
        elif kind == "json":
            _load_strict_json(path, TargetConfigError)
        elif kind == "zip":
            if not zipfile.is_zipfile(path):
                raise TargetConfigError(
                    f"{artifact_id}: configured ZIP is not a ZIP archive"
                )
        elif kind in {"tar", "tgz"}:
            if not tarfile.is_tarfile(path):
                raise TargetConfigError(
                    f"{artifact_id}: configured TAR is not a TAR archive"
                )
    except (OSError, UnicodeError) as exc:
        raise TargetConfigError(
            f"{artifact_id}: cannot validate {kind} artifact: {exc}"
        ) from exc


def load_targets(
    path: Path | str = DEFAULT_TARGETS,
    *,
    repository_root: Path | str = REPOSITORY_ROOT,
) -> Targets:
    """Load and hash every acquisition input without executing any of it."""

    source_path = Path(path)
    root = Path(repository_root).resolve()
    try:
        source_payload = source_path.read_bytes()
    except OSError as exc:
        raise TargetConfigError(f"cannot read targets {source_path}: {exc}") from exc
    document = _load_strict_json_bytes(
        source_payload, str(source_path), TargetConfigError
    )
    root_object = _require_mapping(document, "targets", TargetConfigError)
    _require_exact_keys(root_object, ROOT_KEYS, "targets", TargetConfigError)
    if root_object["schema_version"] != 1:
        raise TargetConfigError("targets.schema_version must be 1")

    acquisition_id = _require_string(
        root_object["acquisition_id"], "targets.acquisition_id", TargetConfigError
    )
    if ID_RE.fullmatch(acquisition_id) is None:
        raise TargetConfigError("targets.acquisition_id has an invalid form")

    source = _require_mapping(root_object["source"], "targets.source", TargetConfigError)
    _require_exact_keys(source, SOURCE_KEYS, "targets.source", TargetConfigError)
    for key in SOURCE_KEYS - {"http_content_length"}:
        _require_string(source[key], f"targets.source.{key}", TargetConfigError)
    if DATE_RE.fullmatch(str(source["retrieved_date"])) is None:
        raise TargetConfigError("targets.source.retrieved_date must be YYYY-MM-DD")
    _require_positive_int(
        source["http_content_length"],
        "targets.source.http_content_length",
        TargetConfigError,
    )

    policy = _require_mapping(root_object["policy"], "targets.policy", TargetConfigError)
    _require_exact_keys(policy, POLICY_KEYS, "targets.policy", TargetConfigError)
    for key in POLICY_KEYS - {"forbidden_actions"}:
        _require_string(policy[key], f"targets.policy.{key}", TargetConfigError)
    forbidden_actions = _require_string_array(
        policy["forbidden_actions"],
        "targets.policy.forbidden_actions",
        TargetConfigError,
    )
    if "compiler-execution" not in forbidden_actions or "device-access" not in forbidden_actions:
        raise TargetConfigError(
            "targets.policy must forbid compiler execution and device access"
        )

    archive_contract = _require_mapping(
        root_object["archive_contract"],
        "targets.archive_contract",
        TargetConfigError,
    )
    _require_exact_keys(
        archive_contract,
        ARCHIVE_CONTRACT_KEYS,
        "targets.archive_contract",
        TargetConfigError,
    )
    for key in ARCHIVE_CONTRACT_KEYS - {"layer_count"}:
        _require_string(
            archive_contract[key],
            f"targets.archive_contract.{key}",
            TargetConfigError,
        )
    for key in ("outer_member_crc32", "inner_member_crc32"):
        if CRC32_RE.fullmatch(str(archive_contract[key])) is None:
            raise TargetConfigError(
                f"targets.archive_contract.{key} must be lowercase CRC-32"
            )
    _require_positive_int(
        archive_contract["layer_count"],
        "targets.archive_contract.layer_count",
        TargetConfigError,
    )

    parser_contract = _require_mapping(
        root_object["parser_contract"],
        "targets.parser_contract",
        TargetConfigError,
    )
    _require_exact_keys(
        parser_contract,
        PARSER_CONTRACT_KEYS,
        "targets.parser_contract",
        TargetConfigError,
    )
    for key in (
        "artifact",
        "role",
        "sdk_version",
        "function_virtual_address",
        "function_file_offset",
        "function_sha256",
        "database_lookup_virtual_address",
    ):
        _require_string(
            parser_contract[key],
            f"targets.parser_contract.{key}",
            TargetConfigError,
        )
    _require_positive_int(
        parser_contract["function_byte_count"],
        "targets.parser_contract.function_byte_count",
        TargetConfigError,
    )
    if SHA256_RE.fullmatch(str(parser_contract["function_sha256"])) is None:
        raise TargetConfigError(
            "targets.parser_contract.function_sha256 must be lowercase SHA-256"
        )
    for key in (
        "function_virtual_address",
        "function_file_offset",
        "database_lookup_virtual_address",
    ):
        _parse_hex(str(parser_contract[key]), f"targets.parser_contract.{key}")
    call_sites = _require_string_array(
        parser_contract["call_sites"],
        "targets.parser_contract.call_sites",
        TargetConfigError,
    )
    for index, call_site in enumerate(call_sites):
        _parse_hex(
            call_site, f"targets.parser_contract.call_sites[{index}]"
        )
    fields = parser_contract["fields"]
    if not isinstance(fields, list) or not fields:
        raise TargetConfigError("targets.parser_contract.fields must be non-empty")
    field_keys: list[str] = []
    for index, raw_field in enumerate(fields):
        location = f"targets.parser_contract.fields[{index}]"
        field = _require_mapping(raw_field, location, TargetConfigError)
        _require_exact_keys(field, PARSER_FIELD_KEYS, location, TargetConfigError)
        field_key = _require_string(field["key"], f"{location}.key", TargetConfigError)
        field_keys.append(field_key)
        _require_literal(field["format"], f"{location}.format", TargetConfigError)
        _parse_hex(
            _require_string(
                field["file_offset"], f"{location}.file_offset", TargetConfigError
            ),
            f"{location}.file_offset",
        )
        _parse_hex(
            _require_string(
                field["destination_virtual_address"],
                f"{location}.destination_virtual_address",
                TargetConfigError,
            ),
            f"{location}.destination_virtual_address",
        )
    if tuple(field_keys) != tuple(field.key for field in compiler_profile.CONFIG_FIELDS):
        raise TargetConfigError(
            "targets.parser_contract.fields must use the canonical five-field order"
        )
    raw_required_strings = parser_contract["required_strings"]
    if not isinstance(raw_required_strings, list) or not raw_required_strings:
        raise TargetConfigError(
            "targets.parser_contract.required_strings must be a non-empty array"
        )
    required_strings = tuple(
        _require_literal(
            item,
            f"targets.parser_contract.required_strings[{index}]",
            TargetConfigError,
        )
        for index, item in enumerate(raw_required_strings)
    )
    if len(required_strings) != len(set(required_strings)):
        raise TargetConfigError(
            "targets.parser_contract.required_strings contains duplicate values"
        )

    artifacts_object = _require_mapping(
        root_object["artifacts"], "targets.artifacts", TargetConfigError
    )
    if not artifacts_object:
        raise TargetConfigError("targets.artifacts must not be empty")
    artifacts: dict[str, Artifact] = {}
    for artifact_id, raw_artifact in sorted(artifacts_object.items()):
        if not isinstance(artifact_id, str) or ID_RE.fullmatch(artifact_id) is None:
            raise TargetConfigError(f"invalid artifact ID: {artifact_id!r}")
        location = f"targets.artifacts.{artifact_id}"
        artifact_object = _require_mapping(
            raw_artifact, location, TargetConfigError
        )
        _require_exact_keys(
            artifact_object, ARTIFACT_KEYS, location, TargetConfigError
        )
        path_text = _require_string(
            artifact_object["path"], f"{location}.path", TargetConfigError
        )
        relative_path = Path(path_text)
        if relative_path.is_absolute() or ".." in relative_path.parts:
            raise TargetConfigError(
                f"{location}.path must be repository-relative and contained"
            )
        resolved_path = _contained_path(root, relative_path, f"{location}.path")
        try:
            observed_bytes = resolved_path.stat().st_size
        except OSError as exc:
            raise TargetConfigError(
                f"cannot stat {artifact_id} at {resolved_path}: {exc}"
            ) from exc
        expected_bytes = _require_positive_int(
            artifact_object["byte_count"],
            f"{location}.byte_count",
            TargetConfigError,
        )
        if observed_bytes != expected_bytes:
            raise TargetConfigError(
                f"{artifact_id}: byte-count mismatch; expected {expected_bytes}, "
                f"observed {observed_bytes}"
            )
        expected_hash = _require_string(
            artifact_object["sha256"], f"{location}.sha256", TargetConfigError
        )
        if SHA256_RE.fullmatch(expected_hash) is None:
            raise TargetConfigError(f"{location}.sha256 must be lowercase SHA-256")
        observed_hash = sha256_file(resolved_path)
        if observed_hash != expected_hash:
            raise TargetConfigError(
                f"{artifact_id}: SHA-256 mismatch; expected {expected_hash}, "
                f"observed {observed_hash}"
            )
        kind = _require_string(
            artifact_object["kind"], f"{location}.kind", TargetConfigError
        )
        if kind not in ARTIFACT_KINDS:
            raise TargetConfigError(
                f"{location}.kind must be one of {sorted(ARTIFACT_KINDS)}"
            )
        _validate_artifact_kind(artifact_id, resolved_path, kind)
        roles = _require_string_array(
            artifact_object["roles"], f"{location}.roles", TargetConfigError
        )
        execution_policy = _require_string(
            artifact_object["execution_policy"],
            f"{location}.execution_policy",
            TargetConfigError,
        )
        if execution_policy not in EXECUTION_POLICIES:
            raise TargetConfigError(
                f"{location}.execution_policy must be one of "
                f"{sorted(EXECUTION_POLICIES)}"
            )
        if kind == "elf" and execution_policy not in {
            "static-only-never-execute",
            "static-only-never-import",
        }:
            raise TargetConfigError(
                f"{artifact_id}: every acquired ELF must remain static-only"
            )
        artifacts[artifact_id] = Artifact(
            artifact_id=artifact_id,
            path=relative_path,
            resolved_path=resolved_path,
            byte_count=observed_bytes,
            sha256=expected_hash,
            kind=kind,
            roles=roles,
            execution_policy=execution_policy,
        )

    required_artifacts = {
        "outer_zip",
        "inner_zip",
        "oci_tar",
        "vendor_readme",
        "oci_index",
        "docker_manifest",
        "oci_manifest",
        "oci_config",
        "cmdtools_version",
        "vsimulator_version",
        "wave_vendor_config",
        "vcompiler",
        "vccompiler",
        "acuity_wheel",
        "ovxlib_casegenerator",
        "timvx_casegenerator",
        "acuity_embedded_sdk",
    }
    if frozenset(artifacts) != frozenset(required_artifacts):
        raise TargetConfigError(
            "targets.artifacts must contain exactly the reviewed acquisition set"
        )
    if parser_contract["artifact"] not in artifacts:
        raise TargetConfigError("parser contract references an unknown artifact")

    if artifacts["outer_zip"].byte_count != source["http_content_length"]:
        raise TargetConfigError(
            "HTTP content length does not match the pinned outer ZIP"
        )

    return Targets(
        repository_root=root,
        source_path=source_path.resolve(),
        source_sha256=hashlib.sha256(source_payload).hexdigest(),
        acquisition_id=acquisition_id,
        source=MappingProxyType(dict(source)),
        policy=MappingProxyType(dict(policy)),
        archive_contract=MappingProxyType(dict(archive_contract)),
        parser_contract=MappingProxyType(dict(parser_contract)),
        artifacts=MappingProxyType(artifacts),
    )


def _parse_hex(value: str, location: str) -> int:
    if HEX_RE.fullmatch(value) is None:
        raise TargetConfigError(f"{location} must be a hexadecimal integer")
    return int(value, 16)


def _safe_archive_name(name: str, location: str) -> PurePosixPath:
    if (
        not name
        or "\x00" in name
        or "\\" in name
        or len(name.encode("utf-8")) > MAX_ARCHIVE_NAME_BYTES
        or re.match(r"^[A-Za-z]:", name)
    ):
        raise ArchiveSafetyError(f"{location}: unsafe archive member name {name!r}")
    trimmed = name[:-1] if name.endswith("/") else name
    if not trimmed:
        raise ArchiveSafetyError(f"{location}: empty archive member path")
    raw_components = trimmed.split("/")
    if any(component in {"", ".", ".."} for component in raw_components):
        raise ArchiveSafetyError(f"{location}: unsafe archive member path {name!r}")
    member_path = PurePosixPath(trimmed)
    if member_path.is_absolute():
        raise ArchiveSafetyError(f"{location}: unsafe archive member path {name!r}")
    return member_path


def inspect_zip(path: Path) -> dict[str, Any]:
    """Validate a ZIP central directory without extracting any member."""

    members: list[dict[str, Any]] = []
    names: set[str] = set()
    try:
        with zipfile.ZipFile(path) as archive:
            for index, info in enumerate(archive.infolist()):
                location = f"{path}:member[{index}]"
                _safe_archive_name(info.filename, location)
                if info.filename in names:
                    raise ArchiveSafetyError(
                        f"{path}: duplicate member {info.filename!r}"
                    )
                names.add(info.filename)
                if info.flag_bits & 0x1:
                    raise ArchiveSafetyError(
                        f"{path}: encrypted member {info.filename!r}"
                    )
                unix_mode = (info.external_attr >> 16) & 0xFFFF
                if stat.S_ISLNK(unix_mode):
                    raise ArchiveSafetyError(
                        f"{path}: symbolic-link member {info.filename!r}"
                    )
                members.append(
                    {
                        "name": info.filename,
                        "byte_count": info.file_size,
                        "compressed_byte_count": info.compress_size,
                        "compression_method": info.compress_type,
                        "crc32": f"{info.CRC:08x}",
                        "is_directory": info.is_dir(),
                    }
                )
    except (OSError, zipfile.BadZipFile, RuntimeError) as exc:
        if isinstance(exc, ArchiveSafetyError):
            raise
        raise ArchiveSafetyError(f"cannot inspect ZIP {path}: {exc}") from exc
    return {"member_count": len(members), "members": members}


def hash_zip_member(path: Path, member_name: str) -> str:
    """Hash one already safety-reviewed ZIP member without extracting it."""

    inspect_zip(path)
    try:
        with zipfile.ZipFile(path) as archive:
            try:
                info = archive.getinfo(member_name)
            except KeyError as exc:
                raise StaticEvidenceError(
                    f"{path}: missing ZIP member {member_name!r}"
                ) from exc
            if info.is_dir():
                raise StaticEvidenceError(
                    f"{path}: ZIP member {member_name!r} is a directory"
                )
            with archive.open(info, "r") as handle:
                return sha256_stream(handle)
    except (OSError, zipfile.BadZipFile, RuntimeError) as exc:
        if isinstance(exc, StaticEvidenceError):
            raise
        raise StaticEvidenceError(
            f"cannot hash ZIP member {member_name!r} in {path}: {exc}"
        ) from exc


def inspect_tar(path: Path, *, mode: str = "r:*") -> dict[str, Any]:
    """Validate TAR names and links without materializing a root filesystem."""

    members: list[dict[str, Any]] = []
    names: set[str] = set()
    try:
        with tarfile.open(path, mode) as archive:
            for index, info in enumerate(archive):
                location = f"{path}:member[{index}]"
                member_path = _safe_archive_name(info.name, location)
                if info.name in names:
                    raise ArchiveSafetyError(
                        f"{path}: duplicate member {info.name!r}"
                    )
                names.add(info.name)
                if info.ischr() or info.isblk() or info.isfifo():
                    raise ArchiveSafetyError(
                        f"{path}: special-device member {info.name!r}"
                    )
                if info.issym() or info.islnk():
                    link_name = info.linkname
                    link_path = PurePosixPath(link_name)
                    if link_path.is_absolute():
                        raise ArchiveSafetyError(
                            f"{path}: absolute link {info.name!r} -> {link_name!r}"
                        )
                    combined = member_path.parent.joinpath(link_path)
                    depth = 0
                    for component in combined.parts:
                        if component == "..":
                            depth -= 1
                        elif component not in {"", "."}:
                            depth += 1
                        if depth < 0:
                            raise ArchiveSafetyError(
                                f"{path}: escaping link {info.name!r} -> "
                                f"{link_name!r}"
                            )
                members.append(
                    {
                        "name": info.name,
                        "byte_count": info.size,
                        "type": (
                            "directory"
                            if info.isdir()
                            else "regular"
                            if info.isfile()
                            else "symlink"
                            if info.issym()
                            else "hardlink"
                            if info.islnk()
                            else "other"
                        ),
                    }
                )
    except (OSError, tarfile.TarError) as exc:
        if isinstance(exc, ArchiveSafetyError):
            raise
        raise ArchiveSafetyError(f"cannot inspect TAR {path}: {exc}") from exc
    return {"member_count": len(members), "members": members}


def _find_member(document: Mapping[str, Any], name: str, location: str) -> Mapping[str, Any]:
    matches = [
        member
        for member in document["members"]
        if isinstance(member, Mapping) and member.get("name") == name
    ]
    if len(matches) != 1:
        raise StaticEvidenceError(
            f"{location}: expected exactly one member {name!r}, found {len(matches)}"
        )
    return matches[0]


def _parse_version(version: str, location: str) -> tuple[int, int, int, int | None]:
    match = VERSION_RE.fullmatch(version)
    if match is None:
        raise StaticEvidenceError(f"{location}: invalid SDK version {version!r}")
    major, minor, patch, build = match.groups()
    return int(major), int(minor), int(patch), int(build) if build else None


def _version_meets_minimum(candidate: str, minimum: str) -> bool:
    candidate_parts = _parse_version(candidate, "candidate version")
    minimum_parts = _parse_version(minimum, "minimum version")
    return candidate_parts[:3] >= minimum_parts[:3]


def _read_bounded(path: Path, maximum: int = MAX_TEXT_BYTES) -> bytes:
    try:
        size = path.stat().st_size
        if size > maximum:
            raise StaticEvidenceError(f"{path} exceeds bounded read size")
        return path.read_bytes()
    except OSError as exc:
        raise StaticEvidenceError(f"cannot read {path}: {exc}") from exc


def _read_exact_offset(path: Path, offset: int, byte_count: int) -> bytes:
    try:
        with path.open("rb") as handle:
            handle.seek(offset)
            payload = handle.read(byte_count)
    except OSError as exc:
        raise StaticEvidenceError(f"cannot read {path} at {offset:#x}: {exc}") from exc
    if len(payload) != byte_count:
        raise StaticEvidenceError(
            f"{path}: truncated read at {offset:#x}; expected {byte_count}, "
            f"observed {len(payload)}"
        )
    return payload


def _search_file(path: Path, needles: Iterable[bytes]) -> dict[bytes, bool]:
    needle_list = tuple(dict.fromkeys(needles))
    if not needle_list or any(not needle for needle in needle_list):
        raise ValueError("needles must be non-empty byte strings")
    maximum = max(len(needle) for needle in needle_list)
    found = {needle: False for needle in needle_list}
    tail = b""
    try:
        with path.open("rb") as handle:
            while True:
                chunk = handle.read(SCAN_CHUNK_BYTES)
                if not chunk:
                    return found
                window = tail + chunk
                for needle in needle_list:
                    if not found[needle] and needle in window:
                        found[needle] = True
                if all(found.values()):
                    return found
                tail = window[-(maximum - 1) :] if maximum > 1 else b""
    except OSError as exc:
        raise StaticEvidenceError(f"cannot scan {path}: {exc}") from exc


def _archive_stream_contains(
    handle: BinaryIO, needles: tuple[bytes, ...]
) -> set[bytes]:
    maximum = max(len(needle) for needle in needles)
    found: set[bytes] = set()
    tail = b""
    while True:
        chunk = handle.read(SCAN_CHUNK_BYTES)
        if not chunk:
            return found
        window = tail + chunk
        for needle in needles:
            if needle not in found and needle in window:
                found.add(needle)
        tail = window[-(maximum - 1) :] if maximum > 1 else b""


def search_zip_members(
    path: Path, needles: Iterable[bytes]
) -> dict[bytes, tuple[str, ...]]:
    needle_list = tuple(dict.fromkeys(needles))
    hits: dict[bytes, list[str]] = {needle: [] for needle in needle_list}
    inspect_zip(path)
    try:
        with zipfile.ZipFile(path) as archive:
            for info in archive.infolist():
                if info.is_dir():
                    continue
                with archive.open(info, "r") as handle:
                    member_hits = _archive_stream_contains(handle, needle_list)
                for needle in member_hits:
                    hits[needle].append(info.filename)
    except (OSError, zipfile.BadZipFile, RuntimeError) as exc:
        raise StaticEvidenceError(f"cannot scan ZIP {path}: {exc}") from exc
    return {needle: tuple(sorted(names)) for needle, names in hits.items()}


def search_tar_members(
    path: Path, needles: Iterable[bytes]
) -> dict[bytes, tuple[str, ...]]:
    needle_list = tuple(dict.fromkeys(needles))
    hits: dict[bytes, list[str]] = {needle: [] for needle in needle_list}
    inspect_tar(path)
    try:
        with tarfile.open(path, "r:*") as archive:
            for info in archive:
                if not info.isfile():
                    continue
                handle = archive.extractfile(info)
                if handle is None:
                    raise StaticEvidenceError(
                        f"{path}: cannot stream member {info.name!r}"
                    )
                with handle:
                    member_hits = _archive_stream_contains(handle, needle_list)
                for needle in member_hits:
                    hits[needle].append(info.name)
    except (OSError, tarfile.TarError) as exc:
        raise StaticEvidenceError(f"cannot scan TAR {path}: {exc}") from exc
    return {needle: tuple(sorted(names)) for needle, names in hits.items()}


def inspect_elf64_static(path: Path) -> dict[str, Any]:
    """Prove an x86-64 ELF has neither an interpreter nor a dynamic segment."""

    header = _read_exact_offset(path, 0, ELF64_HEADER_BYTES)
    if header[:4] != b"\x7fELF" or header[4] != 2 or header[5] != 1:
        raise StaticEvidenceError(f"{path}: expected ELF64 little-endian")
    machine = struct.unpack_from("<H", header, 18)[0]
    if machine != EM_X86_64:
        raise StaticEvidenceError(f"{path}: expected x86-64 ELF machine")
    program_offset = struct.unpack_from("<Q", header, 32)[0]
    program_entry_bytes = struct.unpack_from("<H", header, 54)[0]
    program_count = struct.unpack_from("<H", header, 56)[0]
    if program_entry_bytes != ELF64_PROGRAM_HEADER_BYTES or program_count == 0:
        raise StaticEvidenceError(f"{path}: invalid ELF64 program-header geometry")
    table_bytes = program_entry_bytes * program_count
    if program_offset + table_bytes > path.stat().st_size:
        raise StaticEvidenceError(f"{path}: program headers exceed file bounds")
    table = _read_exact_offset(path, program_offset, table_bytes)
    segment_types = tuple(
        struct.unpack_from("<I", table, index * program_entry_bytes)[0]
        for index in range(program_count)
    )
    has_dynamic = PT_DYNAMIC in segment_types
    has_interpreter = PT_INTERP in segment_types
    if has_dynamic or has_interpreter:
        raise StaticEvidenceError(
            f"{path}: reviewed standalone compiler is not statically linked"
        )
    return {
        "class": "ELF64",
        "endianness": "little",
        "machine": "x86-64",
        "program_header_count": program_count,
        "has_pt_dynamic": has_dynamic,
        "has_pt_interp": has_interpreter,
        "dynamic_dependency_count": 0,
    }


def _decode_relative_call_target(path: Path, virtual_address: int) -> int:
    file_offset = virtual_address - LOAD_BIAS
    instruction = _read_exact_offset(path, file_offset, 5)
    if instruction[0] != 0xE8:
        raise StaticEvidenceError(
            f"{path}: expected relative CALL at {virtual_address:#x}"
        )
    displacement = struct.unpack_from("<i", instruction, 1)[0]
    return virtual_address + 5 + displacement


def recover_parser_contract(targets: Targets) -> dict[str, Any]:
    contract = targets.parser_contract
    artifact = targets.artifacts[str(contract["artifact"])]
    path = artifact.resolved_path
    elf = inspect_elf64_static(path)

    function_offset = _parse_hex(
        str(contract["function_file_offset"]), "parser function file offset"
    )
    function_virtual_address = _parse_hex(
        str(contract["function_virtual_address"]), "parser function virtual address"
    )
    if function_virtual_address - LOAD_BIAS != function_offset:
        raise StaticEvidenceError(
            "parser function virtual address/file offset disagree with ELF load bias"
        )
    function_bytes = _read_exact_offset(
        path, function_offset, int(contract["function_byte_count"])
    )
    function_hash = hashlib.sha256(function_bytes).hexdigest()
    if function_hash != contract["function_sha256"]:
        raise StaticEvidenceError(
            "vcompiler parser-function bytes disagree with the pinned hash"
        )

    required_strings = tuple(
        value.encode("utf-8") for value in contract["required_strings"]
    )
    presence = _search_file(path, required_strings)
    missing = [
        needle.decode("utf-8", errors="replace")
        for needle, found in presence.items()
        if not found
    ]
    if missing:
        raise StaticEvidenceError(f"vcompiler is missing required strings: {missing}")

    fields: list[dict[str, Any]] = []
    for raw_field in contract["fields"]:
        offset = _parse_hex(str(raw_field["file_offset"]), "parser field offset")
        expected = str(raw_field["format"]).encode("utf-8") + b"\x00"
        observed = _read_exact_offset(path, offset, len(expected))
        if observed != expected:
            raise StaticEvidenceError(
                f"vcompiler parser format mismatch for {raw_field['key']}"
            )
        fields.append(
            {
                "key": raw_field["key"],
                "file_offset": f"0x{offset:08x}",
                "format": raw_field["format"],
                "destination_virtual_address": raw_field[
                    "destination_virtual_address"
                ],
            }
        )

    call_sites: list[dict[str, str]] = []
    for raw_call_site in contract["call_sites"]:
        call_site = _parse_hex(str(raw_call_site), "parser call site")
        target = _decode_relative_call_target(path, call_site)
        if target != function_virtual_address:
            raise StaticEvidenceError(
                f"call at {call_site:#x} targets {target:#x}, not parser "
                f"{function_virtual_address:#x}"
            )
        call_sites.append(
            {
                "virtual_address": f"0x{call_site:08x}",
                "target": f"0x{target:08x}",
            }
        )

    lookup_call = 0x007A55AB
    lookup_target = _decode_relative_call_target(path, lookup_call)
    expected_lookup = _parse_hex(
        str(contract["database_lookup_virtual_address"]),
        "database lookup virtual address",
    )
    if lookup_target != expected_lookup:
        raise StaticEvidenceError(
            "vcompiler parser no longer calls the pinned feature-database lookup"
        )

    return {
        "artifact": artifact.artifact_id,
        "artifact_sha256": artifact.sha256,
        "consumer_role": contract["role"],
        "sdk_version": contract["sdk_version"],
        "profile_environment": targets.policy["required_profile_environment"],
        "elf": elf,
        "function": {
            "virtual_address": f"0x{function_virtual_address:08x}",
            "file_offset": f"0x{function_offset:08x}",
            "byte_count": contract["function_byte_count"],
            "sha256": function_hash,
        },
        "call_sites": call_sites,
        "fields": fields,
        "database_lookup": {
            "call_virtual_address": f"0x{lookup_call:08x}",
            "target_virtual_address": f"0x{lookup_target:08x}",
            "argument_order": [field["key"] for field in fields],
        },
        "required_strings_verified": sorted(
            needle.decode("utf-8") for needle in required_strings
        ),
        "execution_count": 0,
    }


def _validate_zip_contract(targets: Targets) -> dict[str, Any]:
    contract = targets.archive_contract
    outer = targets.artifacts["outer_zip"]
    inner = targets.artifacts["inner_zip"]
    oci_tar = targets.artifacts["oci_tar"]
    outer_scan = inspect_zip(outer.resolved_path)
    outer_member = _find_member(
        outer_scan, str(contract["outer_member"]), "outer ZIP"
    )
    if (
        outer_member["byte_count"] != inner.byte_count
        or outer_member["crc32"] != contract["outer_member_crc32"]
    ):
        raise StaticEvidenceError("outer ZIP member metadata disagrees with target")
    outer_member_sha256 = hash_zip_member(
        outer.resolved_path, str(contract["outer_member"])
    )
    if outer_member_sha256 != inner.sha256:
        raise StaticEvidenceError(
            "outer ZIP selected-member SHA-256 disagrees with pinned inner ZIP"
        )
    inner_scan = inspect_zip(inner.resolved_path)
    inner_member = _find_member(
        inner_scan, str(contract["inner_member"]), "inner ZIP"
    )
    if (
        inner_member["byte_count"] != oci_tar.byte_count
        or inner_member["crc32"] != contract["inner_member_crc32"]
    ):
        raise StaticEvidenceError("inner ZIP member metadata disagrees with target")
    inner_member_sha256 = hash_zip_member(
        inner.resolved_path, str(contract["inner_member"])
    )
    if inner_member_sha256 != oci_tar.sha256:
        raise StaticEvidenceError(
            "inner ZIP selected-member SHA-256 disagrees with pinned OCI TAR"
        )
    outer_member_evidence = dict(outer_member)
    outer_member_evidence["sha256"] = outer_member_sha256
    inner_member_evidence = dict(inner_member)
    inner_member_evidence["sha256"] = inner_member_sha256
    return {
        "outer_zip": {
            "artifact": outer.artifact_id,
            "member_count": outer_scan["member_count"],
            "selected_member": outer_member_evidence,
        },
        "inner_zip": {
            "artifact": inner.artifact_id,
            "member_count": inner_scan["member_count"],
            "selected_member": inner_member_evidence,
        },
    }


def _load_artifact_json(targets: Targets, artifact_id: str) -> Any:
    artifact = targets.artifacts[artifact_id]
    return _load_strict_json(artifact.resolved_path, StaticEvidenceError)


def _verify_oci_blob_digests(
    oci_tar: Path, digests: Iterable[str]
) -> tuple[str, ...]:
    expected = tuple(dict.fromkeys(digests))
    if any(
        not digest.startswith("sha256:")
        or SHA256_RE.fullmatch(digest.removeprefix("sha256:")) is None
        for digest in expected
    ):
        raise StaticEvidenceError("OCI document contains an invalid digest")
    verified: list[str] = []
    try:
        with tarfile.open(oci_tar, "r:") as archive:
            for digest in expected:
                hex_digest = digest.removeprefix("sha256:")
                member_name = f"blobs/sha256/{hex_digest}"
                try:
                    info = archive.getmember(member_name)
                except KeyError as exc:
                    raise StaticEvidenceError(
                        f"OCI archive is missing {member_name}"
                    ) from exc
                handle = archive.extractfile(info)
                if handle is None:
                    raise StaticEvidenceError(
                        f"OCI blob {member_name} is not a regular file"
                    )
                with handle:
                    observed = sha256_stream(handle)
                if observed != hex_digest:
                    raise StaticEvidenceError(
                        f"OCI blob digest mismatch for {member_name}"
                    )
                verified.append(digest)
    except (OSError, tarfile.TarError) as exc:
        if isinstance(exc, StaticEvidenceError):
            raise
        raise StaticEvidenceError(f"cannot verify OCI blobs: {exc}") from exc
    return tuple(verified)


def recover_oci_contract(targets: Targets) -> dict[str, Any]:
    contract = targets.archive_contract
    index = _require_mapping(
        _load_artifact_json(targets, "oci_index"),
        "OCI index",
        StaticEvidenceError,
    )
    manifests = index.get("manifests")
    if (
        index.get("schemaVersion") != 2
        or not isinstance(manifests, list)
        or len(manifests) != 1
        or not isinstance(manifests[0], Mapping)
    ):
        raise StaticEvidenceError("OCI index must contain one schema-2 manifest")
    index_manifest = manifests[0]
    if index_manifest.get("digest") != contract["oci_manifest_digest"]:
        raise StaticEvidenceError("OCI index manifest digest mismatch")
    annotations = index_manifest.get("annotations")
    if (
        not isinstance(annotations, Mapping)
        or annotations.get("io.containerd.image.name")
        != contract["image_reference"]
    ):
        raise StaticEvidenceError("OCI image reference mismatch")

    manifest = _require_mapping(
        _load_artifact_json(targets, "oci_manifest"),
        "OCI manifest",
        StaticEvidenceError,
    )
    config_descriptor = manifest.get("config")
    layers = manifest.get("layers")
    if (
        manifest.get("schemaVersion") != 2
        or not isinstance(config_descriptor, Mapping)
        or config_descriptor.get("digest") != contract["oci_config_digest"]
        or not isinstance(layers, list)
        or len(layers) != contract["layer_count"]
    ):
        raise StaticEvidenceError("OCI manifest geometry or config digest mismatch")
    layer_records: list[dict[str, Any]] = []
    layer_digests: list[str] = []
    for index_value, raw_layer in enumerate(layers):
        if not isinstance(raw_layer, Mapping):
            raise StaticEvidenceError(f"OCI layer {index_value} is not an object")
        digest = raw_layer.get("digest")
        media_type = raw_layer.get("mediaType")
        size = raw_layer.get("size")
        if (
            not isinstance(digest, str)
            or not isinstance(media_type, str)
            or not isinstance(size, int)
            or size <= 0
        ):
            raise StaticEvidenceError(f"OCI layer {index_value} is malformed")
        layer_digests.append(digest)
        layer_records.append(
            {
                "index": index_value,
                "digest": digest,
                "byte_count": size,
                "media_type": media_type,
            }
        )

    image_config = _require_mapping(
        _load_artifact_json(targets, "oci_config"),
        "OCI image config",
        StaticEvidenceError,
    )
    if (
        image_config.get("created") != contract["image_created"]
        or image_config.get("architecture") != contract["image_architecture"]
        or image_config.get("os") != contract["image_os"]
    ):
        raise StaticEvidenceError("OCI image identity mismatch")
    rootfs = image_config.get("rootfs")
    if (
        not isinstance(rootfs, Mapping)
        or rootfs.get("type") != "layers"
        or not isinstance(rootfs.get("diff_ids"), list)
        or len(rootfs["diff_ids"]) != contract["layer_count"]
    ):
        raise StaticEvidenceError("OCI rootfs diff-ID geometry mismatch")
    for record, diff_id in zip(layer_records, rootfs["diff_ids"], strict=True):
        if not isinstance(diff_id, str):
            raise StaticEvidenceError("OCI rootfs contains a non-string diff-ID")
        record["diff_id"] = diff_id

    docker_manifest = _load_artifact_json(targets, "docker_manifest")
    if (
        not isinstance(docker_manifest, list)
        or len(docker_manifest) != 1
        or not isinstance(docker_manifest[0], Mapping)
        or docker_manifest[0].get("RepoTags") != ["ubuntu-npu:v2.0.10.2"]
    ):
        raise StaticEvidenceError("Docker compatibility manifest tag mismatch")

    tar_scan = inspect_tar(targets.artifacts["oci_tar"].resolved_path, mode="r:")
    required_top_members = {
        "index.json",
        "manifest.json",
        "oci-layout",
        f"blobs/sha256/{str(contract['oci_manifest_digest']).removeprefix('sha256:')}",
        f"blobs/sha256/{str(contract['oci_config_digest']).removeprefix('sha256:')}",
    }
    observed_top_members = {member["name"] for member in tar_scan["members"]}
    if not required_top_members.issubset(observed_top_members):
        raise StaticEvidenceError("OCI TAR is missing required top-level members")

    verified_digests = _verify_oci_blob_digests(
        targets.artifacts["oci_tar"].resolved_path,
        [
            str(contract["oci_manifest_digest"]),
            str(contract["oci_config_digest"]),
            *layer_digests,
        ],
    )
    return {
        "image_reference": contract["image_reference"],
        "created": contract["image_created"],
        "architecture": contract["image_architecture"],
        "os": contract["image_os"],
        "manifest_digest": contract["oci_manifest_digest"],
        "config_digest": contract["oci_config_digest"],
        "tar_member_count": tar_scan["member_count"],
        "layer_count": len(layer_records),
        "layers": layer_records,
        "verified_blob_digests": list(verified_digests),
    }


def recover_vendor_profile(targets: Targets) -> dict[str, Any]:
    artifact = targets.artifacts["wave_vendor_config"]
    try:
        text = artifact.resolved_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise StaticEvidenceError(f"cannot read vendor Wave config: {exc}") from exc
    try:
        identity = compiler_profile.parse_config_text(text)
    except compiler_profile.CompilerProfileError as exc:
        raise StaticEvidenceError(f"vendor Wave config does not parse: {exc}") from exc
    if identity != WAVE_DATABASE_IDENTITY:
        raise StaticEvidenceError("vendor Wave config identity is not the Wave row")
    if targets.policy["wave_target_name"] not in artifact.resolved_path.name:
        raise StaticEvidenceError("vendor Wave config filename does not match target")
    return {
        "artifact": artifact.artifact_id,
        "filename": artifact.resolved_path.name,
        "byte_count": artifact.byte_count,
        "sha256": artifact.sha256,
        "identity": {
            "chip_model": f"0x{identity.chip_model:08x}",
            "chip_revision": f"0x{identity.chip_revision:08x}",
            "product_id": f"0x{identity.product_id:08x}",
            "eco_id": f"0x{identity.eco_id:08x}",
            "customer_id": f"0x{identity.customer_id:08x}",
        },
        "revision_relationship": {
            "vendor_database_revision": f"0x{identity.chip_revision:08x}",
            "observed_live_revision": f"0x{WAVE_LIVE_REVISION:08x}",
            "same_masked_revision_family": (
                ((identity.chip_revision ^ WAVE_LIVE_REVISION) & 0xFFF0) == 0
            ),
        },
        "provenance": "Allwinner image / Vivante IDE 5.11.0 common/cfg",
        "compiler_qualified": False,
    }


def recover_toolchain_versions(targets: Targets) -> dict[str, Any]:
    readme = _read_bounded(targets.artifacts["vendor_readme"].resolved_path).decode(
        "utf-8"
    )
    cmdtools = _read_bounded(
        targets.artifacts["cmdtools_version"].resolved_path
    ).decode("utf-8")
    vsimulator = _read_bounded(
        targets.artifacts["vsimulator_version"].resolved_path
    ).decode("utf-8")
    for expected in ("acuity工具版本: v6.30.22", "IDE版本: v5.11.0"):
        if expected not in readme:
            raise StaticEvidenceError(f"vendor readme is missing {expected!r}")
    if (
        "IDE5.11.0" not in cmdtools
        or "6.4.18" not in cmdtools
        or "Generate NBG version: 0x00010020" not in cmdtools
        or "6.4.18" not in vsimulator
    ):
        raise StaticEvidenceError("Vivante IDE version records disagree")
    return {
        "distribution": "ubuntu-npu:v2.0.10.2",
        "acuity": "6.30.22",
        "vivante_ide": "5.11.0",
        "ide_sdk_family": "6.4.18",
        "standalone_compiler_build": "6.4.18:967862",
        "generated_nbg_version": "0x00010020",
    }


def recover_wrapper_boundary(targets: Targets) -> dict[str, Any]:
    required = (
        b"VSIMULATOR_CONFIG",
        b"VIVANTE_SDK_DIR",
        b"network_binary.nb",
    )
    wrappers: dict[str, Any] = {}
    for artifact_id in ("ovxlib_casegenerator", "timvx_casegenerator"):
        artifact = targets.artifacts[artifact_id]
        results = _search_file(
            artifact.resolved_path, (*required, b"VIV_GPU_FILE")
        )
        if any(not results[value] for value in required):
            raise StaticEvidenceError(
                f"{artifact_id}: missing simulator/NBG wrapper boundary"
            )
        if results[b"VIV_GPU_FILE"]:
            raise StaticEvidenceError(
                f"{artifact_id}: unexpectedly contains VIV_GPU_FILE"
            )
        wrappers[artifact_id] = {
            "sha256": artifact.sha256,
            "contains_vsimulator_config": True,
            "contains_vivante_sdk_dir": True,
            "contains_network_binary_nb": True,
            "contains_viv_gpu_file": False,
            "execution_count": 0,
        }

    wheel_needles = (
        b"VIV_GPU_FILE",
        b"VSIMULATOR_CONFIG",
        b"VIP9000NANOS_PID0X1000000E",
    )
    wheel_hits = search_zip_members(
        targets.artifacts["acuity_wheel"].resolved_path, wheel_needles
    )
    sdk_needles = (
        b"VIV_GPU_FILE",
        b"VSIMULATOR_CONFIG",
        b"VIP9000NANOS_PID0X1000000E",
        b"6.4.6:769531",
    )
    sdk_hits = search_tar_members(
        targets.artifacts["acuity_embedded_sdk"].resolved_path, sdk_needles
    )
    if wheel_hits[b"VIV_GPU_FILE"] or sdk_hits[b"VIV_GPU_FILE"]:
        raise StaticEvidenceError(
            "Acuity distribution unexpectedly contains a VIV_GPU_FILE consumer"
        )
    if not wheel_hits[b"VSIMULATOR_CONFIG"]:
        raise StaticEvidenceError("Acuity wheel lacks VSIMULATOR_CONFIG")
    if (
        not sdk_hits[b"VSIMULATOR_CONFIG"]
        or not sdk_hits[b"VIP9000NANOS_PID0X1000000E"]
        or not sdk_hits[b"6.4.6:769531"]
    ):
        raise StaticEvidenceError("Acuity embedded simulator evidence is incomplete")

    return {
        "wrappers": wrappers,
        "wheel": {
            "artifact": "acuity_wheel",
            "version": "6.30.22",
            "sha256": targets.artifacts["acuity_wheel"].sha256,
            "viv_gpu_file_hits": list(wheel_hits[b"VIV_GPU_FILE"]),
            "vsimulator_config_hits": list(wheel_hits[b"VSIMULATOR_CONFIG"]),
            "exact_wave_target_hits": list(
                wheel_hits[b"VIP9000NANOS_PID0X1000000E"]
            ),
        },
        "embedded_sdk": {
            "artifact": "acuity_embedded_sdk",
            "sha256": targets.artifacts["acuity_embedded_sdk"].sha256,
            "version": "6.4.6:769531",
            "viv_gpu_file_hits": list(sdk_hits[b"VIV_GPU_FILE"]),
            "vsimulator_config_hits": list(sdk_hits[b"VSIMULATOR_CONFIG"]),
            "exact_wave_target_hits": list(
                sdk_hits[b"VIP9000NANOS_PID0X1000000E"]
            ),
            "version_string_hits": list(sdk_hits[b"6.4.6:769531"]),
        },
        "output_ownership": {
            "wrapper_output_name": "network_binary.nb",
            "wrapper_directly_emits_nbg": False,
            "generated_native_program_emits_nbg": True,
            "native_program_io_abi_proven": False,
        },
    }


def recover_standalone_compilers(targets: Targets) -> dict[str, Any]:
    result: dict[str, Any] = {}
    expectations = {
        "vcompiler": (
            b"VIV_GPU_FILE",
            b"Usage: %s [-v] [-l] [-On] [-c] [-x <shaderType>]",
            b"$VERSION$6.4.18:967862$",
        ),
        "vccompiler": (
            b"VIV_GPU_FILE",
            b"Usage: VCLCompiler",
            b"ERROR: No Kernel in this program.",
            b"$VERSION$6.4.18:967862$",
        ),
    }
    for artifact_id, needles in expectations.items():
        artifact = targets.artifacts[artifact_id]
        elf = inspect_elf64_static(artifact.resolved_path)
        hits = _search_file(artifact.resolved_path, needles)
        if not all(hits.values()):
            raise StaticEvidenceError(
                f"{artifact_id}: standalone compiler contract is incomplete"
            )
        result[artifact_id] = {
            "sha256": artifact.sha256,
            "roles": list(artifact.roles),
            "sdk_version": "6.4.18:967862",
            "elf": elf,
            "profile_environment": "VIV_GPU_FILE",
            "emits_nbg": False,
            "execution_count": 0,
        }
    return result


def _artifact_evidence(targets: Targets) -> dict[str, Any]:
    return {
        artifact_id: {
            "path": artifact.path.as_posix(),
            "byte_count": artifact.byte_count,
            "sha256": artifact.sha256,
            "kind": artifact.kind,
            "roles": list(artifact.roles),
            "execution_policy": artifact.execution_policy,
        }
        for artifact_id, artifact in targets.artifacts.items()
    }


def build_evidence(targets: Targets) -> dict[str, Any]:
    archive = _validate_zip_contract(targets)
    oci = recover_oci_contract(targets)
    profile = recover_vendor_profile(targets)
    versions = recover_toolchain_versions(targets)
    parser = recover_parser_contract(targets)
    standalone = recover_standalone_compilers(targets)
    wrapper = recover_wrapper_boundary(targets)

    minimum = str(targets.policy["minimum_no_kernel_sdk"])
    standalone_version = str(targets.parser_contract["sdk_version"])
    version_gate = _version_meets_minimum(standalone_version, minimum)
    if version_gate:
        raise StaticEvidenceError(
            "qualification logic requires review: acquired SDK unexpectedly "
            "meets the no-kernel minimum"
        )

    gates = [
        {
            "gate": "trusted-vendor-provenance",
            "status": "pass",
            "reason": (
                "Radxa documentation links the public Allwinner distribution; "
                "the outer download and every nested artifact are hash-pinned."
            ),
        },
        {
            "gate": "archive-integrity-and-safe-paths",
            "status": "pass",
            "reason": (
                "Both ZIPs and the OCI TAR have bounded, non-escaping members; "
                "all OCI config, manifest, and layer digests were rehashed."
            ),
        },
        {
            "gate": "exact-wave-vendor-profile",
            "status": "pass",
            "reason": (
                "Vivante IDE supplies VIP9000NANOS_PID0X1000000E.config with "
                "the exact five-word Wave database identity."
            ),
        },
        {
            "gate": "viv-gpu-file-parser-recovered",
            "status": "pass-limited",
            "reason": (
                "The five-field no-kernel parser is recovered and byte-locked "
                "inside standalone GLSL/OpenCL shader compilers, not an NBG "
                "graph compiler."
            ),
        },
        {
            "gate": "no-kernel-sdk-version",
            "status": "fail",
            "reason": (
                f"Acquired no-kernel shader compiler is {standalone_version}; "
                f"TIM-VX requires at least {minimum}."
            ),
        },
        {
            "gate": "no-kernel-nbg-consumer",
            "status": "fail",
            "reason": (
                "Acuity NBG wrappers consume VSIMULATOR_CONFIG and their "
                "embedded 6.4.6 simulator SDK. VIV_GPU_FILE appears only in "
                "standalone shader compilers, which do not emit NBG."
            ),
        },
        {
            "gate": "compiler-io-abi-and-ownership",
            "status": "fail",
            "reason": (
                "The generated native gen_nbg boundary is identified, but its "
                "complete input/output ABI and allocation ownership remain "
                "unproven for a qualifying no-kernel backend."
            ),
        },
        {
            "gate": "fixed-conv-compile",
            "status": "blocked",
            "reason": "Compiler execution remains forbidden until prior gates pass.",
        },
    ]
    if any(
        gate["status"] in {"fail", "blocked"}
        for gate in gates
        if gate["gate"] != "fixed-conv-compile"
    ):
        overall = "blocked"
    else:  # pragma: no cover - guarded by the expected version failure above
        overall = "ready-for-separate-compile-plan"

    return {
        "schema_version": 1,
        "evidence_id": "wave-poc9d-compiler-acquisition-v1",
        "acquisition_id": targets.acquisition_id,
        "targets_manifest": {
            "path": targets.source_path.relative_to(
                targets.repository_root
            ).as_posix(),
            "sha256": targets.source_sha256,
        },
        "source": dict(targets.source),
        "policy": dict(targets.policy),
        "artifacts": _artifact_evidence(targets),
        "archive": archive,
        "oci_image": oci,
        "versions": versions,
        "wave_vendor_profile": profile,
        "viv_gpu_file_parser": parser,
        "standalone_compilers": standalone,
        "acuity_nbg_boundary": wrapper,
        "qualification": {
            "status": overall,
            "gates": gates,
            "compiler_calls": 0,
            "container_registrations": 0,
            "container_runs": 0,
            "device_calls": 0,
            "generated_nbg_files": 0,
        },
        "required_next_acquisition": [
            (
                "A dedicated x86-64 Vivante no-kernel SDK 6.4.22 or newer "
                "whose OpenVX/ovxlib NBG path directly consumes VIV_GPU_FILE."
            ),
            (
                "ovxlib 1.2.26 or newer and, for a VIPLite deployment route, "
                "VIPLite 2.0.0 or newer from the same vendor release."
            ),
            (
                "The complete hash-pinned dynamic dependency closure, target "
                "licence/entitlement, headers, and build/version records."
            ),
            (
                "A statically proven gen_nbg input/output and ownership ABI "
                "before one isolated fixed-Conv compile is authorized."
            ),
        ],
        "onnx_implication": {
            "acuity_accepts_onnx": True,
            "general_onnx_frontend_recovered": False,
            "wave_npu_partition_qualified": False,
            "cpu_fallback_available": True,
            "current_safe_mode": "ONNX Runtime CPU with audited all-CPU placement",
        },
        "non_claims": [
            "No acquired compiler was executed.",
            "The vendor config is not yet qualified for an NBG compiler.",
            "The 6.4.18 shader compilers are not a Wave NBG compiler.",
            "No NBG payload was patched, generated, or sent to the device.",
            "No general ONNX model is currently NPU-resident.",
        ],
    }


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


def generate_evidence(
    targets_path: Path,
    output_path: Path,
    *,
    repository_root: Path = REPOSITORY_ROOT,
) -> dict[str, Any]:
    targets = load_targets(targets_path, repository_root=repository_root)
    evidence = build_evidence(targets)
    _atomic_write(output_path, canonical_json(evidence))
    return evidence


def verify_generated(
    targets_path: Path,
    evidence_path: Path,
    *,
    repository_root: Path = REPOSITORY_ROOT,
) -> None:
    targets = load_targets(targets_path, repository_root=repository_root)
    expected = canonical_json(build_evidence(targets))
    try:
        observed = evidence_path.read_bytes()
    except OSError as exc:
        raise StaticEvidenceError(f"cannot read {evidence_path}: {exc}") from exc
    if observed != expected:
        raise StaticEvidenceError(
            f"{evidence_path} is stale; regenerate compiler-acquisition evidence"
        )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--targets", type=Path, default=DEFAULT_TARGETS, help=argparse.SUPPRESS
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser(
        "validate-targets",
        help="hash and validate every inert acquisition target",
    )
    inspect_parser = subparsers.add_parser(
        "inspect", help="print recovered static evidence without writing it"
    )
    inspect_parser.add_argument(
        "--compact", action="store_true", help="emit compact JSON"
    )
    generate_parser = subparsers.add_parser(
        "generate-evidence", help="write canonical acquisition evidence"
    )
    generate_parser.add_argument("--output", type=Path, default=DEFAULT_EVIDENCE)
    verify_parser = subparsers.add_parser(
        "verify-generated", help="rebuild and compare canonical evidence"
    )
    verify_parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    arguments = parser.parse_args(argv)
    try:
        if arguments.command == "validate-targets":
            targets = load_targets(arguments.targets)
            print(
                json.dumps(
                    {
                        "status": "ok",
                        "acquisition_id": targets.acquisition_id,
                        "artifact_count": len(targets.artifacts),
                        "compiler_calls": 0,
                    },
                    sort_keys=True,
                )
            )
        elif arguments.command == "inspect":
            evidence = build_evidence(load_targets(arguments.targets))
            if arguments.compact:
                print(json.dumps(evidence, sort_keys=True, ensure_ascii=False))
            else:
                print(canonical_json(evidence).decode("utf-8"), end="")
        elif arguments.command == "generate-evidence":
            evidence = generate_evidence(arguments.targets, arguments.output)
            print(
                json.dumps(
                    {
                        "status": evidence["qualification"]["status"],
                        "output": str(arguments.output),
                        "compiler_calls": 0,
                    },
                    sort_keys=True,
                )
            )
        elif arguments.command == "verify-generated":
            verify_generated(arguments.targets, arguments.evidence)
            print(
                json.dumps(
                    {
                        "status": "ok",
                        "evidence": str(arguments.evidence),
                        "compiler_calls": 0,
                    },
                    sort_keys=True,
                )
            )
        else:  # pragma: no cover - argparse constrains the command
            parser.error(f"unknown command {arguments.command}")
    except CompilerAcquisitionError as exc:
        parser.exit(2, f"error: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
