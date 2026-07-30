from __future__ import annotations

import copy

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_corpus import (
    generate_clean_source,
    generate_noise_source,
    mix_at_snr,
)
from wave_onnx_hybrid.erf_mask_metrics import (
    FIXTURE_SEGMENT_SAMPLES,
    FIXTURE_TOTAL_SAMPLES,
    FixtureSpeechItem,
    aggregate_evaluation,
    build_device_fixture,
    evaluate_audio_item,
    noise_attenuation_db,
    rms_db,
    si_sdr,
    stoi_score,
)
from wave_onnx_hybrid.erf_mask_signal import SAMPLE_RATE
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _speech(seed: int, seconds: int = 2) -> np.ndarray:
    return generate_clean_source(
        "voiced",
        SAMPLE_RATE * seconds,
        seed=seed,
    )


def _noise(seed: int, seconds: int = 2) -> np.ndarray:
    return generate_noise_source(
        "pink",
        SAMPLE_RATE * seconds,
        seed=seed,
    )


def test_si_sdr_identity_scale_invariance_and_known_improvement() -> None:
    clean = _speech(1)
    noise = _noise(2)
    noisy = mix_at_snr(clean, noise, 0.0).mixture
    improved = clean + (noisy - clean) * np.float32(0.25)

    assert si_sdr(clean, clean) == 120.0
    assert si_sdr(clean, clean * np.float32(0.37)) == 120.0
    assert si_sdr(clean, improved) > si_sdr(clean, noisy) + 10.0


def test_si_sdr_has_explicit_silence_and_input_policy() -> None:
    zeros = np.zeros(SAMPLE_RATE, dtype=np.float32)
    assert si_sdr(zeros, zeros) is None

    with pytest.raises(HybridError) as mismatch:
        si_sdr(zeros, zeros[:-1])
    assert mismatch.value.code is ErrorCode.EVIDENCE_INVALID


def test_rms_and_noise_attenuation_are_dbfs_and_finite_for_silence() -> None:
    unity = np.ones(4_800, dtype=np.float32)
    silence = np.zeros_like(unity)

    assert rms_db(unity) == pytest.approx(0.0)
    assert rms_db(unity * 0.5) == pytest.approx(-6.0205999)
    assert rms_db(silence) == -120.0
    assert noise_attenuation_db(unity, unity * 0.5) == pytest.approx(6.0205999)


def test_stoi_validates_contract_and_handles_48khz_input() -> None:
    clean = _speech(3)
    noisy = mix_at_snr(clean, _noise(4), 5.0).mixture

    assert stoi_score(clean, clean) == pytest.approx(1.0, abs=1e-6)
    assert 0.0 <= stoi_score(clean, noisy) <= 1.0
    with pytest.raises(HybridError) as sample_rate:
        stoi_score(clean, noisy, sample_rate=16_000)
    assert sample_rate.value.code is ErrorCode.EVIDENCE_INVALID


def test_item_evaluation_records_metrics_hashes_and_diagnostics() -> None:
    clean = _speech(5)
    components = mix_at_snr(clean, _noise(6), 0.0)
    result = evaluate_audio_item(
        identity="synthetic/a",
        category="synthetic-mixture",
        clean=components.clean,
        noise=components.noise,
        mixture=components.mixture,
        enhanced=clean,
    )

    assert result["valid"] is True
    assert result["metrics"]["si_sdr_improvement_db"] > 3.0
    assert result["diagnostics"]["enhanced_nonfinite_samples"] == 0
    assert result["diagnostics"]["enhanced_clipped_samples"] == 0
    assert set(result["sha256"]) == {"clean", "noise", "mixture", "enhanced"}
    assert all(len(value) == 64 for value in result["sha256"].values())

    invalid = components.mixture.copy()
    invalid[10] = np.nan
    observed = evaluate_audio_item(
        identity="synthetic/invalid",
        category="synthetic-mixture",
        clean=components.clean,
        noise=components.noise,
        mixture=components.mixture,
        enhanced=invalid,
    )
    assert observed["valid"] is False
    assert observed["diagnostics"]["enhanced_nonfinite_samples"] == 1
    assert observed["metrics"] is None


