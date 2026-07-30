from __future__ import annotations

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_corpus import generate_clean_source
from wave_onnx_hybrid.erf_mask_inference import (
    DFN_CLEAN_RETAIN_DB,
    DFN_CLEAN_RETAIN_POWER_RATIO,
    ERF_RESIDUAL_MIX,
    DfnErfFrameSelector,
    ErfMaskDenoiser,
    ErfResidualPostfilter,
)
from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    MASK_FLOOR,
    SAMPLE_RATE,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _weights() -> tuple[np.ndarray, np.ndarray]:
    return (
        np.ones(ERF_BINS, dtype=np.float32),
        np.zeros(ERF_BINS, dtype=np.float32),
    )


def test_partition_receives_exact_u8_boundary_and_drives_waveform() -> None:
    slopes, biases = _weights()
    calls: list[np.ndarray] = []

    def half_mask(values: np.ndarray) -> np.ndarray:
        calls.append(values.copy())
        return np.full(values.shape, 128, dtype=np.uint8)

    denoiser = ErfMaskDenoiser(slopes, biases, partition=half_mask)
    audio = generate_clean_source("voiced", SAMPLE_RATE, seed=13)
    enhanced = denoiser.process_audio(audio, compensate_delay=True)

    assert len(calls) == SAMPLE_RATE // HOP_SAMPLES + 1
    assert all(value.shape == (ERF_BINS,) for value in calls)
    assert all(value.dtype == np.uint8 for value in calls)
    assert enhanced.shape == audio.shape
    np.testing.assert_allclose(enhanced, audio * 0.5, atol=2e-6, rtol=0)


def test_public_streaming_output_has_one_hop_delay() -> None:
    slopes, biases = _weights()

    def maximum_mask(values: np.ndarray) -> np.ndarray:
        return np.full(values.shape, 255, dtype=np.uint8)

    denoiser = ErfMaskDenoiser(slopes, biases, partition=maximum_mask)
    audio = generate_clean_source("unvoiced", HOP_SAMPLES * 4, seed=14)
    observed = denoiser.process_audio(audio, compensate_delay=False)

    np.testing.assert_array_equal(
        observed[:HOP_SAMPLES],
        np.zeros(HOP_SAMPLES, dtype=np.float32),
    )
    np.testing.assert_allclose(
        observed[HOP_SAMPLES:],
        audio[:-HOP_SAMPLES] * np.float32(255.0 / 256.0),
        atol=2e-6,
        rtol=0,
    )


def test_partition_wrong_dtype_shape_and_exception_fail_closed() -> None:
    slopes, biases = _weights()
    hop = np.zeros(HOP_SAMPLES, dtype=np.float32)

    wrong_shape = ErfMaskDenoiser(
        slopes,
        biases,
        partition=lambda values: np.zeros(values.size - 1, dtype=np.uint8),
    )
    with pytest.raises(HybridError) as shape:
        wrong_shape.process_hop(hop)
    assert shape.value.code is ErrorCode.NPU_OUTPUT_QUANTIZATION_MISMATCH

    wrong_dtype = ErfMaskDenoiser(
        slopes,
        biases,
        partition=lambda values: np.zeros(values.shape, dtype=np.int8),
    )
    with pytest.raises(HybridError) as dtype:
        wrong_dtype.process_hop(hop)
    assert dtype.value.code is ErrorCode.NPU_OUTPUT_QUANTIZATION_MISMATCH

    def fail(_values: np.ndarray) -> np.ndarray:
        raise RuntimeError("partition failed")

    failed = ErfMaskDenoiser(slopes, biases, partition=fail)
    with pytest.raises(HybridError) as exception:
        failed.process_hop(hop)
    assert exception.value.code is ErrorCode.CPU_FALLBACK_FAILURE


def test_reset_restores_byte_identical_stream() -> None:
    slopes, biases = _weights()
    audio = generate_clean_source("voiced", HOP_SAMPLES * 10, seed=15)
    denoiser = ErfMaskDenoiser(slopes, biases)

    first = denoiser.process_audio(audio, compensate_delay=True)
    denoiser.reset()
    second = denoiser.process_audio(audio, compensate_delay=True)

    np.testing.assert_array_equal(first, second)


