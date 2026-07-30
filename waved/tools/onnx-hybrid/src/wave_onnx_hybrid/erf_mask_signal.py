"""Deterministic signal and quantization primitives for the Erf-mask model."""

from __future__ import annotations

import math
from dataclasses import dataclass
from functools import lru_cache

import numpy as np

from .errors import ErrorCode, HybridError

SAMPLE_RATE = 48_000
HOP_SAMPLES = 480
ANALYSIS_SAMPLES = 960
ERF_BINS = ANALYSIS_SAMPLES // 2 + 1

ERF_INPUT_SCALE = np.float32(1.0 / 32.0)
ERF_INPUT_ZERO_POINT = 128
ERF_OUTPUT_SCALE = np.float32(1.0 / 128.0)
ERF_OUTPUT_ZERO_POINT = 128
MASK_FLOOR = np.float32(0.05)
NOISE_EPSILON = 1e-12

_MIN_LOGIT = -4.0
_MAX_LOGIT = 3.96875
_NOISE_MINIMUM_FRAMES = 100


@dataclass(frozen=True, slots=True)
class ErfMaskConfig:
    """Immutable public tensor and audio contract."""

    sample_rate: int = SAMPLE_RATE
    hop_samples: int = HOP_SAMPLES
    analysis_samples: int = ANALYSIS_SAMPLES
    erf_bins: int = ERF_BINS
    input_scale: np.float32 = ERF_INPUT_SCALE
    input_zero_point: int = ERF_INPUT_ZERO_POINT
    output_scale: np.float32 = ERF_OUTPUT_SCALE
    output_zero_point: int = ERF_OUTPUT_ZERO_POINT
    mask_floor: np.float32 = MASK_FLOOR


def _invalid_input(message: str) -> HybridError:
    return HybridError(ErrorCode.ONNX_INPUT_INVALID, message)


def quantize_logits(logits: np.ndarray) -> np.ndarray:
    """Quantize finite logits with the deployment ties-to-even rule."""

    values = np.asarray(logits)
    if not np.issubdtype(values.dtype, np.floating):
        raise _invalid_input("Erf logits must have a floating-point dtype")
    if not np.isfinite(values).all():
        raise _invalid_input("Erf logits contain non-finite values")
    working = np.asarray(values, dtype=np.float64)
    clipped = np.clip(working, _MIN_LOGIT, _MAX_LOGIT)
    quantized = np.rint(clipped * 32.0 + ERF_INPUT_ZERO_POINT)
    return np.ascontiguousarray(quantized, dtype=np.uint8)


@lru_cache(maxsize=1)
def build_erf_u8_table() -> np.ndarray:
    """Return the immutable independent 256-entry quantized-Erf oracle."""

    values = np.fromiter(
        (
            max(
                0,
                min(
                    255,
                    round(math.erf((input_value - ERF_INPUT_ZERO_POINT) / 32.0) * 128.0)
                    + ERF_OUTPUT_ZERO_POINT,
                ),
            )
            for input_value in range(256)
        ),
        dtype=np.uint8,
        count=256,
    )
    values.flags.writeable = False
    return values


def _require_u8(values: np.ndarray, *, name: str) -> np.ndarray:
    array = np.asarray(values)
    if array.dtype != np.dtype(np.uint8):
        raise _invalid_input(f"{name} must have dtype uint8")
    return array


def cpu_erf_u8(inputs: np.ndarray) -> np.ndarray:
    """Evaluate the deployed Erf partition through the independent LUT."""

    values = _require_u8(inputs, name="Erf input")
    return np.ascontiguousarray(build_erf_u8_table()[values])


def mask_from_erf_u8(outputs: np.ndarray) -> np.ndarray:
    """Map quantized Erf output to the deployed magnitude-mask contract."""

    values = _require_u8(outputs, name="Erf output")
    masks = values.astype(np.float32) * np.float32(1.0 / 256.0)
    return np.ascontiguousarray(np.maximum(masks, MASK_FLOOR))


@lru_cache(maxsize=1)
def periodic_sqrt_hann() -> np.ndarray:
    """Return the immutable periodic square-root Hann analysis window."""

    indexes = np.arange(ANALYSIS_SAMPLES, dtype=np.float64)
    hann = 0.5 - 0.5 * np.cos(2.0 * np.pi * indexes / np.float64(ANALYSIS_SAMPLES))
    window = np.ascontiguousarray(np.sqrt(hann), dtype=np.float64)
    window.flags.writeable = False
    return window


def _finite_real_vector(
    values: np.ndarray,
    *,
    name: str,
    length: int,
) -> np.ndarray:
    array = np.asarray(values)
    if array.shape != (length,):
        raise _invalid_input(f"{name} shape {array.shape} does not match ({length},)")
    if not np.issubdtype(array.dtype, np.floating):
        raise _invalid_input(f"{name} must have a floating-point dtype")
    if not np.isfinite(array).all():
        raise _invalid_input(f"{name} contains non-finite values")
    return np.asarray(array, dtype=np.float64)


def _finite_spectrum(values: np.ndarray) -> np.ndarray:
    array = np.asarray(values)
    if array.shape != (ERF_BINS,):
        raise _invalid_input(
            f"spectrum shape {array.shape} does not match ({ERF_BINS},)"
        )
    if not np.issubdtype(array.dtype, np.complexfloating):
        raise _invalid_input("spectrum must have a complex floating-point dtype")
    if not np.isfinite(array).all():
        raise _invalid_input("spectrum contains non-finite values")
    return np.asarray(array, dtype=np.complex128)


