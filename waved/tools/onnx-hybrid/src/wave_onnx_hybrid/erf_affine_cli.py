"""Host-only builder and verifier for the affine-plus-Erf partition."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any

import numpy as np

from .audit import canonical_json
from .erf_affine_model import (
    QuantizedAffineModel,
    affine_erf_output_u8,
    build_affine_erf_onnx,
    deserialize_quantized_affine,
    quantize_features,
    select_affine_candidate,
    serialize_affine_config,
    serialize_native_affine_constants,
    serialize_quantized_affine,
)
from .erf_affine_package import (
    AFFINE_ARTIFACTS,
    build_unsealed_affine_manifest,
    verify_affine_package,
)
from .erf_affine_qualification import (
    AffineQualificationVectors,
    build_affine_qualification_vectors,
    qualification_vector_manifest,
    serialize_affine_qualification_vectors,
)
from .erf_mask_cli import (
    _atomic_directory,
    _config_from_corpus_manifest,
    _evaluation_items,
    _hash_path,
    _load_frame_store,
    _load_json,
    _load_model_package,
    _write_exclusive,
)
from .erf_mask_signal import build_erf_u8_table
from .erf_mask_stream import (
    DFN_ATTENUATION_LIMIT_DB,
    evaluate_dfn_erf_stream,
    verify_evaluation_stream,
)
from .errors import ErrorCode, HybridError


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


class _StrictParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise HybridError(ErrorCode.ONNX_INPUT_INVALID, message)


def _affine_feature_partition(
    model: QuantizedAffineModel,
) -> Callable[[np.ndarray], np.ndarray]:
    """Return the independent integer CPU oracle at the feature boundary."""

    def execute(feature: np.ndarray) -> np.ndarray:
        quantized = quantize_features(
            np.asarray(feature)[np.newaxis, :],
            model.candidate,
        )
        return np.ascontiguousarray(
            affine_erf_output_u8(quantized, model)[0],
            dtype=np.uint8,
        )

    return execute


def _build_affine_package_payloads(
    model: QuantizedAffineModel,
    vectors: AffineQualificationVectors,
    evaluation: Mapping[str, Any],
    source: Mapping[str, Any],
) -> tuple[dict[str, bytes], dict[str, Any]]:
    if (
        not isinstance(evaluation, Mapping)
        or evaluation.get("schema_version") != 1
        or evaluation.get("passed") is not True
        or evaluation.get("private_inputs_used") is not False
        or not isinstance(evaluation.get("affine"), dict)
        or evaluation["affine"].get("model_contract_sha256") != model.contract_sha256
        or evaluation["affine"].get("selected_candidate") != model.candidate.name
    ):
        raise _invalid("affine evaluation has not passed the frozen contract")
    if (
        not isinstance(source, Mapping)
        or source.get("schema_version") != 1
        or source.get("private_inputs_used") is not False
    ):
        raise _invalid("affine source evidence is invalid")
    vector_manifest = qualification_vector_manifest(vectors, model)
    artifacts = {
        "affine-weights.npz": serialize_quantized_affine(model),
        "affine-model.onnx": build_affine_erf_onnx(model),
        "affine-config.json": serialize_affine_config(model),
        "affine-vectors.bin": serialize_affine_qualification_vectors(
            vectors,
            model,
        ),
        "affine-vectors.json": canonical_json(vector_manifest),
        "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
        "evaluation.json": canonical_json(evaluation),
        "source-model.json": canonical_json(source),
    }
    manifest = build_unsealed_affine_manifest(model, artifacts)
    verify_affine_package(manifest, model, artifacts)
    return artifacts, manifest


def _frame_store_evidence(
    examples: Path,
    corpus_manifest: Mapping[str, Any],
    split: str,
) -> tuple[Any, dict[str, Any]]:
    store = _load_frame_store(examples, split)
    records = corpus_manifest.get("frame_stores")
    if not isinstance(records, dict) or not isinstance(records.get(split), dict):
        raise _invalid(f"source corpus lacks {split} frame-store evidence")
    expected = records[split]
    tensor_path = examples / f"{split}.frames.npz"
    metadata_path = examples / f"{split}.frames.json"
    observed = {
        "frames": int(store.features.shape[0]),
        "payload_sha256": store.payload_sha256,
        "tensor_archive_sha256": _hash_path(tensor_path),
        "metadata_sha256": _hash_path(metadata_path),
    }
    if observed != expected:
        raise _invalid(f"{split} frame store differs from the source model")
    return store, observed


def _source_evidence(
    args: argparse.Namespace,
    source_manifest: Mapping[str, Any],
    source_artifacts: Mapping[str, bytes],
    corpus_manifest: Mapping[str, Any],
    frame_stores: Mapping[str, Any],
    stream_manifest: Mapping[str, Any],
) -> tuple[dict[str, Any], dict[str, Any]]:
    for label, path in (
        ("source evaluation", args.source_evaluation),
        ("DFN run evidence", args.dfn_run),
        ("DFN output", args.dfn_output),
    ):
        try:
            path.lstat()
        except OSError as exc:
            raise _invalid(f"cannot inspect {label}: {exc}") from exc
        if path.is_symlink() or not path.is_file():
            raise _invalid(f"{label} must be a regular nonsymlink file")
    source_evaluation = _load_json(args.source_evaluation)
    dfn_run = _load_json(args.dfn_run)
    source_model_hash = _hash_path(args.model / "model-manifest.json")
    slr31_hash = _hash_path(args.slr31 / "slr31-manifest.json")
    dfn_run_hash = _hash_path(args.dfn_run)
    if (
        source_evaluation.get("schema_version") != 1
        or source_evaluation.get("passed") is not True
        or source_evaluation.get("private_inputs_used") is not False
        or source_evaluation.get("model_manifest_sha256") != source_model_hash
        or source_evaluation.get("slr31_manifest_sha256") != slr31_hash
        or source_evaluation.get("evaluation_stream_sha256")
        != stream_manifest["raw_sha256"]
        or not isinstance(source_evaluation.get("dfn"), dict)
        or source_evaluation["dfn"].get("run_evidence_sha256") != dfn_run_hash
        or source_evaluation["dfn"].get("output_bytes")
        != args.dfn_output.stat().st_size
        or dfn_run
        != {
            "attenuation_limit_db": DFN_ATTENUATION_LIMIT_DB,
            "backend": "deep_filter_cpu",
            "hop_samples": stream_manifest["hop_samples"],
            "samples": stream_manifest["total_samples"],
        }
        or corpus_manifest.get("slr31_manifest_sha256") != slr31_hash
        or corpus_manifest.get("private_inputs_used") is not False
    ):
        raise _invalid("source model, corpus, stream, or DFN evidence differs")
    artifact_evidence = {
        name: {
            "bytes": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }
        for name, payload in sorted(source_artifacts.items())
    }
    source = {
        "schema_version": 1,
        "private_inputs_used": False,
        "source_model": {
            "manifest_sha256": source_model_hash,
            "model_name": source_manifest["model_name"],
            "semantic_version": source_manifest["semantic_version"],
            "artifacts": artifact_evidence,
        },
        "source_evaluation": {
            "sha256": _hash_path(args.source_evaluation),
            "item_count": source_evaluation["item_count"],
            "items_sha256": source_evaluation["items_sha256"],
            "summary": source_evaluation["summary"],
        },
        "frame_stores": dict(frame_stores),
        "slr31_manifest_sha256": slr31_hash,
        "evaluation_stream": {
            "bytes": stream_manifest["raw_bytes"],
            "sha256": stream_manifest["raw_sha256"],
            "item_count": stream_manifest["item_count"],
        },
        "dfn": {
            "binary_sha256": source_evaluation["dfn"]["binary_sha256"],
            "output_bytes": source_evaluation["dfn"]["output_bytes"],
            "output_sha256": source_evaluation["dfn"]["output_sha256"],
            "run_evidence_sha256": dfn_run_hash,
        },
    }
    return source, source_evaluation


def _command_build_host(args: argparse.Namespace) -> int:
    print("loading and verifying the frozen PoC 13A source model", flush=True)
    source_manifest, source_artifacts, slopes, biases = _load_model_package(args.model)
    try:
        corpus_manifest = json.loads(source_artifacts["corpus-manifest.json"])
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise _invalid("source corpus manifest is not JSON") from exc
    if not isinstance(corpus_manifest, dict):
        raise _invalid("source corpus manifest must be an object")
    validation, validation_evidence = _frame_store_evidence(
        args.examples,
        corpus_manifest,
        "validation",
    )
    test, test_evidence = _frame_store_evidence(
        args.examples,
        corpus_manifest,
        "test",
    )
    model, validation_report = select_affine_candidate(
        validation.features,
        slopes,
        biases,
    )
    test_model, test_report = select_affine_candidate(
        test.features,
        slopes,
        biases,
    )
    if (
        model.candidate.name != test_model.candidate.name
        or model.contract_sha256 != test_model.contract_sha256
    ):
        raise _invalid("held-out test does not confirm the validation candidate")
    vectors = build_affine_qualification_vectors(model, validation.features)
    print(
        f"froze {model.candidate.name} contract {model.contract_sha256}",
        flush=True,
    )

    stream_document = _load_json(args.stream / "stream-manifest.json")
    stream_manifest = verify_evaluation_stream(
        stream_document,
        args.stream / "evaluation-input.f32",
    )
    frame_stores = {
        "validation": validation_evidence,
        "test": test_evidence,
    }
    source, source_evaluation = _source_evidence(
        args,
        source_manifest,
        source_artifacts,
        corpus_manifest,
        frame_stores,
        stream_manifest,
    )
    config = _config_from_corpus_manifest(corpus_manifest)
    print(
        "running the complete quantized affine-plus-Erf oracle over "
        f"{stream_manifest['item_count']} held-out items",
        flush=True,
    )
    result = evaluate_dfn_erf_stream(
        _evaluation_items(config, args.slr31),
        stream_manifest,
        args.dfn_output,
        slopes,
        biases,
        feature_partition=_affine_feature_partition(model),
    )
    result["private_inputs_used"] = False
    result["source_model_manifest_sha256"] = source["source_model"]["manifest_sha256"]
    result["source_evaluation_sha256"] = source["source_evaluation"]["sha256"]
    result["slr31_manifest_sha256"] = source["slr31_manifest_sha256"]
    result["dfn"]["binary_sha256"] = source["dfn"]["binary_sha256"]
    result["dfn"]["run_evidence_sha256"] = source["dfn"]["run_evidence_sha256"]
    result["affine"] = {
        "target": "quantized-feature-multiply-add-requantize-erf-u8",
        "selected_candidate": model.candidate.name,
        "model_contract_sha256": model.contract_sha256,
        "selection_split": "validation",
        "validation_candidate_report": validation_report,
        "test_role": "confirmation-only",
        "test_candidate_report": test_report,
        "integer_oracle": (
            "centered-feature-times-int8-weight-plus-int32-bias;"
            "float32-fixed-multiplier-requantization;"
            "qualified-u8-erf-table"
        ),
        "vector_count": len(vectors.inputs),
    }
    if (
        result.get("schema_version") != 1
        or result.get("passed") is not True
        or result.get("invalid_item_count") != 0
        or result["dfn"]["output_sha256"] != source_evaluation["dfn"]["output_sha256"]
    ):
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "affine-plus-Erf held-out quality gates did not pass",
            details={"summary": result.get("summary")},
        )

    first_artifacts, first_manifest = _build_affine_package_payloads(
        model,
        vectors,
        result,
        source,
    )
    second_artifacts, second_manifest = _build_affine_package_payloads(
        model,
        vectors,
        result,
        source,
    )
    if first_artifacts != second_artifacts or canonical_json(
        first_manifest
    ) != canonical_json(second_manifest):
        raise _invalid("affine package regeneration is not deterministic")

    def build(root: Path) -> None:
        for name in sorted(AFFINE_ARTIFACTS):
            _write_exclusive(root / name, first_artifacts[name])
        _write_exclusive(
            root / "model-manifest.json",
            canonical_json(first_manifest),
        )

    _atomic_directory(args.output, build)
    print(f"built unsealed affine host package at {args.output}", flush=True)
    return 0


def _load_affine_package(
    root: Path,
) -> tuple[QuantizedAffineModel, dict[str, bytes], dict[str, Any]]:
    try:
        artifacts = {
            name: (root / name).read_bytes() for name in sorted(AFFINE_ARTIFACTS)
        }
    except OSError as exc:
        raise _invalid(f"cannot read affine package artifact: {exc}") from exc
    model = deserialize_quantized_affine(
        artifacts["affine-config.json"],
        artifacts["affine-weights.npz"],
    )
    manifest = _load_json(root / "model-manifest.json")
    verified = verify_affine_package(manifest, model, artifacts)
    return model, artifacts, verified


def _command_verify_host(args: argparse.Namespace) -> int:
    model, _, manifest = _load_affine_package(args.package)
    print(
        json.dumps(
            {
                "candidate": model.candidate.name,
                "compiler_qualified": manifest["compiler_qualified"],
                "contract_sha256": model.contract_sha256,
                "execution_approved": manifest["execution_approved"],
                "verified": True,
            },
            sort_keys=True,
        )
    )
    return 0


def _command_export_native_constants(args: argparse.Namespace) -> int:
    model, _, _ = _load_affine_package(args.package)
    _write_exclusive(
        args.output,
        serialize_native_affine_constants(model),
    )
    print(f"exported native affine constants to {args.output}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = _StrictParser(
        description="Host-only affine-plus-Erf qualification pipeline"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    build = subparsers.add_parser("build-host")
    build.add_argument("--model", required=True, type=Path)
    build.add_argument("--examples", required=True, type=Path)
    build.add_argument("--source-evaluation", required=True, type=Path)
    build.add_argument("--slr31", required=True, type=Path)
    build.add_argument("--stream", required=True, type=Path)
    build.add_argument("--dfn-output", required=True, type=Path)
    build.add_argument("--dfn-run", required=True, type=Path)
    build.add_argument("--output", required=True, type=Path)
    build.set_defaults(handler=_command_build_host)
    verify = subparsers.add_parser("verify-host")
    verify.add_argument("--package", required=True, type=Path)
    verify.set_defaults(handler=_command_verify_host)
    native = subparsers.add_parser("export-native-constants")
    native.add_argument("--package", required=True, type=Path)
    native.add_argument("--output", required=True, type=Path)
    native.set_defaults(handler=_command_export_native_constants)
    return parser


def _exit_code(code: ErrorCode) -> int:
    if code is ErrorCode.OUTPUT_EXISTS:
        return 3
    if code is ErrorCode.AUDIO_QUALITY_GATE_FAILURE:
        return 4
    return 2


def main(argv: Sequence[str] | None = None) -> int:
    try:
        args = _build_parser().parse_args(argv)
        return int(args.handler(args))
    except HybridError as exc:
        sys.stderr.write(
            json.dumps(
                {"error": exc.to_document()},
                ensure_ascii=False,
                sort_keys=True,
            )
            + "\n"
        )
        return _exit_code(exc.code)


if __name__ == "__main__":
    raise SystemExit(main())
