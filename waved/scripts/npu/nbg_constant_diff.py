#!/usr/bin/env python3
"""Bounded PoC 12B differential analysis for three fixed Wave NBGs.

This module never executes, loads, or writes an NBG.  It admits immutable
VPMN v2 payloads, proves their unchanged round trip, and emits bounded JSON
evidence about A1/A2 determinism and the controlled A/B source-constant
change.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import sys
import tempfile
import zlib
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Mapping, Sequence

try:
    from . import nbg_format, nbg_image
except ImportError:
    import nbg_format  # type: ignore
    import nbg_image  # type: ignore


SCHEMA_VERSION = 1
TARGET = 0x1000000E
MAX_NBG_BYTES = 64 * 1024 * 1024
SOURCE_A = bytes((0, 1, 2, 3))
SOURCE_B = bytes((0, 1, 2, 4))
FOLDED_OUTPUT = bytes((0, 1, 1, 1))
SEARCH_SECTION_TYPES = frozenset(
    {4, 5, 6, 7, 8, 9, 10, 21, 23, 24, 27}
)
CHECKSUM_SECTION_TYPE = 40
CHECKSUM_RECORD_BYTES = 8
HEADER_CRC_START = 12
HEADER_CRC_END = 16
EDGE_BYTES = 16


class DifferentialError(Exception):
    """Expected admission, analysis, or evidence-verification failure."""


@dataclass(frozen=True)
class ByteDifference:
    """One changed file byte before adjacent differences are coalesced."""

    offset: int
    old: int
    new: int
    owner_key: tuple[Any, ...]


@dataclass(frozen=True)
class DifferenceRun:
    """One maximal adjacent changed range under one byte owner."""

    start: int
    end: int
    old: bytes = field(repr=False)
    new: bytes = field(repr=False)
    owner: str
    byte_class: str
    section_index: int | None
    section_type: int | None
    section_type_name: str | None
    section_relative_offset: int | None
    classification: str = "unresolved"
    candidate_interpretations: tuple[str, ...] = ()

    @property
    def length(self) -> int:
        return self.end - self.start

    def to_document(self) -> dict[str, Any]:
        return {
            "byte_class": self.byte_class,
            "candidate_interpretations": list(
                self.candidate_interpretations
            ),
            "classification": self.classification,
            "end": self.end,
            "end_hex": f"0x{self.end:08x}",
            "length": self.length,
            "new_hex": _bounded_hex(self.new),
            "new_sha256": _sha256(self.new),
            "old_hex": _bounded_hex(self.old),
            "old_sha256": _sha256(self.old),
            "owner": self.owner,
            "section_index": self.section_index,
            "section_relative_offset": self.section_relative_offset,
            "section_type": self.section_type,
            "section_type_name": self.section_type_name,
            "start": self.start,
            "start_hex": f"0x{self.start:08x}",
        }


@dataclass(frozen=True)
class SectionDifference:
    """A bounded description of a section whose bytes differ."""

    index: int
    section_type: int
    type_name: str
    a_sha256: str
    b_sha256: str
    a_crc32: int
    b_crc32: int
    size: int

    def to_document(self) -> dict[str, Any]:
        return {
            "a_crc32": f"0x{self.a_crc32:08x}",
            "a_sha256": self.a_sha256,
            "b_crc32": f"0x{self.b_crc32:08x}",
            "b_sha256": self.b_sha256,
            "index": self.index,
            "section_type": self.section_type,
            "size": self.size,
            "type_name": self.type_name,
        }


@dataclass(frozen=True)
class IntegrityCollateral:
    """A type-40 checksum transition proved from a changed section."""

    section_index: int
    section_type: int
    checksum_record_index: int
    checksum_value_offset: int
    a_computed_crc32: int
    b_computed_crc32: int
    a_stored_crc32: int
    b_stored_crc32: int
    explained: bool

    def to_document(self) -> dict[str, Any]:
        return {
            "a_computed_crc32": f"0x{self.a_computed_crc32:08x}",
            "a_stored_crc32": f"0x{self.a_stored_crc32:08x}",
            "b_computed_crc32": f"0x{self.b_computed_crc32:08x}",
            "b_stored_crc32": f"0x{self.b_stored_crc32:08x}",
            "checksum_record_index": self.checksum_record_index,
            "checksum_value_offset": self.checksum_value_offset,
            "explained": self.explained,
            "section_index": self.section_index,
            "section_type": self.section_type,
        }


@dataclass(frozen=True)
class DeterminismResult:
    """Exact A1/A2 control comparison."""

    exact_match: bool
    first_difference: int | None
    a1_bytes: int
    a2_bytes: int
    a1_sha256: str
    a2_sha256: str
    structural_signature_equal: bool
    section_descriptors_equal: bool

    def to_document(self) -> dict[str, Any]:
        return {
            "a1_bytes": self.a1_bytes,
            "a1_sha256": self.a1_sha256,
            "a2_bytes": self.a2_bytes,
            "a2_sha256": self.a2_sha256,
            "exact_match": self.exact_match,
            "first_difference": self.first_difference,
            "section_descriptors_equal": self.section_descriptors_equal,
            "structural_signature_equal": (
                self.structural_signature_equal
            ),
        }


@dataclass(frozen=True)
class StructuralComparison:
    """A/B topology and stable-byte comparison."""

    compatible: bool
    equal_size: bool
    directory_descriptors_equal: bool
    tensor_metadata_equal: bool
    network_metadata_equal: bool
    memory_metadata_equal: bool
    stable_section_bytes_equal: bool
    differing_stable_sections: tuple[int, ...]

    def to_document(self) -> dict[str, Any]:
        return {
            "compatible": self.compatible,
            "differing_stable_sections": list(
                self.differing_stable_sections
            ),
            "directory_descriptors_equal": (
                self.directory_descriptors_equal
            ),
            "equal_size": self.equal_size,
            "memory_metadata_equal": self.memory_metadata_equal,
            "network_metadata_equal": self.network_metadata_equal,
            "stable_section_bytes_equal": (
                self.stable_section_bytes_equal
            ),
            "tensor_metadata_equal": self.tensor_metadata_equal,
        }


@dataclass(frozen=True)
class MappingCandidate:
    """One finite, exact A/B source representation at a shared offset."""

    transform_id: str
    section_index: int
    section_type: int
    type_name: str
    file_offset: int
    section_relative_offset: int
    encoded_a: bytes = field(repr=False)
    encoded_b: bytes = field(repr=False)

    def to_document(self) -> dict[str, Any]:
        return {
            "encoded_bytes": len(self.encoded_a),
            "file_offset": self.file_offset,
            "file_offset_hex": f"0x{self.file_offset:08x}",
            "section_index": self.section_index,
            "section_relative_offset": self.section_relative_offset,
            "section_type": self.section_type,
            "transform_id": self.transform_id,
            "type_name": self.type_name,
        }


@dataclass(frozen=True)
class DifferentialResult:
    """Complete bounded analysis result."""

    document: Mapping[str, Any]

    def to_document(self) -> dict[str, Any]:
        return dict(self.document)


@dataclass(frozen=True)
class FixtureManifest:
    """Exact local file identities for deterministic evidence generation."""

    a1_path: Path
    a2_path: Path
    b_path: Path
    evidence_path: Path
    a1_bytes: int
    a2_bytes: int
    b_bytes: int
    a1_sha256: str
    a2_sha256: str
    b_sha256: str


@dataclass(frozen=True)
class _Admitted:
    payload: bytes = field(repr=False)
    parsed: Mapping[str, Any]
    image: nbg_image.NbgImage


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _bounded_hex(payload: bytes) -> str:
    if len(payload) <= EDGE_BYTES * 2:
        return payload.hex()
    return f"{payload[:EDGE_BYTES].hex()}...{payload[-EDGE_BYTES:].hex()}"


def _first_difference(left: bytes, right: bytes) -> int | None:
    for index, (old, new) in enumerate(zip(left, right)):
        if old != new:
            return index
    if len(left) != len(right):
        return min(len(left), len(right))
    return None


def _admit(payload: bytes, artifact_id: str) -> _Admitted:
    if not isinstance(payload, bytes):
        raise DifferentialError(f"{artifact_id}: payload must be bytes")
    if not 513 <= len(payload) <= MAX_NBG_BYTES:
        raise DifferentialError(
            f"{artifact_id}: NBG size {len(payload)} is outside policy"
        )
    try:
        parsed = nbg_format.parse_bytes(
            payload, display_path=f"<{artifact_id}>"
        )
        image = nbg_image.NbgImage.parse(
            payload,
            artifact_id=artifact_id,
            display_path=f"<{artifact_id}>",
        )
        image.assert_total_ownership()
        rebuilt = image.to_bytes()
    except Exception as exc:
        raise DifferentialError(
            f"{artifact_id}: strict VPMN admission failed: {exc}"
        ) from exc
    if (
        parsed.get("format_family") != "v2-directory"
        or parsed.get("version") != nbg_format.V2_VERSION
        or parsed.get("target") != TARGET
    ):
        raise DifferentialError(
            f"{artifact_id}: expected Wave-targeted exact VPMN v2"
        )
    if rebuilt != payload:
        raise DifferentialError(
            f"{artifact_id}: unchanged round trip was not exact"
        )
    if any(not item.get("matches") for item in parsed.get("checksums", [])):
        raise DifferentialError(
            f"{artifact_id}: a stored section checksum did not validate"
        )
    return _Admitted(payload=payload, parsed=parsed, image=image)


def _section_descriptors(parsed: Mapping[str, Any]) -> tuple[Any, ...]:
    return tuple(
        (
            item["index"],
            item["type"],
            item["offset"],
            item["size"],
            item["stride"],
            item["alignment"],
            item["load_address"],
            tuple(item["reserved_words"]),
        )
        for item in parsed["sections"]
    )


def _determinism(a1: _Admitted, a2: _Admitted) -> DeterminismResult:
    return DeterminismResult(
        exact_match=a1.payload == a2.payload,
        first_difference=_first_difference(a1.payload, a2.payload),
        a1_bytes=len(a1.payload),
        a2_bytes=len(a2.payload),
        a1_sha256=_sha256(a1.payload),
        a2_sha256=_sha256(a2.payload),
        structural_signature_equal=(
            a1.parsed["structural_signature"]
            == a2.parsed["structural_signature"]
        ),
        section_descriptors_equal=(
            _section_descriptors(a1.parsed)
            == _section_descriptors(a2.parsed)
        ),
    )


def _permutation(source: bytes, indices: tuple[int, ...]) -> bytes:
    return bytes(source[index] for index in indices)


def _transform_registry(
    section_alignment: int,
) -> tuple[tuple[str, bytes, bytes], ...]:
    permutations = (
        ("identity", (0, 1, 2, 3)),
        ("fixed-block-permute:reverse-4", (3, 2, 1, 0)),
        ("fixed-block-permute:even-odd-4", (0, 2, 1, 3)),
        ("fixed-block-permute:odd-even-4", (1, 3, 0, 2)),
        ("byte-lane-permute:adjacent-pair-swap-4", (1, 0, 3, 2)),
    )
    result = [
        (
            transform_id,
            _permutation(SOURCE_A, indices),
            _permutation(SOURCE_B, indices),
        )
        for transform_id, indices in permutations
    ]
    if 4 < section_alignment <= 256:
        padding = (-4) % section_alignment
        if padding:
            result.append(
                (
                    f"identity+zero-pad-to-{section_alignment}",
                    SOURCE_A + bytes(padding),
                    SOURCE_B + bytes(padding),
                )
            )
    return tuple(result)


def _mapping_candidates(a: _Admitted, b: _Admitted) -> tuple[
    MappingCandidate, ...
]:
    b_sections = {item.index: item for item in b.image.sections}
    found: list[MappingCandidate] = []
    seen_locations: set[tuple[int, int]] = set()
    for section in a.image.sections:
        if section.section_type not in SEARCH_SECTION_TYPES:
            continue
        other = b_sections.get(section.index)
        if (
            other is None
            or other.section_type != section.section_type
            or other.size != section.size
        ):
            continue
        a_section = a.image.section_bytes(section.index)
        b_section = b.image.section_bytes(section.index)
        for transform_id, encoded_a, encoded_b in _transform_registry(
            section.alignment
        ):
            if len(encoded_a) > len(a_section):
                continue
            cursor = 0
            while True:
                relative = a_section.find(encoded_a, cursor)
                if relative < 0:
                    break
                if (
                    b_section[
                        relative : relative + len(encoded_b)
                    ] == encoded_b
                    and (section.index, relative) not in seen_locations
                ):
                    found.append(
                        MappingCandidate(
                            transform_id=transform_id,
                            section_index=section.index,
                            section_type=section.section_type,
                            type_name=section.type_name,
                            file_offset=section.offset + relative,
                            section_relative_offset=relative,
                            encoded_a=encoded_a,
                            encoded_b=encoded_b,
                        )
                    )
                    seen_locations.add((section.index, relative))
                cursor = relative + 1
    return tuple(
        sorted(
            found,
            key=lambda item: (
                item.file_offset,
                item.section_index,
                item.transform_id,
            ),
        )
    )


def _structural_comparison(
    a: _Admitted,
    b: _Admitted,
    mapping: MappingCandidate | None,
) -> StructuralComparison:
    descriptors_equal = (
        _section_descriptors(a.parsed) == _section_descriptors(b.parsed)
    )
    ignored_indices = {
        section.index
        for section in a.image.sections
        if section.section_type == CHECKSUM_SECTION_TYPE
    }
    if mapping is not None:
        ignored_indices.add(mapping.section_index)
    b_sections = {item.index: item for item in b.image.sections}
    differing_stable: list[int] = []
    for section in a.image.sections:
        if section.index in ignored_indices:
            continue
        other = b_sections.get(section.index)
        if (
            other is None
            or other.section_type != section.section_type
            or a.image.section_bytes(section.index)
            != b.image.section_bytes(section.index)
        ):
            differing_stable.append(section.index)
    tensors_equal = (
        a.parsed.get("inputs") == b.parsed.get("inputs")
        and a.parsed.get("outputs") == b.parsed.get("outputs")
    )
    network_equal = (
        a.parsed.get("network_info") == b.parsed.get("network_info")
    )
    memory_equal = a.parsed.get("memory") == b.parsed.get("memory")
    equal_size = len(a.payload) == len(b.payload)
    stable_equal = not differing_stable
    compatible = all(
        (
            equal_size,
            descriptors_equal,
            tensors_equal,
            network_equal,
            memory_equal,
            stable_equal,
        )
    )
    return StructuralComparison(
        compatible=compatible,
        equal_size=equal_size,
        directory_descriptors_equal=descriptors_equal,
        tensor_metadata_equal=tensors_equal,
        network_metadata_equal=network_equal,
        memory_metadata_equal=memory_equal,
        stable_section_bytes_equal=stable_equal,
        differing_stable_sections=tuple(differing_stable),
    )


def _owner_at(
    image: nbg_image.NbgImage, offset: int
) -> nbg_image.NbgOwnedRange | None:
    for owned in image.ranges:
        if owned.start <= offset < owned.end:
            return owned
    return None


def _owner_key(
    image: nbg_image.NbgImage, offset: int
) -> tuple[Any, ...]:
    owned = _owner_at(image, offset)
    if owned is None:
        return ("unowned", None, None)
    return (
        owned.owner,
        owned.byte_class.value,
        owned.section_index,
    )


def _enumerate_runs(a: _Admitted, b: _Admitted) -> tuple[
    DifferenceRun, ...
]:
    limit = min(len(a.payload), len(b.payload))
    differences = [
        ByteDifference(
            offset=index,
            old=a.payload[index],
            new=b.payload[index],
            owner_key=_owner_key(a.image, index),
        )
        for index in range(limit)
        if a.payload[index] != b.payload[index]
    ]
    if len(a.payload) != len(b.payload):
        longer = a.payload if len(a.payload) > len(b.payload) else b.payload
        for index in range(limit, len(longer)):
            differences.append(
                ByteDifference(
                    offset=index,
                    old=a.payload[index] if index < len(a.payload) else -1,
                    new=b.payload[index] if index < len(b.payload) else -1,
                    owner_key=_owner_key(a.image, index),
                )
            )
    groups: list[list[ByteDifference]] = []
    for item in differences:
        if (
            not groups
            or item.offset != groups[-1][-1].offset + 1
            or item.owner_key != groups[-1][-1].owner_key
            or item.old < 0
            or item.new < 0
        ):
            groups.append([item])
        else:
            groups[-1].append(item)
    sections = {item.index: item for item in a.image.sections}
    runs: list[DifferenceRun] = []
    for group in groups:
        start = group[0].offset
        end = group[-1].offset + 1
        owned = _owner_at(a.image, start)
        section = (
            sections.get(owned.section_index)
            if owned is not None and owned.section_index is not None
            else None
        )
        old = bytes(item.old for item in group if item.old >= 0)
        new = bytes(item.new for item in group if item.new >= 0)
        runs.append(
            DifferenceRun(
                start=start,
                end=end,
                old=old,
                new=new,
                owner=owned.owner if owned is not None else "unowned-tail",
                byte_class=(
                    owned.byte_class.value
                    if owned is not None
                    else "unowned"
                ),
                section_index=(
                    section.index if section is not None else None
                ),
                section_type=(
                    section.section_type if section is not None else None
                ),
                section_type_name=(
                    section.type_name if section is not None else None
                ),
                section_relative_offset=(
                    start - section.offset if section is not None else None
                ),
            )
        )
    return tuple(runs)


def _section_differences(
    a: _Admitted, b: _Admitted
) -> tuple[SectionDifference, ...]:
    b_sections = {item.index: item for item in b.image.sections}
    result: list[SectionDifference] = []
    for section in a.image.sections:
        other = b_sections.get(section.index)
        if (
            other is None
            or other.section_type != section.section_type
            or other.size != section.size
        ):
            continue
        left = a.image.section_bytes(section.index)
        right = b.image.section_bytes(section.index)
        if left == right:
            continue
        result.append(
            SectionDifference(
                index=section.index,
                section_type=section.section_type,
                type_name=section.type_name,
                a_sha256=_sha256(left),
                b_sha256=_sha256(right),
                a_crc32=zlib.crc32(left) & 0xFFFFFFFF,
                b_crc32=zlib.crc32(right) & 0xFFFFFFFF,
                size=section.size,
            )
        )
    return tuple(result)


def _checksum_records(
    admitted: _Admitted,
) -> Mapping[int, tuple[int, int, int]]:
    records: dict[int, tuple[int, int, int]] = {}
    for section in admitted.image.sections:
        if section.section_type != CHECKSUM_SECTION_TYPE:
            continue
        payload = admitted.image.section_bytes(section.index)
        if len(payload) % CHECKSUM_RECORD_BYTES:
            raise DifferentialError("checksum section has invalid geometry")
        for index in range(len(payload) // CHECKSUM_RECORD_BYTES):
            section_type, checksum = struct.unpack_from(
                "<II", payload, index * CHECKSUM_RECORD_BYTES
            )
            if section_type in records:
                raise DifferentialError(
                    f"duplicate checksum for section type {section_type}"
                )
            records[section_type] = (
                checksum,
                section.offset + index * CHECKSUM_RECORD_BYTES + 4,
                index,
            )
    return records


def _integrity_collateral(
    a: _Admitted,
    b: _Admitted,
    changed_sections: Sequence[SectionDifference],
) -> tuple[IntegrityCollateral, ...]:
    a_records = _checksum_records(a)
    b_records = _checksum_records(b)
    result: list[IntegrityCollateral] = []
    for section in changed_sections:
        if section.section_type == CHECKSUM_SECTION_TYPE:
            continue
        a_record = a_records.get(section.section_type)
        b_record = b_records.get(section.section_type)
        if a_record is None or b_record is None:
            continue
        explained = (
            a_record[0] == section.a_crc32
            and b_record[0] == section.b_crc32
            and a_record[1] == b_record[1]
            and a_record[2] == b_record[2]
        )
        result.append(
            IntegrityCollateral(
                section_index=section.index,
                section_type=section.section_type,
                checksum_record_index=a_record[2],
                checksum_value_offset=a_record[1],
                a_computed_crc32=section.a_crc32,
                b_computed_crc32=section.b_crc32,
                a_stored_crc32=a_record[0],
                b_stored_crc32=b_record[0],
                explained=explained,
            )
        )
    return tuple(result)


def _classify_runs(
    runs: Sequence[DifferenceRun],
    mapping: MappingCandidate | None,
    collateral: Sequence[IntegrityCollateral],
) -> tuple[DifferenceRun, ...]:
    result: list[DifferenceRun] = []
    mapping_start = mapping.file_offset if mapping is not None else -1
    mapping_end = (
        mapping.file_offset + len(mapping.encoded_a)
        if mapping is not None
        else -1
    )
    checksum_ranges = {
        (item.checksum_value_offset, item.checksum_value_offset + 4)
        for item in collateral
        if item.explained
    }
    for run in runs:
        classification = "unresolved"
        interpretations: tuple[str, ...] = ()
        if (
            mapping is not None
            and mapping_start <= run.start
            and run.end <= mapping_end
        ):
            classification = "source-constant"
            interpretations = (mapping.transform_id,)
        elif any(
            start <= run.start and run.end <= end
            for start, end in checksum_ranges
        ):
            classification = "explained-integrity-collateral"
            interpretations = ("v2-section-crc32",)
        elif (
            HEADER_CRC_START <= run.start
            and run.end <= HEADER_CRC_END
        ):
            classification = "producer-integrity-collateral"
            interpretations = ("unresolved-header-crc-sum",)
        result.append(
            replace(
                run,
                classification=classification,
                candidate_interpretations=interpretations,
            )
        )
    return tuple(result)


def _reconstruct(
    a: _Admitted,
    b: _Admitted,
    mapping: MappingCandidate | None,
    collateral: Sequence[IntegrityCollateral],
    runs: Sequence[DifferenceRun],
) -> dict[str, Any]:
    if mapping is None:
        return {
            "attempted": False,
            "exact_match": False,
            "first_difference": None,
            "reason": "no unique finite source mapping",
        }
    if any(
        run.classification
        not in {"source-constant", "explained-integrity-collateral"}
        for run in runs
    ):
        return {
            "attempted": False,
            "exact_match": False,
            "first_difference": None,
            "reason": "one or more changed runs remain unresolved",
        }
    rebuilt = bytearray(a.payload)
    start = mapping.file_offset
    end = start + len(mapping.encoded_b)
    if bytes(rebuilt[start:end]) != mapping.encoded_a:
        raise DifferentialError(
            "internal source reconstruction precondition failed"
        )
    rebuilt[start:end] = mapping.encoded_b
    sections = {item.index: item for item in a.image.sections}
    for item in collateral:
        if not item.explained:
            continue
        section = sections[item.section_index]
        checksum = zlib.crc32(
            rebuilt[section.offset : section.end]
        ) & 0xFFFFFFFF
        struct.pack_into(
            "<I", rebuilt, item.checksum_value_offset, checksum
        )
    rebuilt_bytes = bytes(rebuilt)
    return {
        "attempted": True,
        "exact_match": rebuilt_bytes == b.payload,
        "first_difference": _first_difference(rebuilt_bytes, b.payload),
        "reconstructed_sha256": _sha256(rebuilt_bytes),
        "written_to_disk": False,
    }


def analyze_payloads(a1_payload: bytes, a2_payload: bytes, b_payload: bytes) -> dict[
    str, Any
]:
    """Analyze the exact A1/A2/B experiment without writing NBG bytes."""

    a1 = _admit(a1_payload, "a1")
    a2 = _admit(a2_payload, "a2")
    determinism = _determinism(a1, a2)
    base: dict[str, Any] = {
        "artifacts": {
            "a1": {
                "bytes": len(a1.payload),
                "sha256": _sha256(a1.payload),
            },
            "a2": {
                "bytes": len(a2.payload),
                "sha256": _sha256(a2.payload),
            },
        },
        "determinism": determinism.to_document(),
        "finite_registry": {
            "folded_output_u8": list(FOLDED_OUTPUT),
            "search_section_types": sorted(SEARCH_SECTION_TYPES),
            "source_a_u8": list(SOURCE_A),
            "source_b_u8": list(SOURCE_B),
            "transforms": [
                "identity",
                "fixed-block-permute:reverse-4",
                "fixed-block-permute:even-odd-4",
                "fixed-block-permute:odd-even-4",
                "byte-lane-permute:adjacent-pair-swap-4",
                "identity+zero-pad-to-recorded-alignment",
            ],
        },
        "schema_version": SCHEMA_VERSION,
        "scope": {
            "graph_execution": False,
            "nbg_loading": False,
            "raw_payload_emitted": False,
            "result_applies_to": "one fixed U8 constant-input Erf graph",
        },
    }
    if not determinism.exact_match:
        base.update(
            {
                "classification": "serializer-nondeterministic",
                "difference_runs": [],
                "integrity_collateral": [],
                "mapping_candidates": [],
                "observed_b_reconstruction": {
                    "attempted": False,
                    "exact_match": False,
                    "first_difference": None,
                    "reason": "B was not interpreted after determinism failure",
                },
                "poc12c": "not-ready",
                "structural_comparison": {
                    "compatible": False,
                    "reason": "B was not interpreted after determinism failure",
                },
            }
        )
        return base

    b = _admit(b_payload, "b")
    base["artifacts"]["b"] = {
        "bytes": len(b.payload),
        "sha256": _sha256(b.payload),
    }
    if a1.payload == b.payload:
        structural = _structural_comparison(a1, b, None)
        base.update(
            {
                "classification": "constant-folded-or-elided",
                "difference_runs": [],
                "integrity_collateral": [],
                "mapping_candidates": [],
                "observed_b_reconstruction": {
                    "attempted": False,
                    "exact_match": True,
                    "first_difference": None,
                    "reason": (
                        "A and B NBGs are identical while their fixed "
                        "source constants differ and numerical oracles match"
                    ),
                },
                "poc12c": "not-ready",
                "structural_comparison": structural.to_document(),
            }
        )
        return base

    mappings = _mapping_candidates(a1, b)
    unique_mapping = mappings[0] if len(mappings) == 1 else None
    structural = _structural_comparison(a1, b, unique_mapping)
    raw_runs = _enumerate_runs(a1, b)
    changed_sections = _section_differences(a1, b)
    collateral = _integrity_collateral(a1, b, changed_sections)
    runs = _classify_runs(raw_runs, unique_mapping, collateral)
    reconstruction = _reconstruct(
        a1, b, unique_mapping, collateral, runs
    )
    all_runs_explained = all(
        item.classification
        in {"source-constant", "explained-integrity-collateral"}
        for item in runs
    )
    all_collateral_explained = all(
        item.explained for item in collateral
    )
    has_source_run = any(
        item.classification == "source-constant" for item in runs
    )
    mapped = (
        unique_mapping is not None
        and structural.compatible
        and all_runs_explained
        and all_collateral_explained
        and has_source_run
        and bool(reconstruction["exact_match"])
    )
    if mapped and unique_mapping.transform_id == "identity":
        classification = "direct-constant-mapping"
    elif mapped:
        classification = "deterministic-transformed-mapping"
    else:
        classification = "multi-region-unresolved"
    header_changed = a1.payload[
        HEADER_CRC_START:HEADER_CRC_END
    ] != b.payload[HEADER_CRC_START:HEADER_CRC_END]
    base.update(
        {
            "changed_sections": [
                item.to_document() for item in changed_sections
            ],
            "classification": classification,
            "difference_runs": [item.to_document() for item in runs],
            "header_crc_sum": {
                "a_hex": a1.payload[
                    HEADER_CRC_START:HEADER_CRC_END
                ].hex(),
                "algorithm_proved": False,
                "b_hex": b.payload[
                    HEADER_CRC_START:HEADER_CRC_END
                ].hex(),
                "changed": header_changed,
            },
            "integrity_collateral": [
                item.to_document() for item in collateral
            ],
            "mapping_candidates": [
                item.to_document() for item in mappings
            ],
            "observed_b_reconstruction": reconstruction,
            "poc12c": "ready-limited" if mapped else "not-ready",
            "structural_comparison": structural.to_document(),
        }
    )
    return base


def canonical_json(value: Any) -> bytes:
    """Return canonical evidence JSON."""

    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def _regular_file(path: Path, expected_bytes: int, expected_hash: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise DifferentialError(f"{path}: expected a regular non-symlink file")
    payload = path.read_bytes()
    if len(payload) != expected_bytes:
        raise DifferentialError(
            f"{path}: expected {expected_bytes} bytes, observed {len(payload)}"
        )
    digest = _sha256(payload)
    if digest != expected_hash:
        raise DifferentialError(
            f"{path}: expected SHA-256 {expected_hash}, observed {digest}"
        )
    return payload


def fixture_manifest(
    a1_path: Path,
    a2_path: Path,
    b_path: Path,
    evidence_path: Path,
) -> FixtureManifest:
    """Freeze exact identities for caller-owned fixture or live files."""

    payloads = []
    for path in (a1_path, a2_path, b_path):
        if path.is_symlink() or not path.is_file():
            raise DifferentialError(
                f"{path}: expected a regular non-symlink file"
            )
        payload = path.read_bytes()
        payloads.append(payload)
    return FixtureManifest(
        a1_path=a1_path,
        a2_path=a2_path,
        b_path=b_path,
        evidence_path=evidence_path,
        a1_bytes=len(payloads[0]),
        a2_bytes=len(payloads[1]),
        b_bytes=len(payloads[2]),
        a1_sha256=_sha256(payloads[0]),
        a2_sha256=_sha256(payloads[1]),
        b_sha256=_sha256(payloads[2]),
    )


def _generated_document(manifest: FixtureManifest) -> dict[str, Any]:
    a1 = _regular_file(
        manifest.a1_path, manifest.a1_bytes, manifest.a1_sha256
    )
    a2 = _regular_file(
        manifest.a2_path, manifest.a2_bytes, manifest.a2_sha256
    )
    b = _regular_file(
        manifest.b_path, manifest.b_bytes, manifest.b_sha256
    )
    return analyze_payloads(a1, a2, b)


def generate_evidence(manifest: FixtureManifest) -> None:
    """Atomically generate bounded evidence; never write an NBG."""

    payload = canonical_json(_generated_document(manifest))
    manifest.evidence_path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{manifest.evidence_path.name}.",
        suffix=".tmp",
        dir=manifest.evidence_path.parent,
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, manifest.evidence_path)
    except Exception:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def verify_generated(manifest: FixtureManifest) -> None:
    """Reject missing, symlinked, or stale generated evidence."""

    if (
        manifest.evidence_path.is_symlink()
        or not manifest.evidence_path.is_file()
    ):
        raise DifferentialError("generated evidence is missing or unsafe")
    expected = canonical_json(_generated_document(manifest))
    observed = manifest.evidence_path.read_bytes()
    if observed != expected:
        raise DifferentialError("generated evidence is stale")


def _run_manifest(run: Path, evidence: Path) -> FixtureManifest:
    return fixture_manifest(
        run / "a1.nbg",
        run / "a2.nbg",
        run / "b.nbg",
        evidence,
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Analyze the fixed PoC 12B A1/A2/B NBG differential"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    analyze = subparsers.add_parser("analyze")
    analyze.add_argument("--run", type=Path, required=True)
    analyze.add_argument("--output", type=Path, required=True)
    verify = subparsers.add_parser("verify-generated")
    verify.add_argument(
        "--run",
        type=Path,
        default=Path(
            "artifacts/npu/runs/"
            "poc12b-erf-constant-diff-20260729"
        ),
    )
    verify.add_argument(
        "--output",
        type=Path,
        default=Path(
            "docs/npu/format/erf-constant-differential.json"
        ),
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        manifest = _run_manifest(args.run, args.output)
        if args.command == "analyze":
            generate_evidence(manifest)
        else:
            verify_generated(manifest)
        return 0
    except DifferentialError as exc:
        print(f"poc12b-diff: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
