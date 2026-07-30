#!/usr/bin/env python3
"""Bounded ONNX-initializer to exact NBG-section correlation."""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Any, Iterator, Mapping, Sequence

import numpy as np
import onnx
from onnx import numpy_helper
from wave_onnx_hybrid.audit import (
    audit_model_bytes,
    load_policy,
)
from wave_onnx_hybrid.errors import HybridError

try:
    from . import (
        nbg_format,
        nbg_image,
        nbg_serializer_recovery as recovery,
    )
except ImportError:
    import nbg_format  # type: ignore
    import nbg_image  # type: ignore
    import nbg_serializer_recovery as recovery  # type: ignore


CORRELATION_SCHEMA_VERSION = 1
PATTERNS = (
    "adjacent-pair-swap",
    "even-odd",
    "odd-even",
    "reverse",
)
BLOCK_WIDTHS = (2, 4, 8, 16, 32)
BYTE_LANE_WIDTHS = (2, 4, 8)
HALFWORD_LANE_WIDTHS = (2, 4, 8)
MAX_RECORDED_OCCURRENCES = 1024
DEFAULT_ONNX_POLICY = (
    recovery.REPOSITORY_ROOT / "docs/npu/onnx/hybrid-policy.json"
)
DIFFERENTIAL_SAMPLE_BYTES = 1024 * 1024
DIFFERENTIAL_BLOCK_SIZES = (32, 64, 128)


@dataclass(frozen=True)
class CorrelationLimits:
    max_candidate_bytes: int
    max_candidates_per_tensor: int
    max_initializer_bytes: int
    max_search_bytes_per_tensor: int
    max_search_sections: int

    @classmethod
    def from_mapping(
        cls, value: Mapping[str, Any]
    ) -> CorrelationLimits:
        expected = recovery.LIMIT_KEYS
        actual = frozenset(value)
        if actual != expected:
            raise recovery.SerializerError(
                "CORRELATION_LIMIT_EXCEEDED",
                "correlation limit keys differ from the strict schema",
            )
        normalized = {}
        for key in sorted(value):
            item = value[key]
            if (
                isinstance(item, bool)
                or not isinstance(item, int)
                or item <= 0
            ):
                raise recovery.SerializerError(
                    "CORRELATION_LIMIT_EXCEEDED",
                    f"correlation limit {key} must be positive",
                )
            normalized[key] = item
        return cls(**normalized)

    def to_document(self) -> dict[str, int]:
        return {
            "max_candidate_bytes": self.max_candidate_bytes,
            "max_candidates_per_tensor": self.max_candidates_per_tensor,
            "max_initializer_bytes": self.max_initializer_bytes,
            "max_search_bytes_per_tensor": (
                self.max_search_bytes_per_tensor
            ),
            "max_search_sections": self.max_search_sections,
        }


@dataclass(frozen=True)
class TensorSource:
    name: str
    shape: tuple[int, ...]
    dtype: str
    element_count: int
    source_bytes: bytes = field(repr=False)
    source_sha256: str
    _array: np.ndarray = field(repr=False, compare=False)

    @classmethod
    def from_array(
        cls, name: str, array: np.ndarray
    ) -> TensorSource:
        if not isinstance(name, str) or not name:
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                "tensor name must be nonempty",
            )
        value = np.asarray(array)
        if value.dtype != np.float32:
            value = value.astype(np.float32)
        value = np.ascontiguousarray(value.astype("<f4", copy=False))
        if value.ndim not in {1, 2} or any(
            dimension <= 0 for dimension in value.shape
        ):
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                f"tensor {name} must have a positive rank-1 or rank-2 shape",
            )
        payload = value.tobytes(order="C")
        return cls(
            name=name,
            shape=tuple(int(item) for item in value.shape),
            dtype="float32",
            element_count=int(value.size),
            source_bytes=payload,
            source_sha256=hashlib.sha256(payload).hexdigest(),
            _array=value,
        )

    def to_document(self) -> dict[str, Any]:
        return {
            "dtype": self.dtype,
            "element_count": self.element_count,
            "name": self.name,
            "shape": list(self.shape),
            "source_bytes": len(self.source_bytes),
            "source_sha256": self.source_sha256,
        }


