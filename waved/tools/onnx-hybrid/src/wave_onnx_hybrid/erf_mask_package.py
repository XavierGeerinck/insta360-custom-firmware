"""Canonical package manifest for the learned Erf-mask denoiser."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from typing import Any

from .audit import SHA256_RE, canonical_json
from .erf_mask_model import ErfPartitionContract, erf_partition_signature
from .erf_mask_signal import (
    ANALYSIS_SAMPLES,
    ERF_BINS,
    HOP_SAMPLES,
    MASK_FLOOR,
    NOISE_EPSILON,
    SAMPLE_RATE,
    build_erf_u8_table,
)
from .errors import ErrorCode, HybridError

MODEL_ARTIFACTS = frozenset(
    {
        "weights.npz",
        "model.onnx",
        "erf-u8-lut.bin",
        "training-manifest.json",
        "corpus-manifest.json",
        "config.json",
    }
)
ROOT_KEYS = frozenset(
    {
        "schema_version",
        "model_name",
        "semantic_version",
        "execution_approved",
        "compiler_qualified",
        "audio_contract",
        "noise_estimator",
        "cpu_oracle",
        "npu_partition",
        "artifacts",
    }
)


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.MODEL_PACKAGE_INVALID, message)


def _require_artifacts(artifacts: Mapping[str, bytes]) -> dict[str, bytes]:
    if not isinstance(artifacts, Mapping):
        raise _invalid("artifacts must be a mapping")
    if set(artifacts) != MODEL_ARTIFACTS or not all(
        type(name) is str and type(payload) is bytes
        for name, payload in artifacts.items()
    ):
        raise _invalid("model package has an invalid artifact set")
    result = dict(artifacts)
    expected_lut = build_erf_u8_table().tobytes()
    if result["erf-u8-lut.bin"] != expected_lut:
        raise HybridError(
            ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
            "Erf oracle LUT does not match the fixed model contract",
        )
    return result


def _artifact_document(artifacts: Mapping[str, bytes]) -> dict[str, Any]:
    return {
        name: {
            "bytes": len(artifacts[name]),
            "sha256": hashlib.sha256(artifacts[name]).hexdigest(),
        }
        for name in sorted(artifacts)
    }


def _audio_contract() -> dict[str, Any]:
    return {
        "sample_rate": SAMPLE_RATE,
        "channels": 1,
        "hop_samples": HOP_SAMPLES,
        "analysis_samples": ANALYSIS_SAMPLES,
        "fft_samples": ANALYSIS_SAMPLES,
        "frequency_bins": ERF_BINS,
        "window": "periodic-sqrt-hann",
        "output_delay_hops": 1,
        "sample_dtype": "FLOAT32",
    }


def _noise_estimator_contract() -> dict[str, Any]:
    return {
        "algorithm": "causal-smoothed-minimum-statistics",
        "state_version": 1,
        "power_smoothing": {"previous": 0.8, "current": 0.2},
        "minimum_frames": 100,
        "candidate_multiplier": 1.5,
        "noise_dominant_threshold": 3.0,
        "noise_update": {"previous": 0.9, "current": 0.1},
        "speech_update": {"previous": 0.995, "current": 0.005},
        "epsilon": NOISE_EPSILON,
        "feature": "log-posterior-snr",
        "feature_clip": [-8.0, 8.0],
    }


def _npu_partition_document() -> dict[str, Any]:
    signature = erf_partition_signature(ErfPartitionContract())
    return {
        "signature": signature.sha256,
        "contract": signature.document,
        "nbg": None,
        "numerical_evidence_sha256": None,
    }


def build_unsealed_model_manifest(
    artifacts: Mapping[str, bytes],
) -> dict[str, Any]:
    """Build a deterministic manifest that is explicitly not executable."""

    values = _require_artifacts(artifacts)
    return {
        "schema_version": 1,
        "model_name": "wave-erf-mask",
        "semantic_version": "0.1.0",
        "execution_approved": False,
        "compiler_qualified": False,
        "audio_contract": _audio_contract(),
        "noise_estimator": _noise_estimator_contract(),
        "cpu_oracle": {
            "implementation": "wave-onnx-hybrid.erf-u8-lut",
            "version": 1,
            "rounding": "round-to-nearest-ties-to-even",
            "mask_floor": float(MASK_FLOOR),
        },
        "npu_partition": _npu_partition_document(),
        "artifacts": _artifact_document(values),
    }


def _require_manifest_mapping(manifest: Mapping[str, Any]) -> dict[str, Any]:
    if not isinstance(manifest, Mapping) or any(
        type(key) is not str for key in manifest
    ):
        raise _invalid("model manifest must be a string-keyed mapping")
    if set(manifest) != ROOT_KEYS:
        raise _invalid("model manifest has unknown or missing root keys")
    try:
        payload = canonical_json(manifest)
        value = json.loads(payload)
    except (HybridError, TypeError, ValueError) as exc:
        raise _invalid("model manifest is not a strict JSON value") from exc
    if not isinstance(value, dict):
        raise _invalid("model manifest must be a JSON object")
    return value


def _verify_artifact_hashes(
    manifest_artifacts: Any,
    artifacts: Mapping[str, bytes],
) -> None:
    if not isinstance(manifest_artifacts, dict):
        raise _invalid("manifest artifacts must be an object")
    if set(manifest_artifacts) != MODEL_ARTIFACTS:
        raise _invalid("manifest artifact inventory is incomplete")
    for name in sorted(MODEL_ARTIFACTS):
        entry = manifest_artifacts[name]
        if (
            not isinstance(entry, dict)
            or set(entry) != {"bytes", "sha256"}
            or type(entry["bytes"]) is not int
            or entry["bytes"] < 0
            or type(entry["sha256"]) is not str
            or SHA256_RE.fullmatch(entry["sha256"]) is None
        ):
            raise _invalid(f"invalid artifact record for {name}")
        payload = artifacts[name]
        if (
            entry["bytes"] != len(payload)
            or entry["sha256"] != hashlib.sha256(payload).hexdigest()
        ):
            raise HybridError(
                ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
                f"artifact {name} does not match its sealed digest",
            )


def verify_model_package(
    manifest: Mapping[str, Any],
    artifacts: Mapping[str, bytes],
) -> dict[str, Any]:
    """Fail closed on any manifest, artifact, or approval discrepancy."""

    document = _require_manifest_mapping(manifest)
    values = _require_artifacts(artifacts)
    _verify_artifact_hashes(document["artifacts"], values)

    expected = build_unsealed_model_manifest(values)
    for key in ROOT_KEYS - {"execution_approved", "npu_partition"}:
        if document[key] != expected[key]:
            raise _invalid(f"manifest field {key} violates the model contract")

    if type(document["execution_approved"]) is not bool:
        raise _invalid("execution_approved must be boolean")
    partition = document["npu_partition"]
    if not isinstance(partition, dict) or set(partition) != {
        "signature",
        "contract",
        "nbg",
        "numerical_evidence_sha256",
    }:
        raise _invalid("invalid NPU partition document")
    expected_partition = expected["npu_partition"]
    if (
        partition["signature"] != expected_partition["signature"]
        or partition["contract"] != expected_partition["contract"]
    ):
        raise HybridError(
            ErrorCode.NBG_TENSOR_CONTRACT_MISMATCH,
            "NPU partition signature does not match the fixed tensor contract",
        )

    if document["execution_approved"]:
        if (
            partition["nbg"] is None
            or type(partition["numerical_evidence_sha256"]) is not str
            or SHA256_RE.fullmatch(partition["numerical_evidence_sha256"]) is None
        ):
            raise HybridError(
                ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
                "execution requires a sealed NBG and numerical evidence",
            )
        raise HybridError(
            ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
            "sealed-NBG verification is not available before PoC 13C",
        )
    if (
        partition["nbg"] is not None
        or partition["numerical_evidence_sha256"] is not None
    ):
        raise HybridError(
            ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
            "unapproved package cannot carry executable NBG evidence",
        )
    return document
