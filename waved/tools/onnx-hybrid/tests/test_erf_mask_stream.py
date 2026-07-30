from __future__ import annotations

import hashlib
from pathlib import Path

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    SAMPLE_RATE,
)
from wave_onnx_hybrid.erf_mask_stream import (
    DFN_ATTENUATION_LIMIT_DB,
    DFN_OUTPUT_DELAY_SAMPLES,
    STREAM_GUARD_SAMPLES,
    EvaluationAudioItem,
    evaluate_dfn_erf_stream,
    verify_evaluation_stream,
    write_evaluation_stream,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _item(identity: str, category: str, value: float) -> EvaluationAudioItem:
    clean = np.full(HOP_SAMPLES * 4, value, dtype=np.float32)
    noise = np.zeros_like(clean)
    if category == "noise-only":
        noise[:] = value
        clean[:] = 0.0
    return EvaluationAudioItem(
        identity=identity,
        category=category,
        clean=clean,
        noise=noise,
        mixture=clean + noise,
        metadata={"source": "test"},
    )


def test_stream_contract_constants_are_frozen() -> None:
    assert DFN_ATTENUATION_LIMIT_DB == 8.0
    assert DFN_OUTPUT_DELAY_SAMPLES == HOP_SAMPLES
    assert STREAM_GUARD_SAMPLES == SAMPLE_RATE * 2


def test_stream_writer_has_exact_guard_offsets_hashes_and_bytes(
    tmp_path: Path,
) -> None:
    items = [
        _item("a", "speech-only", 0.1),
        _item("b", "noise-only", 0.2),
    ]
    raw = tmp_path / "input.f32"

    manifest = write_evaluation_stream(
        items,
        raw,
        guard_samples=HOP_SAMPLES,
    )

    observed = np.fromfile(raw, dtype="<f4")
    expected = np.concatenate(
        [
            np.zeros(HOP_SAMPLES, dtype=np.float32),
            items[0].mixture,
            np.zeros(HOP_SAMPLES, dtype=np.float32),
            items[1].mixture,
            np.zeros(HOP_SAMPLES, dtype=np.float32),
        ]
    )
    np.testing.assert_array_equal(observed, expected)
    assert manifest["items"][0]["stream_offset_samples"] == HOP_SAMPLES
    assert manifest["items"][1]["stream_offset_samples"] == HOP_SAMPLES * 6
    assert manifest["total_samples"] == expected.size
    assert (
        manifest["raw_sha256"]
        == hashlib.sha256(expected.astype("<f4").tobytes()).hexdigest()
    )
    assert verify_evaluation_stream(manifest, raw) == manifest


def test_stream_writer_is_exclusive_and_cleans_partial_failure(
    tmp_path: Path,
) -> None:
    raw = tmp_path / "input.f32"
    write_evaluation_stream([_item("a", "speech-only", 0.1)], raw)
    with pytest.raises(HybridError) as exists:
        write_evaluation_stream([_item("a", "speech-only", 0.1)], raw)
    assert exists.value.code is ErrorCode.OUTPUT_EXISTS

    invalid = _item("b", "speech-only", 0.2)
    invalid.mixture.flags.writeable = True
    invalid.mixture[0] = np.nan
    failed = tmp_path / "failed.f32"
    with pytest.raises(HybridError) as nonfinite:
        write_evaluation_stream([invalid], failed)
    assert nonfinite.value.code is ErrorCode.EVIDENCE_INVALID
    assert not failed.exists()


def test_stream_rejects_duplicate_identity_and_partial_hop(
    tmp_path: Path,
) -> None:
    item = _item("same", "speech-only", 0.1)
    with pytest.raises(HybridError) as duplicate:
        write_evaluation_stream([item, item], tmp_path / "duplicate.f32")
    assert duplicate.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as partial:
        EvaluationAudioItem(
            identity="partial",
            category="speech-only",
            clean=np.zeros(HOP_SAMPLES + 1, dtype=np.float32),
            noise=np.zeros(HOP_SAMPLES + 1, dtype=np.float32),
            mixture=np.zeros(HOP_SAMPLES + 1, dtype=np.float32),
        )
    assert partial.value.code is ErrorCode.EVIDENCE_INVALID


def test_shifted_base_stream_is_aligned_and_selector_chooses_identity(
    tmp_path: Path,
) -> None:
    items = [
        _item("a", "speech-only", 0.1),
        _item("b", "noise-only", 0.2),
    ]
    raw = tmp_path / "input.f32"
    manifest = write_evaluation_stream(
        items,
        raw,
        guard_samples=HOP_SAMPLES * 2,
    )
    base = np.zeros(manifest["total_samples"], dtype=np.float32)
    for item, document in zip(items, manifest["items"], strict=True):
        start = document["stream_offset_samples"] + DFN_OUTPUT_DELAY_SAMPLES
        base[start : start + item.mixture.size] = item.mixture
    output = tmp_path / "dfn.f32"
    base.astype("<f4").tofile(output)

    result = evaluate_dfn_erf_stream(
        items,
        manifest,
        output,
        np.ones(ERF_BINS, dtype=np.float32),
        np.zeros(ERF_BINS, dtype=np.float32),
    )

    assert result["item_count"] == 2
    assert result["architecture"] == "dfn3-cpu-or-learned-erf-causal-frame-selector"
    assert result["selector"]["erf_selected_frames"] == 0
    assert (
        result["selector"]["dfn_selected_frames"]
        == result["selector"]["partition_calls"]
    )
    assert result["dfn"]["output_delay_samples"] == HOP_SAMPLES
    assert (
        result["dfn"]["output_sha256"]
        == hashlib.sha256(output.read_bytes()).hexdigest()
    )
    for item in result["items"]:
        assert item["valid"] is True
        assert item["sha256"]["mixture"] == item["sha256"]["enhanced"]


def test_stream_evaluation_forwards_complete_feature_partition(
    tmp_path: Path,
) -> None:
    item = _item("feature-partition", "speech-only", 0.1)
    raw = tmp_path / "input.f32"
    manifest = write_evaluation_stream(
        [item],
        raw,
        guard_samples=HOP_SAMPLES,
    )
    base = np.zeros(manifest["total_samples"], dtype=np.float32)
    start = manifest["items"][0]["stream_offset_samples"]
    start += DFN_OUTPUT_DELAY_SAMPLES
    base[start : start + item.mixture.size] = item.mixture
    output = tmp_path / "dfn.f32"
    base.astype("<f4").tofile(output)
    calls = 0

    def feature_partition(values: np.ndarray) -> np.ndarray:
        nonlocal calls
        calls += 1
        assert values.shape == (ERF_BINS,)
        return np.full(ERF_BINS, 128, dtype=np.uint8)

    result = evaluate_dfn_erf_stream(
        [item],
        manifest,
        output,
        np.ones(ERF_BINS, dtype=np.float32),
        np.zeros(ERF_BINS, dtype=np.float32),
        feature_partition=feature_partition,
    )

    assert calls == result["selector"]["partition_calls"]
