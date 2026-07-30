"""Objective audio metrics and the frozen 12-second device fixture."""

from __future__ import annotations

import hashlib
import math
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

import numpy as np
from pystoi import stoi

from .audit import SHA256_RE, canonical_json
from .erf_mask_corpus import (
    generate_noise_source,
    mix_at_snr,
)
from .erf_mask_signal import SAMPLE_RATE
from .errors import ErrorCode, HybridError

METRIC_CONTRACT_VERSION = 1
FIXTURE_SCHEMA_VERSION = 1
FIXTURE_SEGMENT_SAMPLES = SAMPLE_RATE * 2
FIXTURE_TOTAL_SAMPLES = FIXTURE_SEGMENT_SAMPLES * 6
SI_SDR_CAP_DB = 120.0
RMS_FLOOR_DBFS = -120.0

EVALUATION_CATEGORIES = frozenset(
    {
        "synthetic-mixture",
        "natural-mixture",
        "noise-only",
        "speech-only",
        "silence",
    }
)
QUALITY_THRESHOLDS = {
    "synthetic_median_si_sdr_improvement_db_min": 3.0,
    "natural_median_si_sdr_improvement_db_min": 1.0,
    "natural_median_stoi_delta_min": -0.02,
    "noise_median_attenuation_db_min": 6.0,
    "speech_median_abs_rms_delta_db_max": 1.0,
}


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _audio_vector(
    samples: np.ndarray,
    *,
    name: str,
    finite: bool = True,
) -> np.ndarray:
    values = np.asarray(samples)
    if (
        values.ndim != 1
        or values.size == 0
        or not np.issubdtype(values.dtype, np.floating)
    ):
        raise _invalid(f"{name} must be a nonempty floating-point vector")
    if finite and not np.isfinite(values).all():
        raise _invalid(f"{name} contains non-finite samples")
    return np.asarray(values, dtype=np.float64)


def _audio_pair(
    reference: np.ndarray,
    estimate: np.ndarray,
) -> tuple[np.ndarray, np.ndarray]:
    reference_values = _audio_vector(reference, name="reference")
    estimate_values = _audio_vector(estimate, name="estimate")
    if reference_values.shape != estimate_values.shape:
        raise _invalid("reference and estimate lengths differ")
    return reference_values, estimate_values


def _f32_sha256(samples: np.ndarray) -> str:
    values = np.ascontiguousarray(samples, dtype="<f4")
    return hashlib.sha256(values.tobytes()).hexdigest()


def si_sdr(
    reference: np.ndarray,
    estimate: np.ndarray,
) -> float | None:
    """Return capped finite SI-SDR, or ``None`` for a silent reference."""

    reference_values, estimate_values = _audio_pair(reference, estimate)
    reference_energy = float(np.dot(reference_values, reference_values))
    silence_energy = reference_values.size * 1e-20
    if reference_energy <= silence_energy:
        return None
    scale = float(np.dot(estimate_values, reference_values) / reference_energy)
    target = scale * reference_values
    residual = estimate_values - target
    target_energy = float(np.dot(target, target))
    residual_energy = float(np.dot(residual, residual))
    if target_energy <= silence_energy:
        return -SI_SDR_CAP_DB
    if residual_energy <= target_energy * 1e-12:
        return SI_SDR_CAP_DB
    value = 10.0 * math.log10(target_energy / residual_energy)
    return float(np.clip(value, -SI_SDR_CAP_DB, SI_SDR_CAP_DB))


def rms_db(samples: np.ndarray) -> float:
    """Return finite dBFS RMS with a documented -120 dBFS silence floor."""

    values = _audio_vector(samples, name="RMS input")
    rms = float(np.sqrt(np.mean(np.square(values))))
    if rms <= 10.0 ** (RMS_FLOOR_DBFS / 20.0):
        return RMS_FLOOR_DBFS
    return 20.0 * math.log10(rms)


def noise_attenuation_db(
    input_noise: np.ndarray,
    output_noise: np.ndarray,
) -> float:
    input_values, output_values = _audio_pair(input_noise, output_noise)
    return rms_db(input_values) - rms_db(output_values)


