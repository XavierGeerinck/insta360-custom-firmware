"""Causal inference shell around the quantized Erf partition boundary."""

from __future__ import annotations

from collections.abc import Callable

import numpy as np

from .erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    CausalNoiseEstimator,
    StreamingAnalysis,
    StreamingSynthesis,
    cpu_erf_u8,
    mask_from_erf_u8,
    posterior_log_snr_feature,
    quantize_logits,
)
from .errors import ErrorCode, HybridError

ErfPartition = Callable[[np.ndarray], np.ndarray]
FeaturePartition = Callable[[np.ndarray], np.ndarray]
ERF_RESIDUAL_MIX = np.float32(1.0 / 32.0)
DFN_CLEAN_RETAIN_DB = 1.0
DFN_CLEAN_RETAIN_POWER_RATIO = 10.0 ** (-DFN_CLEAN_RETAIN_DB / 10.0)


class HybridErfPartition:
    """Latch to the CPU oracle after the first failed NPU frame.

    The NPU callable remains the primary path until it raises or violates the
    exact UINT8[481] output contract. That frame and all subsequent frames use
    the CPU partition, avoiding repeated device faults in the audio callback.
    """

    def __init__(
        self,
        npu_partition: ErfPartition,
        *,
        cpu_partition: ErfPartition = cpu_erf_u8,
    ) -> None:
        if not callable(npu_partition) or not callable(cpu_partition):
            raise HybridError(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "hybrid Erf partitions must be callable",
            )
        self._npu_partition = npu_partition
        self._cpu_partition = cpu_partition
        self._npu_attempts = 0
        self._npu_successes = 0
        self._fallback_frames = 0
        self._fallback_latched = False
        self._first_failure: dict[str, str] | None = None

    @property
    def npu_attempts(self) -> int:
        return self._npu_attempts

    @property
    def npu_successes(self) -> int:
        return self._npu_successes

    @property
    def fallback_frames(self) -> int:
        return self._fallback_frames

    @property
    def fallback_latched(self) -> bool:
        return self._fallback_latched

    @property
    def first_failure(self) -> dict[str, str] | None:
        if self._first_failure is None:
            return None
        return dict(self._first_failure)

    def _cpu(self, inputs: np.ndarray) -> np.ndarray:
        self._fallback_frames += 1
        try:
            output = np.asarray(self._cpu_partition(inputs))
        except Exception as exc:
            raise HybridError(
                ErrorCode.CPU_FALLBACK_FAILURE,
                f"CPU Erf fallback failed: {exc}",
            ) from exc
        if output.shape != (ERF_BINS,) or output.dtype != np.dtype(np.uint8):
            raise HybridError(
                ErrorCode.CPU_FALLBACK_FAILURE,
                "CPU Erf fallback must return UINT8[481]",
            )
        return np.ascontiguousarray(output)

    def _latch(self, failure: BaseException | str) -> None:
        self._fallback_latched = True
        if self._first_failure is None:
            if isinstance(failure, BaseException):
                kind = type(failure).__name__
                message = str(failure)
            else:
                kind = "ContractViolation"
                message = failure
            self._first_failure = {
                "type": kind[:80],
                "message": message[:240],
            }

    def __call__(self, inputs: np.ndarray) -> np.ndarray:
        values = np.asarray(inputs)
        if values.shape != (ERF_BINS,) or values.dtype != np.dtype(np.uint8):
            raise HybridError(
                ErrorCode.NPU_INPUT_QUANTIZATION_MISMATCH,
                "hybrid Erf input must be UINT8[481]",
            )
        if self._fallback_latched:
            return self._cpu(values)

        self._npu_attempts += 1
        try:
            output = np.asarray(self._npu_partition(values))
        except Exception as exc:
            self._latch(exc)
            return self._cpu(values)
        if output.shape != (ERF_BINS,) or output.dtype != np.dtype(np.uint8):
            self._latch("NPU Erf output must be UINT8[481]")
            return self._cpu(values)
        self._npu_successes += 1
        return np.ascontiguousarray(output)


