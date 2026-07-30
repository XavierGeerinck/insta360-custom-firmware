#!/usr/bin/env python3
"""Lossless, immutable VPMN v2 image and unchanged-byte writer."""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import tempfile
from dataclasses import dataclass, field, replace
from enum import Enum
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping, Sequence

try:
    from . import nbg_format, nbg_serializer_recovery as recovery
except ImportError:
    import nbg_format  # type: ignore
    import nbg_serializer_recovery as recovery  # type: ignore


IMAGE_SCHEMA_VERSION = 1


class ByteClass(str, Enum):
    """Exclusive ownership class for one original byte range."""

    DECODED_RECORD = "decoded-record"
    ALIGNMENT = "alignment"
    PADDING = "padding"
    PRESERVED_OPAQUE = "preserved-opaque"
    CONNECTION_TAIL = "connection-tail"


DECODED_SECTION_SCHEMAS: Mapping[int, tuple[str, int]] = {
    1: ("v2-input-record", nbg_format.V2_IO_RECORD_BYTES),
    2: ("v2-output-record", nbg_format.V2_IO_RECORD_BYTES),
    35: ("v2-network-info", nbg_format.V2_NETWORK_INFO_BYTES),
    37: ("v2-memory-pool-record", nbg_format.V2_MEMORY_POOL_BYTES),
    38: ("v2-vip-sram-record", nbg_format.V2_SRAM_BYTES),
    39: ("v2-axi-sram-record", nbg_format.V2_SRAM_BYTES),
    40: ("v2-checksum-record", nbg_format.V2_CHECKSUM_RECORD_BYTES),
}


@dataclass(frozen=True)
class NbgOwnedRange:
    """One exact source interval with one exclusive owner."""

    start: int
    end: int
    byte_class: ByteClass
    owner: str
    schema: str | None
    raw: bytes = field(repr=False)
    raw_sha256: str
    section_index: int | None = None
    notes: str | None = None

    @property
    def length(self) -> int:
        return self.end - self.start

    def to_document(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "class": self.byte_class.value,
            "end": self.end,
            "end_hex": f"0x{self.end:08x}",
            "length": self.length,
            "owner": self.owner,
            "raw_sha256": self.raw_sha256,
            "start": self.start,
            "start_hex": f"0x{self.start:08x}",
        }
        if self.notes is not None:
            result["notes"] = self.notes
        if self.schema is not None:
            result["schema"] = self.schema
        if self.section_index is not None:
            result["section_index"] = self.section_index
        return result


@dataclass(frozen=True)
class NbgSectionView:
    """Immutable directory view for one VPMN v2 section."""

    index: int
    section_type: int
    type_name: str
    offset: int
    size: int
    stride: int
    alignment: int
    record_count: int | None
    classification: str

    @property
    def end(self) -> int:
        return self.offset + self.size

    def to_document(self) -> dict[str, Any]:
        return {
            "alignment": self.alignment,
            "classification": self.classification,
            "end": self.end,
            "index": self.index,
            "offset": self.offset,
            "record_count": self.record_count,
            "size": self.size,
            "stride": self.stride,
            "type": self.section_type,
            "type_name": self.type_name,
        }


@dataclass(frozen=True)
class RoundtripResult:
    """Byte-exact reconstruction result for one pinned NBG."""

    artifact_id: str
    path: str
    input_bytes: int
    output_bytes: int
    input_sha256: str
    output_sha256: str
    exact_match: bool
    first_difference: int | None
    range_count: int
    class_byte_totals: Mapping[str, int]
    connection_tail_mode: str
    image: NbgImage = field(repr=False, compare=False)

    def to_document(self) -> dict[str, Any]:
        return {
            "artifact_id": self.artifact_id,
            "class_byte_totals": dict(self.class_byte_totals),
            "connection_tail_mode": self.connection_tail_mode,
            "exact_match": self.exact_match,
            "first_difference": self.first_difference,
            "input_bytes": self.input_bytes,
            "input_sha256": self.input_sha256,
            "output_bytes": self.output_bytes,
            "output_sha256": self.output_sha256,
            "path": self.path,
            "range_count": self.range_count,
        }