def stoi_score(
    reference: np.ndarray,
    estimate: np.ndarray,
    *,
    sample_rate: int = SAMPLE_RATE,
) -> float:
    """Compute STOI only for the fixed public 48 kHz audio contract."""

    if type(sample_rate) is not int or sample_rate != SAMPLE_RATE:
        raise _invalid(f"STOI sample_rate must be {SAMPLE_RATE}")
    reference_values, estimate_values = _audio_pair(reference, estimate)
    if reference_values.size < SAMPLE_RATE:
        raise _invalid("STOI inputs must contain at least one second")
    reference_energy = float(np.dot(reference_values, reference_values))
    if reference_energy <= reference_values.size * 1e-20:
        raise _invalid("STOI reference must contain speech energy")
    try:
        value = float(
            stoi(
                reference_values,
                estimate_values,
                sample_rate,
                extended=False,
            )
        )
    except (ValueError, ZeroDivisionError, FloatingPointError) as exc:
        raise _invalid(f"STOI evaluation failed: {exc}") from exc
    if not math.isfinite(value):
        raise _invalid("STOI produced a non-finite score")
    return float(np.clip(value, 0.0, 1.0))


def _diagnostics(samples: np.ndarray, *, prefix: str) -> dict[str, int]:
    values = np.asarray(samples)
    finite = np.isfinite(values)
    return {
        f"{prefix}_nonfinite_samples": int(np.count_nonzero(~finite)),
        f"{prefix}_clipped_samples": int(
            np.count_nonzero(finite & (np.abs(values) > 1.0))
        ),
    }


def evaluate_audio_item(
    *,
    identity: str,
    category: str,
    clean: np.ndarray,
    noise: np.ndarray,
    mixture: np.ndarray,
    enhanced: np.ndarray,
) -> dict[str, Any]:
    """Evaluate one item without concealing non-finite enhanced samples."""

    if not identity or category not in EVALUATION_CATEGORIES:
        raise _invalid("invalid audio evaluation identity or category")
    clean_values = _audio_vector(clean, name="clean")
    noise_values = _audio_vector(noise, name="noise")
    mixture_values = _audio_vector(mixture, name="mixture")
    enhanced_values = _audio_vector(
        enhanced,
        name="enhanced",
        finite=False,
    )
    shape = clean_values.shape
    if (
        noise_values.shape != shape
        or mixture_values.shape != shape
        or enhanced_values.shape != shape
    ):
        raise _invalid("audio evaluation tensor lengths differ")
    reconstruction_error = np.max(
        np.abs((clean_values + noise_values) - mixture_values)
    )
    if reconstruction_error > 2e-6:
        raise _invalid("mixture does not equal clean plus noise")

    diagnostics = {
        **_diagnostics(mixture_values, prefix="mixture"),
        **_diagnostics(enhanced_values, prefix="enhanced"),
    }
    hashes = {
        "clean": _f32_sha256(clean_values),
        "noise": _f32_sha256(noise_values),
        "mixture": _f32_sha256(mixture_values),
        "enhanced": _f32_sha256(enhanced_values),
    }
    valid = (
        diagnostics["enhanced_nonfinite_samples"] == 0
        and diagnostics["enhanced_clipped_samples"] == 0
    )
    result: dict[str, Any] = {
        "schema_version": 1,
        "identity": identity,
        "category": category,
        "samples": clean_values.size,
        "valid": valid,
        "diagnostics": diagnostics,
        "sha256": hashes,
        "metrics": None,
    }
    if not valid:
        return result

    common: dict[str, float | None] = {
        "input_rms_dbfs": rms_db(mixture_values),
        "enhanced_rms_dbfs": rms_db(enhanced_values),
    }
    if category in {"synthetic-mixture", "natural-mixture"}:
        input_si_sdr = si_sdr(clean_values, mixture_values)
        enhanced_si_sdr = si_sdr(clean_values, enhanced_values)
        if input_si_sdr is None or enhanced_si_sdr is None:
            raise _invalid("speech mixture has a silent clean reference")
        input_stoi = stoi_score(clean_values, mixture_values)
        enhanced_stoi = stoi_score(clean_values, enhanced_values)
        common.update(
            {
                "input_si_sdr_db": input_si_sdr,
                "enhanced_si_sdr_db": enhanced_si_sdr,
                "si_sdr_improvement_db": enhanced_si_sdr - input_si_sdr,
                "input_stoi": input_stoi,
                "enhanced_stoi": enhanced_stoi,
                "stoi_delta": enhanced_stoi - input_stoi,
            }
        )
    elif category == "noise-only":
        common["noise_attenuation_db"] = noise_attenuation_db(
            mixture_values,
            enhanced_values,
        )
    elif category == "speech-only":
        delta = rms_db(enhanced_values) - rms_db(clean_values)
        common["speech_rms_delta_db"] = delta
        common["speech_abs_rms_delta_db"] = abs(delta)
    else:
        common["silence_output_rms_dbfs"] = rms_db(enhanced_values)
    result["metrics"] = common
    return result