def _weights(
    slopes: np.ndarray,
    biases: np.ndarray,
) -> tuple[np.ndarray, np.ndarray]:
    slope_values = np.asarray(slopes)
    bias_values = np.asarray(biases)
    if (
        slope_values.shape != (ERF_BINS,)
        or bias_values.shape != (ERF_BINS,)
        or not np.issubdtype(slope_values.dtype, np.floating)
        or not np.issubdtype(bias_values.dtype, np.floating)
        or not np.isfinite(slope_values).all()
        or not np.isfinite(bias_values).all()
        or np.any(slope_values < 0.0)
    ):
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID,
            "invalid Erf-mask deployment weights",
        )
    return (
        np.ascontiguousarray(slope_values, dtype=np.float64),
        np.ascontiguousarray(bias_values, dtype=np.float64),
    )


def _hop(samples: np.ndarray) -> np.ndarray:
    values = np.asarray(samples)
    if (
        values.shape != (HOP_SAMPLES,)
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise HybridError(
            ErrorCode.ONNX_INPUT_INVALID,
            f"audio hop must be {HOP_SAMPLES} finite floating-point samples",
        )
    return np.asarray(values, dtype=np.float64)


def _audio(samples: np.ndarray, *, name: str) -> np.ndarray:
    values = np.asarray(samples)
    if (
        values.ndim != 1
        or values.size == 0
        or values.size % HOP_SAMPLES != 0
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise HybridError(
            ErrorCode.ONNX_INPUT_INVALID,
            f"{name} must be a nonempty hop-aligned finite float vector",
        )
    return values


def _partition_output(
    partition: ErfPartition,
    quantized_input: np.ndarray,
) -> np.ndarray:
    try:
        quantized_output = partition(quantized_input)
    except HybridError:
        raise
    except Exception as exc:
        raise HybridError(
            ErrorCode.CPU_FALLBACK_FAILURE,
            f"Erf partition failed: {exc}",
        ) from exc
    output_values = np.asarray(quantized_output)
    if output_values.shape != (ERF_BINS,) or output_values.dtype != np.dtype(np.uint8):
        raise HybridError(
            ErrorCode.NPU_OUTPUT_QUANTIZATION_MISMATCH,
            "Erf partition output must be UINT8[481]",
        )
    return output_values


class ErfMaskDenoiser:
    """Run CPU feature/synthesis code around an injectable U8 Erf executor."""

    def __init__(
        self,
        slopes: np.ndarray,
        biases: np.ndarray,
        *,
        partition: ErfPartition = cpu_erf_u8,
    ) -> None:
        self._slopes, self._biases = _weights(slopes, biases)
        if not callable(partition):
            raise HybridError(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "Erf partition must be callable",
            )
        self._partition = partition
        self._analysis = StreamingAnalysis()
        self._synthesis = StreamingSynthesis()
        self._noise_estimator = CausalNoiseEstimator()
        self._partition_calls = 0

    @property
    def partition_calls(self) -> int:
        return self._partition_calls

    def process_hop(self, samples: np.ndarray) -> np.ndarray:
        """Process one public 10 ms hop, preserving the one-hop delay."""

        values = _hop(samples)
        spectrum = self._analysis.analyze(values)
        power = np.square(np.abs(spectrum))
        noise_power = self._noise_estimator.estimate(power)
        feature = posterior_log_snr_feature(power, noise_power)
        logits = feature.astype(np.float64) * self._slopes + self._biases
        quantized_input = quantize_logits(logits)
        output_values = _partition_output(
            self._partition,
            quantized_input,
        )
        self._partition_calls += 1
        mask = mask_from_erf_u8(output_values)
        return self._synthesis.synthesize(spectrum * mask)

    def process_audio(
        self,
        samples: np.ndarray,
        *,
        compensate_delay: bool,
    ) -> np.ndarray:
        """Process a hop-aligned vector, optionally aligning offline metrics."""

        values = _audio(samples, name="audio")
        outputs = [
            self.process_hop(values[start : start + HOP_SAMPLES])
            for start in range(0, values.size, HOP_SAMPLES)
        ]
        if compensate_delay:
            outputs.append(self.process_hop(np.zeros(HOP_SAMPLES, dtype=np.float32)))
            joined = np.concatenate(outputs)
            return np.ascontiguousarray(
                joined[HOP_SAMPLES : HOP_SAMPLES + values.size],
                dtype=np.float32,
            )
        return np.ascontiguousarray(np.concatenate(outputs), dtype=np.float32)

    def reset(self) -> None:
        self._analysis.reset()
        self._synthesis.reset()
        self._noise_estimator.reset()
        self._partition_calls = 0


class ErfResidualPostfilter:
    """Apply a bounded learned Erf residual to a stronger base enhancer."""

    def __init__(
        self,
        slopes: np.ndarray,
        biases: np.ndarray,
        *,
        residual_mix: float = float(ERF_RESIDUAL_MIX),
        partition: ErfPartition = cpu_erf_u8,
    ) -> None:
        self._slopes, self._biases = _weights(slopes, biases)
        if (
            not callable(partition)
            or not np.isfinite(residual_mix)
            or not 0.0 <= residual_mix <= 1.0
        ):
            raise HybridError(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "invalid Erf residual partition or mix",
            )
        self._residual_mix = np.float32(residual_mix)
        self._partition = partition
        self._feature_analysis = StreamingAnalysis()
        self._base_analysis = StreamingAnalysis()
        self._synthesis = StreamingSynthesis()
        self._noise_estimator = CausalNoiseEstimator()
        self._partition_calls = 0

    @property
    def partition_calls(self) -> int:
        return self._partition_calls

    def process_hop(
        self,
        noisy_samples: np.ndarray,
        base_enhanced_samples: np.ndarray,
    ) -> np.ndarray:
        noisy_values = _hop(noisy_samples)
        base_values = _hop(base_enhanced_samples)
        noisy_spectrum = self._feature_analysis.analyze(noisy_values)
        power = np.square(np.abs(noisy_spectrum))
        noise_power = self._noise_estimator.estimate(power)
        feature = posterior_log_snr_feature(power, noise_power)
        logits = feature.astype(np.float64) * self._slopes + self._biases
        quantized_input = quantize_logits(logits)
        quantized_output = _partition_output(
            self._partition,
            quantized_input,
        )
        self._partition_calls += 1
        learned_mask = mask_from_erf_u8(quantized_output)
        residual_mask = np.float32(1.0) - self._residual_mix * (
            np.float32(1.0) - learned_mask
        )
        base_spectrum = self._base_analysis.analyze(base_values)
        return self._synthesis.synthesize(base_spectrum * residual_mask)

    def process_audio(
        self,
        noisy_samples: np.ndarray,
        base_enhanced_samples: np.ndarray,
        *,
        compensate_delay: bool,
    ) -> np.ndarray:
        noisy_values = _audio(noisy_samples, name="noisy audio")
        base_values = _audio(
            base_enhanced_samples,
            name="base-enhanced audio",
        )
        if noisy_values.shape != base_values.shape:
            raise HybridError(
                ErrorCode.ONNX_INPUT_INVALID,
                "noisy and base-enhanced audio lengths differ",
            )
        outputs = [
            self.process_hop(
                noisy_values[start : start + HOP_SAMPLES],
                base_values[start : start + HOP_SAMPLES],
            )
            for start in range(0, noisy_values.size, HOP_SAMPLES)
        ]
        if compensate_delay:
            zeros = np.zeros(HOP_SAMPLES, dtype=np.float32)
            outputs.append(self.process_hop(zeros, zeros))
            joined = np.concatenate(outputs)
            return np.ascontiguousarray(
                joined[HOP_SAMPLES : HOP_SAMPLES + noisy_values.size],
                dtype=np.float32,
            )
        return np.ascontiguousarray(np.concatenate(outputs), dtype=np.float32)

    def reset(self) -> None:
        self._feature_analysis.reset()
        self._base_analysis.reset()
        self._synthesis.reset()
        self._noise_estimator.reset()
        self._partition_calls = 0


class DfnErfFrameSelector:
    """Causally select speech-preserving DFN or the learned Erf candidate."""

    def __init__(
        self,
        slopes: np.ndarray,
        biases: np.ndarray,
        *,
        partition: ErfPartition = cpu_erf_u8,
        feature_partition: FeaturePartition | None = None,
    ) -> None:
        self._slopes, self._biases = _weights(slopes, biases)
        if not callable(partition):
            raise HybridError(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "Erf partition must be callable",
            )
        if feature_partition is not None and not callable(feature_partition):
            raise HybridError(
                ErrorCode.MODEL_PACKAGE_INVALID,
                "complete feature partition must be callable",
            )
        self._partition = partition
        self._feature_partition = feature_partition
        self._noisy_analysis = StreamingAnalysis()
        self._base_analysis = StreamingAnalysis()
        self._synthesis = StreamingSynthesis()
        self._noise_estimator = CausalNoiseEstimator()
        self._partition_calls = 0
        self._erf_selected_frames = 0

    @property
    def partition_calls(self) -> int:
        return self._partition_calls

    @property
    def erf_selected_frames(self) -> int:
        return self._erf_selected_frames

    @property
    def dfn_selected_frames(self) -> int:
        return self._partition_calls - self._erf_selected_frames

    @property
    def erf_selection_fraction(self) -> float:
        if self._partition_calls == 0:
            return 0.0
        return self._erf_selected_frames / self._partition_calls

    def process_hop(
        self,
        noisy_samples: np.ndarray,
        base_enhanced_samples: np.ndarray,
    ) -> np.ndarray:
        """Process one hop and execute the learned partition unconditionally."""

        noisy_values = _hop(noisy_samples)
        base_values = _hop(base_enhanced_samples)
        noisy_spectrum = self._noisy_analysis.analyze(noisy_values)
        noisy_power = np.square(np.abs(noisy_spectrum))
        noise_power = self._noise_estimator.estimate(noisy_power)
        feature = posterior_log_snr_feature(noisy_power, noise_power)
        if self._feature_partition is None:
            logits = feature.astype(np.float64) * self._slopes + self._biases
            quantized_input = quantize_logits(logits)
            quantized_output = _partition_output(
                self._partition,
                quantized_input,
            )
        else:
            quantized_output = _partition_output(
                self._feature_partition,
                np.ascontiguousarray(feature, dtype=np.float64),
            )
        self._partition_calls += 1
        learned_mask = mask_from_erf_u8(quantized_output)
        learned_spectrum = noisy_spectrum * learned_mask

        base_spectrum = self._base_analysis.analyze(base_values)
        noisy_energy = float(np.sum(noisy_power, dtype=np.float64))
        base_energy = float(np.sum(np.square(np.abs(base_spectrum)), dtype=np.float64))
        if (
            noisy_energy == 0.0
            or base_energy >= DFN_CLEAN_RETAIN_POWER_RATIO * noisy_energy
        ):
            selected_spectrum = base_spectrum
        else:
            selected_spectrum = learned_spectrum
            self._erf_selected_frames += 1
        return self._synthesis.synthesize(selected_spectrum)

    def process_audio(
        self,
        noisy_samples: np.ndarray,
        base_enhanced_samples: np.ndarray,
        *,
        compensate_delay: bool,
    ) -> np.ndarray:
        """Process aligned audio and optionally remove the selector hop delay."""

        noisy_values = _audio(noisy_samples, name="noisy audio")
        base_values = _audio(
            base_enhanced_samples,
            name="base-enhanced audio",
        )
        if noisy_values.shape != base_values.shape:
            raise HybridError(
                ErrorCode.ONNX_INPUT_INVALID,
                "noisy and base-enhanced audio lengths differ",
            )
        outputs = [
            self.process_hop(
                noisy_values[start : start + HOP_SAMPLES],
                base_values[start : start + HOP_SAMPLES],
            )
            for start in range(0, noisy_values.size, HOP_SAMPLES)
        ]
        if compensate_delay:
            zeros = np.zeros(HOP_SAMPLES, dtype=np.float32)
            outputs.append(self.process_hop(zeros, zeros))
            joined = np.concatenate(outputs)
            return np.ascontiguousarray(
                joined[HOP_SAMPLES : HOP_SAMPLES + noisy_values.size],
                dtype=np.float32,
            )
        return np.ascontiguousarray(np.concatenate(outputs), dtype=np.float32)

    def reset(self) -> None:
        self._noisy_analysis.reset()
        self._base_analysis.reset()
        self._synthesis.reset()
        self._noise_estimator.reset()
        self._partition_calls = 0
        self._erf_selected_frames = 0
