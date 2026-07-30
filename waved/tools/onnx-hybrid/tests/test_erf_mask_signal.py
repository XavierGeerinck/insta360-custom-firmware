from __future__ import annotations

import hashlib

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_signal import (
    ANALYSIS_SAMPLES,
    ERF_BINS,
    ERF_INPUT_SCALE,
    ERF_INPUT_ZERO_POINT,
    ERF_OUTPUT_SCALE,
    ERF_OUTPUT_ZERO_POINT,
    HOP_SAMPLES,
    MASK_FLOOR,
    NOISE_EPSILON,
    SAMPLE_RATE,
    CausalNoiseEstimator,
    ErfMaskConfig,
    StreamingAnalysis,
    StreamingStft,
    StreamingSynthesis,
    build_erf_u8_table,
    cpu_erf_u8,
    mask_from_erf_u8,
    periodic_sqrt_hann,
    posterior_log_snr_feature,
    quantize_logits,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def test_signal_contract_constants_are_exact() -> None:
    assert SAMPLE_RATE == 48_000
    assert HOP_SAMPLES == 480
    assert ANALYSIS_SAMPLES == 960
    assert ERF_BINS == 481
    assert np.float32(0.03125) == ERF_INPUT_SCALE
    assert ERF_INPUT_ZERO_POINT == 128
    assert np.float32(0.0078125) == ERF_OUTPUT_SCALE
    assert ERF_OUTPUT_ZERO_POINT == 128
    assert np.float32(0.05) == MASK_FLOOR


def test_config_is_frozen_and_matches_contract() -> None:
    config = ErfMaskConfig()
    assert config.sample_rate == SAMPLE_RATE
    assert config.hop_samples == HOP_SAMPLES
    assert config.analysis_samples == ANALYSIS_SAMPLES
    assert config.erf_bins == ERF_BINS
    with pytest.raises(AttributeError):
        config.sample_rate = 16_000  # type: ignore[misc]


def test_quantize_logits_has_exact_endpoints_and_zero_point() -> None:
    logits = np.array([-5.0, -4.0, 0.0, 3.96875, 5.0], dtype=np.float32)
    original = logits.copy()

    observed = quantize_logits(logits)

    np.testing.assert_array_equal(
        observed,
        np.array([0, 0, 128, 255, 255], dtype=np.uint8),
    )
    np.testing.assert_array_equal(logits, original)
    assert observed.dtype == np.uint8
    assert observed.flags.c_contiguous


def test_quantize_logits_uses_ties_to_even() -> None:
    raw_quantized_values = np.array([0.5, 1.5, 126.5, 127.5], dtype=np.float64)
    logits = (raw_quantized_values - 128.0) / 32.0

    observed = quantize_logits(logits)

    np.testing.assert_array_equal(
        observed,
        np.array([0, 2, 126, 128], dtype=np.uint8),
    )


@pytest.mark.parametrize(
    "values",
    [
        np.array([np.nan], dtype=np.float32),
        np.array([np.inf], dtype=np.float32),
        np.array([-np.inf], dtype=np.float32),
    ],
)
def test_quantize_logits_rejects_nonfinite(values: np.ndarray) -> None:
    with pytest.raises(HybridError) as raised:
        quantize_logits(values)
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_erf_table_is_immutable_monotonic_and_hash_stable() -> None:
    table = build_erf_u8_table()

    assert table.shape == (256,)
    assert table.dtype == np.uint8
    assert table.flags.c_contiguous
    assert not table.flags.writeable
    assert np.all(table[1:] >= table[:-1])
    assert table[0] == 0
    assert table[96] == 20
    assert table[128] == 128
    assert table[160] == 236
    assert table[255] == 255
    assert (
        hashlib.sha256(table.tobytes()).hexdigest()
        == "c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296"
    )
    with pytest.raises(ValueError):
        table[0] = 1


def test_cpu_erf_u8_looks_up_without_mutating_input() -> None:
    inputs = np.array([0, 96, 128, 160, 255], dtype=np.uint8)
    original = inputs.copy()

    observed = cpu_erf_u8(inputs)

    np.testing.assert_array_equal(
        observed,
        np.array([0, 20, 128, 236, 255], dtype=np.uint8),
    )
    np.testing.assert_array_equal(inputs, original)
    assert observed.flags.c_contiguous


def test_cpu_erf_u8_rejects_wrong_dtype() -> None:
    with pytest.raises(HybridError) as raised:
        cpu_erf_u8(np.array([0, 128, 255], dtype=np.int16))
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_mask_conversion_applies_exact_floor_and_scale() -> None:
    values = np.array([0, 12, 13, 128, 255], dtype=np.uint8)

    observed = mask_from_erf_u8(values)

    np.testing.assert_array_equal(
        observed,
        np.array(
            [
                np.float32(0.05),
                np.float32(0.05),
                np.float32(13 / 256),
                np.float32(0.5),
                np.float32(255 / 256),
            ],
            dtype=np.float32,
        ),
    )
    assert observed.dtype == np.float32
    assert observed.flags.c_contiguous


def test_mask_conversion_rejects_wrong_dtype() -> None:
    with pytest.raises(HybridError) as raised:
        mask_from_erf_u8(np.array([0.0], dtype=np.float32))
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_periodic_sqrt_hann_has_exact_overlap_contract() -> None:
    window = periodic_sqrt_hann()

    assert window.shape == (ANALYSIS_SAMPLES,)
    assert window.dtype == np.float64
    assert not window.flags.writeable
    assert window[0] == 0.0
    assert window[HOP_SAMPLES] == 1.0
    overlap = np.square(window[:HOP_SAMPLES]) + np.square(window[HOP_SAMPLES:])
    np.testing.assert_allclose(overlap, np.ones(HOP_SAMPLES), atol=2e-15, rtol=0)


def test_analysis_returns_one_sided_spectrum_without_mutating_hop() -> None:
    analysis = StreamingAnalysis()
    hop = np.linspace(-0.5, 0.5, HOP_SAMPLES, dtype=np.float32)
    original = hop.copy()

    spectrum = analysis.analyze(hop)

    assert spectrum.shape == (ERF_BINS,)
    assert spectrum.dtype == np.complex128
    assert spectrum.flags.c_contiguous
    np.testing.assert_array_equal(hop, original)


def test_analysis_does_not_depend_on_a_future_hop() -> None:
    first = np.linspace(-0.25, 0.25, HOP_SAMPLES, dtype=np.float32)
    second = np.full(HOP_SAMPLES, 0.1, dtype=np.float32)
    future_a = np.zeros(HOP_SAMPLES, dtype=np.float32)
    future_b = np.ones(HOP_SAMPLES, dtype=np.float32)
    left = StreamingAnalysis()
    right = StreamingAnalysis()

    left.analyze(first)
    right.analyze(first)
    observed_left = left.analyze(second)
    observed_right = right.analyze(second)
    left.analyze(future_a)
    right.analyze(future_b)

    np.testing.assert_array_equal(observed_left, observed_right)


def test_synthesis_rejects_wrong_spectrum_shape() -> None:
    synthesis = StreamingSynthesis()
    with pytest.raises(HybridError) as raised:
        synthesis.synthesize(np.zeros(ERF_BINS - 1, dtype=np.complex128))
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def _flush_identity(
    processor: StreamingStft,
    hops: list[np.ndarray],
) -> np.ndarray:
    identity = np.ones(ERF_BINS, dtype=np.float32)
    outputs = [processor.process(hop, identity) for hop in hops]
    outputs.append(processor.process(np.zeros(HOP_SAMPLES, dtype=np.float32), identity))
    return np.concatenate(outputs[1:])


def test_identity_mask_reconstructs_seeded_signal_after_startup() -> None:
    rng = np.random.Generator(np.random.PCG64(0x57335645))
    hops = [rng.uniform(-0.75, 0.75, HOP_SAMPLES).astype(np.float32) for _ in range(32)]
    expected = np.concatenate(hops)

    observed = _flush_identity(StreamingStft(), hops)

    assert observed.dtype == np.float32
    assert observed.shape == expected.shape
    assert np.max(np.abs(observed - expected)) < 2e-6
    assert np.sqrt(np.mean(np.square(observed - expected))) < 1e-7


def test_identity_mask_reconstructs_impulse_and_sine() -> None:
    impulse = np.zeros(HOP_SAMPLES * 4, dtype=np.float32)
    impulse[713] = 0.75
    time = np.arange(HOP_SAMPLES * 4, dtype=np.float64)
    sine = (0.4 * np.sin(2.0 * np.pi * 997.0 * time / SAMPLE_RATE)).astype(np.float32)

    for signal in (impulse, sine):
        hops = list(signal.reshape(-1, HOP_SAMPLES))
        observed = _flush_identity(StreamingStft(), hops)
        np.testing.assert_allclose(observed, signal, atol=2e-6, rtol=0)


def test_identity_mask_completes_10_000_hops_without_state_drift() -> None:
    processor = StreamingStft()
    identity = np.ones(ERF_BINS, dtype=np.float32)
    previous = np.zeros(HOP_SAMPLES, dtype=np.float32)
    maximum_error = 0.0

    for index in range(10_000):
        phase = np.arange(HOP_SAMPLES, dtype=np.float64) + index * HOP_SAMPLES
        current = (0.3 * np.sin(2.0 * np.pi * 431.0 * phase / SAMPLE_RATE)).astype(
            np.float32
        )
        output = processor.process(current, identity)
        if index == 0:
            np.testing.assert_array_equal(
                output,
                np.zeros(HOP_SAMPLES, dtype=np.float32),
            )
        else:
            maximum_error = max(
                maximum_error,
                float(np.max(np.abs(output - previous))),
            )
        previous = current

    assert maximum_error < 2e-6


def test_streaming_reset_restores_byte_equivalent_initial_state() -> None:
    processor = StreamingStft()
    identity = np.ones(ERF_BINS, dtype=np.float32)
    first = np.linspace(-0.2, 0.2, HOP_SAMPLES, dtype=np.float32)
    second = np.linspace(0.3, -0.3, HOP_SAMPLES, dtype=np.float32)

    initial_outputs = (
        processor.process(first, identity),
        processor.process(second, identity),
    )
    processor.reset()
    reset_outputs = (
        processor.process(first, identity),
        processor.process(second, identity),
    )

    for initial, reset in zip(initial_outputs, reset_outputs, strict=True):
        np.testing.assert_array_equal(initial, reset)


@pytest.mark.parametrize(
    "hop",
    [
        np.zeros(HOP_SAMPLES - 1, dtype=np.float32),
        np.zeros(HOP_SAMPLES + 1, dtype=np.float32),
        np.full(HOP_SAMPLES, np.nan, dtype=np.float32),
        np.full(HOP_SAMPLES, np.inf, dtype=np.float32),
    ],
)
def test_streaming_rejects_invalid_hop(hop: np.ndarray) -> None:
    with pytest.raises(HybridError) as raised:
        StreamingStft().process(hop, np.ones(ERF_BINS, dtype=np.float32))
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


@pytest.mark.parametrize(
    "mask",
    [
        np.ones(ERF_BINS - 1, dtype=np.float32),
        np.ones(ERF_BINS + 1, dtype=np.float32),
        np.full(ERF_BINS, np.nan, dtype=np.float32),
        np.full(ERF_BINS, np.inf, dtype=np.float32),
    ],
)
def test_streaming_rejects_invalid_mask(mask: np.ndarray) -> None:
    with pytest.raises(HybridError) as raised:
        StreamingStft().process(
            np.zeros(HOP_SAMPLES, dtype=np.float32),
            mask,
        )
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_noise_estimator_first_frame_initializes_exactly() -> None:
    estimator = CausalNoiseEstimator()
    power = np.linspace(0.0, 2.0, ERF_BINS, dtype=np.float64)
    original = power.copy()

    observed = estimator.estimate(power)

    np.testing.assert_array_equal(observed, np.maximum(power, NOISE_EPSILON))
    np.testing.assert_array_equal(power, original)
    assert observed.dtype == np.float64
    assert observed.flags.c_contiguous


def test_noise_estimator_uses_noise_and_speech_update_coefficients() -> None:
    noise_dominant = CausalNoiseEstimator()
    speech_dominant = CausalNoiseEstimator()
    baseline = np.ones(ERF_BINS, dtype=np.float64)
    noise_dominant.estimate(baseline)
    speech_dominant.estimate(baseline)

    observed_noise = noise_dominant.estimate(np.full(ERF_BINS, 0.5, dtype=np.float64))
    observed_speech = speech_dominant.estimate(
        np.full(ERF_BINS, 10.0, dtype=np.float64)
    )

    np.testing.assert_allclose(observed_noise, 1.035, atol=1e-15, rtol=0)
    np.testing.assert_allclose(observed_speech, 1.0025, atol=1e-15, rtol=0)


def _scalar_noise_trace(power: np.ndarray) -> np.ndarray:
    smoothed = 0.0
    noise = 0.0
    ring: list[float] = []
    output: list[float] = []
    for index, current in enumerate(power):
        bounded = max(float(current), NOISE_EPSILON)
        if index == 0:
            smoothed = bounded
            noise = bounded
        else:
            smoothed = 0.8 * smoothed + 0.2 * bounded
            ring.append(smoothed)
            if len(ring) > 100:
                ring.pop(0)
            candidate = max(1.5 * min(ring), NOISE_EPSILON)
            if bounded <= 3.0 * noise:
                noise = 0.9 * noise + 0.1 * candidate
            else:
                noise = 0.995 * noise + 0.005 * candidate
        if index == 0:
            ring.append(smoothed)
        output.append(max(noise, NOISE_EPSILON))
    return np.asarray(output, dtype=np.float64)


def test_noise_estimator_matches_slow_scalar_oracle_for_1_000_frames() -> None:
    rng = np.random.Generator(np.random.PCG64(0x1304))
    powers = np.exp(
        rng.uniform(
            np.log(1e-8),
            np.log(10.0),
            size=(1_000, ERF_BINS),
        )
    )
    estimator = CausalNoiseEstimator()
    observed = np.stack([estimator.estimate(frame) for frame in powers])

    for bin_index in (0, 137, ERF_BINS - 1):
        np.testing.assert_allclose(
            observed[:, bin_index],
            _scalar_noise_trace(powers[:, bin_index]),
            atol=2e-15,
            rtol=2e-15,
        )


def test_noise_estimator_reset_is_deterministic() -> None:
    estimator = CausalNoiseEstimator()
    first = np.linspace(0.01, 1.0, ERF_BINS, dtype=np.float64)
    second = np.linspace(2.0, 0.02, ERF_BINS, dtype=np.float64)

    initial = (estimator.estimate(first), estimator.estimate(second))
    estimator.reset()
    reset = (estimator.estimate(first), estimator.estimate(second))

    for expected, observed in zip(initial, reset, strict=True):
        np.testing.assert_array_equal(observed, expected)


@pytest.mark.parametrize(
    "power",
    [
        np.ones(ERF_BINS - 1, dtype=np.float64),
        np.full(ERF_BINS, -1.0, dtype=np.float64),
        np.full(ERF_BINS, np.nan, dtype=np.float64),
        np.full(ERF_BINS, np.inf, dtype=np.float64),
    ],
)
def test_noise_estimator_rejects_invalid_power(power: np.ndarray) -> None:
    with pytest.raises(HybridError) as raised:
        CausalNoiseEstimator().estimate(power)
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_posterior_feature_has_exact_range_and_zero() -> None:
    power = np.array(
        [1.0, np.exp(20.0), 0.0] + [1.0] * (ERF_BINS - 3),
        dtype=np.float64,
    )
    noise = np.ones(ERF_BINS, dtype=np.float64)

    observed = posterior_log_snr_feature(power, noise)

    assert observed.dtype == np.float32
    assert observed[0] == np.float32(0.0)
    assert observed[1] == np.float32(8.0)
    assert observed[2] == np.float32(-8.0)
    assert np.min(observed) >= -8.0
    assert np.max(observed) <= 8.0


def test_posterior_feature_rejects_nonpositive_noise() -> None:
    with pytest.raises(HybridError) as raised:
        posterior_log_snr_feature(
            np.ones(ERF_BINS, dtype=np.float64),
            np.zeros(ERF_BINS, dtype=np.float64),
        )
    assert raised.value.code is ErrorCode.ONNX_INPUT_INVALID
