from __future__ import annotations

import hashlib

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_corpus import generate_clean_source
from wave_onnx_hybrid.erf_mask_qualification import (
    FNV64_OFFSET_BASIS,
    QUALIFICATION_VECTOR_IDS,
    VECTOR_PACK_HEADER,
    build_qualification_vectors,
    fnv1a64,
    parse_qualification_vectors,
    qualification_vector_manifest,
    serialize_qualification_vectors,
)
from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    cpu_erf_u8,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _collection():
    slopes = np.ones(ERF_BINS, dtype=np.float32)
    biases = np.zeros(ERF_BINS, dtype=np.float32)
    audio = generate_clean_source("voiced", HOP_SAMPLES * 20, seed=901)
    return build_qualification_vectors(slopes, biases, audio)


def test_vectors_are_exact_deterministic_non_degenerate_and_immutable() -> None:
    left = _collection()
    right = _collection()

    assert len(left.vectors) == 6
    assert QUALIFICATION_VECTOR_IDS[-1] == "learned-fixture"
    assert left.learned_frame_index == right.learned_frame_index
    for first, second in zip(left.vectors, right.vectors, strict=True):
        np.testing.assert_array_equal(first, second)
        assert first.shape == (ERF_BINS,)
        assert first.dtype == np.uint8
        assert first.flags.writeable is False
    assert np.all(left.vectors[0] == 128)
    assert np.all(left.vectors[1] == 0)
    assert np.all(left.vectors[2] == 255)
    np.testing.assert_array_equal(
        left.vectors[3],
        np.arange(ERF_BINS, dtype=np.uint16).astype(np.uint8),
    )
    assert np.unique(left.vectors[4]).size > 100
    assert np.unique(left.vectors[5]).size > 1
    assert np.unique(cpu_erf_u8(left.vectors[5])).size > 1


def test_vector_pack_round_trip_and_hash_manifest_are_exact() -> None:
    collection = _collection()
    payload = serialize_qualification_vectors(collection)
    observed = parse_qualification_vectors(payload)
    manifest = qualification_vector_manifest(collection)

    assert len(payload) == VECTOR_PACK_HEADER.size + 6 * ERF_BINS
    for expected, actual in zip(collection.vectors, observed, strict=True):
        np.testing.assert_array_equal(actual, expected)
    assert manifest["vector_count"] == 6
    assert manifest["width"] == ERF_BINS
    for record, vector in zip(
        manifest["vectors"],
        collection.vectors,
        strict=True,
    ):
        assert record["input_sha256"] == hashlib.sha256(vector).hexdigest()
        assert record["output_sha256"] == hashlib.sha256(cpu_erf_u8(vector)).hexdigest()


def test_pack_rejects_truncation_header_change_and_trailing_bytes() -> None:
    payload = serialize_qualification_vectors(_collection())
    for changed in (
        payload[:-1],
        bytes([payload[0] ^ 1]) + payload[1:],
        payload + b"x",
    ):
        with pytest.raises(HybridError) as error:
            parse_qualification_vectors(changed)
        assert error.value.code is ErrorCode.EVIDENCE_INVALID


def test_fnv1a64_has_frozen_oracle() -> None:
    assert fnv1a64(b"") == FNV64_OFFSET_BASIS
    assert fnv1a64(b"hello") == 0xA430D84680AABD0B