@dataclass(frozen=True)
class TransformCandidate:
    source_name: str
    transform_id: str
    parameters: Mapping[str, Any]
    inverse: Mapping[str, Any]
    payload: bytes = field(repr=False)
    payload_sha256: str

    def to_summary_document(self) -> dict[str, Any]:
        return {
            "inverse": dict(self.inverse),
            "output_bytes": len(self.payload),
            "output_sha256": self.payload_sha256,
            "parameters": dict(self.parameters),
            "source_name": self.source_name,
            "transform_id": self.transform_id,
        }


@dataclass(frozen=True)
class SearchSection:
    artifact_id: str
    section_index: int
    section_type: int
    type_name: str
    file_offset: int
    payload: bytes = field(repr=False)

    def __post_init__(self) -> None:
        if (
            not self.artifact_id
            or self.section_index < 0
            or self.section_type < 0
            or self.file_offset < 0
            or not isinstance(self.payload, bytes)
        ):
            raise ValueError("invalid search section")

    def to_document(self) -> dict[str, Any]:
        return {
            "artifact_id": self.artifact_id,
            "file_offset": self.file_offset,
            "section_index": self.section_index,
            "section_sha256": hashlib.sha256(self.payload).hexdigest(),
            "section_size": len(self.payload),
            "section_type": self.section_type,
            "type_name": self.type_name,
        }


@dataclass(frozen=True)
class MatchOccurrence:
    artifact_id: str
    section_index: int
    section_type: int
    relative_offset: int
    absolute_offset: int
    length: int

    def to_document(self) -> dict[str, Any]:
        return {
            "absolute_offset": self.absolute_offset,
            "absolute_offset_hex": f"0x{self.absolute_offset:08x}",
            "artifact_id": self.artifact_id,
            "length": self.length,
            "relative_offset": self.relative_offset,
            "section_index": self.section_index,
            "section_type": self.section_type,
        }


@dataclass(frozen=True)
class CandidateSearchResult:
    candidate: TransformCandidate = field(repr=False)
    status: str
    evidence_class: str
    error_code: str | None
    occurrence_count: int
    occurrences: tuple[MatchOccurrence, ...]

    def to_document(self) -> dict[str, Any]:
        result = self.candidate.to_summary_document()
        result.update(
            {
                "error_code": self.error_code,
                "evidence_class": self.evidence_class,
                "occurrence_count": self.occurrence_count,
                "occurrences": [
                    item.to_document() for item in self.occurrences
                ],
                "status": self.status,
            }
        )
        return result


def _candidate(
    source: TensorSource,
    transform_id: str,
    payload: bytes,
    parameters: Mapping[str, Any],
    inverse: Mapping[str, Any],
    limits: CorrelationLimits,
) -> TransformCandidate:
    if transform_id not in recovery.TRANSFORM_IDS:
        raise recovery.SerializerError(
            "INTERNAL_ERROR",
            f"unregistered transform {transform_id}",
        )
    if len(payload) > limits.max_candidate_bytes:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"{source.name} transform {transform_id} emits "
            f"{len(payload)} bytes, limit is "
            f"{limits.max_candidate_bytes}",
        )
    return TransformCandidate(
        source_name=source.name,
        transform_id=transform_id,
        parameters=MappingProxyType(dict(sorted(parameters.items()))),
        inverse=MappingProxyType(dict(sorted(inverse.items()))),
        payload=payload,
        payload_sha256=hashlib.sha256(payload).hexdigest(),
    )


def _bf16_rne_bytes(array: np.ndarray) -> bytes:
    values = np.ascontiguousarray(array.astype("<f4", copy=False))
    words = values.view("<u4")
    rounded = words + np.uint32(0x7FFF) + ((words >> 16) & 1)
    return (rounded >> 16).astype("<u2").tobytes(order="C")


def _permutation_indices(width: int, pattern: str) -> np.ndarray:
    if width <= 1:
        raise ValueError("permutation width must exceed one")
    if pattern == "reverse":
        result = list(reversed(range(width)))
    elif pattern == "even-odd":
        result = list(range(0, width, 2)) + list(range(1, width, 2))
    elif pattern == "odd-even":
        result = list(range(1, width, 2)) + list(range(0, width, 2))
    elif pattern == "adjacent-pair-swap":
        result = []
        for index in range(0, width, 2):
            if index + 1 < width:
                result.extend((index + 1, index))
            else:
                result.append(index)
    else:
        raise ValueError(f"unknown permutation pattern {pattern}")
    return np.asarray(result, dtype=np.int64)


