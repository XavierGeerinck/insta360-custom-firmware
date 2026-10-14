"""Checkpoint, preprocessing and real network parity tests."""

import numpy as np
import pytest

from wave_onnx_hybrid.errors import HybridError

pytest.importorskip("torch")

from wave_onnx_hybrid.marblenet_model import (
    load_checkpoint,
    preprocess_audio,
)


def test_changed_checkpoint_refuses(tmp_path):
    path = tmp_path / "changed.nemo"
    path.write_bytes(b"not the pinned checkpoint")
    with pytest.raises(HybridError):
        load_checkpoint(path)


def test_wrong_audio_length_refuses():
    with pytest.raises(HybridError):
        preprocess_audio(np.zeros(64000, np.float32), {})


@pytest.fixture(scope="module")
def pinned_checkpoint():
    from pathlib import Path

    path = (
        Path(__file__).resolve().parents[4]
        / "artifacts/npu/poc18/checkpoint/frame_vad_multilingual_marblenet_v2.0.nemo"
    )
    if not path.is_file():
        pytest.skip("ignored pinned checkpoint not downloaded")
    return path


@pytest.fixture(scope="module")
def checkpoint_data(pinned_checkpoint):
    return load_checkpoint(pinned_checkpoint)


def test_all_state_tensors_accounted_and_changed_state_refuses(checkpoint_data):
    import torch

    from wave_onnx_hybrid.marblenet_model import validate_state_dict

    state = checkpoint_data["state"]
    assert len(state) == 84
    for mutation in ["missing", "extra", "dtype", "nonfinite", "negative-variance"]:
        bad = dict(state)
        if mutation == "missing":
            del bad["encoder.encoder.0.mconv.2.num_batches_tracked"]
        elif mutation == "extra":
            bad["unused"] = torch.zeros(1)
        elif mutation == "dtype":
            bad["decoder.layer0.weight"] = bad["decoder.layer0.weight"].half()
        elif mutation == "nonfinite":
            bad["decoder.layer0.bias"] = torch.full((2,), float("nan"))
        elif mutation == "negative-variance":
            bad["encoder.encoder.0.mconv.2.running_var"] = torch.full((128,), -1.0)
        with pytest.raises(HybridError):
            validate_state_dict(bad)


def test_exact_features_and_no_inference_dither(checkpoint_data):
    state = checkpoint_data["state"]
    wave = np.zeros(63840, np.float32)
    features = preprocess_audio(wave, state)
    assert features.shape == (1, 80, 400)
    np.testing.assert_array_equal(
        features, np.full((1, 80, 400), np.log(np.float32(2**-24)), np.float32)
    )
    np.testing.assert_array_equal(features, preprocess_audio(wave, state))


def test_unfused_bn_epsilon_and_no_intermediate_activation(checkpoint_data):
    import torch

    from wave_onnx_hybrid.marblenet_model import SourceMatchedMarbleNet

    model = SourceMatchedMarbleNet(checkpoint_data["state"])
    unit = model.blocks[0].units[0]
    assert unit.bn.eps == 0.001
    x = torch.from_numpy(
        np.random.default_rng(3).normal(size=(1, 80, 400)).astype(np.float32)
    )
    with torch.no_grad():
        dw = unit.depthwise(x)
        actual = unit(x)
        torch.testing.assert_close(actual, unit.bn(unit.pointwise(dw)), rtol=0, atol=0)
        incorrect = unit.bn(unit.pointwise(torch.relu(dw)))
    assert bool((dw < 0).any()) and float((actual - incorrect).abs().max()) > 0.02


def test_fold_and_classifier_parity_catch_wrong_epsilon(checkpoint_data):
    import torch

    from wave_onnx_hybrid.marblenet_model import FoldedMarbleNet, SourceMatchedMarbleNet

    state = checkpoint_data["state"]
    source = SourceMatchedMarbleNet(state)
    folded = FoldedMarbleNet(state)
    x = torch.from_numpy(preprocess_audio(np.zeros(63840, np.float32), state))
    with torch.no_grad():
        expected = source(x).transpose(1, 2)
        actual = folded(x)
        torch.testing.assert_close(actual, expected, rtol=1e-5, atol=1e-4)
        source.blocks[0].units[0].bn.eps = 1e-5
        wrong = source(x).transpose(1, 2)
    assert float((wrong - actual).abs().max()) > 1e-4


def test_actual_four_clip_preparation_and_all_cpu_gates(pinned_checkpoint, tmp_path):
    import hashlib
    import json

    from wave_onnx_hybrid.marblenet_model import prepare_marblenet

    speech = pinned_checkpoint.parents[1] / "audio/2086-149220-0033.wav"
    if not speech.is_file():
        pytest.skip("ignored public speech fixture not downloaded")
    output = tmp_path / "prepared"
    receipt = prepare_marblenet(pinned_checkpoint, output, speech)
    assert receipt["learned_parameters"] == 91378 and receipt["tensor_count"] == 84
    assert receipt["controls"]["18d"]["nodes"] == 33
    assert set(receipt["clips"]) == {"silence", "noise", "speech", "noisy-speech"}
    for clip in receipt["clips"].values():
        assert all(metric["status"] == "PASS" for metric in clip["parity"].values())
    for name, spec in receipt["files"].items():
        assert (
            hashlib.sha256((output / name).read_bytes()).hexdigest() == spec["sha256"]
        )
    assert (
        json.loads((output / "preparation.json").read_text())["reference_kind"]
        == "source-matched-reconstruction"
    )
    with pytest.raises(FileExistsError):
        prepare_marblenet(pinned_checkpoint, output, speech)
