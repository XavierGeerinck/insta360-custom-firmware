from __future__ import annotations

import hashlib

import numpy as np
import pytest

from wave_onnx_hybrid.erf_mask_corpus import (
    ProceduralCorpusConfig,
    generate_procedural_split,
)
from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    NOISE_EPSILON,
)
from wave_onnx_hybrid.erf_mask_train import (
    AdamState,
    FrameStore,
    TrainingConfig,
    adam_update,
    build_balanced_frame_store,
    build_feature_target_pair,
    deployed_slopes,
    initialize_adam,
    initialize_parameters,
    loss_and_gradient,
    predict_quantized_masks,
    select_best_checkpoint,
    square_root_wiener_target,
    train_erf_mask,
)
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _small_items(split: str = "train"):
    config = ProceduralCorpusConfig(
        duration_samples=HOP_SAMPLES * 10,
        train_seeds=(13001,),
        validation_seeds=(23001,),
        test_seeds=(33001,),
    )
    return generate_procedural_split(config, split)


def test_square_root_wiener_target_is_exact_and_bounded() -> None:
    clean = np.array([0.0, 1.0, 3.0], dtype=np.float64)
    noise = np.array([0.0, 1.0, 4.0], dtype=np.float64)

    observed = square_root_wiener_target(clean, noise)

    np.testing.assert_allclose(
        observed,
        np.array(
            [
                0.0,
                np.sqrt(1.0 / (2.0 + NOISE_EPSILON)),
                np.sqrt(3.0 / (7.0 + NOISE_EPSILON)),
            ],
            dtype=np.float32,
        ),
        atol=1e-7,
        rtol=0,
    )
    assert observed.dtype == np.float32
    assert np.min(observed) >= 0.0
    assert np.max(observed) <= 1.0


def test_square_root_wiener_target_rejects_invalid_power() -> None:
    with pytest.raises(HybridError) as negative:
        square_root_wiener_target(
            np.array([-1.0], dtype=np.float64),
            np.array([1.0], dtype=np.float64),
        )
    assert negative.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as shape:
        square_root_wiener_target(
            np.ones(2, dtype=np.float64),
            np.ones(3, dtype=np.float64),
        )
    assert shape.value.code is ErrorCode.EVIDENCE_INVALID


def test_feature_target_pair_has_exact_shape_range_and_hash() -> None:
    item = next(item for item in _small_items() if item.category == "mixture")

    pair = build_feature_target_pair(
        item.mixture,
        item.clean,
        item.noise,
    )

    assert pair.features.shape == (10, ERF_BINS)
    assert pair.targets.shape == (10, ERF_BINS)
    assert pair.features.dtype == np.float32
    assert pair.targets.dtype == np.float32
    assert np.min(pair.features) >= -8.0
    assert np.max(pair.features) <= 8.0
    assert np.min(pair.targets) >= 0.0
    assert np.max(pair.targets) <= 1.0
    assert not pair.features.flags.writeable
    assert not pair.targets.flags.writeable
    expected_hash = hashlib.sha256(
        pair.features.astype("<f4", copy=False).tobytes()
        + pair.targets.astype("<f4", copy=False).tobytes()
    ).hexdigest()
    assert pair.payload_sha256 == expected_hash


def test_feature_target_pair_is_deterministic_and_does_not_mutate_audio() -> None:
    item = next(item for item in _small_items() if item.category == "mixture")
    mixture = item.mixture.copy()
    clean = item.clean.copy()
    noise = item.noise.copy()

    left = build_feature_target_pair(mixture, clean, noise)
    right = build_feature_target_pair(mixture, clean, noise)

    assert left.payload_sha256 == right.payload_sha256
    np.testing.assert_array_equal(left.features, right.features)
    np.testing.assert_array_equal(left.targets, right.targets)
    np.testing.assert_array_equal(mixture, item.mixture)
    np.testing.assert_array_equal(clean, item.clean)
    np.testing.assert_array_equal(noise, item.noise)