def _permute_blocks(
    values: np.ndarray, width: int, pattern: str
) -> tuple[np.ndarray, list[int]]:
    result = np.ascontiguousarray(values).copy()
    complete = (result.size // width) * width
    indices = _permutation_indices(width, pattern)
    if complete:
        blocks = result[:complete].reshape(-1, width)
        result[:complete] = blocks[:, indices].reshape(-1)
    inverse = np.argsort(indices).astype(int).tolist()
    return result, inverse


def _inverse_document(
    *,
    available: bool,
    reason: str,
    indices: list[int] | None = None,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "available": available,
        "reason": reason,
    }
    if indices is not None:
        result["inverse_indices"] = indices
    return result


def iter_transform_candidates(
    source: TensorSource,
    limits: CorrelationLimits,
) -> Iterator[TransformCandidate]:
    """Yield the reviewed finite transform registry in canonical order."""

    if len(source.source_bytes) > limits.max_initializer_bytes:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"initializer {source.name} is {len(source.source_bytes)} "
            f"bytes, limit is {limits.max_initializer_bytes}",
        )
    candidates: list[TransformCandidate] = []
    fp32 = np.ascontiguousarray(source._array.astype("<f4", copy=False))
    fp16 = np.ascontiguousarray(source._array.astype("<f2"))
    candidates.append(
        _candidate(
            source,
            "identity-fp32-le",
            source.source_bytes,
            {"encoding": "float32-le", "order": "C"},
            _inverse_document(
                available=True, reason="identity transform"
            ),
            limits,
        )
    )
    candidates.append(
        _candidate(
            source,
            "cast-fp16-rne-le",
            fp16.tobytes(order="C"),
            {"encoding": "float16-le", "rounding": "nearest-even"},
            _inverse_document(
                available=False,
                reason="precision is lost during FP32 to FP16 conversion",
            ),
            limits,
        )
    )
    candidates.append(
        _candidate(
            source,
            "cast-bf16-rne-le",
            _bf16_rne_bytes(fp32),
            {"encoding": "bfloat16-le", "rounding": "nearest-even"},
            _inverse_document(
                available=False,
                reason="precision is lost during FP32 to BF16 conversion",
            ),
            limits,
        )
    )
    if fp32.ndim == 2:
        transposed = np.ascontiguousarray(fp32.T)
        candidates.extend(
            (
                _candidate(
                    source,
                    "transpose-2d",
                    transposed.tobytes(order="C"),
                    {"axes": [1, 0], "encoding": "float32-le"},
                    _inverse_document(
                        available=True,
                        reason="2D transpose is self-inverse",
                    ),
                    limits,
                ),
                _candidate(
                    source,
                    "transpose-2d+cast-fp16-rne-le",
                    transposed.astype("<f2").tobytes(order="C"),
                    {
                        "axes": [1, 0],
                        "encoding": "float16-le",
                        "rounding": "nearest-even",
                    },
                    _inverse_document(
                        available=False,
                        reason=(
                            "transpose is invertible but precision is lost "
                            "during FP16 conversion"
                        ),
                    ),
                    limits,
                ),
                _candidate(
                    source,
                    "transpose-2d+cast-bf16-rne-le",
                    _bf16_rne_bytes(transposed),
                    {
                        "axes": [1, 0],
                        "encoding": "bfloat16-le",
                        "rounding": "nearest-even",
                    },
                    _inverse_document(
                        available=False,
                        reason=(
                            "transpose is invertible but precision is lost "
                            "during BF16 conversion"
                        ),
                    ),
                    limits,
                ),
            )
        )

    flat_fp32 = fp32.reshape(-1)
    for width in BLOCK_WIDTHS:
        for pattern in PATTERNS:
            transformed, inverse = _permute_blocks(
                flat_fp32, width, pattern
            )
            candidates.append(
                _candidate(
                    source,
                    "fixed-block-permute",
                    transformed.astype("<f4", copy=False).tobytes(),
                    {
                        "block_elements": width,
                        "encoding": "float32-le",
                        "pattern": pattern,
                        "tail": "unchanged",
                    },
                    _inverse_document(
                        available=True,
                        reason="fixed permutation has an exact inverse",
                        indices=inverse,
                    ),
                    limits,
                )
            )

    fp16_bytes = np.frombuffer(
        fp16.tobytes(order="C"), dtype=np.uint8
    )
    for width in BYTE_LANE_WIDTHS:
        for pattern in PATTERNS:
            transformed, inverse = _permute_blocks(
                fp16_bytes, width, pattern
            )
            candidates.append(
                _candidate(
                    source,
                    "byte-lane-permute",
                    transformed.tobytes(),
                    {
                        "encoding": "float16-le-bytes",
                        "lane_bytes": width,
                        "pattern": pattern,
                        "tail": "unchanged",
                    },
                    _inverse_document(
                        available=True,
                        reason="byte-lane permutation has an exact inverse",
                        indices=inverse,
                    ),
                    limits,
                )
            )

    fp16_halfwords = np.frombuffer(
        fp16.tobytes(order="C"), dtype="<u2"
    )
    for width in HALFWORD_LANE_WIDTHS:
        for pattern in PATTERNS:
            transformed, inverse = _permute_blocks(
                fp16_halfwords, width, pattern
            )
            candidates.append(
                _candidate(
                    source,
                    "halfword-lane-permute",
                    transformed.astype("<u2", copy=False).tobytes(),
                    {
                        "encoding": "float16-le-halfwords",
                        "lane_halfwords": width,
                        "pattern": pattern,
                        "tail": "unchanged",
                    },
                    _inverse_document(
                        available=True,
                        reason=(
                            "halfword-lane permutation has an exact inverse"
                        ),
                        indices=inverse,
                    ),
                    limits,
                )
            )
    candidates.sort(
        key=lambda item: (
            item.transform_id,
            recovery.canonical_json(dict(item.parameters)),
        )
    )
    if len(candidates) > limits.max_candidates_per_tensor:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"tensor {source.name} has {len(candidates)} candidates, "
            f"limit is {limits.max_candidates_per_tensor}",
        )
    yield from candidates