@dataclass(frozen=True)
class NbgImage:
    """Immutable lossless image; emission is allowed only when unchanged."""

    artifact_id: str | None
    display_path: str
    source_size: int
    source_sha256: str
    version: int
    target: int
    ranges: tuple[NbgOwnedRange, ...]
    sections: tuple[NbgSectionView, ...]
    connection_tail_mode: str
    connection_tail_view_offset: int
    connection_tail_view_size: int
    changed: bool = False
    change_reason: str | None = None
    _source_bytes: bytes = field(default=b"", repr=False, compare=False)

    @classmethod
    def parse(
        cls,
        payload: bytes,
        *,
        artifact_id: str | None = None,
        display_path: str = "<memory>",
    ) -> NbgImage:
        """Parse one immutable VPMN v2 payload and own every byte."""

        if not isinstance(payload, bytes):
            raise recovery.SerializerError(
                "NBG_PARSE_FAILURE",
                "NBG image input must be immutable bytes",
                artifact_id=artifact_id,
                location=display_path,
            )
        try:
            parsed = nbg_format.parse_bytes(
                payload, display_path=display_path
            )
        except nbg_format.NbgParseError as exc:
            location = (
                f"0x{exc.offset:x}" if exc.offset is not None else exc.stage
            )
            raise recovery.SerializerError(
                "NBG_PARSE_FAILURE",
                str(exc),
                artifact_id=artifact_id,
                location=location,
            ) from exc
        if parsed.get("format_family") != "v2-directory":
            raise recovery.SerializerError(
                "NBG_PARSE_FAILURE",
                "lossless images in PoC 12A require VPMN v2",
                artifact_id=artifact_id,
                location="version",
            )
        if parsed.get("version") != nbg_format.V2_VERSION:
            raise recovery.SerializerError(
                "NBG_PARSE_FAILURE",
                "lossless images in PoC 12A require exact VPMN v2",
                artifact_id=artifact_id,
                location="version",
            )

        sections = tuple(
            NbgSectionView(
                index=_integer(item.get("index"), "section.index"),
                section_type=_integer(item.get("type"), "section.type"),
                type_name=_text(item.get("type_name"), "section.type_name"),
                offset=_integer(item.get("offset"), "section.offset"),
                size=_integer(item.get("size"), "section.size"),
                stride=_integer(item.get("stride"), "section.stride"),
                alignment=_integer(
                    item.get("alignment"), "section.alignment"
                ),
                record_count=_optional_integer(
                    item.get("record_count"), "section.record_count"
                ),
                classification=_text(
                    item.get("classification"), "section.classification"
                ),
            )
            for item in parsed["sections"]
        )
        base_ranges = _decoded_root_ranges(payload, parsed)
        for section in sections:
            base_ranges.extend(_section_ranges(payload, section, artifact_id))

        content_end = max(
            (item.end for item in base_ranges),
            default=0,
        )
        parser_tail = parsed["connection_tail"]
        tail_offset = _integer(
            parser_tail.get("offset"), "connection_tail.offset"
        )
        tail_size = _integer(
            parser_tail.get("size"), "connection_tail.size"
        )
        if tail_size != nbg_format.CONNECTION_TAIL_BYTES:
            raise recovery.SerializerError(
                "NBG_CONNECTION_TAIL_INVALID",
                "parser tail view has an unexpected size",
                artifact_id=artifact_id,
                location="connection_tail",
            )
        if tail_offset >= content_end:
            connection_mode = "independent"
            base_ranges.append(
                _owned_range(
                    payload,
                    tail_offset,
                    tail_offset + tail_size,
                    ByteClass.CONNECTION_TAIL,
                    "root.connection-tail",
                    "connection-tail",
                    notes="independent trailing bytes outside all sections",
                )
            )
        else:
            # Exact Wave VPMN v2 files commonly account for EOF through
            # section directory entries. The generic parser's last-512-byte
            # fingerprint is then an overlapping view, not another owner.
            connection_mode = "section-covered-view"

        ordered = _fill_and_validate_ranges(
            payload,
            base_ranges,
            sections,
            artifact_id=artifact_id,
            display_path=display_path,
        )
        image = cls(
            artifact_id=artifact_id,
            display_path=display_path,
            source_size=len(payload),
            source_sha256=hashlib.sha256(payload).hexdigest(),
            version=_integer(parsed.get("version"), "version"),
            target=_integer(parsed.get("target"), "target"),
            ranges=ordered,
            sections=sections,
            connection_tail_mode=connection_mode,
            connection_tail_view_offset=tail_offset,
            connection_tail_view_size=tail_size,
            _source_bytes=payload,
        )
        image.assert_total_ownership()
        return image

    def assert_total_ownership(self) -> None:
        """Prove exact, exclusive ownership of [0, source_size)."""

        cursor = 0
        for index, item in enumerate(self.ranges):
            if item.start > cursor:
                raise recovery.SerializerError(
                    "NBG_RANGE_GAP",
                    f"unowned bytes [0x{cursor:x},0x{item.start:x})",
                    artifact_id=self.artifact_id,
                    location=f"ranges[{index}]",
                )
            if item.start < cursor:
                raise recovery.SerializerError(
                    "NBG_RANGE_OVERLAP",
                    f"range starts at 0x{item.start:x} before prior end "
                    f"0x{cursor:x}",
                    artifact_id=self.artifact_id,
                    location=f"ranges[{index}]",
                )
            if (
                item.end <= item.start
                or item.end > self.source_size
                or len(item.raw) != item.length
            ):
                raise recovery.SerializerError(
                    "NBG_SECTION_GEOMETRY",
                    "owned range has invalid bounds or raw length",
                    artifact_id=self.artifact_id,
                    location=f"ranges[{index}]",
                )
            observed_hash = hashlib.sha256(item.raw).hexdigest()
            if observed_hash != item.raw_sha256:
                raise recovery.SerializerError(
                    "NBG_SECTION_GEOMETRY",
                    "owned range raw SHA-256 is stale",
                    artifact_id=self.artifact_id,
                    location=f"ranges[{index}]",
                )
            cursor = item.end
        if cursor != self.source_size:
            raise recovery.SerializerError(
                "NBG_RANGE_GAP",
                f"unowned bytes [0x{cursor:x},0x{self.source_size:x})",
                artifact_id=self.artifact_id,
                location="EOF",
            )

    def to_bytes(self) -> bytes:
        """Reconstruct unchanged bytes from owned ranges."""

        if self.changed:
            reason = self.change_reason or "unspecified semantic change"
            raise recovery.SerializerError(
                "NBG_SEMANTIC_CHANGE_REFUSED",
                f"unchanged writer refuses image: {reason}",
                artifact_id=self.artifact_id,
                location=self.display_path,
            )
        self.assert_total_ownership()
        rebuilt = b"".join(item.raw for item in self.ranges)
        if len(rebuilt) != self.source_size:
            raise recovery.SerializerError(
                "NBG_ROUNDTRIP_LENGTH_MISMATCH",
                f"rebuilt {len(rebuilt)} bytes, expected {self.source_size}",
                artifact_id=self.artifact_id,
                location=self.display_path,
            )
        if rebuilt != self._source_bytes:
            mismatch = next(
                (
                    index
                    for index, (left, right) in enumerate(
                        zip(rebuilt, self._source_bytes)
                    )
                    if left != right
                ),
                None,
            )
            raise recovery.SerializerError(
                "NBG_ROUNDTRIP_BYTE_MISMATCH",
                f"first rebuilt byte mismatch at 0x{mismatch or 0:x}",
                artifact_id=self.artifact_id,
                location=self.display_path,
            )
        digest = hashlib.sha256(rebuilt).hexdigest()
        if digest != self.source_sha256:
            raise recovery.SerializerError(
                "NBG_ROUNDTRIP_BYTE_MISMATCH",
                f"rebuilt SHA-256 is {digest}, expected "
                f"{self.source_sha256}",
                artifact_id=self.artifact_id,
                location=self.display_path,
            )
        return rebuilt

    def mark_changed(self, reason: str) -> NbgImage:
        """Return a changed image that the PoC 12A writer must refuse."""

        if not isinstance(reason, str) or not reason:
            raise ValueError("change reason must be a nonempty string")
        return replace(self, changed=True, change_reason=reason)

    def ranges_for_section(
        self, section_index: int
    ) -> tuple[NbgOwnedRange, ...]:
        self._section(section_index)
        return tuple(
            item
            for item in self.ranges
            if item.section_index == section_index
        )

    def section_bytes(self, section_index: int) -> bytes:
        section = self._section(section_index)
        payload = b"".join(
            item.raw for item in self.ranges_for_section(section_index)
        )
        if len(payload) != section.size:
            raise recovery.SerializerError(
                "NBG_SECTION_GEOMETRY",
                f"section {section_index} owns {len(payload)} bytes, "
                f"expected {section.size}",
                artifact_id=self.artifact_id,
                location=f"section[{section_index}]",
            )
        return payload

    def ownership_document(self) -> dict[str, Any]:
        totals = {item.value: 0 for item in ByteClass}
        for owned in self.ranges:
            totals[owned.byte_class.value] += owned.length
        return {
            "artifact_id": self.artifact_id,
            "class_byte_totals": totals,
            "connection_tail": {
                "mode": self.connection_tail_mode,
                "parser_view_offset": self.connection_tail_view_offset,
                "parser_view_size": self.connection_tail_view_size,
            },
            "path": self.display_path,
            "ranges": [item.to_document() for item in self.ranges],
            "schema_version": IMAGE_SCHEMA_VERSION,
            "sections": [item.to_document() for item in self.sections],
            "source_sha256": self.source_sha256,
            "source_size": self.source_size,
            "target": self.target,
            "target_hex": f"0x{self.target:08x}",
            "version": self.version,
            "version_hex": f"0x{self.version:08x}",
        }

    def _section(self, section_index: int) -> NbgSectionView:
        matches = [
            item for item in self.sections if item.index == section_index
        ]
        if len(matches) != 1:
            raise KeyError(f"unknown or duplicate section index {section_index}")
        return matches[0]