def test_feature_target_pair_rejects_misaligned_or_invalid_audio() -> None:
    base = np.zeros(HOP_SAMPLES * 2, dtype=np.float32)
    with pytest.raises(HybridError) as alignment:
        build_feature_target_pair(base, base, np.ones_like(base))
    assert alignment.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as length:
        build_feature_target_pair(base[:-1], base[:-1], base[:-1])
    assert length.value.code is ErrorCode.EVIDENCE_INVALID

    nonfinite = base.copy()
    nonfinite[0] = np.nan
    with pytest.raises(HybridError) as finite:
        build_feature_target_pair(nonfinite, base, base)
    assert finite.value.code is ErrorCode.EVIDENCE_INVALID


def test_balanced_store_has_exact_budget_unique_rows_and_groups() -> None:
    store = build_balanced_frame_store(
        _small_items(),
        max_frames=137,
        seed=0x57335645,
    )

    assert store.split == "train"
    assert store.features.shape == (137, ERF_BINS)
    assert store.targets.shape == (137, ERF_BINS)
    assert len(store.row_ids) == 137
    assert len(set(store.row_ids)) == 137
    assert sum(store.group_counts.values()) == 137
    assert max(store.group_counts.values()) - min(store.group_counts.values()) <= 1
    assert not store.features.flags.writeable
    assert not store.targets.flags.writeable


def test_balanced_store_is_deterministic_and_global_rng_independent() -> None:
    items = _small_items("validation")
    left = build_balanced_frame_store(items, max_frames=101, seed=0x1307)
    np.random.seed(9_999)
    right = build_balanced_frame_store(items, max_frames=101, seed=0x1307)

    assert left.payload_sha256 == right.payload_sha256
    assert left.row_ids == right.row_ids
    assert left.group_counts == right.group_counts
    np.testing.assert_array_equal(left.features, right.features)
    np.testing.assert_array_equal(left.targets, right.targets)


def test_balanced_store_refuses_mixed_splits_empty_and_bad_budget() -> None:
    train = _small_items("train")
    validation = _small_items("validation")
    with pytest.raises(HybridError) as mixed:
        build_balanced_frame_store(
            [train[0], validation[0]],
            max_frames=10,
            seed=1,
        )
    assert mixed.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as empty:
        build_balanced_frame_store([], max_frames=10, seed=1)
    assert empty.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as budget:
        build_balanced_frame_store(train, max_frames=0, seed=1)
    assert budget.value.code is ErrorCode.EVIDENCE_INVALID


def _training_store(
    split: str,
    *,
    seed: int,
    frames: int = 128,
) -> FrameStore:
    rng = np.random.Generator(np.random.PCG64(seed))
    features = rng.uniform(-3.0, 3.0, size=(frames, ERF_BINS)).astype(np.float32)
    frequency_bias = np.linspace(-0.5, 0.3, ERF_BINS, dtype=np.float32)
    targets = (
        0.5
        * (1.0 + np.tanh(np.float32(0.8) * features + frequency_bias[np.newaxis, :]))
    ).astype(np.float32)
    return FrameStore(
        split=split,
        features=features,
        targets=targets,
        row_ids=tuple(f"{split}/{index}" for index in range(frames)),
        group_counts={"synthetic": frames},
    )


def test_training_config_and_initial_parameters_are_exact() -> None:
    config = TrainingConfig()
    parameters = initialize_parameters()

    assert config.seed == 0x57335645
    assert config.batch_size == 512
    assert config.steps == 2_000
    assert config.learning_rate == 0.01
    assert config.validation_interval == 50
    assert parameters.alpha.shape == (ERF_BINS,)
    assert parameters.bias.shape == (ERF_BINS,)
    np.testing.assert_allclose(
        deployed_slopes(parameters.alpha),
        np.ones(ERF_BINS),
        atol=1e-7,
        rtol=0,
    )
    np.testing.assert_array_equal(
        parameters.bias,
        np.zeros(ERF_BINS, dtype=np.float64),
    )


def test_prediction_uses_quantized_deployment_path() -> None:
    features = np.array(
        [[-8.0, -1.0, 0.0, 1.0, 8.0] + [0.0] * (ERF_BINS - 5)],
        dtype=np.float32,
    )
    slopes = np.ones(ERF_BINS, dtype=np.float64)
    biases = np.zeros(ERF_BINS, dtype=np.float64)

    observed = predict_quantized_masks(features, slopes, biases)

    assert observed.shape == features.shape
    assert observed.dtype == np.float32
    np.testing.assert_array_equal(
        observed[0, :5],
        np.array(
            [0.05, 20 / 256, 0.5, 236 / 256, 255 / 256],
            dtype=np.float32,
        ),
    )