def search_candidate(
    candidate: TransformCandidate,
    sections: Sequence[SearchSection],
    limits: CorrelationLimits,
) -> CandidateSearchResult:
    """Search one exact candidate in bounded admitted section payloads."""

    if len(sections) > limits.max_search_sections:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"search has {len(sections)} sections, limit is "
            f"{limits.max_search_sections}",
        )
    searched_bytes = sum(len(item.payload) for item in sections)
    if searched_bytes > limits.max_search_bytes_per_tensor:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"one candidate search covers {searched_bytes} bytes, limit is "
            f"{limits.max_search_bytes_per_tensor}",
        )
    if not candidate.payload:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            "empty transform candidates are forbidden",
        )
    occurrences: list[MatchOccurrence] = []
    total_occurrences = 0
    for section in sections:
        cursor = 0
        while cursor <= len(section.payload) - len(candidate.payload):
            offset = section.payload.find(candidate.payload, cursor)
            if offset < 0:
                break
            total_occurrences += 1
            if len(occurrences) < MAX_RECORDED_OCCURRENCES:
                occurrences.append(
                    MatchOccurrence(
                        artifact_id=section.artifact_id,
                        section_index=section.section_index,
                        section_type=section.section_type,
                        relative_offset=offset,
                        absolute_offset=section.file_offset + offset,
                        length=len(candidate.payload),
                    )
                )
            cursor = offset + 1
    if total_occurrences == 0:
        status = "no-match"
        evidence_class = "unresolved"
        error_code = None
    elif total_occurrences == 1:
        status = "unique"
        evidence_class = (
            "direct-match"
            if candidate.transform_id == "identity-fp32-le"
            else "transformed-match"
        )
        error_code = None
    else:
        status = "ambiguous"
        evidence_class = "unresolved"
        error_code = "CORRELATION_AMBIGUOUS"
    return CandidateSearchResult(
        candidate=candidate,
        status=status,
        evidence_class=evidence_class,
        error_code=error_code,
        occurrence_count=total_occurrences,
        occurrences=tuple(occurrences),
    )


