"""Feature/target construction and deterministic frame selection."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from types import MappingProxyType

import numpy as np

from .erf_mask_corpus import CorpusItem
from .erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    NOISE_EPSILON,
    CausalNoiseEstimator,
    StreamingAnalysis,
    cpu_erf_u8,
    mask_from_erf_u8,
    posterior_log_snr_feature,
    quantize_logits,
)
from .errors import ErrorCode, HybridError


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _readonly_f32(values: np.ndarray) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=np.float32)
    result.flags.writeable = False
    return result


def square_root_wiener_target(
    clean_power: np.ndarray,
    noise_power: np.ndarray,
) -> np.ndarray:
    clean = np.asarray(clean_power)
    noise = np.asarray(noise_power)
    if (
        clean.shape != noise.shape
        or clean.size == 0
        or not np.issubdtype(clean.dtype, np.floating)
        or not np.issubdtype(noise.dtype, np.floating)
        or not np.isfinite(clean).all()
        or not np.isfinite(noise).all()
        or np.any(clean < 0.0)
        or np.any(noise < 0.0)
    ):
        raise _invalid("invalid clean/noise power for Wiener target")
    clean64 = np.asarray(clean, dtype=np.float64)
    noise64 = np.asarray(noise, dtype=np.float64)
    target = np.sqrt(clean64 / (clean64 + noise64 + NOISE_EPSILON))
    return np.ascontiguousarray(np.clip(target, 0.0, 1.0), dtype=np.float32)


def _audio_vector(values: np.ndarray, *, name: str) -> np.ndarray:
    array = np.asarray(values)
    if (
        array.ndim != 1
        or array.size == 0
        or array.size % HOP_SAMPLES != 0
        or not np.issubdtype(array.dtype, np.floating)
        or not np.isfinite(array).all()
    ):
        raise _invalid(
            f"{name} must be a finite floating vector with a whole hop count"
        )
    return np.asarray(array, dtype=np.float64)


@dataclass(frozen=True, slots=True)
class FeatureTargetPair:
    features: np.ndarray = field(repr=False)
    targets: np.ndarray = field(repr=False)
    payload_sha256: str = field(init=False)

    def __post_init__(self) -> None:
        features = _readonly_f32(self.features)
        targets = _readonly_f32(self.targets)
        if (
            features.ndim != 2
            or features.shape != targets.shape
            or features.shape[1] != ERF_BINS
        ):
            raise _invalid("feature/target tensor contract mismatch")
        digest = hashlib.sha256(
            features.astype("<f4", copy=False).tobytes()
            + targets.astype("<f4", copy=False).tobytes()
        ).hexdigest()
        object.__setattr__(self, "features", features)
        object.__setattr__(self, "targets", targets)
        object.__setattr__(self, "payload_sha256", digest)


def build_feature_target_pair(
    mixture: np.ndarray,
    clean: np.ndarray,
    noise: np.ndarray,
) -> FeatureTargetPair:
    mixture_values = _audio_vector(mixture, name="mixture")
    clean_values = _audio_vector(clean, name="clean reference")
    noise_values = _audio_vector(noise, name="noise reference")
    if (
        clean_values.shape != mixture_values.shape
        or noise_values.shape != mixture_values.shape
        or not np.allclose(
            mixture_values,
            clean_values + noise_values,
            atol=2e-6,
            rtol=1e-6,
        )
    ):
        raise _invalid("mixture is not aligned clean plus noise")

    mixture_analysis = StreamingAnalysis()
    clean_analysis = StreamingAnalysis()
    noise_analysis = StreamingAnalysis()
    estimator = CausalNoiseEstimator()
    frame_count = mixture_values.size // HOP_SAMPLES
    features = np.empty((frame_count, ERF_BINS), dtype=np.float32)
    targets = np.empty((frame_count, ERF_BINS), dtype=np.float32)

    for frame_index in range(frame_count):
        start = frame_index * HOP_SAMPLES
        stop = start + HOP_SAMPLES
        mixture_spectrum = mixture_analysis.analyze(mixture_values[start:stop])
        clean_spectrum = clean_analysis.analyze(clean_values[start:stop])
        noise_spectrum = noise_analysis.analyze(noise_values[start:stop])
        mixture_power = np.square(np.abs(mixture_spectrum))
        estimated_noise = estimator.estimate(mixture_power)
        features[frame_index] = posterior_log_snr_feature(
            mixture_power,
            estimated_noise,
        )
        targets[frame_index] = square_root_wiener_target(
            np.square(np.abs(clean_spectrum)),
            np.square(np.abs(noise_spectrum)),
        )
    return FeatureTargetPair(features=features, targets=targets)


def _group_key(item: CorpusItem) -> str:
    snr = "-" if item.snr_db is None else f"{item.snr_db:+.1f}"
    return "|".join(
        (
            item.category,
            item.clean_kind or "-",
            item.noise_kind or "-",
            snr,
        )
    )


def _row_priority(seed: int, identity: str, frame_index: int) -> int:
    document = f"{seed}|{identity}|{frame_index}".encode()
    return int.from_bytes(hashlib.sha256(document).digest()[:8], "big")


@dataclass(frozen=True, slots=True)
class FrameStore:
    split: str
    features: np.ndarray = field(repr=False)
    targets: np.ndarray = field(repr=False)
    row_ids: tuple[str, ...]
    group_counts: Mapping[str, int]
    payload_sha256: str = field(init=False)

    def __post_init__(self) -> None:
        features = _readonly_f32(self.features)
        targets = _readonly_f32(self.targets)
        if (
            features.ndim != 2
            or features.shape != targets.shape
            or features.shape[1] != ERF_BINS
            or features.shape[0] != len(self.row_ids)
            or sum(self.group_counts.values()) != len(self.row_ids)
        ):
            raise _invalid("frame-store tensor or metadata contract mismatch")
        canonical_counts = {
            key: self.group_counts[key] for key in sorted(self.group_counts)
        }
        digest = hashlib.sha256()
        digest.update(features.astype("<f4", copy=False).tobytes())
        digest.update(targets.astype("<f4", copy=False).tobytes())
        digest.update(
            (
                json.dumps(
                    {
                        "split": self.split,
                        "row_ids": list(self.row_ids),
                        "group_counts": canonical_counts,
                    },
                    ensure_ascii=False,
                    separators=(",", ":"),
                    sort_keys=True,
                )
                + "\n"
            ).encode()
        )
        object.__setattr__(self, "features", features)
        object.__setattr__(self, "targets", targets)
        object.__setattr__(
            self,
            "group_counts",
            MappingProxyType(canonical_counts),
        )
        object.__setattr__(self, "payload_sha256", digest.hexdigest())


@dataclass(frozen=True, slots=True)
class _Candidate:
    priority: int
    item_index: int
    frame_index: int
    group: str
    row_id: str


def build_balanced_frame_store(
    items: list[CorpusItem],
    *,
    max_frames: int,
    seed: int,
) -> FrameStore:
    if not items:
        raise _invalid("cannot build a frame store from no items")
    if (
        isinstance(max_frames, bool)
        or not isinstance(max_frames, int)
        or max_frames <= 0
        or isinstance(seed, bool)
        or not isinstance(seed, int)
        or seed < 0
    ):
        raise _invalid("invalid frame-store budget or seed")
    splits = {item.split for item in items}
    if len(splits) != 1:
        raise _invalid("frame-store items belong to mixed splits")
    split = next(iter(splits))

    grouped: dict[str, list[_Candidate]] = {}
    for item_index, item in enumerate(items):
        if item.mixture.size % HOP_SAMPLES != 0:
            raise _invalid(f"item has a partial hop: {item.identity}")
        group = _group_key(item)
        candidates = grouped.setdefault(group, [])
        for frame_index in range(item.mixture.size // HOP_SAMPLES):
            row_id = f"{item.identity}#{frame_index:08d}"
            candidates.append(
                _Candidate(
                    priority=_row_priority(seed, item.identity, frame_index),
                    item_index=item_index,
                    frame_index=frame_index,
                    group=group,
                    row_id=row_id,
                )
            )
    for candidates in grouped.values():
        candidates.sort(key=lambda item: (item.priority, item.row_id))

    target_count = min(
        max_frames,
        sum(len(candidates) for candidates in grouped.values()),
    )
    groups = sorted(grouped)
    base, remainder = divmod(target_count, len(groups))
    selected: list[_Candidate] = []
    selected_ids: set[str] = set()
    for group_index, group in enumerate(groups):
        quota = base + int(group_index < remainder)
        for candidate in grouped[group][:quota]:
            selected.append(candidate)
            selected_ids.add(candidate.row_id)

    if len(selected) < target_count:
        remainder_candidates = sorted(
            (
                candidate
                for candidates in grouped.values()
                for candidate in candidates
                if candidate.row_id not in selected_ids
            ),
            key=lambda item: (item.priority, item.row_id),
        )
        selected.extend(remainder_candidates[: target_count - len(selected)])
    selected.sort(key=lambda item: (item.priority, item.row_id))

    pair_cache: dict[int, FeatureTargetPair] = {}
    feature_rows: list[np.ndarray] = []
    target_rows: list[np.ndarray] = []
    group_counts: dict[str, int] = {group: 0 for group in groups}
    for candidate in selected:
        if candidate.item_index not in pair_cache:
            item = items[candidate.item_index]
            pair_cache[candidate.item_index] = build_feature_target_pair(
                item.mixture,
                item.clean,
                item.noise,
            )
        pair = pair_cache[candidate.item_index]
        feature_rows.append(pair.features[candidate.frame_index])
        target_rows.append(pair.targets[candidate.frame_index])
        group_counts[candidate.group] += 1

    return FrameStore(
        split=split,
        features=np.stack(feature_rows),
        targets=np.stack(target_rows),
        row_ids=tuple(candidate.row_id for candidate in selected),
        group_counts=group_counts,
    )


def _readonly_f64(values: np.ndarray) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=np.float64)
    result.flags.writeable = False
    return result


@dataclass(frozen=True, slots=True)
class TrainingConfig:
    seed: int = 0x57335645
    batch_size: int = 512
    steps: int = 2_000
    learning_rate: float = 0.01
    beta1: float = 0.9
    beta2: float = 0.999
    adam_epsilon: float = 1e-8
    validation_interval: int = 50
    regularization: float = 1e-4

    def __post_init__(self) -> None:
        integer_values = (self.seed, self.batch_size, self.steps)
        if (
            any(
                isinstance(value, bool) or not isinstance(value, int)
                for value in integer_values
            )
            or self.seed < 0
            or self.batch_size <= 0
            or self.steps <= 0
            or isinstance(self.validation_interval, bool)
            or not isinstance(self.validation_interval, int)
            or self.validation_interval <= 0
            or not self.learning_rate > 0.0
            or not 0.0 <= self.beta1 < 1.0
            or not 0.0 <= self.beta2 < 1.0
            or not self.adam_epsilon > 0.0
            or not self.regularization >= 0.0
            or not all(
                np.isfinite(value)
                for value in (
                    self.learning_rate,
                    self.beta1,
                    self.beta2,
                    self.adam_epsilon,
                    self.regularization,
                )
            )
        ):
            raise _invalid("invalid Erf-mask training configuration")


@dataclass(frozen=True, slots=True)
class TrainingParameters:
    alpha: np.ndarray = field(repr=False)
    bias: np.ndarray = field(repr=False)

    def __post_init__(self) -> None:
        alpha = _readonly_f64(self.alpha)
        bias = _readonly_f64(self.bias)
        if (
            alpha.shape != (ERF_BINS,)
            or bias.shape != (ERF_BINS,)
            or not np.isfinite(alpha).all()
            or not np.isfinite(bias).all()
        ):
            raise _invalid("invalid training parameter tensors")
        object.__setattr__(self, "alpha", alpha)
        object.__setattr__(self, "bias", bias)


def initialize_parameters() -> TrainingParameters:
    target_softplus = 1.0 - 1e-4
    initial_alpha = np.log(np.expm1(target_softplus))
    return TrainingParameters(
        alpha=np.full(ERF_BINS, initial_alpha, dtype=np.float64),
        bias=np.zeros(ERF_BINS, dtype=np.float64),
    )


def _softplus(values: np.ndarray) -> np.ndarray:
    return np.log1p(np.exp(-np.abs(values))) + np.maximum(values, 0.0)


def _sigmoid(values: np.ndarray) -> np.ndarray:
    result = np.empty_like(values, dtype=np.float64)
    positive = values >= 0.0
    result[positive] = 1.0 / (1.0 + np.exp(-values[positive]))
    negative_exp = np.exp(values[~positive])
    result[~positive] = negative_exp / (1.0 + negative_exp)
    return result


def deployed_slopes(alpha: np.ndarray) -> np.ndarray:
    values = np.asarray(alpha)
    if (
        values.shape != (ERF_BINS,)
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid("alpha must be a finite 481-element floating vector")
    return np.ascontiguousarray(_softplus(np.asarray(values, dtype=np.float64)) + 1e-4)


def _model_inputs(
    features: np.ndarray,
    slopes: np.ndarray,
    biases: np.ndarray,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    feature_values = np.asarray(features)
    slope_values = np.asarray(slopes)
    bias_values = np.asarray(biases)
    if (
        feature_values.ndim != 2
        or feature_values.shape[1] != ERF_BINS
        or feature_values.shape[0] == 0
        or not np.issubdtype(feature_values.dtype, np.floating)
        or not np.isfinite(feature_values).all()
        or slope_values.shape != (ERF_BINS,)
        or bias_values.shape != (ERF_BINS,)
        or not np.issubdtype(slope_values.dtype, np.floating)
        or not np.issubdtype(bias_values.dtype, np.floating)
        or not np.isfinite(slope_values).all()
        or not np.isfinite(bias_values).all()
        or np.any(slope_values < 0.0)
    ):
        raise _invalid("invalid Erf-mask model input or parameter tensor")
    return (
        np.asarray(feature_values, dtype=np.float64),
        np.asarray(slope_values, dtype=np.float64),
        np.asarray(bias_values, dtype=np.float64),
    )


def predict_quantized_masks(
    features: np.ndarray,
    slopes: np.ndarray,
    biases: np.ndarray,
) -> np.ndarray:
    feature_values, slope_values, bias_values = _model_inputs(
        features,
        slopes,
        biases,
    )
    logits = feature_values * slope_values[np.newaxis, :]
    logits += bias_values[np.newaxis, :]
    return mask_from_erf_u8(cpu_erf_u8(quantize_logits(logits)))


def _regularization_and_gradient(
    values: np.ndarray,
    coefficient: float,
) -> tuple[float, np.ndarray]:
    differences = values[1:] - values[:-1]
    loss = coefficient * float(np.mean(np.square(differences)))
    gradient = np.zeros_like(values)
    scaled = (2.0 * coefficient / differences.size) * differences
    gradient[:-1] -= scaled
    gradient[1:] += scaled
    return loss, gradient


@dataclass(frozen=True, slots=True)
class LossGradient:
    loss: float
    mean_squared_error: float
    alpha_gradient: np.ndarray = field(repr=False)
    bias_gradient: np.ndarray = field(repr=False)
    predictions: np.ndarray = field(repr=False)


def loss_and_gradient(
    features: np.ndarray,
    targets: np.ndarray,
    alpha: np.ndarray,
    bias: np.ndarray,
    *,
    regularization: float = 1e-4,
) -> LossGradient:
    target_values = np.asarray(targets)
    if (
        target_values.ndim != 2
        or target_values.shape != np.asarray(features).shape
        or target_values.shape[1] != ERF_BINS
        or not np.issubdtype(target_values.dtype, np.floating)
        or not np.isfinite(target_values).all()
        or np.any(target_values < 0.0)
        or np.any(target_values > 1.0)
        or not np.isfinite(regularization)
        or regularization < 0.0
    ):
        raise _invalid("invalid Erf-mask training target")
    alpha_values = np.asarray(alpha)
    bias_values = np.asarray(bias)
    slopes = deployed_slopes(alpha_values)
    feature_values, slopes, bias_values = _model_inputs(
        features,
        slopes,
        bias_values,
    )

    logits = feature_values * slopes[np.newaxis, :]
    logits += bias_values[np.newaxis, :]
    quantized_inputs = quantize_logits(logits)
    quantized_outputs = cpu_erf_u8(quantized_inputs)
    predictions = mask_from_erf_u8(quantized_outputs)
    target64 = np.asarray(target_values, dtype=np.float64)
    error = np.asarray(predictions, dtype=np.float64) - target64
    mean_squared_error = float(np.mean(np.square(error)))

    slope_regularization, slope_regularization_gradient = _regularization_and_gradient(
        slopes, regularization
    )
    bias_regularization, bias_regularization_gradient = _regularization_and_gradient(
        bias_values, regularization
    )
    loss = mean_squared_error + slope_regularization + bias_regularization

    quantized_logits = (quantized_inputs.astype(np.float64) - 128.0) / 32.0
    raw_masks = quantized_outputs.astype(np.float64) / 256.0
    derivative = np.exp(-np.square(quantized_logits)) / np.sqrt(np.pi)
    derivative[raw_masks <= 0.05] = 0.0
    derivative[(logits < -4.0) | (logits > 3.96875)] = 0.0
    gradient_logits = (2.0 * error / error.size) * derivative
    slope_gradient = np.sum(
        gradient_logits * feature_values,
        axis=0,
    )
    slope_gradient += slope_regularization_gradient
    bias_gradient = np.sum(gradient_logits, axis=0)
    bias_gradient += bias_regularization_gradient
    alpha_gradient = slope_gradient * _sigmoid(
        np.asarray(alpha_values, dtype=np.float64)
    )

    return LossGradient(
        loss=float(loss),
        mean_squared_error=mean_squared_error,
        alpha_gradient=np.ascontiguousarray(alpha_gradient),
        bias_gradient=np.ascontiguousarray(bias_gradient),
        predictions=np.ascontiguousarray(predictions),
    )


@dataclass(frozen=True, slots=True)
class AdamState:
    step: int
    parameters: TrainingParameters
    alpha_first_moment: np.ndarray = field(repr=False)
    alpha_second_moment: np.ndarray = field(repr=False)
    bias_first_moment: np.ndarray = field(repr=False)
    bias_second_moment: np.ndarray = field(repr=False)


def initialize_adam(parameters: TrainingParameters) -> AdamState:
    zeros = np.zeros(ERF_BINS, dtype=np.float64)
    return AdamState(
        step=0,
        parameters=parameters,
        alpha_first_moment=zeros.copy(),
        alpha_second_moment=zeros.copy(),
        bias_first_moment=zeros.copy(),
        bias_second_moment=zeros.copy(),
    )


def adam_update(
    state: AdamState,
    gradient: LossGradient,
    config: TrainingConfig,
) -> AdamState:
    step = state.step + 1

    def update_moments(
        first: np.ndarray,
        second: np.ndarray,
        values: np.ndarray,
    ) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        next_first = config.beta1 * first + (1.0 - config.beta1) * values
        next_second = config.beta2 * second + (1.0 - config.beta2) * np.square(values)
        corrected_first = next_first / (1.0 - config.beta1**step)
        corrected_second = next_second / (1.0 - config.beta2**step)
        delta = (
            config.learning_rate
            * corrected_first
            / (np.sqrt(corrected_second) + config.adam_epsilon)
        )
        return next_first, next_second, delta

    alpha_first, alpha_second, alpha_delta = update_moments(
        state.alpha_first_moment,
        state.alpha_second_moment,
        gradient.alpha_gradient,
    )
    bias_first, bias_second, bias_delta = update_moments(
        state.bias_first_moment,
        state.bias_second_moment,
        gradient.bias_gradient,
    )
    parameters = TrainingParameters(
        alpha=state.parameters.alpha - alpha_delta,
        bias=state.parameters.bias - bias_delta,
    )
    return AdamState(
        step=step,
        parameters=parameters,
        alpha_first_moment=np.ascontiguousarray(alpha_first),
        alpha_second_moment=np.ascontiguousarray(alpha_second),
        bias_first_moment=np.ascontiguousarray(bias_first),
        bias_second_moment=np.ascontiguousarray(bias_second),
    )


def _validation_loss(
    store: FrameStore,
    parameters: TrainingParameters,
    *,
    regularization: float,
) -> float:
    slopes = deployed_slopes(parameters.alpha)
    squared_error = 0.0
    value_count = 0
    for start in range(0, store.features.shape[0], 4_096):
        stop = min(start + 4_096, store.features.shape[0])
        predictions = predict_quantized_masks(
            store.features[start:stop],
            slopes,
            parameters.bias,
        )
        differences = np.asarray(predictions, dtype=np.float64) - np.asarray(
            store.targets[start:stop], dtype=np.float64
        )
        squared_error += float(np.sum(np.square(differences)))
        value_count += differences.size
    slope_regularization, _ = _regularization_and_gradient(
        slopes,
        regularization,
    )
    bias_regularization, _ = _regularization_and_gradient(
        parameters.bias,
        regularization,
    )
    return squared_error / value_count + slope_regularization + bias_regularization


def select_best_checkpoint(records: tuple[dict[str, float | int], ...]) -> int:
    if not records:
        raise _invalid("training history is empty")
    selected_step: int | None = None
    selected_loss = np.inf
    for record in records:
        if set(record) != {"step", "validation_loss"}:
            raise _invalid("training checkpoint record keys differ")
        step = record["step"]
        loss = record["validation_loss"]
        if (
            isinstance(step, bool)
            or not isinstance(step, int)
            or step < 0
            or isinstance(loss, bool)
            or not isinstance(loss, (int, float))
            or not np.isfinite(loss)
        ):
            raise _invalid("invalid training checkpoint record")
        if float(loss) < selected_loss:
            selected_loss = float(loss)
            selected_step = step
    if selected_step is None:
        raise _invalid("training history has no selectable checkpoint")
    return selected_step


@dataclass(frozen=True, slots=True)
class TrainingResult:
    slopes: np.ndarray = field(repr=False)
    biases: np.ndarray = field(repr=False)
    selected_step: int
    initial_validation_loss: float
    selected_validation_loss: float
    history: tuple[dict[str, float | int], ...]
    payload_sha256: str = field(init=False)

    def __post_init__(self) -> None:
        slopes = _readonly_f32(self.slopes)
        biases = _readonly_f32(self.biases)
        if (
            slopes.shape != (ERF_BINS,)
            or biases.shape != (ERF_BINS,)
            or np.any(slopes < 0.0)
            or not np.isfinite(slopes).all()
            or not np.isfinite(biases).all()
        ):
            raise _invalid("invalid trained deployment weights")
        digest = hashlib.sha256()
        digest.update(slopes.astype("<f4", copy=False).tobytes())
        digest.update(biases.astype("<f4", copy=False).tobytes())
        digest.update(
            (
                json.dumps(
                    {
                        "selected_step": self.selected_step,
                        "initial_validation_loss": self.initial_validation_loss,
                        "selected_validation_loss": self.selected_validation_loss,
                        "history": self.history,
                    },
                    ensure_ascii=False,
                    separators=(",", ":"),
                    sort_keys=True,
                )
                + "\n"
            ).encode()
        )
        object.__setattr__(self, "slopes", slopes)
        object.__setattr__(self, "biases", biases)
        object.__setattr__(self, "payload_sha256", digest.hexdigest())


def train_erf_mask(
    train: FrameStore,
    validation: FrameStore,
    config: TrainingConfig,
) -> TrainingResult:
    if train.split == validation.split or set(train.row_ids) & set(validation.row_ids):
        raise _invalid("training and validation stores overlap")
    state = initialize_adam(initialize_parameters())
    initial_loss = _validation_loss(
        validation,
        state.parameters,
        regularization=config.regularization,
    )
    initial_training_loss = _validation_loss(
        train,
        state.parameters,
        regularization=config.regularization,
    )
    history: list[dict[str, float | int]] = [
        {
            "step": 0,
            "training_loss": initial_training_loss,
            "validation_loss": initial_loss,
        }
    ]
    checkpoints: dict[int, TrainingParameters] = {0: state.parameters}
    generator = np.random.Generator(np.random.PCG64(config.seed))

    for step in range(1, config.steps + 1):
        indexes = generator.integers(
            0,
            train.features.shape[0],
            size=config.batch_size,
        )
        gradient = loss_and_gradient(
            train.features[indexes],
            train.targets[indexes],
            state.parameters.alpha,
            state.parameters.bias,
            regularization=config.regularization,
        )
        state = adam_update(state, gradient, config)
        if step % config.validation_interval == 0 or step == config.steps:
            validation_loss = _validation_loss(
                validation,
                state.parameters,
                regularization=config.regularization,
            )
            history.append(
                {
                    "step": step,
                    "training_loss": gradient.loss,
                    "validation_loss": validation_loss,
                }
            )
            checkpoints[step] = state.parameters

    checkpoint_records = tuple(
        {
            "step": int(record["step"]),
            "validation_loss": float(record["validation_loss"]),
        }
        for record in history
    )
    selected_step = select_best_checkpoint(checkpoint_records)
    selected = checkpoints[selected_step]
    selected_loss = next(
        float(record["validation_loss"])
        for record in history
        if record["step"] == selected_step
    )
    return TrainingResult(
        slopes=deployed_slopes(selected.alpha),
        biases=selected.bias,
        selected_step=selected_step,
        initial_validation_loss=initial_loss,
        selected_validation_loss=selected_loss,
        history=tuple(history),
    )