def test_loss_and_gradient_are_finite_exact_shape_and_regularized() -> None:
    store = _training_store("train", seed=1, frames=16)
    parameters = initialize_parameters()

    result = loss_and_gradient(
        store.features,
        store.targets,
        parameters.alpha,
        parameters.bias,
    )

    assert result.loss >= result.mean_squared_error >= 0.0
    assert np.isfinite(result.loss)
    assert result.alpha_gradient.shape == (ERF_BINS,)
    assert result.bias_gradient.shape == (ERF_BINS,)
    assert np.isfinite(result.alpha_gradient).all()
    assert np.isfinite(result.bias_gradient).all()
    assert result.predictions.shape == store.targets.shape


def test_adam_update_is_deterministic_and_advances_once() -> None:
    store = _training_store("train", seed=2, frames=16)
    config = TrainingConfig(steps=1, batch_size=16, validation_interval=1)
    state = initialize_adam(initialize_parameters())
    gradient = loss_and_gradient(
        store.features,
        store.targets,
        state.parameters.alpha,
        state.parameters.bias,
    )

    left = adam_update(state, gradient, config)
    right = adam_update(state, gradient, config)

    assert isinstance(left, AdamState)
    assert left.step == 1
    assert right.step == 1
    np.testing.assert_array_equal(
        left.parameters.alpha,
        right.parameters.alpha,
    )
    np.testing.assert_array_equal(left.parameters.bias, right.parameters.bias)
    np.testing.assert_array_equal(left.alpha_first_moment, right.alpha_first_moment)
    np.testing.assert_array_equal(left.bias_second_moment, right.bias_second_moment)


def test_checkpoint_selection_uses_earliest_exact_tie() -> None:
    records = (
        {"step": 0, "validation_loss": 0.5},
        {"step": 50, "validation_loss": 0.4},
        {"step": 100, "validation_loss": 0.4},
        {"step": 150, "validation_loss": 0.45},
    )

    assert select_best_checkpoint(records) == 50


def test_tiny_training_is_deterministic_and_beats_initialization() -> None:
    train = _training_store("train", seed=3)
    validation = _training_store("validation", seed=4, frames=64)
    config = TrainingConfig(
        seed=0x1308,
        batch_size=32,
        steps=120,
        learning_rate=0.03,
        validation_interval=20,
    )

    left = train_erf_mask(train, validation, config)
    right = train_erf_mask(train, validation, config)

    assert left.selected_validation_loss < left.initial_validation_loss
    assert left.selected_step > 0
    assert left.selected_step == right.selected_step
    assert left.history == right.history
    np.testing.assert_array_equal(left.slopes, right.slopes)
    np.testing.assert_array_equal(left.biases, right.biases)
    assert left.payload_sha256 == right.payload_sha256


def test_training_rejects_nan_wrong_shape_and_same_split() -> None:
    train = _training_store("train", seed=5, frames=16)
    validation = _training_store("validation", seed=6, frames=16)
    parameters = initialize_parameters()

    bad = train.features.copy()
    bad[0, 0] = np.nan
    with pytest.raises(HybridError) as nonfinite:
        loss_and_gradient(
            bad,
            train.targets,
            parameters.alpha,
            parameters.bias,
        )
    assert nonfinite.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as shape:
        predict_quantized_masks(
            train.features[:, :-1],
            np.ones(ERF_BINS, dtype=np.float64),
            np.zeros(ERF_BINS, dtype=np.float64),
        )
    assert shape.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as same:
        train_erf_mask(
            train,
            FrameStore(
                split="train",
                features=validation.features,
                targets=validation.targets,
                row_ids=validation.row_ids,
                group_counts=validation.group_counts,
            ),
            TrainingConfig(steps=1, batch_size=8, validation_interval=1),
        )
    assert same.value.code is ErrorCode.EVIDENCE_INVALID