def correlate_tensor(
    source: TensorSource,
    sections: Sequence[SearchSection],
    limits: CorrelationLimits,
) -> dict[str, Any]:
    """Run every reviewed finite candidate and classify exact evidence."""

    candidates = list(iter_transform_candidates(source, limits))
    section_bytes = sum(len(item.payload) for item in sections)
    total_search_bytes = section_bytes * len(candidates)
    if total_search_bytes > limits.max_search_bytes_per_tensor:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"tensor {source.name} exact-search budget is "
            f"{total_search_bytes} bytes, limit is "
            f"{limits.max_search_bytes_per_tensor}",
        )
    results = [
        search_candidate(candidate, sections, limits)
        for candidate in candidates
    ]
    direct = [
        item
        for item in results
        if item.status == "unique"
        and item.evidence_class == "direct-match"
    ]
    transformed = [
        item
        for item in results
        if item.status == "unique"
        and item.evidence_class == "transformed-match"
    ]
    if len(direct) == 1:
        selected = direct[0]
        relationship = {
            "class": "direct-match",
            "reason": "one unambiguous exact source-byte occurrence",
            "result": selected.to_document(),
        }
    elif not direct and len(transformed) == 1:
        selected = transformed[0]
        relationship = {
            "class": "transformed-match",
            "reason": "one named transform has one exact occurrence",
            "result": selected.to_document(),
        }
    elif direct or transformed:
        relationship = {
            "class": "unresolved",
            "reason": (
                "multiple exact transform hypotheses remain; no candidate "
                "was promoted"
            ),
            "unique_hypothesis_count": len(direct) + len(transformed),
        }
    else:
        relationship = {
            "class": "unresolved",
            "reason": "no reviewed finite transform has an exact occurrence",
            "unique_hypothesis_count": 0,
        }
    plain_fp16 = next(
        item
        for item in results
        if item.candidate.transform_id == "cast-fp16-rne-le"
    )
    return {
        "candidate_count": len(results),
        "candidates": [item.to_document() for item in results],
        "plain_fp16": plain_fp16.to_document(),
        "relationship": relationship,
        "search": {
            "candidate_comparison_bytes": total_search_bytes,
            "section_bytes_per_candidate": section_bytes,
            "section_count": len(sections),
        },
        "source": source.to_document(),
    }


def _strict_anchor_model(
    manifest: recovery.SerializerManifest,
) -> tuple[dict[str, Any], tuple[TensorSource, ...]]:
    anchor = manifest.correlation.anchor
    artifact = manifest.artifacts[anchor.onnx_artifact_id]
    path = manifest.repository_root / artifact.path
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            f"cannot read ONNX anchor: {exc}",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    digest = hashlib.sha256(payload).hexdigest()
    if len(payload) != artifact.byte_count or digest != artifact.sha256:
        raise recovery.SerializerError(
            "TARGET_HASH_MISMATCH",
            "ONNX anchor size or SHA-256 differs from the manifest",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        )
    try:
        policy = load_policy(
            DEFAULT_ONNX_POLICY,
            repository_root=manifest.repository_root,
        )
        audit = audit_model_bytes(
            payload,
            source_name=artifact.path.name,
            policy=policy,
        )
    except HybridError as exc:
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            str(exc),
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    try:
        model = onnx.load_model_from_string(payload)
    except Exception as exc:
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            f"cannot parse admitted ONNX model: {exc}",
            artifact_id=artifact.artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    graph = model.graph
    node_types = tuple(node.op_type for node in graph.node)
    if node_types != ("MatMul", "Add", "LogSoftmax"):
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            f"expected MatMul -> Add -> LogSoftmax, observed "
            f"{node_types}",
            artifact_id=artifact.artifact_id,
            location="graph.node",
        )
    expected_connections = (
        (
            ("hidden", "onnx::MatMul_7"),
            ("/ctc_lo/MatMul_output_0",),
        ),
        (
            ("ctc_lo.bias", "/ctc_lo/MatMul_output_0"),
            ("/ctc_lo/Add_output_0",),
        ),
        (
            ("/ctc_lo/Add_output_0",),
            ("probs",),
        ),
    )
    observed_connections = tuple(
        (tuple(node.input), tuple(node.output)) for node in graph.node
    )
    if observed_connections != expected_connections:
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            "CTC anchor graph connectivity differs from the pinned contract",
            artifact_id=artifact.artifact_id,
            location="graph.node",
        )
    expected_initializers = {
        "ctc_lo.bias": (4999,),
        "onnx::MatMul_7": (512, 4999),
    }
    by_name = {item.name: item for item in graph.initializer}
    if set(by_name) != set(expected_initializers):
        raise recovery.SerializerError(
            "ONNX_ADMISSION_FAILURE",
            f"CTC initializer names differ: {sorted(by_name)}",
            artifact_id=artifact.artifact_id,
            location="graph.initializer",
        )
    sources = []
    initializer_documents = []
    for name in sorted(expected_initializers):
        proto = by_name[name]
        if proto.data_type != onnx.TensorProto.FLOAT:
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                f"initializer {name} is not FP32",
                artifact_id=artifact.artifact_id,
                location=name,
            )
        shape = tuple(int(item) for item in proto.dims)
        if shape != expected_initializers[name]:
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                f"initializer {name} shape is {shape}, expected "
                f"{expected_initializers[name]}",
                artifact_id=artifact.artifact_id,
                location=name,
            )
        try:
            array = np.asarray(numpy_helper.to_array(proto))
        except Exception as exc:
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                f"cannot decode initializer {name}: {exc}",
                artifact_id=artifact.artifact_id,
                location=name,
            ) from exc
        if array.dtype != np.float32 or not np.isfinite(array).all():
            raise recovery.SerializerError(
                "ONNX_ADMISSION_FAILURE",
                f"initializer {name} is not finite FP32",
                artifact_id=artifact.artifact_id,
                location=name,
            )
        source = TensorSource.from_array(name, array)
        if len(source.source_bytes) > manifest.correlation.limits[
            "max_initializer_bytes"
        ]:
            raise recovery.SerializerError(
                "CORRELATION_LIMIT_EXCEEDED",
                f"initializer {name} exceeds manifest limit",
                artifact_id=artifact.artifact_id,
                location=name,
            )
        sources.append(source)
        initializer_documents.append(source.to_document())
    audit_document = {
        "cpu_admissible": audit.document["summary"]["cpu_admissible"],
        "graph_signature": audit.graph_signature,
        "initializers": initializer_documents,
        "model_bytes": len(payload),
        "model_sha256": digest,
        "node_types": list(node_types),
        "onnx_version": onnx.__version__,
        "policy": audit.document["policy"],
        "source_artifact_id": artifact.artifact_id,
        "source_path": artifact.path.as_posix(),
    }
    return audit_document, tuple(sources)


