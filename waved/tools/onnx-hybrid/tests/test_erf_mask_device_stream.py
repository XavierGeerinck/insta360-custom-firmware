from __future__ import annotations

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_device_stream import (
    DFN_GUARD_SAMPLES,
    STREAM_INPUT_MAGIC,
    STREAM_OUTPUT_MAGIC,
    ErfFrameStream,
    ReplayErfPartition,
    align_dfn_fixture_output,
    capture_erf_inputs,
    cpu_output_stream,
    dfn_fixture_input_bytes,
    parse_erf_stream,
    serialize_erf_stream,
)
from wave_onnx_hybrid.erf_mask_inference import HybridErfPartition
from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    cpu_erf_u8,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _frame(offset: int = 0) -> np.ndarray:
    values = (np.arange(ERF_BINS, dtype=np.uint16) + offset) % 256
    result = np.ascontiguousarray(values, dtype=np.uint8)
    result.flags.writeable = False
    return result


def test_stream_serialization_and_cpu_outputs_round_trip() -> None:
    inputs = ErfFrameStream((_frame(), _frame(17)))
    outputs = cpu_output_stream(inputs)

    input_payload = serialize_erf_stream(inputs, magic=STREAM_INPUT_MAGIC)
    output_payload = serialize_erf_stream(outputs, magic=STREAM_OUTPUT_MAGIC)

    parsed_inputs = parse_erf_stream(
        input_payload,
        magic=STREAM_INPUT_MAGIC,
        expected_count=2,
    )
    parsed_outputs = parse_erf_stream(
        output_payload,
        magic=STREAM_OUTPUT_MAGIC,
        expected_count=2,
    )
    for observed, expected in zip(parsed_inputs.frames, inputs.frames, strict=True):
        np.testing.assert_array_equal(observed, expected)
    for observed, expected in zip(parsed_outputs.frames, outputs.frames, strict=True):
        np.testing.assert_array_equal(observed, expected)


def test_capture_includes_offline_flush_hop() -> None:
    slopes = np.zeros(ERF_BINS, dtype=np.float32)
    biases = np.zeros(ERF_BINS, dtype=np.float32)
    mixture = np.zeros(2 * HOP_SAMPLES, dtype=np.float32)

    stream = capture_erf_inputs(slopes, biases, mixture)

    assert stream.count == 3
    assert all(frame.shape == (ERF_BINS,) for frame in stream.frames)


def test_replay_requires_exact_live_inputs_and_exhaustion() -> None:
    inputs = ErfFrameStream((_frame(),))
    outputs = cpu_output_stream(inputs)
    replay = ReplayErfPartition(inputs, outputs)

    np.testing.assert_array_equal(replay(_frame()), outputs.frames[0])
    assert replay.complete
    with pytest.raises(HybridError) as raised:
        replay(_frame())
    assert raised.value.code is ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH

    mismatch = ReplayErfPartition(inputs, outputs)
    with pytest.raises(HybridError) as raised:
        mismatch(_frame(1))
    assert raised.value.code is ErrorCode.NPU_INPUT_QUANTIZATION_MISMATCH


def test_dfn_fixture_layout_and_delay_alignment_are_exact() -> None:
    mixture = np.arange(2 * HOP_SAMPLES, dtype=np.float32) / 1000.0
    payload = dfn_fixture_input_bytes(mixture)
    values = np.frombuffer(payload, dtype="<f4")

    assert values.size == mixture.size + 2 * DFN_GUARD_SAMPLES
    np.testing.assert_array_equal(
        values[:DFN_GUARD_SAMPLES],
        np.zeros(DFN_GUARD_SAMPLES, dtype=np.float32),
    )
    np.testing.assert_array_equal(
        values[DFN_GUARD_SAMPLES : DFN_GUARD_SAMPLES + mixture.size],
        mixture,
    )
    aligned = align_dfn_fixture_output(payload, mixture_samples=mixture.size)
    np.testing.assert_array_equal(
        aligned,
        values[2 * HOP_SAMPLES : 2 * HOP_SAMPLES + mixture.size],
    )


def test_hybrid_partition_uses_npu_until_success() -> None:
    calls = 0

    def npu(values: np.ndarray) -> np.ndarray:
        nonlocal calls
        calls += 1
        return cpu_erf_u8(values)

    hybrid = HybridErfPartition(npu)
    expected = cpu_erf_u8(_frame())

    np.testing.assert_array_equal(hybrid(_frame()), expected)
    np.testing.assert_array_equal(hybrid(_frame()), expected)
    assert calls == 2
    assert hybrid.npu_attempts == 2
    assert hybrid.npu_successes == 2
    assert hybrid.fallback_frames == 0
    assert not hybrid.fallback_latched


@pytest.mark.parametrize(
    "failure",
    [
        RuntimeError("device unavailable"),
        np.zeros(ERF_BINS, dtype=np.int16),
    ],
)
def test_hybrid_partition_latches_byte_exact_cpu_fallback(
    failure: Exception | np.ndarray,
) -> None:
    calls = 0

    def npu(_: np.ndarray) -> np.ndarray:
        nonlocal calls
        calls += 1
        if isinstance(failure, Exception):
            raise failure
        return failure

    hybrid = HybridErfPartition(npu)
    first = _frame()
    second = _frame(9)

    np.testing.assert_array_equal(hybrid(first), cpu_erf_u8(first))
    np.testing.assert_array_equal(hybrid(second), cpu_erf_u8(second))
    assert calls == 1
    assert hybrid.npu_attempts == 1
    assert hybrid.npu_successes == 0
    assert hybrid.fallback_frames == 2
    assert hybrid.fallback_latched
    assert hybrid.first_failure is not None
