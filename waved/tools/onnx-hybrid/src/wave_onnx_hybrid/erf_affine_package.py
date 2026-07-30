"""Strict unsealed package for the PoC 14 affine-plus-Erf target."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from typing import Any

from .audit import SHA256_RE, canonical_json
from .erf_affine_model import (
    QuantizedAffineModel,
    build_affine_erf_onnx,
    serialize_affine_config,
    serialize_quantized_affine,
)
from .erf_affine_qualification import (
    parse_affine_qualification_vectors,
    verify_qualification_vector_manifest,
)
from .erf_mask_signal import build_erf_u8_table
from .errors import ErrorCode, HybridError

AFFINE_ARTIFACTS = frozenset(
    {
        "affine-weights.npz",
        "affine-model.onnx",
        "affine-config.json",
        "affine-vectors.bin",
        "affine-vectors.json",
        "erf-u8-lut.bin",
        "evaluation.json",
        "source-model.json",
    }
)
ROOT_KEYS = frozenset(
    {
        "schema_version",
        "model_name",
        "semantic_version",
        "execution_approved",
        "compiler_qualified",
        "partition",
        "artifacts",
    }
)


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.MODEL_PACKAGE_INVALID, message)


def _require_artifacts(
    model: QuantizedAffineModel,
    artifacts: Mapping[str, bytes],
) -> dict[str, bytes]:
    if not isinstance(artifacts, Mapping) or set(artifacts) != AFFINE_ARTIFACTS:
        raise _invalid("affine package has an invalid artifact set")
    if any(
        type(name) is not str or type(payload) is not bytes
        for name, payload in artifacts.items()
    ):
        raise _invalid("affine package artifacts must be byte payloads")
    result = dict(artifacts)
    expected = {
        "affine-weights.npz": serialize_quantized_affine(model),
        "affine-model.onnx": build_affine_erf_onnx(model),
        "affine-config.json": serialize_affine_config(model),
        "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
    }
    for name, payload in expected.items():
        if result[name] != payload:
            raise HybridError(
                ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
                f"affine artifact {name} violates the frozen model",
            )
    try:
        vector_document = json.loads(result["affine-vectors.json"])
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise _invalid("affine artifact affine-vectors.json is not JSON") from exc
    vector_inputs, vector_outputs = parse_affine_qualification_vectors(
        result["affine-vectors.bin"],
        model,
    )
    verify_qualification_vector_manifest(
        vector_document,
        vector_inputs,
        vector_outputs,
        model,
    )
    for name in ("evaluation.json", "source-model.json"):
        try:
            document = json.loads(result[name])
        except (UnicodeError, json.JSONDecodeError) as exc:
            raise _invalid(f"affine artifact {name} is not JSON") from exc
        if not isinstance(document, dict) or document.get("schema_version") != 1:
            raise _invalid(f"affine artifact {name} has the wrong schema")
    return result


def _artifact_document(
    artifacts: Mapping[str, bytes],
) -> dict[str, dict[str, int | str]]:
    return {
        name: {
            "bytes": len(artifacts[name]),
            "sha256": hashlib.sha256(artifacts[name]).hexdigest(),
        }
        for name in sorted(artifacts)
    }


def _partition(model: QuantizedAffineModel) -> dict[str, Any]:
    candidate = model.candidate
    return {
        "signature": model.contract_sha256,
        "candidate": candidate.name,
        "operations": ["Multiply", "Add", "Erf"],
        "input": {
            "shape": [481],
            "dtype": candidate.feature_dtype,
            "scale": candidate.feature_scale,
            "zero_point": candidate.feature_zero_point,
        },
        "output": {
            "shape": [481],
            "dtype": "UINT8",
            "scale": 0.0078125,
            "zero_point": 128,
        },
        "quantized_model": model.to_document(),
        "nbg": None,
        "numerical_evidence_sha256": None,
    }


def build_unsealed_affine_manifest(
    model: QuantizedAffineModel,
    artifacts: Mapping[str, bytes],
) -> dict[str, Any]:
    values = _require_artifacts(model, artifacts)
    return {
        "schema_version": 1,
        "model_name": "wave-erf-affine-mask",
        "semantic_version": "0.1.0",
        "execution_approved": False,
        "compiler_qualified": False,
        "partition": _partition(model),
        "artifacts": _artifact_document(values),
    }


def _strict_manifest(manifest: Mapping[str, Any]) -> dict[str, Any]:
    if (
        not isinstance(manifest, Mapping)
        or set(manifest) != ROOT_KEYS
        or any(type(key) is not str for key in manifest)
    ):
        raise _invalid("affine manifest has unknown or missing root keys")
    try:
        document = json.loads(canonical_json(manifest))
    except (HybridError, TypeError, ValueError) as exc:
        raise _invalid("affine manifest is not strict JSON") from exc
    if not isinstance(document, dict):
        raise _invalid("affine manifest must be an object")
    return document


def _verify_hashes(
    records: Any,
    artifacts: Mapping[str, bytes],
) -> None:
    if not isinstance(records, dict) or set(records) != AFFINE_ARTIFACTS:
        raise _invalid("affine artifact inventory differs")
    for name in sorted(AFFINE_ARTIFACTS):
        record = records[name]
        if (
            not isinstance(record, dict)
            or set(record) != {"bytes", "sha256"}
            or type(record["bytes"]) is not int
            or record["bytes"] < 0
            or type(record["sha256"]) is not str
            or SHA256_RE.fullmatch(record["sha256"]) is None
        ):
            raise _invalid(f"invalid affine artifact record: {name}")
        payload = artifacts[name]
        if (
            record["bytes"] != len(payload)
            or record["sha256"] != hashlib.sha256(payload).hexdigest()
        ):
            raise HybridError(
                ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
                f"affine artifact changed: {name}",
            )


def verify_affine_package(
    manifest: Mapping[str, Any],
    model: QuantizedAffineModel,
    artifacts: Mapping[str, bytes],
) -> dict[str, Any]:
    document = _strict_manifest(manifest)
    if not isinstance(artifacts, Mapping) or set(artifacts) != AFFINE_ARTIFACTS:
        raise _invalid("affine package has an invalid artifact set")
    if any(
        type(name) is not str or type(payload) is not bytes
        for name, payload in artifacts.items()
    ):
        raise _invalid("affine package artifacts must be byte payloads")
    values = dict(artifacts)
    _verify_hashes(document["artifacts"], values)
    values = _require_artifacts(model, values)
    expected = build_unsealed_affine_manifest(model, values)
    for key in ROOT_KEYS - {"partition", "execution_approved"}:
        if document[key] != expected[key]:
            raise _invalid(f"affine manifest field differs: {key}")
    partition = document["partition"]
    if not isinstance(partition, dict):
        raise _invalid("affine partition must be an object")
    expected_partition = expected["partition"]
    contract_keys = set(expected_partition) - {
        "nbg",
        "numerical_evidence_sha256",
    }
    if any(partition.get(key) != expected_partition[key] for key in contract_keys):
        raise HybridError(
            ErrorCode.NBG_TENSOR_CONTRACT_MISMATCH,
            "affine partition differs from the frozen tensor contract",
        )
    if set(partition) != set(expected_partition):
        raise _invalid("affine partition keys differ")
    if type(document["execution_approved"]) is not bool:
        raise _invalid("execution_approved must be boolean")
    if document["execution_approved"]:
        raise HybridError(
            ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
            "affine execution remains blocked before serializer qualification",
        )
    if (
        partition["nbg"] is not None
        or partition["numerical_evidence_sha256"] is not None
    ):
        raise HybridError(
            ErrorCode.NBG_SIGNATURE_NOT_APPROVED,
            "unapproved affine package cannot carry executable evidence",
        )
    return document
