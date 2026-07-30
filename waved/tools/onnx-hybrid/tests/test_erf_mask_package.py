from __future__ import annotations

import copy
import hashlib
import json

import numpy as np
import pytest

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.erf_mask_model import (
    build_erf_mask_onnx,
    serialize_erf_mask_weights,
)
from wave_onnx_hybrid.erf_mask_package import (
    build_unsealed_model_manifest,
    verify_model_package,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS, build_erf_u8_table
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _artifacts() -> dict[str, bytes]:
    slopes = np.linspace(0.5, 1.5, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.3, 0.3, ERF_BINS, dtype=np.float32)
    return {
        "weights.npz": serialize_erf_mask_weights(slopes, biases),
        "model.onnx": build_erf_mask_onnx(slopes, biases),
        "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
        "training-manifest.json": b'{"schema_version":1,"run":"test"}\n',
        "corpus-manifest.json": b'{"schema_version":1,"corpus":"test"}\n',
        "config.json": b'{"schema_version":1,"config":"test"}\n',
    }


def test_unsealed_manifest_is_deterministic_and_hashes_every_artifact() -> None:
    artifacts = _artifacts()

    left = build_unsealed_model_manifest(artifacts)
    right = build_unsealed_model_manifest(dict(reversed(artifacts.items())))

    assert canonical_json(left) == canonical_json(right)
    assert left["schema_version"] == 1
    assert left["model_name"] == "wave-erf-mask"
    assert left["execution_approved"] is False
    assert left["compiler_qualified"] is False
    assert left["npu_partition"]["nbg"] is None
    assert left["npu_partition"]["numerical_evidence_sha256"] is None
    assert set(left["artifacts"]) == set(artifacts)
    for name, payload in artifacts.items():
        assert left["artifacts"][name] == {
            "bytes": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }


def test_unsealed_package_verifies_and_rejects_unknown_or_missing_key() -> None:
    artifacts = _artifacts()
    manifest = build_unsealed_model_manifest(artifacts)

    verified = verify_model_package(manifest, artifacts)

    assert verified["execution_approved"] is False
    unknown = copy.deepcopy(manifest)
    unknown["surprise"] = True
    with pytest.raises(HybridError) as extra:
        verify_model_package(unknown, artifacts)
    assert extra.value.code is ErrorCode.MODEL_PACKAGE_INVALID

    missing = copy.deepcopy(manifest)
    missing.pop("audio_contract")
    with pytest.raises(HybridError) as absent:
        verify_model_package(missing, artifacts)
    assert absent.value.code is ErrorCode.MODEL_PACKAGE_INVALID


@pytest.mark.parametrize(
    "artifact_name",
    [
        "weights.npz",
        "model.onnx",
        "erf-u8-lut.bin",
        "training-manifest.json",
        "corpus-manifest.json",
        "config.json",
    ],
)
def test_package_rejects_every_artifact_change(artifact_name: str) -> None:
    artifacts = _artifacts()
    manifest = build_unsealed_model_manifest(artifacts)
    changed = dict(artifacts)
    changed[artifact_name] += b"x"

    with pytest.raises(HybridError) as mismatch:
        verify_model_package(manifest, changed)
    assert mismatch.value.code is ErrorCode.MODEL_WEIGHT_HASH_MISMATCH


def test_package_cannot_approve_execution_without_nbg_and_numerical_evidence() -> None:
    artifacts = _artifacts()
    manifest = build_unsealed_model_manifest(artifacts)
    manifest["execution_approved"] = True

    with pytest.raises(HybridError) as approval:
        verify_model_package(manifest, artifacts)
    assert approval.value.code is ErrorCode.NBG_SIGNATURE_NOT_APPROVED


def test_package_rejects_noncanonical_manifest_value() -> None:
    artifacts = _artifacts()
    manifest = build_unsealed_model_manifest(artifacts)
    manifest["audio_contract"]["sample_rate"] = 16_000

    with pytest.raises(HybridError) as changed:
        verify_model_package(manifest, artifacts)
    assert changed.value.code is ErrorCode.MODEL_PACKAGE_INVALID


def test_manifest_round_trips_through_strict_json_value() -> None:
    artifacts = _artifacts()
    expected = build_unsealed_model_manifest(artifacts)
    payload = canonical_json(expected)

    observed = json.loads(payload)

    assert verify_model_package(observed, artifacts) == expected