def test_residual_postfilter_has_frozen_mix_and_preserves_base_at_zero() -> None:
    slopes, biases = _weights()
    noisy = generate_clean_source("voiced", SAMPLE_RATE, seed=16)
    base = generate_clean_source("unvoiced", SAMPLE_RATE, seed=17)

    assert np.float32(1.0 / 32.0) == ERF_RESIDUAL_MIX
    postfilter = ErfResidualPostfilter(
        slopes,
        biases,
        residual_mix=0.0,
    )
    observed = postfilter.process_audio(
        noisy,
        base,
        compensate_delay=True,
    )

    np.testing.assert_allclose(observed, base, atol=2e-6, rtol=0)


def test_residual_partition_values_directly_change_enhanced_waveform() -> None:
    slopes, biases = _weights()
    noisy = generate_clean_source("voiced", SAMPLE_RATE, seed=18)
    base = noisy.copy()

    def low(values: np.ndarray) -> np.ndarray:
        return np.zeros(values.shape, dtype=np.uint8)

    def high(values: np.ndarray) -> np.ndarray:
        return np.full(values.shape, 255, dtype=np.uint8)

    low_output = ErfResidualPostfilter(
        slopes,
        biases,
        partition=low,
    ).process_audio(noisy, base, compensate_delay=True)
    high_output = ErfResidualPostfilter(
        slopes,
        biases,
        partition=high,
    ).process_audio(noisy, base, compensate_delay=True)

    assert not np.array_equal(low_output, high_output)
    np.testing.assert_allclose(
        low_output,
        base * np.float32(1.0 - ERF_RESIDUAL_MIX * 0.95),
        atol=2e-6,
        rtol=0,
    )
    np.testing.assert_allclose(
        high_output,
        base * np.float32(1.0 - ERF_RESIDUAL_MIX * (1.0 - 255.0 / 256.0)),
        atol=2e-6,
        rtol=0,
    )


def test_residual_postfilter_rejects_misaligned_base_audio() -> None:
    slopes, biases = _weights()
    audio = np.zeros(HOP_SAMPLES * 2, dtype=np.float32)
    postfilter = ErfResidualPostfilter(slopes, biases)

    with pytest.raises(HybridError) as mismatch:
        postfilter.process_audio(
            audio,
            audio[:-HOP_SAMPLES],
            compensate_delay=True,
        )
    assert mismatch.value.code is ErrorCode.ONNX_INPUT_INVALID


def test_selector_contract_and_clean_retained_path_choose_dfn() -> None:
    slopes, biases = _weights()
    audio = generate_clean_source("voiced", SAMPLE_RATE, seed=19)
    partition_inputs: list[np.ndarray] = []

    def zero_mask(values: np.ndarray) -> np.ndarray:
        partition_inputs.append(values.copy())
        return np.zeros(values.shape, dtype=np.uint8)

    selector = DfnErfFrameSelector(
        slopes,
        biases,
        partition=zero_mask,
    )
    observed = selector.process_audio(audio, audio, compensate_delay=True)

    assert DFN_CLEAN_RETAIN_DB == 1.0
    assert pytest.approx(10.0**-0.1) == DFN_CLEAN_RETAIN_POWER_RATIO
    assert selector.partition_calls == SAMPLE_RATE // HOP_SAMPLES + 1
    assert len(partition_inputs) == selector.partition_calls
    assert all(value.dtype == np.uint8 for value in partition_inputs)
    assert selector.erf_selected_frames == 0
    assert selector.dfn_selected_frames == selector.partition_calls
    assert selector.erf_selection_fraction == 0.0
    np.testing.assert_allclose(observed, audio, atol=2e-6, rtol=0)