class StreamingAnalysis:
    """Causal 960-point analysis over the previous and current hops."""

    def __init__(self) -> None:
        self._previous = np.zeros(HOP_SAMPLES, dtype=np.float64)

    def analyze(self, hop: np.ndarray) -> np.ndarray:
        values = _finite_real_vector(
            hop,
            name="audio hop",
            length=HOP_SAMPLES,
        )
        frame = np.empty(ANALYSIS_SAMPLES, dtype=np.float64)
        frame[:HOP_SAMPLES] = self._previous
        frame[HOP_SAMPLES:] = values
        self._previous[:] = values
        return np.ascontiguousarray(np.fft.rfft(frame * periodic_sqrt_hann()))

    def reset(self) -> None:
        self._previous.fill(0.0)


class StreamingSynthesis:
    """One-hop-delayed periodic square-root Hann overlap-add synthesis."""

    def __init__(self) -> None:
        self._overlap = np.zeros(HOP_SAMPLES, dtype=np.float64)
        self._primed = False

    def synthesize(self, spectrum: np.ndarray) -> np.ndarray:
        values = _finite_spectrum(spectrum)
        frame = np.fft.irfft(values, n=ANALYSIS_SAMPLES)
        frame *= periodic_sqrt_hann()
        output = frame[:HOP_SAMPLES] + self._overlap
        self._overlap[:] = frame[HOP_SAMPLES:]
        if not self._primed:
            self._primed = True
            output.fill(0.0)
        return np.ascontiguousarray(output, dtype=np.float32)

    def reset(self) -> None:
        self._overlap.fill(0.0)
        self._primed = False


class StreamingStft:
    """Compose deterministic analysis, real masking, and synthesis."""

    def __init__(self) -> None:
        self._analysis = StreamingAnalysis()
        self._synthesis = StreamingSynthesis()

    def process(self, hop: np.ndarray, mask: np.ndarray) -> np.ndarray:
        masks = _finite_real_vector(
            mask,
            name="spectral mask",
            length=ERF_BINS,
        )
        spectrum = self._analysis.analyze(hop)
        return self._synthesis.synthesize(spectrum * masks)

    def reset(self) -> None:
        self._analysis.reset()
        self._synthesis.reset()


class CausalNoiseEstimator:
    """Exact 100-frame minimum-controlled causal power estimator."""

    def __init__(self) -> None:
        self._smoothed = np.zeros(ERF_BINS, dtype=np.float64)
        self._noise = np.zeros(ERF_BINS, dtype=np.float64)
        self._ring = np.zeros(
            (_NOISE_MINIMUM_FRAMES, ERF_BINS),
            dtype=np.float64,
        )
        self._ring_count = 0
        self._ring_index = 0
        self._initialized = False

    def estimate(self, power: np.ndarray) -> np.ndarray:
        values = _finite_real_vector(
            power,
            name="power spectrum",
            length=ERF_BINS,
        )
        if np.any(values < 0.0):
            raise _invalid_input("power spectrum contains negative values")
        bounded = np.maximum(values, NOISE_EPSILON)

        if not self._initialized:
            self._smoothed[:] = bounded
            self._noise[:] = bounded
            self._ring[0] = bounded
            self._ring_count = 1
            self._ring_index = 1
            self._initialized = True
            return np.ascontiguousarray(self._noise.copy())

        self._smoothed *= 0.8
        self._smoothed += 0.2 * bounded
        self._ring[self._ring_index] = self._smoothed
        self._ring_index = (self._ring_index + 1) % _NOISE_MINIMUM_FRAMES
        self._ring_count = min(
            self._ring_count + 1,
            _NOISE_MINIMUM_FRAMES,
        )

        minimum = np.min(self._ring[: self._ring_count], axis=0)
        candidate = np.maximum(1.5 * minimum, NOISE_EPSILON)
        noise_dominant = bounded <= 3.0 * self._noise
        self._noise[:] = np.where(
            noise_dominant,
            0.9 * self._noise + 0.1 * candidate,
            0.995 * self._noise + 0.005 * candidate,
        )
        np.maximum(self._noise, NOISE_EPSILON, out=self._noise)
        return np.ascontiguousarray(self._noise.copy())

    def reset(self) -> None:
        self._smoothed.fill(0.0)
        self._noise.fill(0.0)
        self._ring.fill(0.0)
        self._ring_count = 0
        self._ring_index = 0
        self._initialized = False


def posterior_log_snr_feature(
    power: np.ndarray,
    noise: np.ndarray,
) -> np.ndarray:
    """Compute the clipped log posterior-SNR feature."""

    power_values = _finite_real_vector(
        power,
        name="power spectrum",
        length=ERF_BINS,
    )
    noise_values = _finite_real_vector(
        noise,
        name="noise spectrum",
        length=ERF_BINS,
    )
    if np.any(power_values < 0.0):
        raise _invalid_input("power spectrum contains negative values")
    if np.any(noise_values <= 0.0):
        raise _invalid_input("noise spectrum must be strictly positive")
    feature = np.log(power_values + NOISE_EPSILON) - np.log(
        noise_values + NOISE_EPSILON
    )
    return np.ascontiguousarray(np.clip(feature, -8.0, 8.0), dtype=np.float32)