def _integer(value: Any, location: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            f"{location} must be a nonnegative integer",
            location=location,
        )
    return value


def _optional_integer(value: Any, location: str) -> int | None:
    if value is None:
        return None
    return _integer(value, location)


def _text(value: Any, location: str) -> str:
    if not isinstance(value, str) or not value:
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            f"{location} must be a nonempty string",
            location=location,
        )
    return value


def _owned_range(
    payload: bytes,
    start: int,
    end: int,
    byte_class: ByteClass,
    owner: str,
    schema: str | None,
    *,
    section_index: int | None = None,
    notes: str | None = None,
) -> NbgOwnedRange:
    if start < 0 or end <= start or end > len(payload):
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            f"invalid range [0x{start:x},0x{end:x})",
            location=owner,
        )
    raw = payload[start:end]
    return NbgOwnedRange(
        start=start,
        end=end,
        byte_class=byte_class,
        owner=owner,
        schema=schema,
        raw=raw,
        raw_sha256=hashlib.sha256(raw).hexdigest(),
        section_index=section_index,
        notes=notes,
    )


def _decoded_root_ranges(
    payload: bytes, parsed: Mapping[str, Any]
) -> list[NbgOwnedRange]:
    directory = parsed["directory"]
    header = parsed["header"]
    sub_header_offset = _integer(
        directory.get("sub_header_offset"), "directory.sub_header_offset"
    )
    sub_header_size = _integer(
        header.get("sub_header_size"), "header.sub_header_size"
    )
    records_offset = _integer(
        directory.get("section_headers_offset"),
        "directory.section_headers_offset",
    )
    record_size = _integer(
        directory.get("section_header_size"),
        "directory.section_header_size",
    )
    record_count = _integer(
        directory.get("section_header_count"),
        "directory.section_header_count",
    )
    if sub_header_size != nbg_format.V2_SUB_HEADER_BYTES:
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            "v2 sub-header size is not the proven 12-byte record",
            location="directory.sub-header",
        )
    if record_size != nbg_format.V2_DIRECTORY_RECORD_BYTES:
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            "v2 directory record size is not the proven 44-byte record",
            location="directory.records",
        )
    result = [
        _owned_range(
            payload,
            0,
            nbg_format.V2_HEADER_BYTES,
            ByteClass.DECODED_RECORD,
            "root.header",
            "v2-header",
        ),
        _owned_range(
            payload,
            sub_header_offset,
            sub_header_offset + sub_header_size,
            ByteClass.DECODED_RECORD,
            "root.directory.sub-header",
            "v2-directory-sub-header",
        ),
    ]
    for index in range(record_count):
        start = records_offset + index * record_size
        result.append(
            _owned_range(
                payload,
                start,
                start + record_size,
                ByteClass.DECODED_RECORD,
                f"root.directory.section[{index}]",
                "v2-directory-record",
                notes=f"descriptor for section index {index}",
            )
        )
    return result