def _nbg_image_for_artifact(
    manifest: recovery.SerializerManifest,
    artifact_id: str,
) -> nbg_image.NbgImage:
    artifact = manifest.artifacts[artifact_id]
    path = manifest.repository_root / artifact.path
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise recovery.SerializerError(
            "NBG_PARSE_FAILURE",
            f"cannot read NBG correlation input: {exc}",
            artifact_id=artifact_id,
            location=artifact.path.as_posix(),
        ) from exc
    if (
        len(payload) != artifact.byte_count
        or hashlib.sha256(payload).hexdigest() != artifact.sha256
    ):
        raise recovery.SerializerError(
            "TARGET_HASH_MISMATCH",
            "NBG correlation input differs from manifest",
            artifact_id=artifact_id,
            location=artifact.path.as_posix(),
        )
    return nbg_image.NbgImage.parse(
        payload,
        artifact_id=artifact_id,
        display_path=artifact.path.as_posix(),
    )


def _search_sections(
    manifest: recovery.SerializerManifest,
    image: nbg_image.NbgImage,
) -> tuple[SearchSection, ...]:
    allowed = set(manifest.correlation.search_section_types)
    sections = tuple(
        SearchSection(
            artifact_id=image.artifact_id or "<unknown>",
            section_index=section.index,
            section_type=section.section_type,
            type_name=section.type_name,
            file_offset=section.offset,
            payload=image.section_bytes(section.index),
        )
        for section in image.sections
        if section.section_type in allowed
    )
    limits = CorrelationLimits.from_mapping(
        manifest.correlation.limits
    )
    if len(sections) > limits.max_search_sections:
        raise recovery.SerializerError(
            "CORRELATION_LIMIT_EXCEEDED",
            f"NBG has {len(sections)} searchable sections, limit is "
            f"{limits.max_search_sections}",
            artifact_id=image.artifact_id,
        )
    return sections


def _common_prefix(left: bytes, right: bytes, maximum: int) -> int:
    limit = min(len(left), len(right), maximum)
    for index in range(limit):
        if left[index] != right[index]:
            return index
    return limit


def _common_suffix(left: bytes, right: bytes, maximum: int) -> int:
    limit = min(len(left), len(right), maximum)
    for count in range(1, limit + 1):
        if left[-count] != right[-count]:
            return count - 1
    return limit


