#!/usr/bin/env python3
"""Strict host-only orchestration for Wave NBG serializer recovery."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping, Sequence

try:
    from . import ghidra_abi
except ImportError:
    import ghidra_abi  # type: ignore


SCHEMA_VERSION = 1
POC_ID = "poc12a-open-nbg-serializer"
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST_PATH = Path(__file__).with_name(
    "nbg_serializer_targets.json"
)

ROOT_KEYS = frozenset(
    {
        "artifacts",
        "correlation",
        "ghidra_batches",
        "outputs",
        "poc_id",
        "policy",
        "roundtrip_nbg_ids",
        "schema_version",
    }
)
ARTIFACT_KEYS = frozenset({"bytes", "kind", "path", "roles", "sha256"})
GHIDRA_BATCH_KEYS = frozenset(
    {"artifact_id", "functions", "roles", "string_filters"}
)
POLICY_KEYS = frozenset({"allowed_operations", "prohibited_actions"})
CORRELATION_KEYS = frozenset(
    {
        "anchor",
        "control_nbg_ids",
        "limits",
        "search_section_types",
        "transforms",
    }
)
ANCHOR_KEYS = frozenset(
    {
        "id",
        "nbg_artifact_id",
        "onnx_artifact_id",
        "relationship",
    }
)
LIMIT_KEYS = frozenset(
    {
        "max_candidate_bytes",
        "max_candidates_per_tensor",
        "max_initializer_bytes",
        "max_search_bytes_per_tensor",
        "max_search_sections",
    }
)
OUTPUT_KEYS = frozenset({"correlation", "ghidra", "ownership", "roundtrip"})

ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")

ARTIFACT_KINDS = frozenset({"elf", "nbg", "onnx"})
ARTIFACT_ROLES = frozenset(
    {
        "differential-control",
        "exact-wave",
        "generated-control",
        "roundtrip-control",
        "serializer-caller",
        "serializer-provider",
        "source-anchor",
        "source-anchor-target",
    }
)
ALLOWED_OPERATIONS = frozenset(
    {
        "compute-bounded-transforms",
        "hash-files",
        "parse-elf",
        "parse-nbg",
        "parse-onnx",
        "run-ghidra-headless",
        "write-identical-nbg-copy",
        "write-json-evidence",
        "write-markdown-evidence",
    }
)
PROHIBITED_ACTION_KEYS = frozenset(
    {
        "adb_calls",
        "compiler_calls",
        "container_registrations",
        "container_runs",
        "device_calls",
        "generated_custom_nbg_files",
        "mutated_nbg_files",
        "native_extension_imports",
        "simulator_calls",
    }
)

CALLER_FUNCTIONS = frozenset({"vsi_nn_GenerateNBG"})
PROVIDER_FUNCTIONS = frozenset(
    {
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
)
APPROVED_FUNCTIONS = CALLER_FUNCTIONS | PROVIDER_FUNCTIONS

TRANSFORM_IDS = frozenset(
    {
        "identity-fp32-le",
        "cast-fp16-rne-le",
        "cast-bf16-rne-le",
        "transpose-2d",
        "transpose-2d+cast-fp16-rne-le",
        "transpose-2d+cast-bf16-rne-le",
        "fixed-block-permute",
        "byte-lane-permute",
        "halfword-lane-permute",
    }
)
SEARCH_SECTION_TYPES = frozenset({4, 5, 6, 7, 8, 9, 10, 21, 23, 24, 27})
CORRELATION_LIMITS = MappingProxyType(
    {
        "max_candidate_bytes": 64 * 1024 * 1024,
        "max_candidates_per_tensor": 128,
        "max_initializer_bytes": 64 * 1024 * 1024,
        "max_search_bytes_per_tensor": 256 * 1024 * 1024,
        "max_search_sections": 16,
    }
)
EXPECTED_OUTPUTS = MappingProxyType(
    {
        "correlation": "docs/npu/format/onnx-nbg-correlation.json",
        "ghidra": "docs/npu/format/serializer-ghidra-evidence.json",
        "ownership": "docs/npu/format/serializer-byte-ownership.json",
        "roundtrip": "docs/npu/format/serializer-roundtrip-evidence.json",
    }
)

ELF_CLASS_64 = 2
ELF_DATA_LITTLE = 1
ELF_MACHINE_AARCH64 = 183
ELF_HEADER_BYTES = 64
ELF_PROGRAM_HEADER_BYTES = 56
PT_LOAD = 1
VPMN_MAGIC = b"VPMN"
VPMN_V2_VERSION = 0x00020000
WAVE_TARGET = 0x1000000E
MINIMUM_NBG_BYTES = 513
SHT_DYNSYM = 11
SHT_NOBITS = 8
SHN_UNDEF = 0
STT_NOTYPE = 0
STT_FUNC = 2
EXPECTED_EXPORT_THUNK = "vxoBinaryGraph_SaveNetworkBinaryGraph"
SERIALIZER_MAX_DECOMPILED_CHARS = 131072


class SerializerError(RuntimeError):
    """Expected fail-closed serializer-recovery error."""

    def __init__(
        self,
        code: str,
        message: str,
        *,
        artifact_id: str | None = None,
        location: str | None = None,
    ) -> None:
        self.code = code
        self.artifact_id = artifact_id
        self.location = location
        pieces = [code]
        if artifact_id is not None:
            pieces.append(f"artifact={artifact_id}")
        if location is not None:
            pieces.append(f"location={location}")
        super().__init__(": ".join(pieces) + f": {message}")


@dataclass(frozen=True)
class ArtifactSpec:
    artifact_id: str
    byte_count: int
    kind: str
    path: Path
    roles: tuple[str, ...]
    sha256: str

    def to_document(self) -> dict[str, Any]:
        return {
            "bytes": self.byte_count,
            "kind": self.kind,
            "path": self.path.as_posix(),
            "roles": list(self.roles),
            "sha256": self.sha256,
        }


@dataclass(frozen=True)
class GhidraBatch:
    batch_id: str
    artifact_id: str
    functions: tuple[str, ...]
    roles: tuple[str, ...]
    string_filters: tuple[str, ...]

    def to_document(self) -> dict[str, Any]:
        return {
            "artifact_id": self.artifact_id,
            "functions": list(self.functions),
            "roles": list(self.roles),
            "string_filters": list(self.string_filters),
        }


@dataclass(frozen=True)
class CorrelationAnchor:
    anchor_id: str
    nbg_artifact_id: str
    onnx_artifact_id: str
    relationship: str

    def to_document(self) -> dict[str, str]:
        return {
            "id": self.anchor_id,
            "nbg_artifact_id": self.nbg_artifact_id,
            "onnx_artifact_id": self.onnx_artifact_id,
            "relationship": self.relationship,
        }


@dataclass(frozen=True)
class CorrelationSpec:
    anchor: CorrelationAnchor
    control_nbg_ids: tuple[str, ...]
    limits: Mapping[str, int]
    search_section_types: tuple[int, ...]
    transforms: tuple[str, ...]

    def to_document(self) -> dict[str, Any]:
        return {
            "anchor": self.anchor.to_document(),
            "control_nbg_ids": list(self.control_nbg_ids),
            "limits": dict(self.limits),
            "search_section_types": list(self.search_section_types),
            "transforms": list(self.transforms),
        }


@dataclass(frozen=True)
class SerializerPolicy:
    allowed_operations: tuple[str, ...]
    prohibited_actions: Mapping[str, int]

    def to_document(self) -> dict[str, Any]:
        return {
            "allowed_operations": list(self.allowed_operations),
            "prohibited_actions": dict(self.prohibited_actions),
        }


@dataclass(frozen=True)
class SerializerManifest:
    repository_root: Path
    source_path: Path
    source_sha256: str
    schema_version: int
    poc_id: str
    artifacts: Mapping[str, ArtifactSpec]
    correlation: CorrelationSpec
    ghidra_batches: Mapping[str, GhidraBatch]
    outputs: Mapping[str, Path]
    policy: SerializerPolicy
    roundtrip_nbg_ids: tuple[str, ...]

    def to_document(self) -> dict[str, Any]:
        return {
            "artifacts": {
                artifact_id: artifact.to_document()
                for artifact_id, artifact in self.artifacts.items()
            },
            "correlation": self.correlation.to_document(),
            "ghidra_batches": {
                batch_id: batch.to_document()
                for batch_id, batch in self.ghidra_batches.items()
            },
            "outputs": {
                output_id: path.as_posix()
                for output_id, path in self.outputs.items()
            },
            "poc_id": self.poc_id,
            "policy": self.policy.to_document(),
            "roundtrip_nbg_ids": list(self.roundtrip_nbg_ids),
            "schema_version": self.schema_version,
        }


def canonical_json(value: Any) -> bytes:
    """Return deterministic evidence JSON."""

    try:
        return (
            json.dumps(
                value,
                allow_nan=False,
                ensure_ascii=False,
                indent=2,
                sort_keys=True,
            )
            + "\n"
        ).encode("utf-8")
    except (TypeError, ValueError) as exc:
        raise SerializerError(
            "INTERNAL_ERROR", f"cannot serialize canonical JSON: {exc}"
        ) from exc


def _object_without_duplicates(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"duplicate JSON key: {key!r}",
            )
        result[key] = value
    return result


def _strict_keys(
    value: Mapping[str, Any],
    expected: frozenset[str],
    location: str,
) -> None:
    actual = frozenset(value)
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    if missing:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} missing keys: {', '.join(missing)}",
            location=location,
        )
    if unknown:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} has unknown keys: {', '.join(unknown)}",
            location=location,
        )


def _require_mapping(value: Any, location: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must be an object",
            location=location,
        )
    return value


def _require_string(value: Any, location: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or CONTROL_RE.search(value) is not None
    ):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must be a nonempty control-free string",
            location=location,
        )
    return value


def _require_id(value: Any, location: str) -> str:
    text = _require_string(value, location)
    if ID_RE.fullmatch(text) is None:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"invalid {location}: {text!r}",
            location=location,
        )
    return text


def _require_positive_int(value: Any, location: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must be a positive integer",
            location=location,
        )
    return value


def _require_string_list(
    value: Any,
    location: str,
    *,
    allow_empty: bool = False,
) -> tuple[str, ...]:
    if not isinstance(value, list) or (not value and not allow_empty):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must be a "
            f"{'possibly empty' if allow_empty else 'nonempty'} array",
            location=location,
        )
    result = tuple(
        _require_string(item, f"{location}[{index}]")
        for index, item in enumerate(value)
    )
    if len(result) != len(set(result)):
        noun = "function" if location.endswith(".functions") else "value"
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} contains a duplicate {noun}",
            location=location,
        )
    return tuple(sorted(result))


def _relative_path(value: Any, location: str) -> Path:
    text = _require_string(value, location)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} may not escape the repository",
            location=location,
        )
    if path == Path("."):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must name a file",
            location=location,
        )
    return path


def _parse_artifacts(
    value: Any,
) -> Mapping[str, ArtifactSpec]:
    raw = _require_mapping(value, "artifacts")
    if len(raw) != 8:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"artifacts must contain the eight pinned PoC inputs, observed "
            f"{len(raw)}",
            location="artifacts",
        )
    artifacts: dict[str, ArtifactSpec] = {}
    for raw_id, raw_item in raw.items():
        artifact_id = _require_id(raw_id, "artifact ID")
        location = f"artifacts.{artifact_id}"
        item = _require_mapping(raw_item, location)
        _strict_keys(item, ARTIFACT_KEYS, location)
        kind = _require_string(item["kind"], f"{location}.kind")
        if kind not in ARTIFACT_KINDS:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.kind is not an approved kind: {kind!r}",
                location=f"{location}.kind",
            )
        roles = _require_string_list(item["roles"], f"{location}.roles")
        unknown_roles = sorted(set(roles) - ARTIFACT_ROLES)
        if unknown_roles:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.roles contains unknown roles: "
                f"{', '.join(unknown_roles)}",
                location=f"{location}.roles",
            )
        digest = _require_string(item["sha256"], f"{location}.sha256")
        if SHA256_RE.fullmatch(digest) is None:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.sha256 must be a lowercase SHA-256",
                location=f"{location}.sha256",
            )
        role_set = set(roles)
        if kind == "elf" and not (
            "exact-wave" in role_set
            and bool(
                role_set
                & {"serializer-caller", "serializer-provider"}
            )
        ):
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.roles are invalid for an ELF",
                location=f"{location}.roles",
            )
        if kind == "nbg" and not (
            {"exact-wave", "roundtrip-control"} <= role_set
        ):
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.roles are invalid for an NBG",
                location=f"{location}.roles",
            )
        if kind == "onnx" and role_set != {"source-anchor"}:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.roles are invalid for an ONNX anchor",
                location=f"{location}.roles",
            )
        artifacts[artifact_id] = ArtifactSpec(
            artifact_id=artifact_id,
            byte_count=_require_positive_int(
                item["bytes"], f"{location}.bytes"
            ),
            kind=kind,
            path=_relative_path(item["path"], f"{location}.path"),
            roles=roles,
            sha256=digest,
        )

    kind_counts = {
        kind: sum(artifact.kind == kind for artifact in artifacts.values())
        for kind in ARTIFACT_KINDS
    }
    expected_counts = {"elf": 2, "nbg": 5, "onnx": 1}
    if kind_counts != expected_counts:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"artifact kind counts must be {expected_counts}, observed "
            f"{kind_counts}",
            location="artifacts",
        )
    return MappingProxyType(dict(sorted(artifacts.items())))


def _parse_policy(value: Any) -> SerializerPolicy:
    raw = _require_mapping(value, "policy")
    _strict_keys(raw, POLICY_KEYS, "policy")
    operations = _require_string_list(
        raw["allowed_operations"], "policy.allowed_operations"
    )
    if set(operations) != ALLOWED_OPERATIONS:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "policy.allowed_operations must equal the reviewed static-only "
            "operation set",
            location="policy.allowed_operations",
        )
    prohibited = _require_mapping(
        raw["prohibited_actions"], "policy.prohibited_actions"
    )
    _strict_keys(
        prohibited,
        PROHIBITED_ACTION_KEYS,
        "policy.prohibited_actions",
    )
    normalized: dict[str, int] = {}
    for key in sorted(prohibited):
        item = prohibited[key]
        if isinstance(item, bool) or not isinstance(item, int) or item != 0:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"prohibited action {key} must be zero",
                location=f"policy.prohibited_actions.{key}",
            )
        normalized[key] = item
    return SerializerPolicy(
        allowed_operations=operations,
        prohibited_actions=MappingProxyType(normalized),
    )


def _parse_ghidra_batches(
    value: Any,
    artifacts: Mapping[str, ArtifactSpec],
) -> Mapping[str, GhidraBatch]:
    raw = _require_mapping(value, "ghidra_batches")
    if not raw:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "ghidra_batches must not be empty",
            location="ghidra_batches",
        )
    batches: dict[str, GhidraBatch] = {}
    all_functions: list[str] = []
    for raw_id, raw_item in raw.items():
        batch_id = _require_id(raw_id, "Ghidra batch ID")
        location = f"ghidra_batches.{batch_id}"
        item = _require_mapping(raw_item, location)
        _strict_keys(item, GHIDRA_BATCH_KEYS, location)
        artifact_id = _require_id(
            item["artifact_id"], f"{location}.artifact_id"
        )
        artifact = artifacts.get(artifact_id)
        if artifact is None:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location} references unknown artifact {artifact_id!r}",
                location=f"{location}.artifact_id",
            )
        if artifact.kind != "elf":
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location} target artifact is not ELF",
                location=f"{location}.artifact_id",
            )
        functions = _require_string_list(
            item["functions"], f"{location}.functions"
        )
        unknown = sorted(set(functions) - APPROVED_FUNCTIONS)
        if unknown:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location} contains an unapproved function: "
                f"{', '.join(unknown)}",
                location=f"{location}.functions",
            )
        artifact_roles = set(artifact.roles)
        for function in functions:
            if (
                function in CALLER_FUNCTIONS
                and "serializer-caller" not in artifact_roles
            ) or (
                function in PROVIDER_FUNCTIONS
                and "serializer-provider" not in artifact_roles
            ):
                raise SerializerError(
                    "TARGET_MANIFEST_INVALID",
                    f"{function} is assigned to the wrong ELF role",
                    location=f"{location}.functions",
                )
        roles = _require_string_list(item["roles"], f"{location}.roles")
        if not set(roles) <= artifact_roles:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location}.roles are not provided by artifact "
                f"{artifact_id}",
                location=f"{location}.roles",
            )
        filters = _require_string_list(
            item["string_filters"], f"{location}.string_filters"
        )
        if any("," in item for item in (*functions, *filters)):
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location} values may not contain commas",
                location=location,
            )
        all_functions.extend(functions)
        batches[batch_id] = GhidraBatch(
            batch_id=batch_id,
            artifact_id=artifact_id,
            functions=functions,
            roles=roles,
            string_filters=filters,
        )

    duplicates = sorted(
        {
            function
            for function in all_functions
            if all_functions.count(function) > 1
        }
    )
    if duplicates:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"duplicate function across Ghidra batches: "
            f"{', '.join(duplicates)}",
            location="ghidra_batches",
        )
    observed = set(all_functions)
    if observed != APPROVED_FUNCTIONS:
        missing = sorted(APPROVED_FUNCTIONS - observed)
        unknown = sorted(observed - APPROVED_FUNCTIONS)
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unknown:
            details.append("unknown " + ", ".join(unknown))
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "configured serializer function set differs: "
            + "; ".join(details),
            location="ghidra_batches",
        )
    return MappingProxyType(dict(sorted(batches.items())))


def _artifact_of_kind(
    artifacts: Mapping[str, ArtifactSpec],
    artifact_id: str,
    expected_kind: str,
    location: str,
) -> ArtifactSpec:
    artifact = artifacts.get(artifact_id)
    if artifact is None:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} references unknown artifact {artifact_id!r}",
            location=location,
        )
    if artifact.kind != expected_kind:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"{location} must reference {expected_kind}, observed "
            f"{artifact.kind}",
            location=location,
        )
    return artifact


def _parse_correlation(
    value: Any,
    artifacts: Mapping[str, ArtifactSpec],
) -> CorrelationSpec:
    raw = _require_mapping(value, "correlation")
    _strict_keys(raw, CORRELATION_KEYS, "correlation")
    raw_anchor = _require_mapping(raw["anchor"], "correlation.anchor")
    _strict_keys(raw_anchor, ANCHOR_KEYS, "correlation.anchor")
    anchor_id = _require_id(raw_anchor["id"], "correlation.anchor.id")
    onnx_id = _require_id(
        raw_anchor["onnx_artifact_id"],
        "correlation.anchor.onnx_artifact_id",
    )
    nbg_id = _require_id(
        raw_anchor["nbg_artifact_id"],
        "correlation.anchor.nbg_artifact_id",
    )
    _artifact_of_kind(
        artifacts,
        onnx_id,
        "onnx",
        "correlation.anchor.onnx_artifact_id",
    )
    _artifact_of_kind(
        artifacts,
        nbg_id,
        "nbg",
        "correlation.anchor.nbg_artifact_id",
    )
    relationship = _require_string(
        raw_anchor["relationship"], "correlation.anchor.relationship"
    )
    if relationship != "source-binary-anchor":
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.anchor.relationship must be source-binary-anchor",
            location="correlation.anchor.relationship",
        )

    controls = _require_string_list(
        raw["control_nbg_ids"], "correlation.control_nbg_ids"
    )
    if len(controls) != 3:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.control_nbg_ids must contain three controls",
            location="correlation.control_nbg_ids",
        )
    if nbg_id in controls:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation anchor may not also be a differential control",
            location="correlation.control_nbg_ids",
        )
    for index, control in enumerate(controls):
        artifact = _artifact_of_kind(
            artifacts,
            control,
            "nbg",
            f"correlation.control_nbg_ids[{index}]",
        )
        if "differential-control" not in artifact.roles:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"correlation control {control} lacks differential-control "
                "role",
                location=f"correlation.control_nbg_ids[{index}]",
            )

    raw_limits = _require_mapping(raw["limits"], "correlation.limits")
    _strict_keys(raw_limits, LIMIT_KEYS, "correlation.limits")
    limits = {
        key: _require_positive_int(
            raw_limits[key], f"correlation.limits.{key}"
        )
        for key in sorted(raw_limits)
    }
    if limits != dict(CORRELATION_LIMITS):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.limits differ from the reviewed bounded limits",
            location="correlation.limits",
        )

    raw_sections = raw["search_section_types"]
    if not isinstance(raw_sections, list) or not raw_sections:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.search_section_types must be a nonempty array",
            location="correlation.search_section_types",
        )
    sections: list[int] = []
    for index, item in enumerate(raw_sections):
        if isinstance(item, bool) or not isinstance(item, int) or item < 0:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                "correlation.search_section_types contains an invalid "
                f"value at index {index}",
                location=f"correlation.search_section_types[{index}]",
            )
        sections.append(item)
    if len(sections) != len(set(sections)):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.search_section_types contains duplicates",
            location="correlation.search_section_types",
        )
    if set(sections) != SEARCH_SECTION_TYPES:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.search_section_types differs from the reviewed "
            "allowlist",
            location="correlation.search_section_types",
        )

    transforms = _require_string_list(
        raw["transforms"], "correlation.transforms"
    )
    if set(transforms) != TRANSFORM_IDS:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "correlation.transforms differs from the reviewed transform "
            "registry",
            location="correlation.transforms",
        )
    return CorrelationSpec(
        anchor=CorrelationAnchor(
            anchor_id=anchor_id,
            nbg_artifact_id=nbg_id,
            onnx_artifact_id=onnx_id,
            relationship=relationship,
        ),
        control_nbg_ids=controls,
        limits=MappingProxyType(limits),
        search_section_types=tuple(sorted(sections)),
        transforms=transforms,
    )


def _parse_outputs(value: Any) -> Mapping[str, Path]:
    raw = _require_mapping(value, "outputs")
    _strict_keys(raw, OUTPUT_KEYS, "outputs")
    outputs: dict[str, Path] = {}
    for output_id in sorted(raw):
        location = f"outputs.{output_id}"
        path = _relative_path(raw[output_id], location)
        expected = EXPECTED_OUTPUTS[output_id]
        if path.as_posix() != expected:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{location} must be {expected!r}",
                location=location,
            )
        outputs[output_id] = path
    return MappingProxyType(outputs)


def _parse_roundtrip_ids(
    value: Any,
    artifacts: Mapping[str, ArtifactSpec],
    correlation: CorrelationSpec,
) -> tuple[str, ...]:
    identifiers = _require_string_list(value, "roundtrip_nbg_ids")
    if len(identifiers) != 5:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "roundtrip_nbg_ids must contain the five selected NBG controls",
            location="roundtrip_nbg_ids",
        )
    for index, artifact_id in enumerate(identifiers):
        artifact = _artifact_of_kind(
            artifacts,
            artifact_id,
            "nbg",
            f"roundtrip_nbg_ids[{index}]",
        )
        if "roundtrip-control" not in artifact.roles:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"{artifact_id} lacks roundtrip-control role",
                location=f"roundtrip_nbg_ids[{index}]",
            )
    expected = {
        correlation.anchor.nbg_artifact_id,
        *correlation.control_nbg_ids,
    }
    generated = {
        artifact.artifact_id
        for artifact in artifacts.values()
        if "generated-control" in artifact.roles
    }
    if len(generated) != 1:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "exactly one generated-control NBG is required",
            location="artifacts",
        )
    expected.update(generated)
    if set(identifiers) != expected:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "roundtrip_nbg_ids does not equal the generated, anchor, and "
            "differential control set",
            location="roundtrip_nbg_ids",
        )
    return identifiers


def load_manifest(
    path: Path = DEFAULT_MANIFEST_PATH,
    *,
    repository_root: Path | None = None,
) -> SerializerManifest:
    """Load the strict PoC 12A target manifest without touching artifacts."""

    root = (
        repository_root.resolve()
        if repository_root is not None
        else REPOSITORY_ROOT
    )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"cannot read target manifest {path}: {exc}",
        ) from exc
    try:
        document = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except SerializerError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"cannot decode target manifest: {exc}",
        ) from exc
    raw = _require_mapping(document, "root")
    _strict_keys(raw, ROOT_KEYS, "root")
    version = raw["schema_version"]
    if (
        isinstance(version, bool)
        or not isinstance(version, int)
        or version != SCHEMA_VERSION
    ):
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"schema_version must be {SCHEMA_VERSION}",
            location="schema_version",
        )
    poc_id = _require_string(raw["poc_id"], "poc_id")
    if poc_id != POC_ID:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"poc_id must be {POC_ID!r}",
            location="poc_id",
        )

    artifacts = _parse_artifacts(raw["artifacts"])
    policy = _parse_policy(raw["policy"])
    batches = _parse_ghidra_batches(raw["ghidra_batches"], artifacts)
    correlation = _parse_correlation(raw["correlation"], artifacts)
    roundtrip_ids = _parse_roundtrip_ids(
        raw["roundtrip_nbg_ids"], artifacts, correlation
    )
    outputs = _parse_outputs(raw["outputs"])
    return SerializerManifest(
        repository_root=root,
        source_path=path.resolve(),
        source_sha256=hashlib.sha256(payload).hexdigest(),
        schema_version=version,
        poc_id=poc_id,
        artifacts=artifacts,
        correlation=correlation,
        ghidra_batches=batches,
        outputs=outputs,
        policy=policy,
        roundtrip_nbg_ids=roundtrip_ids,
    )


def _resolve_artifact(
    manifest: SerializerManifest,
    artifact: ArtifactSpec,
) -> Path:
    root = manifest.repository_root
    current = root
    for component in artifact.path.parts:
        current = current / component
        if current.is_symlink():
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                "artifact path contains a symlink",
                artifact_id=artifact.artifact_id,
                location=artifact.path.as_posix(),
            )
    try:
        resolved = current.resolve(strict=True)
    except OSError as exc:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"artifact is missing: {exc}",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    try:
        resolved.relative_to(root)
    except ValueError as exc:
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "artifact resolves outside the repository",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    if not resolved.is_file():
        raise SerializerError(
            "TARGET_MANIFEST_INVALID",
            "artifact is not a regular file",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        )
    return resolved


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _validate_elf(payload: bytes, artifact: ArtifactSpec) -> None:
    if len(payload) < ELF_HEADER_BYTES or payload[:4] != b"\x7fELF":
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "artifact is not an ELF64 AArch64 file",
            artifact_id=artifact.artifact_id,
            location="ELF header",
        )
    if payload[4] != ELF_CLASS_64:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF class is not ELF64",
            artifact_id=artifact.artifact_id,
            location="e_ident.class",
        )
    if payload[5] != ELF_DATA_LITTLE:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF byte order is not little-endian",
            artifact_id=artifact.artifact_id,
            location="e_ident.data",
        )
    if payload[6] != 1:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF identity version is not one",
            artifact_id=artifact.artifact_id,
            location="e_ident.version",
        )
    try:
        (
            _elf_type,
            machine,
            elf_version,
            _entry,
            program_offset,
            _section_offset,
            _flags,
            header_size,
            program_entry_size,
            program_count,
            _section_entry_size,
            _section_count,
            _string_section,
        ) = struct.unpack_from("<HHIQQQIHHHHHH", payload, 16)
    except struct.error as exc:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF header is truncated",
            artifact_id=artifact.artifact_id,
            location="ELF header",
        ) from exc
    if machine != ELF_MACHINE_AARCH64:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            f"ELF machine is {machine}, expected AArch64",
            artifact_id=artifact.artifact_id,
            location="e_machine",
        )
    if elf_version != 1 or header_size != ELF_HEADER_BYTES:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF version or header size is invalid",
            artifact_id=artifact.artifact_id,
            location="ELF header",
        )
    if (
        program_count <= 0
        or program_entry_size < ELF_PROGRAM_HEADER_BYTES
        or program_offset < ELF_HEADER_BYTES
    ):
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF program-header geometry is invalid",
            artifact_id=artifact.artifact_id,
            location="program-header table",
        )
    program_end = program_offset + program_entry_size * program_count
    if program_end < program_offset or program_end > len(payload):
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF program-header table is outside the file",
            artifact_id=artifact.artifact_id,
            location="program-header table",
        )
    load_count = 0
    for index in range(program_count):
        offset = program_offset + index * program_entry_size
        try:
            (
                segment_type,
                _segment_flags,
                file_offset,
                _virtual_address,
                _physical_address,
                file_size,
                memory_size,
                alignment,
            ) = struct.unpack_from("<IIQQQQQQ", payload, offset)
        except struct.error as exc:
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                "ELF program-header record is truncated",
                artifact_id=artifact.artifact_id,
                location=f"program-header[{index}]",
            ) from exc
        if segment_type != PT_LOAD:
            continue
        load_count += 1
        file_end = file_offset + file_size
        if (
            file_end < file_offset
            or file_end > len(payload)
            or memory_size < file_size
        ):
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                "ELF PT_LOAD file geometry is invalid",
                artifact_id=artifact.artifact_id,
                location=f"program-header[{index}]",
            )
        if alignment not in {0, 1} and alignment & (alignment - 1):
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                "ELF PT_LOAD alignment is not a power of two",
                artifact_id=artifact.artifact_id,
                location=f"program-header[{index}]",
            )
    if load_count == 0:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF contains no PT_LOAD program header",
            artifact_id=artifact.artifact_id,
            location="program-header table",
        )


def _validate_nbg(payload: bytes, artifact: ArtifactSpec) -> None:
    if len(payload) < MINIMUM_NBG_BYTES:
        raise SerializerError(
            "NBG_PARSE_FAILURE",
            f"NBG is smaller than {MINIMUM_NBG_BYTES} bytes",
            artifact_id=artifact.artifact_id,
            location="envelope",
        )
    if payload[:4] != VPMN_MAGIC:
        raise SerializerError(
            "NBG_PARSE_FAILURE",
            "NBG does not begin with VPMN magic",
            artifact_id=artifact.artifact_id,
            location="magic",
        )
    version, target = struct.unpack_from("<II", payload, 4)
    if version != VPMN_V2_VERSION:
        raise SerializerError(
            "NBG_PARSE_FAILURE",
            f"NBG version is 0x{version:08x}, expected VPMN v2",
            artifact_id=artifact.artifact_id,
            location="version",
        )
    if target != WAVE_TARGET:
        raise SerializerError(
            "NBG_PARSE_FAILURE",
            f"NBG target is 0x{target:08x}, expected 0x{WAVE_TARGET:08x}",
            artifact_id=artifact.artifact_id,
            location="target",
        )


def validate_artifacts(
    manifest: SerializerManifest,
) -> Mapping[str, Mapping[str, Any]]:
    """Validate every configured input without executing or importing it."""

    validated: dict[str, Mapping[str, Any]] = {}
    for artifact_id, artifact in manifest.artifacts.items():
        path = _resolve_artifact(manifest, artifact)
        observed_size = path.stat().st_size
        if observed_size != artifact.byte_count:
            raise SerializerError(
                "TARGET_HASH_MISMATCH",
                f"artifact byte count is {observed_size}, expected "
                f"{artifact.byte_count}",
                artifact_id=artifact_id,
                location=artifact.path.as_posix(),
            )
        digest = _hash_file(path)
        if digest != artifact.sha256:
            raise SerializerError(
                "TARGET_HASH_MISMATCH",
                f"artifact SHA-256 is {digest}, expected {artifact.sha256}",
                artifact_id=artifact_id,
                location=artifact.path.as_posix(),
            )
        try:
            payload = path.read_bytes()
        except OSError as exc:
            raise SerializerError(
                "TARGET_MANIFEST_INVALID",
                f"cannot read artifact: {exc}",
                artifact_id=artifact_id,
                location=artifact.path.as_posix(),
            ) from exc
        if artifact.kind == "elf":
            _validate_elf(payload, artifact)
        elif artifact.kind == "nbg":
            _validate_nbg(payload, artifact)
        elif artifact.kind == "onnx" and not payload:
            raise SerializerError(
                "ONNX_ADMISSION_FAILURE",
                "ONNX source anchor is empty",
                artifact_id=artifact_id,
                location=artifact.path.as_posix(),
            )
        validated[artifact_id] = MappingProxyType(
            {
                "bytes": observed_size,
                "kind": artifact.kind,
                "path": artifact.path.as_posix(),
                "sha256": digest,
            }
        )
    return MappingProxyType(validated)


def _elf_program_loads(payload: bytes) -> tuple[tuple[int, int, int], ...]:
    (
        _elf_type,
        _machine,
        _elf_version,
        _entry,
        program_offset,
        _section_offset,
        _flags,
        _header_size,
        program_entry_size,
        program_count,
        _section_entry_size,
        _section_count,
        _string_section,
    ) = struct.unpack_from("<HHIQQQIHHHHHH", payload, 16)
    loads = []
    for index in range(program_count):
        offset = program_offset + index * program_entry_size
        (
            segment_type,
            _segment_flags,
            file_offset,
            virtual_address,
            _physical_address,
            file_size,
            _memory_size,
            _alignment,
        ) = struct.unpack_from("<IIQQQQQQ", payload, offset)
        if segment_type == PT_LOAD:
            loads.append((virtual_address, file_offset, file_size))
    return tuple(loads)


def _virtual_to_file_offset(
    virtual_address: int,
    size: int,
    loads: Sequence[tuple[int, int, int]],
    *,
    artifact_id: str,
    function_name: str,
) -> int:
    matches = []
    for load_vaddr, load_offset, load_size in loads:
        load_end = load_vaddr + load_size
        symbol_end = virtual_address + size
        if (
            virtual_address >= load_vaddr
            and symbol_end >= virtual_address
            and symbol_end <= load_end
        ):
            matches.append(load_offset + virtual_address - load_vaddr)
    if len(matches) != 1:
        raise SerializerError(
            "GHIDRA_FUNCTION_DRIFT",
            f"{function_name} maps to {len(matches)} ELF PT_LOAD ranges",
            artifact_id=artifact_id,
            location=function_name,
        )
    return matches[0]


def _decode_aarch64_branch(
    body: bytes, virtual_address: int
) -> tuple[str | None, int | None]:
    if len(body) != 4:
        return None, None
    instruction = int.from_bytes(body, "little")
    if instruction >> 26 != 0b000101:
        return None, None
    immediate = instruction & ((1 << 26) - 1)
    if immediate & (1 << 25):
        immediate -= 1 << 26
    target = virtual_address + (immediate << 2)
    return "aarch64-b", target


def _elf_dynamic_exports(
    payload: bytes,
    *,
    artifact_id: str,
    requested: frozenset[str],
) -> Mapping[str, Mapping[str, Any]]:
    """Read exact dynamic-symbol bytes without loading the ELF."""

    _validate_elf(
        payload,
        ArtifactSpec(
            artifact_id=artifact_id,
            byte_count=len(payload),
            kind="elf",
            path=Path("<memory>"),
            roles=("exact-wave", "serializer-provider"),
            sha256=hashlib.sha256(payload).hexdigest(),
        ),
    )
    (
        _elf_type,
        _machine,
        _elf_version,
        _entry,
        _program_offset,
        section_offset,
        _flags,
        _header_size,
        _program_entry_size,
        _program_count,
        section_entry_size,
        section_count,
        _string_section,
    ) = struct.unpack_from("<HHIQQQIHHHHHH", payload, 16)
    if (
        section_count <= 0
        or section_entry_size < 64
        or section_offset < ELF_HEADER_BYTES
    ):
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF section-header geometry is invalid",
            artifact_id=artifact_id,
            location="section-header table",
        )
    section_end = section_offset + section_entry_size * section_count
    if section_end < section_offset or section_end > len(payload):
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "ELF section-header table is outside the file",
            artifact_id=artifact_id,
            location="section-header table",
        )
    sections: list[tuple[int, int, int, int, int]] = []
    for index in range(section_count):
        offset = section_offset + index * section_entry_size
        (
            _name,
            section_type,
            _section_flags,
            _section_address,
            file_offset,
            size,
            link,
            _info,
            _alignment,
            entry_size,
        ) = struct.unpack_from("<IIQQQQIIQQ", payload, offset)
        if section_type != SHT_NOBITS and (
            file_offset > len(payload)
            or size > len(payload) - file_offset
        ):
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                f"ELF section {index} is outside the file",
                artifact_id=artifact_id,
                location=f"section-header[{index}]",
            )
        sections.append(
            (section_type, file_offset, size, link, entry_size)
        )
    dynamic_indices = [
        index
        for index, section in enumerate(sections)
        if section[0] == SHT_DYNSYM
    ]
    if len(dynamic_indices) != 1:
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            f"expected one dynamic symbol table, observed "
            f"{len(dynamic_indices)}",
            artifact_id=artifact_id,
            location="dynamic-symbol-table",
        )
    dynamic = sections[dynamic_indices[0]]
    _, symbol_offset, symbol_size, string_index, symbol_stride = dynamic
    if (
        symbol_stride < 24
        or symbol_size % symbol_stride
        or string_index >= len(sections)
    ):
        raise SerializerError(
            "ELF_IDENTITY_MISMATCH",
            "dynamic symbol table geometry is invalid",
            artifact_id=artifact_id,
            location="dynamic-symbol-table",
        )
    _, strings_offset, strings_size, _strings_link, _strings_stride = (
        sections[string_index]
    )
    strings = payload[strings_offset : strings_offset + strings_size]
    loads = _elf_program_loads(payload)
    results: dict[str, Mapping[str, Any]] = {}
    for index in range(symbol_size // symbol_stride):
        offset = symbol_offset + index * symbol_stride
        (
            name_offset,
            info,
            _other,
            section_index,
            virtual_address,
            size,
        ) = struct.unpack_from("<IBBHQQ", payload, offset)
        if section_index == SHN_UNDEF or name_offset >= len(strings):
            continue
        terminator = strings.find(b"\0", name_offset)
        if terminator < 0:
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                "dynamic symbol name is unterminated",
                artifact_id=artifact_id,
                location=f"dynsym[{index}]",
            )
        try:
            name = strings[name_offset:terminator].decode("utf-8")
        except UnicodeError as exc:
            raise SerializerError(
                "ELF_IDENTITY_MISMATCH",
                "dynamic symbol name is not UTF-8",
                artifact_id=artifact_id,
                location=f"dynsym[{index}]",
            ) from exc
        if name not in requested:
            continue
        symbol_type = info & 0x0F
        if symbol_type not in {STT_NOTYPE, STT_FUNC} or size <= 0:
            raise SerializerError(
                "GHIDRA_FUNCTION_DRIFT",
                f"{name} lacks a positive function symbol size",
                artifact_id=artifact_id,
                location=name,
            )
        if name in results:
            raise SerializerError(
                "GHIDRA_FUNCTION_DRIFT",
                f"{name} has duplicate dynamic definitions",
                artifact_id=artifact_id,
                location=name,
            )
        file_offset = _virtual_to_file_offset(
            virtual_address,
            size,
            loads,
            artifact_id=artifact_id,
            function_name=name,
        )
        body = payload[file_offset : file_offset + size]
        branch_kind, branch_target = _decode_aarch64_branch(
            body, virtual_address
        )
        results[name] = MappingProxyType(
            {
                "artifact_id": artifact_id,
                "branch_kind": branch_kind,
                "branch_target_vaddr": branch_target,
                "function_sha256": hashlib.sha256(body).hexdigest(),
                "name": name,
                "paddr": file_offset,
                "probable_thunk": size <= 16,
                "size": size,
                "symbol_type": (
                    "FUNC" if symbol_type == STT_FUNC else "NOTYPE"
                ),
                "vaddr": virtual_address,
            }
        )
    missing = sorted(requested - set(results))
    if missing:
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            "ELF dynamic exports are missing: " + ", ".join(missing),
            artifact_id=artifact_id,
            location="dynamic-symbol-table",
        )
    return MappingProxyType(dict(sorted(results.items())))


def load_serializer_exports(
    manifest: SerializerManifest,
) -> Mapping[str, Mapping[str, Any]]:
    """Hash the 21 configured exported function bodies from exact ELFs."""

    validate_artifacts(manifest)
    by_artifact: dict[str, set[str]] = {}
    for batch in manifest.ghidra_batches.values():
        by_artifact.setdefault(batch.artifact_id, set()).update(
            batch.functions
        )
    exports: dict[str, Mapping[str, Any]] = {}
    for artifact_id, function_names in sorted(by_artifact.items()):
        artifact = manifest.artifacts[artifact_id]
        path = manifest.repository_root / artifact.path
        records = _elf_dynamic_exports(
            path.read_bytes(),
            artifact_id=artifact_id,
            requested=frozenset(function_names),
        )
        for name, record in records.items():
            if name in exports:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"duplicate serializer export {name}",
                    artifact_id=artifact_id,
                    location=name,
                )
            exports[name] = record
    if set(exports) != APPROVED_FUNCTIONS:
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            "resolved serializer export set differs from manifest",
        )
    for name, record in exports.items():
        probable = bool(record["probable_thunk"])
        if name == EXPECTED_EXPORT_THUNK:
            if (
                not probable
                or record["branch_kind"] != "aarch64-b"
            ):
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"expected export alias {name} is not the proven "
                    "AArch64 branch thunk",
                    artifact_id=str(record["artifact_id"]),
                    location=name,
                )
        elif probable:
            raise SerializerError(
                "GHIDRA_FUNCTION_DRIFT",
                f"unexpected probable thunk: {name}",
                artifact_id=str(record["artifact_id"]),
                location=name,
            )
    return MappingProxyType(dict(sorted(exports.items())))


def _ghidra_config_from_manifest(
    manifest: SerializerManifest,
) -> ghidra_abi.GhidraConfig:
    functions_by_artifact: dict[str, set[str]] = {}
    filters_by_artifact: dict[str, set[str]] = {}
    roles_by_artifact: dict[str, set[str]] = {}
    for batch in manifest.ghidra_batches.values():
        functions_by_artifact.setdefault(batch.artifact_id, set()).update(
            batch.functions
        )
        filters_by_artifact.setdefault(batch.artifact_id, set()).update(
            batch.string_filters
        )
        roles_by_artifact.setdefault(batch.artifact_id, set()).update(
            batch.roles
        )
    targets = {}
    for artifact_id in sorted(functions_by_artifact):
        artifact = manifest.artifacts[artifact_id]
        evidence_id = f"{artifact_id}-serializer"
        targets[evidence_id] = ghidra_abi.GhidraTarget(
            evidence_id=evidence_id,
            firmware="fw_v1.1.6.6",
            path=artifact.path,
            sha256=artifact.sha256,
            roles=tuple(sorted(roles_by_artifact[artifact_id])),
            functions=tuple(sorted(functions_by_artifact[artifact_id])),
            string_filters=tuple(
                sorted(filters_by_artifact[artifact_id])
            ),
        )
    return ghidra_abi.GhidraConfig(
        repository_root=manifest.repository_root,
        source_path=manifest.source_path,
        source_sha256=manifest.source_sha256,
        targets=MappingProxyType(dict(sorted(targets.items()))),
    )


def _parse_ghidra_address(
    value: Any,
    *,
    batch_id: str,
    function_name: str,
) -> int:
    if not isinstance(value, str) or not value:
        raise SerializerError(
            "GHIDRA_FUNCTION_DRIFT",
            f"{function_name} has no Ghidra entry address",
            location=batch_id,
        )
    candidate = value.rsplit(":", 1)[-1]
    try:
        return int(candidate, 16)
    except ValueError as exc:
        raise SerializerError(
            "GHIDRA_FUNCTION_DRIFT",
            f"{function_name} has invalid Ghidra entry {value!r}",
            location=batch_id,
        ) from exc


def _safe_function_context(value: Mapping[str, Any]) -> dict[str, Any]:
    keys = (
        "body_bytes",
        "callees",
        "callers",
        "decompile_error",
        "decompile_success",
        "decompile_truncated",
        "decompiled_c",
        "diagnostics",
        "entry",
        "external",
        "name",
        "qualified_name",
        "role",
        "thunk",
    )
    return {key: value.get(key) for key in keys}


def _infer_ghidra_load_bias(
    analysis_id: str,
    analysis: Mapping[str, Any],
    function_names: Sequence[str],
    exports: Mapping[str, Mapping[str, Any]],
) -> int:
    target_records = analysis.get("targets")
    if not isinstance(target_records, list):
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            "Ghidra batch target records are missing",
            location=analysis_id,
        )
    records_by_name = {
        item.get("target"): item
        for item in target_records
        if isinstance(item, Mapping)
    }
    common: set[int] | None = None
    for name in function_names:
        record = records_by_name.get(name)
        export = exports.get(name)
        if not isinstance(record, Mapping) or not isinstance(
            export, Mapping
        ):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                f"cannot infer load bias without {name}",
                location=analysis_id,
            )
        contexts = record.get("functions")
        if not isinstance(contexts, list):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                f"Ghidra emitted no function context for {name}",
                location=analysis_id,
            )
        biases = set()
        for context in contexts:
            if (
                not isinstance(context, Mapping)
                or context.get("role") != "target"
            ):
                continue
            entry = _parse_ghidra_address(
                context.get("entry"),
                batch_id=analysis_id,
                function_name=name,
            )
            bias = entry - int(export["vaddr"])
            if bias >= 0:
                biases.add(bias)
        if not biases:
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                f"{name} has no nonnegative load-bias candidate",
                location=analysis_id,
            )
        common = biases if common is None else common & biases
    if common is None or len(common) != 1:
        raise SerializerError(
            "GHIDRA_FUNCTION_DRIFT",
            f"expected one common Ghidra load bias, observed "
            f"{sorted(common or set())}",
            location=analysis_id,
        )
    return next(iter(common))


def build_serializer_ghidra_evidence(
    manifest: SerializerManifest,
    *,
    executor: Any | None = None,
    export_records: Mapping[str, Mapping[str, Any]] | None = None,
) -> dict[str, Any]:
    """Build bounded exact-function Ghidra evidence."""

    validate_artifacts(manifest)
    exports = export_records or load_serializer_exports(manifest)
    config = _ghidra_config_from_manifest(manifest)
    effective_executor = executor
    if effective_executor is None:
        effective_executor = ghidra_abi.HeadlessGhidra(
            config,
            max_decompiled_chars=SERIALIZER_MAX_DECOMPILED_CHARS,
        )
    try:
        raw = ghidra_abi.build_evidence(
            config, executor=effective_executor
        )
    except (
        ghidra_abi.GhidraConfigError,
        ghidra_abi.GhidraExecutionError,
    ) as exc:
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            str(exc),
        ) from exc
    raw_targets = raw.get("targets")
    if not isinstance(raw_targets, list):
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            "Ghidra adapter evidence has no targets array",
        )
    by_batch = {
        item.get("evidence_id"): item
        for item in raw_targets
        if isinstance(item, Mapping)
    }
    expected_analysis_ids = {
        f"{batch.artifact_id}-serializer"
        for batch in manifest.ghidra_batches.values()
    }
    if set(by_batch) != expected_analysis_ids:
        raise SerializerError(
            "GHIDRA_TARGET_MISSING",
            "Ghidra evidence artifact set differs from the manifest",
        )

    load_bias_by_analysis = {}
    for analysis_id, target_envelope in sorted(by_batch.items()):
        analysis = target_envelope.get("analysis")
        if not isinstance(analysis, Mapping):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                "Ghidra artifact has no analysis document",
                location=analysis_id,
            )
        load_bias_by_analysis[analysis_id] = _infer_ghidra_load_bias(
            analysis_id,
            analysis,
            config.targets[analysis_id].functions,
            exports,
        )

    batches = []
    tool_versions: set[str] = set()
    for batch_id, batch in manifest.ghidra_batches.items():
        analysis_id = f"{batch.artifact_id}-serializer"
        target_envelope = by_batch[analysis_id]
        analysis = target_envelope.get("analysis")
        if not isinstance(analysis, Mapping):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                "Ghidra batch has no analysis document",
                location=batch_id,
            )
        warnings = analysis.get("warnings")
        if warnings != []:
            raise SerializerError(
                "GHIDRA_FUNCTION_DRIFT",
                f"Ghidra batch warnings are not empty: {warnings!r}",
                location=batch_id,
            )
        tool = analysis.get("tool")
        if isinstance(tool, Mapping) and isinstance(
            tool.get("version"), str
        ):
            tool_versions.add(tool["version"])
        target_records = analysis.get("targets")
        if not isinstance(target_records, list):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                "Ghidra batch target records are missing",
                location=batch_id,
            )
        records_by_name = {
            item.get("target"): item
            for item in target_records
            if isinstance(item, Mapping)
        }
        if not set(batch.functions) <= set(records_by_name):
            raise SerializerError(
                "GHIDRA_TARGET_MISSING",
                "Ghidra target set does not cover configured batch",
                location=batch_id,
            )

        functions = []
        for name in batch.functions:
            target_record = records_by_name[name]
            if target_record.get("found") is not True:
                raise SerializerError(
                    "GHIDRA_TARGET_MISSING",
                    f"Ghidra did not find {name}",
                    location=batch_id,
                )
            contexts = target_record.get("functions")
            if not isinstance(contexts, list):
                raise SerializerError(
                    "GHIDRA_TARGET_MISSING",
                    f"Ghidra emitted no function context for {name}",
                    location=batch_id,
                )
            target_candidates = [
                item
                for item in contexts
                if isinstance(item, Mapping)
                and item.get("role") == "target"
            ]
            export = exports.get(name)
            if not isinstance(export, Mapping):
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} has no exact ELF export record",
                    location=batch_id,
                )
            load_bias = load_bias_by_analysis[analysis_id]
            matched_targets = [
                item
                for item in target_candidates
                if _parse_ghidra_address(
                    item.get("entry"),
                    batch_id=batch_id,
                    function_name=name,
                )
                - load_bias
                == int(export["vaddr"])
            ]
            if len(matched_targets) != 1:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} has {len(matched_targets)} functions at the "
                    "normalized ELF address",
                    location=batch_id,
                )
            function = matched_targets[0]
            entry = _parse_ghidra_address(
                function.get("entry"),
                batch_id=batch_id,
                function_name=name,
            )
            elf_vaddr = int(export["vaddr"])
            if entry - elf_vaddr != load_bias:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} does not match inferred Ghidra load bias "
                    f"0x{load_bias:x}",
                    location=batch_id,
                )
            if function.get("external") is not False:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} resolved as external",
                    location=batch_id,
                )
            is_expected_alias = name == EXPECTED_EXPORT_THUNK
            if function.get("thunk") is True and not is_expected_alias:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} unexpectedly resolved as a thunk",
                    location=batch_id,
                )
            if function.get("decompile_truncated") is not False:
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} decompilation is truncated",
                    location=batch_id,
                )
            if (
                function.get("decompile_success") is not True
                and not is_expected_alias
            ):
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} decompilation failed",
                    location=batch_id,
                )
            body_bytes = function.get("body_bytes")
            if (
                isinstance(body_bytes, bool)
                or not isinstance(body_bytes, int)
                or body_bytes <= 0
            ):
                raise SerializerError(
                    "GHIDRA_FUNCTION_DRIFT",
                    f"{name} has invalid Ghidra body size",
                    location=batch_id,
                )
            direct_callers = [
                _safe_function_context(item)
                for item in contexts
                if isinstance(item, Mapping)
                and item.get("role") == "direct-caller"
            ]
            direct_callers.sort(
                key=lambda item: (
                    str(item.get("entry")),
                    str(item.get("qualified_name")),
                )
            )
            symbol_aliases = [
                _safe_function_context(item)
                for item in target_candidates
                if item is not function
            ]
            symbol_aliases.sort(
                key=lambda item: (
                    str(item.get("entry")),
                    str(item.get("qualified_name")),
                )
            )
            functions.append(
                {
                    "direct_caller_context": direct_callers,
                    "elf": dict(export),
                    "function_kind": (
                        "export-thunk-alias"
                        if is_expected_alias
                        else "function"
                    ),
                    "ghidra": _safe_function_context(function),
                    "name": name,
                    "symbol_alias_context": symbol_aliases,
                }
            )
        functions.sort(key=lambda item: item["name"])
        batches.append(
            {
                "artifact_id": batch.artifact_id,
                "batch_id": batch_id,
                "functions": functions,
                "ghidra_load_bias": load_bias_by_analysis[analysis_id],
                "ghidra_load_bias_hex": (
                    f"0x{load_bias_by_analysis[analysis_id]:x}"
                ),
                "path": manifest.artifacts[
                    batch.artifact_id
                ].path.as_posix(),
                "sha256": manifest.artifacts[
                    batch.artifact_id
                ].sha256,
            }
        )
    batches.sort(key=lambda item: item["batch_id"])
    return {
        "batches": batches,
        "generator": "waved/scripts/npu/nbg_serializer_recovery.py",
        "manifest": {
            "path": manifest.source_path.relative_to(
                manifest.repository_root
            ).as_posix(),
            "sha256": manifest.source_sha256,
        },
        "poc_id": manifest.poc_id,
        "safety": dict(manifest.policy.prohibited_actions),
        "schema_version": 1,
        "tools": {
            "max_decompiled_chars": SERIALIZER_MAX_DECOMPILED_CHARS,
            "ghidra_load_bias_by_analysis": {
                analysis_id: {
                    "decimal": bias,
                    "hex": f"0x{bias:x}",
                }
                for analysis_id, bias in sorted(
                    load_bias_by_analysis.items()
                )
            },
            "ghidra_versions": sorted(tool_versions),
            "static_adapter": "waved/scripts/npu/ghidra_abi.py",
        },
    }


def _atomic_write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=path.parent,
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def generate_ghidra_evidence(
    manifest: SerializerManifest,
    *,
    executor: Any | None = None,
) -> dict[str, Any]:
    document = build_serializer_ghidra_evidence(
        manifest, executor=executor
    )
    output = manifest.repository_root / manifest.outputs["ghidra"]
    _atomic_write(output, canonical_json(document))
    return document


def verify_ghidra_evidence(
    manifest: SerializerManifest,
    *,
    executor: Any | None = None,
) -> None:
    generated = canonical_json(
        build_serializer_ghidra_evidence(
            manifest, executor=executor
        )
    )
    output = manifest.repository_root / manifest.outputs["ghidra"]
    try:
        tracked = output.read_bytes()
    except OSError as exc:
        raise SerializerError(
            "EVIDENCE_STALE",
            f"cannot read serializer Ghidra evidence: {exc}",
            location=manifest.outputs["ghidra"].as_posix(),
        ) from exc
    if tracked != generated:
        raise SerializerError(
            "EVIDENCE_STALE",
            "tracked serializer Ghidra evidence differs",
            location=manifest.outputs["ghidra"].as_posix(),
        )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Host-only Wave NBG serializer recovery."
    )
    parser.add_argument(
        "--manifest", type=Path, default=DEFAULT_MANIFEST_PATH
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser(
        "validate", help="validate the strict manifest and pinned artifacts"
    )
    subparsers.add_parser(
        "generate-ghidra",
        help="run bounded host-only serializer Ghidra analysis",
    )
    subparsers.add_parser(
        "verify-ghidra",
        help="rerun and verify tracked serializer Ghidra evidence",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        manifest = load_manifest(args.manifest)
        if args.command == "validate":
            validated = validate_artifacts(manifest)
            print(
                f"serializer manifest valid: artifacts={len(validated)} "
                f"functions={len(APPROVED_FUNCTIONS)}"
            )
            return 0
        if args.command == "generate-ghidra":
            document = generate_ghidra_evidence(manifest)
            function_count = sum(
                len(batch["functions"])
                for batch in document["batches"]
            )
            print(
                "serializer Ghidra evidence generated: "
                f"functions={function_count}"
            )
            return 0
        if args.command == "verify-ghidra":
            verify_ghidra_evidence(manifest)
            print("serializer Ghidra evidence is current")
            return 0
        return 2
    except SerializerError as exc:
        print(f"nbg-serializer: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
