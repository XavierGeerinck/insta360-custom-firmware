"""Sealed full-fixture stream exchange for the Wave Erf NPU partition."""

from __future__ import annotations

import struct
from dataclasses import dataclass

import numpy as np

from .erf_mask_inference import ErfMaskDenoiser
from .erf_mask_signal import ERF_BINS, HOP_SAMPLES, cpu_erf_u8
from .errors import ErrorCode, HybridError

STREAM_SCHEMA_VERSION = 1
STREAM_INPUT_MAGIC = b"ERI1"
STREAM_OUTPUT_MAGIC = b"ERO1"
STREAM_HEADER = struct.Struct("<4sIII")
DFN_GUARD_SAMPLES = HOP_SAMPLES
DFN_OUTPUT_DELAY_SAMPLES = HOP_SAMPLES


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _readonly_u8(values: np.ndarray) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=np.uint8)
    result.flags.writeable = False
    return result


@dataclass(frozen=True, slots=True)
class ErfFrameStream:
    frames: tuple[np.ndarray, ...]

    def __post_init__(self) -> None:
        if not self.frames:
            raise _invalid("Erf frame stream cannot be empty")
        for frame in self.frames:
            if (
                not isinstance(frame, np.ndarray)
                or frame.shape != (ERF_BINS,)
                or frame.dtype != np.dtype(np.uint8)
                or frame.flags.writeable
                or not frame.flags.c_contiguous
            ):
                raise _invalid("Erf frame stream contains an invalid frame")

    @property
    def count(self) -> int:
        return len(self.frames)


def capture_erf_inputs(
    slopes: np.ndarray,
    biases: np.ndarray,
    mixture: np.ndarray,
) -> ErfFrameStream:
    """Capture every exact partition input, including the offline flush hop."""

    captured: list[np.ndarray] = []

    def capture(values: np.ndarray) -> np.ndarray:
        captured.append(_readonly_u8(values))
        return cpu_erf_u8(values)

    denoiser = ErfMaskDenoiser(slopes, biases, partition=capture)
    denoiser.process_audio(mixture, compensate_delay=True)
    if denoiser.partition_calls != len(captured):
        raise _invalid("captured Erf call count differs")
    return ErfFrameStream(tuple(captured))


def serialize_erf_stream(
    stream: ErfFrameStream,
    *,
    magic: bytes,
) -> bytes:
    if magic not in {STREAM_INPUT_MAGIC, STREAM_OUTPUT_MAGIC}:
        raise _invalid("unknown Erf stream magic")
    return STREAM_HEADER.pack(
        magic,
        STREAM_SCHEMA_VERSION,
        stream.count,
        ERF_BINS,
    ) + b"".join(frame.tobytes() for frame in stream.frames)


def parse_erf_stream(
    payload: bytes,
    *,
    magic: bytes,
    expected_count: int | None = None,
) -> ErfFrameStream:
    if not isinstance(payload, bytes) or len(payload) < STREAM_HEADER.size:
        raise _invalid("Erf stream payload is truncated")
    observed_magic, schema, count, width = STREAM_HEADER.unpack_from(payload)
    expected_bytes = STREAM_HEADER.size + count * width
    if (
        magic not in {STREAM_INPUT_MAGIC, STREAM_OUTPUT_MAGIC}
        or observed_magic != magic
        or schema != STREAM_SCHEMA_VERSION
        or count <= 0
        or width != ERF_BINS
        or len(payload) != expected_bytes
        or (expected_count is not None and count != expected_count)
    ):
        raise _invalid("Erf stream header or length differs")
    matrix = np.frombuffer(
        payload,
        dtype=np.uint8,
        offset=STREAM_HEADER.size,
    ).reshape(count, width)
    return ErfFrameStream(tuple(_readonly_u8(row) for row in matrix))


def cpu_output_stream(inputs: ErfFrameStream) -> ErfFrameStream:
    return ErfFrameStream(
        tuple(_readonly_u8(cpu_erf_u8(frame)) for frame in inputs.frames)
    )


class ReplayErfPartition:
    """Replay sealed device outputs only when every live input is identical."""

    def __init__(
        self,
        inputs: ErfFrameStream,
        outputs: ErfFrameStream,
    ) -> None:
        if inputs.count != outputs.count:
            raise _invalid("Erf replay input/output frame counts differ")
        self._inputs = inputs
        self._outputs = outputs
        self._index = 0

    @property
    def calls(self) -> int:
        return self._index

    @property
    def complete(self) -> bool:
        return self._index == self._inputs.count

    def __call__(self, values: np.ndarray) -> np.ndarray:
        observed = np.asarray(values)
        if self._index >= self._inputs.count:
            raise HybridError(
                ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
                "device Erf output stream was exhausted",
            )
        expected = self._inputs.frames[self._index]
        if (
            observed.shape != (ERF_BINS,)
            or observed.dtype != np.dtype(np.uint8)
            or not np.array_equal(observed, expected)
        ):
            raise HybridError(
                ErrorCode.NPU_INPUT_QUANTIZATION_MISMATCH,
                f"live Erf input differs at frame {self._index}",
            )
        output = self._outputs.frames[self._index]
        self._index += 1
        return np.ascontiguousarray(output)


def dfn_fixture_input_bytes(mixture: np.ndarray) -> bytes:
    values = np.asarray(mixture)
    if (
        values.ndim != 1
        or values.size == 0
        or values.size % HOP_SAMPLES != 0
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid("DFN fixture mixture contract differs")
    guard = np.zeros(DFN_GUARD_SAMPLES, dtype="<f4")
    payload = np.concatenate(
        (
            guard,
            np.ascontiguousarray(values, dtype="<f4"),
            guard,
        )
    )
    return payload.astype("<f4", copy=False).tobytes()


def align_dfn_fixture_output(
    payload: bytes,
    *,
    mixture_samples: int,
) -> np.ndarray:
    total_samples = DFN_GUARD_SAMPLES + mixture_samples + DFN_GUARD_SAMPLES
    if (
        not isinstance(payload, bytes)
        or len(payload) != total_samples * np.dtype(np.float32).itemsize
    ):
        raise _invalid("DFN fixture output byte count differs")
    values = np.frombuffer(payload, dtype="<f4")
    if not np.isfinite(values).all():
        raise _invalid("DFN fixture output contains non-finite samples")
    start = DFN_GUARD_SAMPLES + DFN_OUTPUT_DELAY_SAMPLES
    stop = start + mixture_samples
    if stop > values.size:
        raise _invalid("DFN fixture output is truncated after delay")
    return np.ascontiguousarray(values[start:stop], dtype=np.float32)
