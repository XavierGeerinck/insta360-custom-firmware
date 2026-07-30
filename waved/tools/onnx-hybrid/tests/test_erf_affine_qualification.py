from __future__ import annotations

import hashlib

import numpy as np
import pytest

from wave_onnx_hybrid.erf_affine_model import (
    affine_erf_output_u8,
    quantization_candidates,
    quantize_affine_model,
)
from wave_onnx_hybrid.erf_affine_qualification import (
    AFFINE_VECTOR_HEADER,
    AFFINE_VECTOR_IDS,
    build_affine_qualification_vectors,
    parse_affine_qualification_vectors,
    qualification_vector_manifest,
    serialize_affine_qualification_vectors,
    verify_qualification_vector_manifest,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _collection():
    slopes = np.linspace(0.01, 1.8, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.9, 0.7, ERF_BINS, dtype=np.float32)
    model = quantize_affine_model(
        slopes,
        biases,
        quantization_candidates()[1],
    )
    rng = np.random.Generator(np.random.PCG64(0x14A4))
    features = rng.uniform(-8.0, 8.0, size=(64, ERF_BINS)).astype(np.float32)
    return model, build_affine_qualification_vectors(model, features)


def test_affine_vectors_are_deterministic_non_degenerate_and_immutable() -> None:
    model, left = _collection()
    _, right = _collection()

    assert len(left.inputs) == 6
    assert len(left.outputs) == 6
    assert AFFINE_VECTOR_IDS[-1] == "learned-validation"
    assert left.learned_frame_index == right.learned_frame_index
    for first, second, output in zip(
        left.inputs,
        right.inputs,
        left.outputs,
        strict=True,
    ):
        np.testing.assert_array_equal(first, second)
        np.testing.assert_array_equal(
            output,
            affine_erf_output_u8(first[np.newaxis, :], model)[0],
        )
        assert first.shape == (ERF_BINS,)
        assert first.dtype == np.uint8
        assert first.flags.writeable is False
        assert output.shape == (ERF_BINS,)
        assert output.dtype == np.uint8
        assert output.flags.writeable is False
    assert np.all(left.inputs[0] == model.candidate.feature_zero_point)
    assert np.all(left.inputs[1] == model.candidate.qmin)
    assert np.all(left.inputs[2] == model.candidate.qmax)
    assert np.unique(left.inputs[4]).size > 100
    assert np.unique(left.inputs[5]).size > 1
    assert np.unique(left.outputs[5]).size > 1


def test_affine_vector_pack_round_trip_and_manifest_are_exact() -> None:
    model, collection = _collection()
    payload = serialize_affine_qualification_vectors(collection, model)
    inputs, outputs = parse_affine_qualification_vectors(payload, model)
    manifest = qualification_vector_manifest(collection, model)

    assert len(payload) == AFFINE_VECTOR_HEADER.size + 12 * ERF_BINS
    for expected, observed in zip(collection.inputs, inputs, strict=True):
        np.testing.assert_array_equal(observed, expected)
    for expected, observed in zip(collection.outputs, outputs, strict=True):
        np.testing.assert_array_equal(observed, expected)
    assert manifest["vector_count"] == 6
    assert manifest["width"] == ERF_BINS
    assert manifest["model_contract_sha256"] == model.contract_sha256
    for record, inputs_value, output in zip(
        manifest["vectors"],
        collection.inputs,
        collection.outputs,
        strict=True,
    ):
        assert record["input_sha256"] == hashlib.sha256(inputs_value).hexdigest()
        assert record["output_sha256"] == hashlib.sha256(output).hexdigest()
    assert (
        verify_qualification_vector_manifest(
            manifest,
            inputs,
            outputs,
            model,
        )
        == manifest
    )


def test_affine_vector_pack_rejects_corruption_and_wrong_model() -> None:
    model, collection = _collection()
    payload = serialize_affine_qualification_vectors(collection, model)
    slopes = np.ones(ERF_BINS, dtype=np.float32)
    biases = np.zeros(ERF_BINS, dtype=np.float32)
    wrong_model = quantize_affine_model(
        slopes,
        biases,
        quantization_candidates()[1],
    )

    for changed in (
        payload[:-1],
        bytes([payload[0] ^ 1]) + payload[1:],
        payload + b"x",
    ):
        with pytest.raises(HybridError) as error:
            parse_affine_qualification_vectors(changed, model)
        assert error.value.code is ErrorCode.EVIDENCE_INVALID
    with pytest.raises(HybridError) as contract:
        parse_affine_qualification_vectors(payload, wrong_model)
    assert contract.value.code is ErrorCode.EVIDENCE_INVALID