def _median(
    items: Sequence[dict[str, Any]],
    *,
    category: str,
    metric: str,
) -> float | None:
    values = [
        item["metrics"][metric]
        for item in items
        if item.get("valid")
        and item.get("category") == category
        and isinstance(item.get("metrics"), dict)
        and metric in item["metrics"]
    ]
    if not values:
        return None
    return float(np.median(np.asarray(values, dtype=np.float64)))


def _speech_gate_population(
    items: Sequence[dict[str, Any]],
) -> tuple[list[dict[str, Any]], str]:
    speech = [
        item
        for item in items
        if item.get("valid")
        and item.get("category") == "speech-only"
        and isinstance(item.get("metrics"), dict)
        and "speech_abs_rms_delta_db" in item["metrics"]
    ]
    natural = [
        item
        for item in speech
        if isinstance(item.get("metadata"), dict)
        and item["metadata"].get("tier") == "slr31-test"
    ]
    if natural:
        return natural, "slr31-test-clean"
    return speech, "all-speech-only-compatibility"


def aggregate_evaluation(
    evaluations: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    """Aggregate fixed quality gates without dropping invalid items."""

    if not evaluations or not all(isinstance(item, dict) for item in evaluations):
        raise _invalid("evaluation set must be a nonempty sequence of objects")
    identities = [item.get("identity") for item in evaluations]
    if any(type(identity) is not str or not identity for identity in identities) or len(
        set(identities)
    ) != len(identities):
        raise _invalid("evaluation identities must be unique strings")
    ordered = sorted(evaluations, key=lambda item: item["identity"])
    speech_gate_items, speech_gate_population = _speech_gate_population(ordered)
    speech_gate_value = (
        float(
            np.median(
                np.asarray(
                    [
                        item["metrics"]["speech_abs_rms_delta_db"]
                        for item in speech_gate_items
                    ],
                    dtype=np.float64,
                )
            )
        )
        if speech_gate_items
        else None
    )
    summary = {
        "synthetic_median_si_sdr_improvement_db": _median(
            ordered,
            category="synthetic-mixture",
            metric="si_sdr_improvement_db",
        ),
        "natural_median_si_sdr_improvement_db": _median(
            ordered,
            category="natural-mixture",
            metric="si_sdr_improvement_db",
        ),
        "natural_median_stoi_delta": _median(
            ordered,
            category="natural-mixture",
            metric="stoi_delta",
        ),
        "noise_median_attenuation_db": _median(
            ordered,
            category="noise-only",
            metric="noise_attenuation_db",
        ),
        "speech_median_abs_rms_delta_db": speech_gate_value,
    }
    values_present = all(value is not None for value in summary.values())
    passed = (
        values_present
        and all(bool(item.get("valid")) for item in ordered)
        and summary["synthetic_median_si_sdr_improvement_db"]
        >= QUALITY_THRESHOLDS["synthetic_median_si_sdr_improvement_db_min"]
        and summary["natural_median_si_sdr_improvement_db"]
        >= QUALITY_THRESHOLDS["natural_median_si_sdr_improvement_db_min"]
        and summary["natural_median_stoi_delta"]
        >= QUALITY_THRESHOLDS["natural_median_stoi_delta_min"]
        and summary["noise_median_attenuation_db"]
        >= QUALITY_THRESHOLDS["noise_median_attenuation_db_min"]
        and summary["speech_median_abs_rms_delta_db"]
        <= QUALITY_THRESHOLDS["speech_median_abs_rms_delta_db_max"]
    )
    item_documents = list(ordered)
    return {
        "schema_version": 1,
        "metric_contract": {
            "version": METRIC_CONTRACT_VERSION,
            "sample_rate": SAMPLE_RATE,
            "si_sdr_cap_db": SI_SDR_CAP_DB,
            "si_sdr_silence": None,
            "rms_floor_dbfs": RMS_FLOOR_DBFS,
            "stoi": {
                "implementation": "pystoi",
                "extended": False,
                "input_sample_rate": SAMPLE_RATE,
            },
        },
        "thresholds": dict(QUALITY_THRESHOLDS),
        "speech_gate": {
            "population": speech_gate_population,
            "item_count": len(speech_gate_items),
            "selection": (
                "valid speech-only items with metadata.tier == slr31-test; "
                "all speech-only items only when no tier metadata exists"
            ),
        },
        "item_count": len(item_documents),
        "invalid_item_count": sum(
            not bool(item.get("valid")) for item in item_documents
        ),
        "items_sha256": hashlib.sha256(canonical_json(item_documents)).hexdigest(),
        "summary": summary,
        "passed": bool(passed),
        "items": item_documents,
    }


def _readonly_f32(samples: np.ndarray) -> np.ndarray:
    values = np.ascontiguousarray(samples, dtype=np.float32)
    values.flags.writeable = False
    return values


@dataclass(frozen=True, slots=True)
class FixtureSpeechItem:
    identity: str
    source_sha256: str
    samples: np.ndarray = field(repr=False)

    def __post_init__(self) -> None:
        values = _audio_vector(self.samples, name="fixture speech")
        if not self.identity or SHA256_RE.fullmatch(self.source_sha256) is None:
            raise _invalid("invalid fixture speech identity or source hash")
        object.__setattr__(self, "samples", _readonly_f32(values))


@dataclass(frozen=True, slots=True)
class DeviceFixture:
    mixture: np.ndarray = field(repr=False)
    clean: np.ndarray = field(repr=False)
    noise: np.ndarray = field(repr=False)
    manifest: dict[str, Any]

    def __post_init__(self) -> None:
        for name in ("mixture", "clean", "noise"):
            values = _readonly_f32(getattr(self, name))
            if values.shape != (FIXTURE_TOTAL_SAMPLES,):
                raise _invalid(f"device fixture {name} length differs")
            object.__setattr__(self, name, values)


def _fixture_speech(
    speech_items: Sequence[FixtureSpeechItem],
) -> tuple[np.ndarray, list[dict[str, Any]], list[str]]:
    if not speech_items or not all(
        isinstance(item, FixtureSpeechItem) for item in speech_items
    ):
        raise _invalid("fixture requires speech items")
    ordered = sorted(speech_items, key=lambda item: item.identity)
    identities = [item.identity for item in ordered]
    if len(set(identities)) != len(identities):
        raise _invalid("fixture speech identities must be unique")
    required = FIXTURE_SEGMENT_SAMPLES * 4
    chunks: list[np.ndarray] = []
    consumed: list[dict[str, Any]] = []
    used_order: list[str] = []
    remaining = required
    for item in ordered:
        if remaining == 0:
            break
        count = min(remaining, item.samples.size)
        if count == 0:
            continue
        chunk = item.samples[:count]
        chunks.append(chunk)
        consumed.append(
            {
                "identity": item.identity,
                "source_sha256": item.source_sha256,
                "source_samples": item.samples.size,
                "consumed_offset": 0,
                "consumed_samples": count,
                "consumed_f32_sha256": _f32_sha256(chunk),
            }
        )
        used_order.append(item.identity)
        remaining -= count
    if remaining:
        raise _invalid(
            "held-out fixture split cannot supply eight seconds without looping"
        )
    return (
        np.ascontiguousarray(np.concatenate(chunks), dtype=np.float32),
        consumed,
        used_order,
    )


def _segment_document(
    *,
    index: int,
    kind: str,
    clean: np.ndarray,
    noise: np.ndarray,
    mixture: np.ndarray,
    snr_db: float | None,
    seeds: list[int],
) -> dict[str, Any]:
    return {
        "index": index,
        "kind": kind,
        "offset_samples": index * FIXTURE_SEGMENT_SAMPLES,
        "samples": FIXTURE_SEGMENT_SAMPLES,
        "snr_db": snr_db,
        "seeds": seeds,
        "sha256": {
            "clean": _f32_sha256(clean),
            "noise": _f32_sha256(noise),
            "mixture": _f32_sha256(mixture),
        },
    }


def build_device_fixture(
    speech_items: Sequence[FixtureSpeechItem],
) -> DeviceFixture:
    """Build the exact six-segment, 12-second qualification fixture."""

    speech, consumed, source_order = _fixture_speech(speech_items)
    size = FIXTURE_SEGMENT_SAMPLES
    zero = np.zeros(size, dtype=np.float32)
    speech_segments = [speech[index * size : (index + 1) * size] for index in range(4)]

    pink_only = generate_noise_source("pink", size, seed=1_301_001)
    stationary = generate_noise_source("pink", size, seed=1_301_002).astype(
        np.float64
    ) + generate_noise_source("hum", size, seed=1_301_003).astype(np.float64)
    stationary_mix = mix_at_snr(speech_segments[1], stationary, 0.0)
    modulated_mix = mix_at_snr(
        speech_segments[2],
        generate_noise_source(
            "amplitude-modulated",
            size,
            seed=1_301_004,
        ),
        -5.0,
    )
    impulsive_mix = mix_at_snr(
        speech_segments[3],
        generate_noise_source("impulsive", size, seed=1_301_005),
        5.0,
    )

    components = [
        ("pink-noise-only", zero, pink_only, pink_only, None, [1_301_001]),
        (
            "clean-speech",
            speech_segments[0],
            zero,
            speech_segments[0],
            None,
            [],
        ),
        (
            "stationary-pink-hum-0db",
            stationary_mix.clean,
            stationary_mix.noise,
            stationary_mix.mixture,
            0.0,
            [1_301_002, 1_301_003],
        ),
        (
            "amplitude-modulated-noise--5db",
            modulated_mix.clean,
            modulated_mix.noise,
            modulated_mix.mixture,
            -5.0,
            [1_301_004],
        ),
        (
            "impulsive-noise-5db",
            impulsive_mix.clean,
            impulsive_mix.noise,
            impulsive_mix.mixture,
            5.0,
            [1_301_005],
        ),
        ("silence", zero, zero, zero, None, []),
    ]
    clean = np.ascontiguousarray(
        np.concatenate([item[1] for item in components]),
        dtype=np.float32,
    )
    noise = np.ascontiguousarray(
        np.concatenate([item[2] for item in components]),
        dtype=np.float32,
    )
    mixture = np.ascontiguousarray(
        np.concatenate([item[3] for item in components]),
        dtype=np.float32,
    )
    segments = [
        _segment_document(
            index=index,
            kind=item[0],
            clean=item[1],
            noise=item[2],
            mixture=item[3],
            snr_db=item[4],
            seeds=item[5],
        )
        for index, item in enumerate(components)
    ]
    manifest = {
        "schema_version": FIXTURE_SCHEMA_VERSION,
        "sample_rate": SAMPLE_RATE,
        "channels": 1,
        "sample_dtype": "FLOAT32",
        "segment_samples": FIXTURE_SEGMENT_SAMPLES,
        "total_samples": FIXTURE_TOTAL_SAMPLES,
        "speech_required_samples": FIXTURE_SEGMENT_SAMPLES * 4,
        "speech_source_order": source_order,
        "speech_sources": consumed,
        "segments": segments,
        "sha256": {
            "clean": _f32_sha256(clean),
            "noise": _f32_sha256(noise),
            "mixture": _f32_sha256(mixture),
        },
    }
    return DeviceFixture(
        mixture=mixture,
        clean=clean,
        noise=noise,
        manifest=manifest,
    )