def _sampled_block_hashes(
    payload: bytes, block_size: int
) -> tuple[Counter[str], int]:
    if len(payload) <= DIFFERENTIAL_SAMPLE_BYTES:
        sample = payload
    else:
        half = DIFFERENTIAL_SAMPLE_BYTES // 2
        sample = payload[:half] + payload[-half:]
    complete = (len(sample) // block_size) * block_size
    hashes = Counter(
        hashlib.sha256(sample[offset : offset + block_size]).hexdigest()
        for offset in range(0, complete, block_size)
    )
    return hashes, len(sample)


def _section_profile(
    manifest: recovery.SerializerManifest,
    artifact_id: str,
    image: nbg_image.NbgImage,
) -> dict[str, Any]:
    artifact = manifest.artifacts[artifact_id]
    parsed = nbg_format.parse_path(
        manifest.repository_root / artifact.path,
        display_path=artifact.path.as_posix(),
    )
    selected = [
        section
        for section in image.sections
        if section.section_type
        in set(manifest.correlation.search_section_types)
    ]
    return {
        "artifact_id": artifact_id,
        "output_dimensions": [
            item["dimensions"] for item in parsed["outputs"]
        ],
        "path": artifact.path.as_posix(),
        "sections": [
            {
                **section.to_document(),
                "sha256": hashlib.sha256(
                    image.section_bytes(section.index)
                ).hexdigest(),
            }
            for section in selected
        ],
        "sha256": artifact.sha256,
    }


def _differential_comparison(
    anchor_id: str,
    anchor_image: nbg_image.NbgImage,
    control_id: str,
    control_image: nbg_image.NbgImage,
    allowed_types: set[int],
) -> dict[str, Any]:
    anchor_sections = {
        item.section_type: item
        for item in anchor_image.sections
        if item.section_type in allowed_types
    }
    control_sections = {
        item.section_type: item
        for item in control_image.sections
        if item.section_type in allowed_types
    }
    rows = []
    for section_type in sorted(
        set(anchor_sections) | set(control_sections)
    ):
        left_section = anchor_sections.get(section_type)
        right_section = control_sections.get(section_type)
        if left_section is None or right_section is None:
            rows.append(
                {
                    "anchor_present": left_section is not None,
                    "control_present": right_section is not None,
                    "section_type": section_type,
                    "status": "presence-differs",
                }
            )
            continue
        left = anchor_image.section_bytes(left_section.index)
        right = control_image.section_bytes(right_section.index)
        block_documents = []
        for block_size in DIFFERENTIAL_BLOCK_SIZES:
            left_hashes, left_sampled = _sampled_block_hashes(
                left, block_size
            )
            right_hashes, right_sampled = _sampled_block_hashes(
                right, block_size
            )
            common = sum(
                min(count, right_hashes.get(digest, 0))
                for digest, count in left_hashes.items()
            )
            block_documents.append(
                {
                    "anchor_sampled_bytes": left_sampled,
                    "block_bytes": block_size,
                    "common_block_count": common,
                    "control_sampled_bytes": right_sampled,
                }
            )
        rows.append(
            {
                "anchor_bytes": len(left),
                "anchor_sha256": hashlib.sha256(left).hexdigest(),
                "block_samples": block_documents,
                "common_prefix_bytes": _common_prefix(
                    left, right, DIFFERENTIAL_SAMPLE_BYTES
                ),
                "common_suffix_bytes": _common_suffix(
                    left, right, DIFFERENTIAL_SAMPLE_BYTES
                ),
                "control_bytes": len(right),
                "control_sha256": hashlib.sha256(right).hexdigest(),
                "equal_bytes": left == right,
                "equal_size": len(left) == len(right),
                "section_type": section_type,
                "size_delta": len(right) - len(left),
                "status": "compared",
                "type_name": left_section.type_name,
            }
        )
    return {
        "anchor_artifact_id": anchor_id,
        "control_artifact_id": control_id,
        "sections": rows,
    }


def build_correlation_evidence(
    manifest: recovery.SerializerManifest,
) -> dict[str, Any]:
    """Build bounded source/compiled-anchor correlation evidence."""

    recovery.validate_artifacts(manifest)
    limits = CorrelationLimits.from_mapping(
        manifest.correlation.limits
    )
    audit_document, sources = _strict_anchor_model(manifest)
    anchor_id = manifest.correlation.anchor.nbg_artifact_id
    anchor_image = _nbg_image_for_artifact(manifest, anchor_id)
    sections = _search_sections(manifest, anchor_image)
    tensor_results = [
        correlate_tensor(source, sections, limits)
        for source in sources
    ]

    image_by_id = {anchor_id: anchor_image}
    for artifact_id in manifest.correlation.control_nbg_ids:
        image_by_id[artifact_id] = _nbg_image_for_artifact(
            manifest, artifact_id
        )
    profiles = [
        _section_profile(manifest, artifact_id, image)
        for artifact_id, image in sorted(image_by_id.items())
    ]
    comparisons = [
        _differential_comparison(
            anchor_id,
            anchor_image,
            control_id,
            image_by_id[control_id],
            set(manifest.correlation.search_section_types),
        )
        for control_id in manifest.correlation.control_nbg_ids
    ]

    transformed = [
        item
        for item in tensor_results
        if item["relationship"]["class"] == "transformed-match"
    ]
    transform_ids = {
        item["relationship"]["result"]["transform_id"]
        for item in transformed
    }
    ready = len(transformed) >= 2 and len(transform_ids) == 1
    decision = {
        "poc12b": "ready" if ready else "not-ready",
        "reason": (
            "one exact transform is corroborated across at least two tensors"
            if ready
            else (
                "no deterministic invertible coefficient transform is "
                "corroborated across two tensors or graphs"
            )
        ),
    }
    return {
        "anchor": {
            "nbg_artifact_id": anchor_id,
            "onnx": audit_document,
            "relationship": (
                manifest.correlation.anchor.relationship
            ),
            "search_sections": [
                item.to_document() for item in sections
            ],
        },
        "decision": decision,
        "differential_controls": {
            "comparisons": comparisons,
            "profiles": profiles,
            "relationship_class": "differentially-supported",
            "warning": (
                "language, topology, compiler, and vocabulary differences "
                "are confounded; comparisons do not prove a transform"
            ),
        },
        "generator": "waved/scripts/npu/nbg_correlate.py",
        "limits": limits.to_document(),
        "manifest": {
            "path": manifest.source_path.relative_to(
                manifest.repository_root
            ).as_posix(),
            "sha256": manifest.source_sha256,
        },
        "non_claims": [
            "No coefficient layout was emitted.",
            "No compiler, simulator, runtime, or device was called.",
            "The ONNX/NBG anchor is not proven same-build provenance.",
            "Unchanged NBG round-trip does not establish semantic mutation.",
        ],
        "poc_id": manifest.poc_id,
        "safety": dict(manifest.policy.prohibited_actions),
        "schema_version": CORRELATION_SCHEMA_VERSION,
        "tensors": tensor_results,
        "tools": {
            "numpy_version": np.__version__,
            "onnx_version": onnx.__version__,
            "onnx_runtime_imported": False,
            "policy_path": DEFAULT_ONNX_POLICY.relative_to(
                manifest.repository_root
            ).as_posix(),
        },
        "transform_registry": sorted(recovery.TRANSFORM_IDS),
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


def generate_evidence(
    manifest: recovery.SerializerManifest,
) -> dict[str, Any]:
    document = build_correlation_evidence(manifest)
    output = manifest.repository_root / manifest.outputs["correlation"]
    _atomic_write(output, recovery.canonical_json(document))
    return document


def verify_generated(
    manifest: recovery.SerializerManifest,
) -> None:
    generated = recovery.canonical_json(
        build_correlation_evidence(manifest)
    )
    output = manifest.repository_root / manifest.outputs["correlation"]
    try:
        tracked = output.read_bytes()
    except OSError as exc:
        raise recovery.SerializerError(
            "EVIDENCE_STALE",
            f"cannot read correlation evidence: {exc}",
            location=manifest.outputs["correlation"].as_posix(),
        ) from exc
    if tracked != generated:
        raise recovery.SerializerError(
            "EVIDENCE_STALE",
            "tracked ONNX/NBG correlation evidence differs",
            location=manifest.outputs["correlation"].as_posix(),
        )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Bounded host-only ONNX/NBG correlation."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=recovery.DEFAULT_MANIFEST_PATH,
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("generate")
    subparsers.add_parser("verify-generated")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        manifest = recovery.load_manifest(args.manifest)
        if args.command == "generate":
            document = generate_evidence(manifest)
            print(
                "ONNX/NBG correlation generated: "
                f"decision={document['decision']['poc12b']}"
            )
            return 0
        if args.command == "verify-generated":
            verify_generated(manifest)
            print("ONNX/NBG correlation evidence is current")
            return 0
        return 2
    except recovery.SerializerError as exc:
        print(f"nbg-correlate: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