def _passing_evaluations() -> list[dict[str, object]]:
    clean = _speech(7)
    components = mix_at_snr(clean, _noise(8), 0.0)
    synthetic = evaluate_audio_item(
        identity="synthetic/pass",
        category="synthetic-mixture",
        clean=components.clean,
        noise=components.noise,
        mixture=components.mixture,
        enhanced=clean,
    )
    natural = evaluate_audio_item(
        identity="natural/pass",
        category="natural-mixture",
        clean=components.clean,
        noise=components.noise,
        mixture=components.mixture,
        enhanced=clean,
    )
    noise_only = evaluate_audio_item(
        identity="noise/pass",
        category="noise-only",
        clean=np.zeros_like(clean),
        noise=components.noise,
        mixture=components.noise,
        enhanced=components.noise * np.float32(0.25),
    )
    speech_only = evaluate_audio_item(
        identity="speech/pass",
        category="speech-only",
        clean=clean,
        noise=np.zeros_like(clean),
        mixture=clean,
        enhanced=clean,
    )
    speech_only["metadata"] = {"tier": "slr31-test"}
    return [synthetic, natural, noise_only, speech_only]


def test_aggregate_evaluation_has_fixed_config_medians_and_gate() -> None:
    result = aggregate_evaluation(_passing_evaluations())

    assert result["schema_version"] == 1
    assert result["metric_contract"]["version"] == 1
    assert result["thresholds"] == {
        "synthetic_median_si_sdr_improvement_db_min": 3.0,
        "natural_median_si_sdr_improvement_db_min": 1.0,
        "natural_median_stoi_delta_min": -0.02,
        "noise_median_attenuation_db_min": 6.0,
        "speech_median_abs_rms_delta_db_max": 1.0,
    }
    assert result["summary"]["synthetic_median_si_sdr_improvement_db"] > 3.0
    assert result["speech_gate"] == {
        "population": "slr31-test-clean",
        "item_count": 1,
        "selection": (
            "valid speech-only items with metadata.tier == slr31-test; "
            "all speech-only items only when no tier metadata exists"
        ),
    }
    assert result["passed"] is True

    failed_items = _passing_evaluations()
    failed_items[0] = copy.deepcopy(failed_items[0])
    failed_items[0]["metrics"]["si_sdr_improvement_db"] = 2.99
    assert aggregate_evaluation(failed_items)["passed"] is False


def test_natural_clean_gate_excludes_procedural_speech_diagnostic() -> None:
    items = _passing_evaluations()
    procedural = copy.deepcopy(items[-1])
    procedural["identity"] = "procedural/speech-only"
    procedural["metadata"] = {"tier": "procedural"}
    procedural["metrics"]["speech_abs_rms_delta_db"] = 12.0

    result = aggregate_evaluation([*items, procedural])

    assert result["passed"] is True
    assert result["speech_gate"]["population"] == "slr31-test-clean"
    assert result["speech_gate"]["item_count"] == 1
    assert result["summary"]["speech_median_abs_rms_delta_db"] == 0.0


def _fixture_sources() -> list[FixtureSpeechItem]:
    return [
        FixtureSpeechItem(
            identity=f"speaker/{identity}",
            source_sha256=f"{identity:064x}",
            samples=_speech(100 + identity),
        )
        for identity in (4, 2, 3, 1)
    ]


def test_device_fixture_is_exact_deterministic_and_order_independent() -> None:
    sources = _fixture_sources()

    left = build_device_fixture(sources)
    right = build_device_fixture(list(reversed(sources)))

    assert left.manifest == right.manifest
    np.testing.assert_array_equal(left.mixture, right.mixture)
    assert left.mixture.shape == (FIXTURE_TOTAL_SAMPLES,)
    assert left.clean.shape == (FIXTURE_TOTAL_SAMPLES,)
    assert left.noise.shape == (FIXTURE_TOTAL_SAMPLES,)
    assert FIXTURE_SEGMENT_SAMPLES == SAMPLE_RATE * 2
    assert [item["kind"] for item in left.manifest["segments"]] == [
        "pink-noise-only",
        "clean-speech",
        "stationary-pink-hum-0db",
        "amplitude-modulated-noise--5db",
        "impulsive-noise-5db",
        "silence",
    ]
    assert left.manifest["speech_source_order"] == [
        "speaker/1",
        "speaker/2",
        "speaker/3",
        "speaker/4",
    ]
    assert set(left.manifest["sha256"]) == {"clean", "noise", "mixture"}
    assert np.max(np.abs(left.mixture)) <= 0.98


def test_device_fixture_never_loops_insufficient_speech() -> None:
    short = [
        FixtureSpeechItem(
            identity="too-short",
            source_sha256="a" * 64,
            samples=_speech(999, seconds=2)[:-1],
        )
    ]
    with pytest.raises(HybridError) as insufficient:
        build_device_fixture(short)
    assert insufficient.value.code is ErrorCode.EVIDENCE_INVALID
