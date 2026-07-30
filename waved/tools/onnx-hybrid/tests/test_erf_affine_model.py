from __future__ import annotations

import io
import json

import numpy as np
import onnx
import onnxruntime as ort
import pytest

from wave_onnx_hybrid.erf_affine_model import (
    AFFINE_FEATURE_SCALE,
    NATIVE_AFFINE_HEADER,
    affine_erf_input_u8,
    affine_erf_output_u8,
    build_affine_erf_onnx,
    dequantize_affine,
    deserialize_quantized_affine,
    parse_native_affine_constants,
    quantization_candidates,
    quantize_affine_model,
    quantize_features,
    select_affine_candidate,
    serialize_affine_config,
    serialize_native_affine_constants,
    serialize_quantized_affine,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _weights() -> tuple[np.ndarray, np.ndarray]:
    slopes = np.linspace(0.01, 1.75, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.8, 0.6, ERF_BINS, dtype=np.float32)
    return slopes, biases


def test_candidate_registry_is_finite_and_covers_required_modes() -> None:
    candidates = quantization_candidates()

    assert [candidate.name for candidate in candidates] == [
        "u8-per-tensor",
        "u8-per-channel",
        "i8-per-tensor",
        "i8-per-channel",
    ]
    assert {candidate.feature_dtype for candidate in candidates} == {
        "UINT8",
        "INT8",
    }
    assert {candidate.weight_granularity for candidate in candidates} == {
        "per-tensor",
        "per-channel",
    }
    assert all(
        candidate.feature_scale == AFFINE_FEATURE_SCALE for candidate in candidates
    )


def test_feature_quantization_uses_ties_to_even_and_saturates() -> None:
    u8 = quantization_candidates()[0]
    values = np.array(
        [
            -100.0,
            -8.0,
            -7.96875,
            -7.90625,
            -0.03125,
            0.03125,
            7.90625,
            100.0,
        ],
        dtype=np.float32,
    )

    features = np.zeros((1, ERF_BINS), dtype=np.float32)
    features[0, : values.size] = values
    observed = quantize_features(features, u8)[0, : values.size]

    np.testing.assert_array_equal(
        observed,
        np.array([0, 0, 0, 2, 128, 128, 254, 255], dtype=np.uint8),
    )


@pytest.mark.parametrize("candidate_index", range(4))
def test_integer_affine_matches_explicit_dequantized_equation(
    candidate_index: int,
) -> None:
    slopes, biases = _weights()
    candidate = quantization_candidates()[candidate_index]
    model = quantize_affine_model(slopes, biases, candidate)
    rng = np.random.Generator(np.random.PCG64(0x14A0 + candidate_index))
    features = rng.uniform(-8.5, 8.5, size=(3, ERF_BINS)).astype(np.float32)
    quantized = quantize_features(features, candidate)

    observed = dequantize_affine(quantized, model)
    centered = quantized.astype(np.int64) - candidate.feature_zero_point
    accumulator = centered * model.weights.astype(np.int64)
    accumulator += model.biases.astype(np.int64)
    expected = accumulator.astype(np.float64) * model.accumulator_scales

    np.testing.assert_array_equal(observed, expected)
    assert model.weights.dtype == np.int8
    assert model.biases.dtype == np.int32
    assert model.weights.shape == (ERF_BINS,)
    assert model.biases.shape == (ERF_BINS,)


def test_u8_and_i8_feature_encodings_have_identical_centered_oracle() -> None:
    slopes, biases = _weights()
    u8 = quantize_affine_model(slopes, biases, quantization_candidates()[1])
    i8 = quantize_affine_model(slopes, biases, quantization_candidates()[3])
    grid = np.arange(-128, 128, dtype=np.int16).astype(np.float32)
    features = np.resize(grid * np.float32(AFFINE_FEATURE_SCALE), (2, ERF_BINS))

    u8_output = affine_erf_output_u8(quantize_features(features, u8.candidate), u8)
    i8_output = affine_erf_output_u8(quantize_features(features, i8.candidate), i8)

    np.testing.assert_array_equal(u8_output, i8_output)


def test_affine_requantization_saturates_and_returns_erf_input_u8() -> None:
    slopes = np.full(ERF_BINS, 1000.0, dtype=np.float32)
    biases = np.linspace(-1000.0, 1000.0, ERF_BINS, dtype=np.float32)
    model = quantize_affine_model(slopes, biases, quantization_candidates()[1])
    low = np.zeros((1, ERF_BINS), dtype=np.uint8)
    high = np.full((1, ERF_BINS), 255, dtype=np.uint8)

    low_result = affine_erf_input_u8(low, model)
    high_result = affine_erf_input_u8(high, model)

    assert low_result.dtype == np.uint8
    assert high_result.dtype == np.uint8
    assert int(low_result.min()) == 0
    assert int(high_result.max()) == 255


def test_serialization_and_onnx_are_deterministic_and_exact() -> None:
    slopes, biases = _weights()
    model = quantize_affine_model(slopes, biases, quantization_candidates()[1])

    left_weights = serialize_quantized_affine(model)
    right_weights = serialize_quantized_affine(model)
    left_onnx = build_affine_erf_onnx(model)
    right_onnx = build_affine_erf_onnx(model)

    assert left_weights == right_weights
    assert left_onnx == right_onnx
    with np.load(io.BytesIO(left_weights), allow_pickle=False) as archive:
        assert archive.files == [
            "accumulator_scales",
            "biases",
            "weight_scales",
            "weights",
        ]
    parsed = onnx.load_model_from_string(left_onnx)
    onnx.checker.check_model(parsed, full_check=True)
    assert [node.op_type for node in parsed.graph.node] == [
        "Cast",
        "Sub",
        "Cast",
        "Mul",
        "Add",
        "Cast",
        "Mul",
        "Round",
        "Add",
        "Clip",
        "Cast",
        "DequantizeLinear",
        "Erf",
        "QuantizeLinear",
    ]

    session = ort.InferenceSession(
        left_onnx,
        providers=["CPUExecutionProvider"],
    )
    rng = np.random.Generator(np.random.PCG64(0x14A1))
    features = rng.uniform(-8.0, 8.0, size=(1, ERF_BINS)).astype(np.float32)
    quantized = quantize_features(features, model.candidate)
    observed = session.run(["erf.output.u8"], {"feature.q": quantized})[0]
    expected = affine_erf_output_u8(quantized, model)
    np.testing.assert_array_equal(observed, expected)


def test_quantized_affine_deserialization_is_canonical_and_fail_closed() -> None:
    slopes, biases = _weights()
    model = quantize_affine_model(slopes, biases, quantization_candidates()[1])
    weights = serialize_quantized_affine(model)
    config = serialize_affine_config(model)

    restored = deserialize_quantized_affine(config, weights)

    assert restored.contract_sha256 == model.contract_sha256
    np.testing.assert_array_equal(restored.weights, model.weights)
    assert serialize_quantized_affine(restored) == weights
    changed = json.loads(config)
    changed["candidate"]["feature"]["zero_point"] += 1
    with pytest.raises(HybridError) as contract:
        deserialize_quantized_affine(
            json.dumps(changed, sort_keys=True).encode(),
            weights,
        )
    assert contract.value.code is ErrorCode.MODEL_PACKAGE_INVALID
    with pytest.raises(HybridError) as archive:
        deserialize_quantized_affine(config, weights + b"x")
    assert archive.value.code is ErrorCode.MODEL_PACKAGE_INVALID


def test_native_affine_constants_are_exact_and_contract_bound() -> None:
    slopes, biases = _weights()
    model = quantize_affine_model(slopes, biases, quantization_candidates()[1])

    payload = serialize_native_affine_constants(model)
    restored = parse_native_affine_constants(payload, model)

    assert len(payload) == NATIVE_AFFINE_HEADER.size + ERF_BINS * 13
    assert payload == serialize_native_affine_constants(model)
    np.testing.assert_array_equal(restored.weights, model.weights)
    np.testing.assert_array_equal(restored.biases, model.biases)
    for changed in (
        payload[:-1],
        bytes([payload[0] ^ 1]) + payload[1:],
        payload + b"x",
    ):
        with pytest.raises(HybridError) as invalid:
            parse_native_affine_constants(changed, model)
        assert invalid.value.code is ErrorCode.MODEL_PACKAGE_INVALID


def test_candidate_selection_is_deterministic_and_prefers_u8_on_tie() -> None:
    slopes, biases = _weights()
    rng = np.random.Generator(np.random.PCG64(0x14A2))
    features = rng.uniform(-8.0, 8.0, size=(8, ERF_BINS)).astype(np.float32)

    first, first_report = select_affine_candidate(features, slopes, biases)
    second, second_report = select_affine_candidate(features, slopes, biases)

    assert first.candidate.name == "u8-per-channel"
    assert first_report == second_report
    np.testing.assert_array_equal(first.weights, second.weights)
    assert len(first_report["candidates"]) == 4
    assert first_report["selected"] == "u8-per-channel"


def test_invalid_model_or_quantized_input_is_rejected() -> None:
    slopes, biases = _weights()
    with pytest.raises(HybridError) as invalid:
        quantize_affine_model(slopes[:-1], biases, quantization_candidates()[0])
    assert invalid.value.code is ErrorCode.MODEL_PACKAGE_INVALID

    model = quantize_affine_model(slopes, biases, quantization_candidates()[0])
    with pytest.raises(HybridError) as wrong_dtype:
        affine_erf_output_u8(
            np.zeros((1, ERF_BINS), dtype=np.int8),
            model,
        )
    assert wrong_dtype.value.code is ErrorCode.ONNX_INPUT_INVALID
