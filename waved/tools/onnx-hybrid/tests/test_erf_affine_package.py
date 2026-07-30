from __future__ import annotations

import copy
import hashlib

import numpy as np
import pytest

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.erf_affine_model import (
    build_affine_erf_onnx,
    quantization_candidates,
    quantize_affine_model,
    serialize_affine_config,
    serialize_quantized_affine,
)
from wave_onnx_hybrid.erf_affine_package import (
    build_unsealed_affine_manifest,
    verify_affine_package,
)
from wave_onnx_hybrid.erf_affine_qualification import (
    build_affine_qualification_vectors,
    qualification_vector_manifest,
    serialize_affine_qualification_vectors,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS, build_erf_u8_table
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _package() -> tuple[object, dict[str, bytes]]:
    slopes = np.linspace(0.1, 1.5, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.5, 0.5, ERF_BINS, dtype=np.float32)
    model = quantize_affine_model(
        slopes,
        biases,
        quantization_candidates()[1],
    )
    features = np.resize(
        np.linspace(-8.0, 8.0, ERF_BINS, dtype=np.float32),
        (8, ERF_BINS),
    )
    vectors = build_affine_qualification_vectors(model, features)
    artifacts = {
        "affine-weights.npz": serialize_quantized_affine(model),
        "affine-model.onnx": build_affine_erf_onnx(model),
        "affine-config.json": serialize_affine_config(model),
        "affine-vectors.bin": serialize_affine_qualification_vectors(
            vectors,
            model,
        ),
        "affine-vectors.json": canonical_json(
            qualification_vector_manifest(vectors, model)
        ),
        "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
        "evaluation.json": b'{"passed":true,"schema_version":1}\n',
        "source-model.json": b'{"schema_version":1,"source":"poc13a-v4"}\n',
    }
    return model, artifacts


def test_unsealed_affine_manifest_is_deterministic_and_cross_hashed() -> None:
    model, artifacts = _package()

    left = build_unsealed_affine_manifest(model, artifacts)
    right = build_unsealed_affine_manifest(
        model,
        dict(reversed(artifacts.items())),
    )

    assert canonical_json(left) == canonical_json(right)
    assert left["model_name"] == "wave-erf-affine-mask"
    assert left["execution_approved"] is False
    assert left["compiler_qualified"] is False
    assert left["partition"]["operations"] == ["Multiply", "Add", "Erf"]
    assert left["partition"]["candidate"] == "u8-per-channel"
    for name, payload in artifacts.items():
        assert left["artifacts"][name] == {
            "bytes": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }


def test_affine_package_verifies_and_rejects_every_artifact_change() -> None:
    model, artifacts = _package()
    manifest = build_unsealed_affine_manifest(model, artifacts)

    assert verify_affine_package(manifest, model, artifacts) == manifest
    for name in artifacts:
        changed = dict(artifacts)
        changed[name] += b"x"
        with pytest.raises(HybridError) as mismatch:
            verify_affine_package(manifest, model, changed)
        assert mismatch.value.code is ErrorCode.MODEL_WEIGHT_HASH_MISMATCH


def test_affine_package_rejects_contract_change_or_approval() -> None:
    model, artifacts = _package()
    manifest = build_unsealed_affine_manifest(model, artifacts)

    changed = copy.deepcopy(manifest)
    changed["partition"]["input"]["zero_point"] += 1
    with pytest.raises(HybridError) as contract:
        verify_affine_package(changed, model, artifacts)
    assert contract.value.code is ErrorCode.NBG_TENSOR_CONTRACT_MISMATCH

    approved = copy.deepcopy(manifest)
    approved["execution_approved"] = True
    with pytest.raises(HybridError) as execution:
        verify_affine_package(approved, model, artifacts)
    assert execution.value.code is ErrorCode.NBG_SIGNATURE_NOT_APPROVED

    unknown = copy.deepcopy(manifest)
    unknown["surprise"] = True
    with pytest.raises(HybridError) as root:
        verify_affine_package(unknown, model, artifacts)
    assert root.value.code is ErrorCode.MODEL_PACKAGE_INVALID