def _section_ranges(
    payload: bytes,
    section: NbgSectionView,
    artifact_id: str | None,
) -> list[NbgOwnedRange]:
    if section.size == 0:
        return []
    decoded = DECODED_SECTION_SCHEMAS.get(section.section_type)
    if decoded is None:
        return [
            _owned_range(
                payload,
                section.offset,
                section.end,
                ByteClass.PRESERVED_OPAQUE,
                f"section[{section.index}]",
                None,
                section_index=section.index,
                notes=section.type_name,
            )
        ]
    schema, stride = decoded
    if section.stride != stride or section.size % stride:
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            f"decoded section {section.index} has size/stride "
            f"{section.size}/{section.stride}, expected stride {stride}",
            artifact_id=artifact_id,
            location=f"section[{section.index}]",
        )
    result = []
    for record_index in range(section.size // stride):
        start = section.offset + record_index * stride
        result.append(
            _owned_range(
                payload,
                start,
                start + stride,
                ByteClass.DECODED_RECORD,
                f"section[{section.index}].record[{record_index}]",
                schema,
                section_index=section.index,
                notes=section.type_name,
            )
        )
    return result


def _gap_ranges(
    payload: bytes,
    start: int,
    end: int,
    *,
    next_range: NbgOwnedRange,
    section_by_index: Mapping[int, NbgSectionView],
) -> list[NbgOwnedRange]:
    if end <= start:
        return []
    section = (
        section_by_index.get(next_range.section_index)
        if next_range.section_index is not None
        else None
    )
    alignment_bytes = 0
    if (
        section is not None
        and next_range.start == section.offset
        and section.alignment > 1
    ):
        if section.offset % section.alignment:
            raise recovery.SerializerError(
                "NBG_SECTION_GEOMETRY",
                f"section {section.index} offset 0x{section.offset:x} "
                f"violates alignment {section.alignment}",
                location=f"section[{section.index}]",
            )
        alignment_bytes = (-start) % section.alignment
        if alignment_bytes > end - start:
            raise recovery.SerializerError(
                "NBG_SECTION_GEOMETRY",
                f"gap before section {section.index} cannot satisfy "
                f"alignment {section.alignment}",
                location=f"section[{section.index}]",
            )
    result: list[NbgOwnedRange] = []
    cursor = start
    if alignment_bytes:
        result.append(
            _owned_range(
                payload,
                cursor,
                cursor + alignment_bytes,
                ByteClass.ALIGNMENT,
                f"gap-before-section[{section.index}]",
                None,
                notes=f"original alignment {section.alignment}",
            )
        )
        cursor += alignment_bytes
    if cursor < end:
        owner = (
            f"gap-before-section[{section.index}]"
            if section is not None
            else f"root.gap-before-{next_range.owner}"
        )
        result.append(
            _owned_range(
                payload,
                cursor,
                end,
                ByteClass.PADDING,
                owner,
                None,
                notes="preserved original padding bytes",
            )
        )
    return result


def _fill_and_validate_ranges(
    payload: bytes,
    base_ranges: list[NbgOwnedRange],
    sections: tuple[NbgSectionView, ...],
    *,
    artifact_id: str | None,
    display_path: str,
) -> tuple[NbgOwnedRange, ...]:
    ordered_base = sorted(
        base_ranges,
        key=lambda item: (item.start, item.end, item.owner),
    )
    section_by_index = {item.index: item for item in sections}
    if len(section_by_index) != len(sections):
        raise recovery.SerializerError(
            "NBG_SECTION_GEOMETRY",
            "duplicate section index",
            artifact_id=artifact_id,
            location=display_path,
        )
    result: list[NbgOwnedRange] = []
    cursor = 0
    for item in ordered_base:
        if item.start < cursor:
            raise recovery.SerializerError(
                "NBG_RANGE_OVERLAP",
                f"{item.owner} begins at 0x{item.start:x} before prior end "
                f"0x{cursor:x}",
                artifact_id=artifact_id,
                location=item.owner,
            )
        if item.start > cursor:
            result.extend(
                _gap_ranges(
                    payload,
                    cursor,
                    item.start,
                    next_range=item,
                    section_by_index=section_by_index,
                )
            )
        result.append(item)
        cursor = item.end
    if cursor < len(payload):
        result.append(
            _owned_range(
                payload,
                cursor,
                len(payload),
                ByteClass.PADDING,
                "root.trailing-padding",
                None,
                notes="trailing bytes not independently described",
            )
        )
    return tuple(result)


def roundtrip_bytes(
    payload: bytes,
    *,
    artifact_id: str,
    display_path: str,
) -> RoundtripResult:
    """Parse and reconstruct one immutable NBG in memory."""

    image = NbgImage.parse(
        payload,
        artifact_id=artifact_id,
        display_path=display_path,
    )
    rebuilt = image.to_bytes()
    first_difference = next(
        (
            index
            for index, (left, right) in enumerate(zip(payload, rebuilt))
            if left != right
        ),
        None,
    )
    if first_difference is None and len(payload) != len(rebuilt):
        first_difference = min(len(payload), len(rebuilt))
    totals = {
        item.value: sum(
            owned.length
            for owned in image.ranges
            if owned.byte_class is item
        )
        for item in ByteClass
    }
    return RoundtripResult(
        artifact_id=artifact_id,
        path=display_path,
        input_bytes=len(payload),
        output_bytes=len(rebuilt),
        input_sha256=hashlib.sha256(payload).hexdigest(),
        output_sha256=hashlib.sha256(rebuilt).hexdigest(),
        exact_match=payload == rebuilt,
        first_difference=first_difference,
        range_count=len(image.ranges),
        class_byte_totals=MappingProxyType(totals),
        connection_tail_mode=image.connection_tail_mode,
        image=image,
    )


def roundtrip_path(
    path: Path,
    *,
    artifact_id: str,
    display_path: str,
    expected_size: int,
    expected_sha256: str,
) -> RoundtripResult:
    """Read, validate, and reconstruct one pinned regular NBG file."""

    if path.is_symlink() or not path.is_file():
        raise recovery.SerializerError(
            "NBG_PARSE_FAILURE",
            "round-trip input must be a regular non-symlink file",
            artifact_id=artifact_id,
            location=display_path,
        )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise recovery.SerializerError(
            "NBG_PARSE_FAILURE",
            f"cannot read round-trip input: {exc}",
            artifact_id=artifact_id,
            location=display_path,
        ) from exc
    digest = hashlib.sha256(payload).hexdigest()
    if len(payload) != expected_size or digest != expected_sha256:
        raise recovery.SerializerError(
            "TARGET_HASH_MISMATCH",
            f"round-trip input is {len(payload)} bytes with SHA-256 "
            f"{digest}; expected {expected_size} bytes and "
            f"{expected_sha256}",
            artifact_id=artifact_id,
            location=display_path,
        )
    return roundtrip_bytes(
        payload,
        artifact_id=artifact_id,
        display_path=display_path,
    )


def _roundtrip_results(
    manifest: recovery.SerializerManifest,
) -> tuple[RoundtripResult, ...]:
    recovery.validate_artifacts(manifest)
    results = []
    for artifact_id in manifest.roundtrip_nbg_ids:
        artifact = manifest.artifacts[artifact_id]
        results.append(
            roundtrip_path(
                manifest.repository_root / artifact.path,
                artifact_id=artifact_id,
                display_path=artifact.path.as_posix(),
                expected_size=artifact.byte_count,
                expected_sha256=artifact.sha256,
            )
        )
    return tuple(results)


def _manifest_reference(
    manifest: recovery.SerializerManifest,
) -> dict[str, str]:
    try:
        path = manifest.source_path.relative_to(
            manifest.repository_root
        ).as_posix()
    except ValueError:
        path = manifest.source_path.name
    return {
        "path": path,
        "sha256": manifest.source_sha256,
    }


def _evidence_envelope(
    manifest: recovery.SerializerManifest,
    *,
    generator: str,
) -> dict[str, Any]:
    return {
        "generator": generator,
        "manifest": _manifest_reference(manifest),
        "poc_id": manifest.poc_id,
        "safety": dict(manifest.policy.prohibited_actions),
        "schema_version": IMAGE_SCHEMA_VERSION,
    }


def build_ownership_evidence(
    manifest: recovery.SerializerManifest,
) -> dict[str, Any]:
    """Build canonical total-byte-ownership evidence for the pinned corpus."""

    document = _evidence_envelope(
        manifest, generator="waved/scripts/npu/nbg_image.py"
    )
    results = _roundtrip_results(manifest)
    document["artifacts"] = [
        result.image.ownership_document() for result in results
    ]
    document["claim"] = (
        "Every source byte has one exclusive owner; no NBG was modified."
    )
    return document


def build_roundtrip_evidence(
    manifest: recovery.SerializerManifest,
) -> dict[str, Any]:
    """Build canonical unchanged-re-emission evidence for the pinned corpus."""

    document = _evidence_envelope(
        manifest, generator="waved/scripts/npu/nbg_image.py"
    )
    results = _roundtrip_results(manifest)
    document["artifacts"] = [item.to_document() for item in results]
    document["semantic_change_policy"] = {
        "error_code": "NBG_SEMANTIC_CHANGE_REFUSED",
        "status": "refused",
    }
    document["copies_written"] = 0
    document["claim"] = (
        "Selected unchanged Wave NBGs reconstruct byte-for-byte; this is "
        "not custom graph generation."
    )
    return document


def _resolved_output_below(
    destination: Path, repository_root: Path
) -> Path:
    root = repository_root.resolve()
    candidate = (
        destination
        if destination.is_absolute()
        else root / destination
    )
    try:
        parent = candidate.parent.resolve(strict=True)
    except OSError as exc:
        raise recovery.SerializerError(
            "OUTPUT_EXISTS",
            f"output parent is unavailable: {exc}",
            location=str(destination),
        ) from exc
    try:
        parent.relative_to(root)
    except ValueError as exc:
        raise recovery.SerializerError(
            "OUTPUT_EXISTS",
            "output path escapes the approved repository root",
            location=str(destination),
        ) from exc
    resolved = parent / candidate.name
    if not candidate.name or resolved.exists() or resolved.is_symlink():
        raise recovery.SerializerError(
            "OUTPUT_EXISTS",
            "output path already exists or is invalid",
            location=str(destination),
        )
    return resolved


def write_identical_copy_exclusive(
    image: NbgImage,
    destination: Path,
    *,
    repository_root: Path,
) -> dict[str, Any]:
    """Write one already-verified identical copy without overwrite."""

    rebuilt = image.to_bytes()
    target = _resolved_output_below(destination, repository_root)
    descriptor: int | None = None
    created = False
    try:
        try:
            descriptor = os.open(
                target,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                0o600,
            )
            created = True
        except FileExistsError as exc:
            raise recovery.SerializerError(
                "OUTPUT_EXISTS",
                "output path already exists",
                artifact_id=image.artifact_id,
                location=str(destination),
            ) from exc
        view = memoryview(rebuilt)
        written = 0
        while written < len(view):
            count = os.write(descriptor, view[written:])
            if count <= 0:
                raise OSError("short write")
            written += count
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = None
        observed = target.read_bytes()
        digest = hashlib.sha256(observed).hexdigest()
        if observed != rebuilt or digest != image.source_sha256:
            raise recovery.SerializerError(
                "NBG_ROUNDTRIP_BYTE_MISMATCH",
                "post-write copy verification failed",
                artifact_id=image.artifact_id,
                location=str(destination),
            )
        return {
            "bytes": len(observed),
            "path": target.relative_to(repository_root.resolve()).as_posix(),
            "sha256": digest,
        }
    except Exception:
        if descriptor is not None:
            os.close(descriptor)
        if created:
            try:
                target.unlink()
            except FileNotFoundError:
                pass
        raise


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


def generate_evidence(manifest: recovery.SerializerManifest) -> None:
    ownership = recovery.canonical_json(
        build_ownership_evidence(manifest)
    )
    roundtrip = recovery.canonical_json(
        build_roundtrip_evidence(manifest)
    )
    _atomic_write(
        manifest.repository_root / manifest.outputs["ownership"],
        ownership,
    )
    _atomic_write(
        manifest.repository_root / manifest.outputs["roundtrip"],
        roundtrip,
    )


def verify_generated(manifest: recovery.SerializerManifest) -> None:
    expected = {
        "ownership": recovery.canonical_json(
            build_ownership_evidence(manifest)
        ),
        "roundtrip": recovery.canonical_json(
            build_roundtrip_evidence(manifest)
        ),
    }
    for output_id, generated in expected.items():
        path = manifest.repository_root / manifest.outputs[output_id]
        try:
            tracked = path.read_bytes()
        except OSError as exc:
            raise recovery.SerializerError(
                "EVIDENCE_STALE",
                f"cannot read tracked {output_id} evidence: {exc}",
                location=manifest.outputs[output_id].as_posix(),
            ) from exc
        if tracked != generated:
            raise recovery.SerializerError(
                "EVIDENCE_STALE",
                f"tracked {output_id} evidence differs",
                location=manifest.outputs[output_id].as_posix(),
            )


def _artifact_image(
    manifest: recovery.SerializerManifest, artifact_id: str
) -> NbgImage:
    artifact = manifest.artifacts.get(artifact_id)
    if artifact is None or artifact.kind != "nbg":
        raise recovery.SerializerError(
            "TARGET_MANIFEST_INVALID",
            f"unknown NBG artifact ID {artifact_id!r}",
        )
    result = roundtrip_path(
        manifest.repository_root / artifact.path,
        artifact_id=artifact_id,
        display_path=artifact.path.as_posix(),
        expected_size=artifact.byte_count,
        expected_sha256=artifact.sha256,
    )
    return result.image


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Lossless host-only Wave VPMN v2 image."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=recovery.DEFAULT_MANIFEST_PATH,
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    inspect = subparsers.add_parser("inspect")
    inspect.add_argument("artifact_id")
    roundtrip = subparsers.add_parser("roundtrip")
    roundtrip.add_argument("artifact_id")
    roundtrip.add_argument("--copy-to", type=Path)
    subparsers.add_parser("generate-evidence")
    subparsers.add_parser("verify-generated")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        manifest = recovery.load_manifest(args.manifest)
        if args.command == "inspect":
            image = _artifact_image(manifest, args.artifact_id)
            sys.stdout.buffer.write(
                recovery.canonical_json(image.ownership_document())
            )
            return 0
        if args.command == "roundtrip":
            image = _artifact_image(manifest, args.artifact_id)
            document: dict[str, Any] = {
                "artifact_id": args.artifact_id,
                "bytes": image.source_size,
                "exact_match": image.to_bytes() == image._source_bytes,
                "sha256": image.source_sha256,
            }
            if args.copy_to is not None:
                document["copy"] = write_identical_copy_exclusive(
                    image,
                    args.copy_to,
                    repository_root=manifest.repository_root,
                )
            sys.stdout.buffer.write(recovery.canonical_json(document))
            return 0
        if args.command == "generate-evidence":
            generate_evidence(manifest)
            print("serializer ownership and round-trip evidence generated")
            return 0
        if args.command == "verify-generated":
            verify_generated(manifest)
            print("serializer ownership and round-trip evidence is current")
            return 0
        return 2
    except recovery.SerializerError as exc:
        print(f"nbg-image: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