def test_selector_suppressed_base_chooses_erf_and_output_drives_audio() -> None:
    slopes, biases = _weights()
    audio = generate_clean_source("unvoiced", SAMPLE_RATE, seed=20)
    base = np.zeros_like(audio)

    def low(values: np.ndarray) -> np.ndarray:
        return np.zeros(values.shape, dtype=np.uint8)

    def high(values: np.ndarray) -> np.ndarray:
        return np.full(values.shape, 255, dtype=np.uint8)

    low_selector = DfnErfFrameSelector(slopes, biases, partition=low)
    low_output = low_selector.process_audio(
        audio,
        base,
        compensate_delay=True,
    )
    high_selector = DfnErfFrameSelector(slopes, biases, partition=high)
    high_output = high_selector.process_audio(
        audio,
        base,
        compensate_delay=True,
    )

    assert low_selector.erf_selected_frames > 0
    assert low_selector.erf_selected_frames == high_selector.erf_selected_frames
    assert not np.array_equal(low_output, high_output)
    np.testing.assert_allclose(
        low_output,
        audio * MASK_FLOOR,
        atol=2e-6,
        rtol=0,
    )
    np.testing.assert_allclose(
        high_output,
        audio * np.float32(255.0 / 256.0),
        atol=2e-6,
        rtol=0,
    )


def test_selector_can_execute_a_complete_float_feature_partition() -> None:
    slopes, biases = _weights()
    audio = generate_clean_source("unvoiced", SAMPLE_RATE, seed=21)
    base = np.zeros_like(audio)
    observed_features: list[np.ndarray] = []

    def feature_partition(values: np.ndarray) -> np.ndarray:
        observed_features.append(values.copy())
        return np.full(ERF_BINS, 255, dtype=np.uint8)

    selector = DfnErfFrameSelector(
        slopes,
        biases,
        feature_partition=feature_partition,
    )
    observed = selector.process_audio(audio, base, compensate_delay=True)

    assert len(observed_features) == selector.partition_calls
    assert all(value.shape == (ERF_BINS,) for value in observed_features)
    assert all(value.dtype == np.float64 for value in observed_features)
    assert selector.erf_selected_frames > 0
    np.testing.assert_allclose(
        observed,
        audio * np.float32(255.0 / 256.0),
        atol=2e-6,
        rtol=0,
    )


def test_selector_zero_energy_is_safe_and_reset_is_byte_identical() -> None:
    slopes, biases = _weights()
    zeros = np.zeros(HOP_SAMPLES * 4, dtype=np.float32)
    selector = DfnErfFrameSelector(slopes, biases)

    first = selector.process_audio(zeros, zeros, compensate_delay=True)
    assert selector.partition_calls == 5
    assert selector.erf_selected_frames == 0
    selector.reset()
    second = selector.process_audio(zeros, zeros, compensate_delay=True)

    assert selector.partition_calls == 5
    assert selector.erf_selected_frames == 0
    np.testing.assert_array_equal(first, second)


def test_selector_rejects_bad_partition_and_misaligned_base() -> None:
    slopes, biases = _weights()
    audio = np.zeros(HOP_SAMPLES * 2, dtype=np.float32)
    selector = DfnErfFrameSelector(
        slopes,
        biases,
        partition=lambda values: np.zeros(values.size - 1, dtype=np.uint8),
    )

    with pytest.raises(HybridError) as partition:
        selector.process_hop(audio[:HOP_SAMPLES], audio[:HOP_SAMPLES])
    assert partition.value.code is ErrorCode.NPU_OUTPUT_QUANTIZATION_MISMATCH

    valid = DfnErfFrameSelector(slopes, biases)
    with pytest.raises(HybridError) as mismatch:
        valid.process_audio(
            audio,
            audio[:-HOP_SAMPLES],
            compensate_delay=True,
        )
    assert mismatch.value.code is ErrorCode.ONNX_INPUT_INVALID

    with pytest.raises(HybridError) as not_callable:
        DfnErfFrameSelector(
            slopes,
            biases,
            feature_partition=object(),  # type: ignore[arg-type]
        )
    assert not_callable.value.code is ErrorCode.MODEL_PACKAGE_INVALID

    bad_feature = DfnErfFrameSelector(
        slopes,
        biases,
        feature_partition=lambda values: np.zeros(
            values.size,
            dtype=np.float32,
        ),
    )
    with pytest.raises(HybridError) as feature_output:
        bad_feature.process_hop(audio[:HOP_SAMPLES], audio[:HOP_SAMPLES])
    assert feature_output.value.code is ErrorCode.NPU_OUTPUT_QUANTIZATION_MISMATCH
