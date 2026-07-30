"""Deterministic six-vector qualification pack for the sealed Erf NBG."""

from __future__ import annotations

import hashlib
import struct
from dataclasses import dataclass

import numpy as np

from .erf_mask_inference import ErfMaskDenoiser
from .erf_mask_signal import ERF_BINS, cpu_erf_u8
from .errors import ErrorCode, HybridError

VECTOR_PACK_MAGIC = b"ERF1"
VECTOR_PACK_SCHEMA_VERSION = 1
VECTOR_PACK_HEADER = struct.Struct("<4sIII")
QUALIFICATION_VECTOR_IDS = (
    "zero-point",
    "minimum",
    "maximum",
    "repeated-ramp",
    "seeded-random",
    "learned-fixture",
)
QUALIFICATION_RANDOM_SEED = 0x57335645
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


@dataclass(frozen=True, slots=True)
class QualificationVectors:
    vectors: tuple[np.ndarray, ...]
    learned_frame_index: int

    def __post_init__(self) -> None:
        if (
            len(self.vectors) != len(QUALIFICATION_VECTOR_IDS)
            or type(self.learned_frame_index) is not int
            or self.learned_frame_index < 0
        ):
            raise _invalid("invalid qualification vector collection")
        for vector in self.vectors:
            if (
                not isinstance(vector, np.ndarray)
                or vector.shape != (ERF_BINS,)
                or vector.dtype != np.dtype(np.uint8)
                or vector.flags.writeable
                or not vector.flags.c_contiguous
            ):
                raise _invalid("invalid immutable qualification vector")


def _readonly_u8(values: np.ndarray) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=np.uint8)
    result.flags.writeable = False
    return result


def _seeded_random_vector() -> np.ndarray:
    values = np.empty(ERF_BINS, dtype=np.uint8)
    state = QUALIFICATION_RANDOM_SEED
    for index in range(ERF_BINS):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        state &= 0xFFFFFFFF
        values[index] = state & 0xFF
    return _readonly_u8(values)


def build_qualification_vectors(
    slopes: np.ndarray,
    biases: np.ndarray,
    fixture_mixture: np.ndarray,
) -> QualificationVectors:
    """Build fixed boundary vectors and select one real learned frame."""

    captured: list[np.ndarray] = []

    def capture(values: np.ndarray) -> np.ndarray:
        captured.append(_readonly_u8(values))
        return cpu_erf_u8(values)

    ErfMaskDenoiser(
        slopes,
        biases,
        partition=capture,
    ).process_audio(fixture_mixture, compensate_delay=True)
    if not captured:
        raise _invalid("fixture produced no learned partition vectors")
    ranked = [
        (
            int(np.unique(cpu_erf_u8(vector)).size),
            int(np.unique(vector).size),
            float(np.var(vector.astype(np.float64))),
            -index,
            index,
        )
        for index, vector in enumerate(captured)
    ]
    learned_frame_index = max(ranked)[-1]
    learned = captured[learned_frame_index]
    learned_output = cpu_erf_u8(learned)
    if (
        np.unique(learned).size < 2
        or np.unique(learned_output).size < 2
        or np.array_equal(learned, learned_output)
    ):
        raise _invalid("learned qualification vector is degenerate")
    ramp = np.arange(ERF_BINS, dtype=np.uint16) % 256
    return QualificationVectors(
        vectors=(
            _readonly_u8(np.full(ERF_BINS, 128, dtype=np.uint8)),
            _readonly_u8(np.zeros(ERF_BINS, dtype=np.uint8)),
            _readonly_u8(np.full(ERF_BINS, 255, dtype=np.uint8)),
            _readonly_u8(ramp),
            _seeded_random_vector(),
            learned,
        ),
        learned_frame_index=learned_frame_index,
    )


def serialize_qualification_vectors(
    collection: QualificationVectors,
) -> bytes:
    payload = b"".join(vector.tobytes() for vector in collection.vectors)
    return (
        VECTOR_PACK_HEADER.pack(
            VECTOR_PACK_MAGIC,
            VECTOR_PACK_SCHEMA_VERSION,
            len(collection.vectors),
            ERF_BINS,
        )
        + payload
    )


def parse_qualification_vectors(payload: bytes) -> tuple[np.ndarray, ...]:
    if not isinstance(payload, bytes) or len(payload) < VECTOR_PACK_HEADER.size:
        raise _invalid("qualification vector pack is truncated")
    magic, schema, count, width = VECTOR_PACK_HEADER.unpack_from(payload)
    expected_bytes = VECTOR_PACK_HEADER.size + count * width
    if (
        magic != VECTOR_PACK_MAGIC
        or schema != VECTOR_PACK_SCHEMA_VERSION
        or count != len(QUALIFICATION_VECTOR_IDS)
        or width != ERF_BINS
        or len(payload) != expected_bytes
    ):
        raise _invalid("qualification vector pack header or length differs")
    matrix = np.frombuffer(
        payload,
        dtype=np.uint8,
        offset=VECTOR_PACK_HEADER.size,
    ).reshape(count, width)
    return tuple(_readonly_u8(row) for row in matrix)


def fnv1a64(payload: bytes) -> int:
    if not isinstance(payload, bytes):
        raise _invalid("FNV input must be immutable bytes")
    value = FNV64_OFFSET_BASIS
    for byte in payload:
        value ^= byte
        value = (value * FNV64_PRIME) & 0xFFFFFFFFFFFFFFFF
    return value


def qualification_vector_manifest(
    collection: QualificationVectors,
) -> dict[str, object]:
    records = []
    for vector_id, vector in zip(
        QUALIFICATION_VECTOR_IDS,
        collection.vectors,
        strict=True,
    ):
        output = cpu_erf_u8(vector)
        records.append(
            {
                "id": vector_id,
                "input_sha256": hashlib.sha256(vector).hexdigest(),
                "output_sha256": hashlib.sha256(output).hexdigest(),
                "input_min": int(vector.min()),
                "input_max": int(vector.max()),
                "input_unique": int(np.unique(vector).size),
                "output_min": int(output.min()),
                "output_max": int(output.max()),
                "output_unique": int(np.unique(output).size),
            }
        )
    return {
        "schema_version": 1,
        "vector_count": len(records),
        "width": ERF_BINS,
        "random_seed": QUALIFICATION_RANDOM_SEED,
        "learned_frame_index": collection.learned_frame_index,
        "vectors": records,
    }
