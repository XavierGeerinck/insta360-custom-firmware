"""Host-only command line for the learned Erf-mask qualification pipeline."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import platform
import shutil
import statistics
import sys
from collections.abc import Callable, Iterator, Mapping, Sequence
from pathlib import Path
from typing import Any

import numpy as np

from .audit import SHA256_RE, canonical_json
from .erf_mask_corpus import (
    SLR31_ARCHIVES,
    ProceduralCorpusConfig,
    build_slr31_manifest,
    deterministic_wav_bytes,
    download_slr31_archive,
    extract_slr31_archive,
    generate_noise_source,
    generate_procedural_split,
    load_and_resample_slr31_item,
    load_procedural_config,
    mix_at_snr,
    select_slr31_items,
    verify_slr31_archive,
)
from .erf_mask_device_stream import (
    STREAM_INPUT_MAGIC,
    STREAM_OUTPUT_MAGIC,
    ReplayErfPartition,
    align_dfn_fixture_output,
    capture_erf_inputs,
    cpu_output_stream,
    dfn_fixture_input_bytes,
    parse_erf_stream,
    serialize_erf_stream,
)
from .erf_mask_inference import (
    DFN_CLEAN_RETAIN_DB,
    DFN_CLEAN_RETAIN_POWER_RATIO,
    DfnErfFrameSelector,
    HybridErfPartition,
)
from .erf_mask_metrics import (
    FIXTURE_TOTAL_SAMPLES,
    FixtureSpeechItem,
    build_device_fixture,
    noise_attenuation_db,
    rms_db,
    si_sdr,
    stoi_score,
)
from .erf_mask_model import (
    build_erf_mask_onnx,
    serialize_erf_mask_weights,
)
from .erf_mask_package import (
    MODEL_ARTIFACTS,
    build_unsealed_model_manifest,
    verify_model_package,
)
from .erf_mask_qualification import (
    build_qualification_vectors,
    fnv1a64,
    qualification_vector_manifest,
    serialize_qualification_vectors,
)
from .erf_mask_signal import (
    ERF_BINS,
    HOP_SAMPLES,
    SAMPLE_RATE,
    build_erf_u8_table,
)
from .erf_mask_stream import (
    DFN_ATTENUATION_LIMIT_DB,
    EvaluationAudioItem,
    evaluate_dfn_erf_stream,
    verify_evaluation_stream,
    write_evaluation_stream,
)
from .erf_mask_train import (
    FrameStore,
    TrainingConfig,
    build_balanced_frame_store,
    train_erf_mask,
)
from .errors import ErrorCode, HybridError
from .fixtures import deterministic_npz_bytes

FRAME_BUDGETS = {
    "train": 65_536,
    "validation": 16_384,
    "test": 32_768,
}
FRAME_STORE_SEED = 0x57335645
FIXTURE_TRANSITION_CLEAN_ABS_RMS_DELTA_DB_MAX = 1.5
FIXTURE_SILENCE_RMS_DBFS_MAX = -80.0


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


class _StrictParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise HybridError(ErrorCode.ONNX_INPUT_INVALID, message)


def _strict_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise _invalid(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json(path: Path) -> dict[str, Any]:
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise _invalid(f"cannot read JSON file {path}: {exc}") from exc
    if len(payload) > 128 * 1024 * 1024:
        raise _invalid(f"JSON file exceeds size limit: {path}")
    try:
        value = json.loads(
            payload,
            object_pairs_hook=_strict_pairs,
            parse_constant=lambda token: (_ for _ in ()).throw(
                ValueError(f"non-finite JSON token: {token}")
            ),
        )
    except (UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise _invalid(f"cannot parse strict JSON file {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise _invalid(f"JSON root must be an object: {path}")
    return value


def _lexists(path: Path) -> bool:
    return os.path.lexists(path)


def _write_exclusive(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, 0o644)
    except FileExistsError as exc:
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {path}",
        ) from exc
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _atomic_directory(
    output: Path,
    build: Callable[[Path], None],
) -> None:
    if output.name in {"", ".", ".."}:
        raise _invalid("output directory must have a concrete final name")
    partial = output.with_name(f".{output.name}.partial")
    if _lexists(output) or _lexists(partial):
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output or partial directory already exists: {output}",
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    partial.mkdir(mode=0o700)
    try:
        build(partial)
        os.rename(partial, output)
    except BaseException:
        if partial.is_dir() and not partial.is_symlink():
            shutil.rmtree(partial)
        else:
            partial.unlink(missing_ok=True)
        raise


def _hash_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _hash_path(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as exc:
        raise _invalid(f"cannot hash {path}: {exc}") from exc
    return digest.hexdigest()


def _command_generate_procedural(args: argparse.Namespace) -> int:
    config = load_procedural_config(args.config)
    config_payload = canonical_json(config.to_document())
    per_seed = (
        len(config.clean_kinds) * len(config.noise_kinds) * len(config.snr_values_db)
        + len(config.clean_kinds)
        + len(config.noise_kinds)
        + 1
    )
    manifest = {
        "schema_version": 1,
        "corpus": "wave-erf-mask-procedural",
        "sample_rate": config.sample_rate,
        "duration_samples": config.duration_samples,
        "generator": {
            "implementation": "wave_onnx_hybrid.erf_mask_corpus",
            "version": 1,
            "rng": "numpy.random.PCG64",
        },
        "config_sha256": _hash_bytes(config_payload),
        "split_item_counts": {
            "train": per_seed * len(config.train_seeds),
            "validation": per_seed * len(config.validation_seeds),
            "test": per_seed * len(config.test_seeds),
        },
        "private_inputs_used": False,
    }

    def build(root: Path) -> None:
        _write_exclusive(root / "config.json", config_payload)
        _write_exclusive(
            root / "corpus-manifest.json",
            canonical_json(manifest),
        )

    _atomic_directory(args.output, build)
    print(f"wrote procedural definition to {args.output}")
    return 0


def _command_acquire_slr31(args: argparse.Namespace) -> int:
    cache = args.archive_cache
    if cache is not None and cache.is_symlink():
        raise _invalid("SLR31 archive cache cannot be a symlink")

    def build(root: Path) -> None:
        archive_root = root / "archives"
        extracted_root = root / "extracted"
        evidence: dict[str, dict[str, Any]] = {}
        extraction: dict[str, dict[str, Any]] = {}
        for subset, spec in sorted(SLR31_ARCHIVES.items()):
            archive_path = archive_root / spec.filename
            if cache is None:
                evidence[subset] = download_slr31_archive(
                    spec,
                    archive_path,
                )
            else:
                cached_path = cache / spec.filename
                if not cached_path.exists():
                    evidence[subset] = download_slr31_archive(
                        spec,
                        cached_path,
                    )
                else:
                    evidence[subset] = verify_slr31_archive(
                        spec,
                        cached_path,
                    )
                archive_path.parent.mkdir(parents=True, exist_ok=True)
                try:
                    shutil.copyfile(cached_path, archive_path)
                    archive_path.chmod(0o444)
                except OSError as exc:
                    raise _invalid(
                        f"cannot copy verified archive from cache: {exc}"
                    ) from exc
            extraction[subset] = extract_slr31_archive(
                spec,
                archive_path,
                extracted_root / subset,
            )
        selection = select_slr31_items(
            extracted_root / "train-clean-5",
            extracted_root / "dev-clean-2",
        )
        manifest = build_slr31_manifest(selection, evidence)
        manifest["extraction"] = extraction
        manifest["private_inputs_used"] = False
        _write_exclusive(
            root / "slr31-manifest.json",
            canonical_json(manifest),
        )

    _atomic_directory(args.output, build)
    print(f"acquired and verified SLR31 below {args.output}")
    return 0


def _frame_store_payload(store: FrameStore) -> tuple[bytes, bytes]:
    tensors = deterministic_npz_bytes(
        {
            "features": store.features,
            "targets": store.targets,
        }
    )
    metadata = canonical_json(
        {
            "schema_version": 1,
            "split": store.split,
            "shape": list(store.features.shape),
            "feature_dtype": "FLOAT32",
            "target_dtype": "FLOAT32",
            "row_ids": list(store.row_ids),
            "group_counts": dict(store.group_counts),
            "payload_sha256": store.payload_sha256,
            "tensor_archive_sha256": _hash_bytes(tensors),
        }
    )
    return tensors, metadata


def _load_frame_store(root: Path, split: str) -> FrameStore:
    metadata = _load_json(root / f"{split}.frames.json")
    expected_keys = {
        "schema_version",
        "split",
        "shape",
        "feature_dtype",
        "target_dtype",
        "row_ids",
        "group_counts",
        "payload_sha256",
        "tensor_archive_sha256",
    }
    if (
        set(metadata) != expected_keys
        or metadata["schema_version"] != 1
        or metadata["split"] != split
        or metadata["feature_dtype"] != "FLOAT32"
        or metadata["target_dtype"] != "FLOAT32"
    ):
        raise _invalid(f"invalid {split} frame-store metadata")
    tensor_path = root / f"{split}.frames.npz"
    if _hash_path(tensor_path) != metadata["tensor_archive_sha256"]:
        raise _invalid(f"{split} frame tensor archive hash mismatch")
    try:
        with np.load(tensor_path, allow_pickle=False) as archive:
            if archive.files != ["features", "targets"]:
                raise _invalid(f"{split} frame tensor names differ")
            features = np.array(archive["features"], copy=True, order="C")
            targets = np.array(archive["targets"], copy=True, order="C")
    except (OSError, ValueError) as exc:
        raise _invalid(f"cannot load {split} frame tensors: {exc}") from exc
    if list(features.shape) != metadata["shape"]:
        raise _invalid(f"{split} frame tensor shape differs")
    store = FrameStore(
        split=split,
        features=features,
        targets=targets,
        row_ids=tuple(metadata["row_ids"]),
        group_counts=metadata["group_counts"],
    )
    if store.payload_sha256 != metadata["payload_sha256"]:
        raise _invalid(f"{split} frame-store payload hash mismatch")
    return store


def _slr31_roots(root: Path) -> tuple[Path, Path]:
    manifest = root / "slr31-manifest.json"
    train = root / "extracted" / "train-clean-5"
    dev = root / "extracted" / "dev-clean-2"
    if not manifest.is_file():
        raise _invalid(f"missing SLR31 manifest: {manifest}")
    return train, dev


def _command_build_examples(args: argparse.Namespace) -> int:
    config = load_procedural_config(args.procedural_config)
    train_root, dev_root = _slr31_roots(args.slr31)
    selection = select_slr31_items(train_root, dev_root)
    slr_manifest_path = args.slr31 / "slr31-manifest.json"
    slr_manifest = _load_json(slr_manifest_path)
    selected_counts = {split: len(items) for split, items in selection.items()}
    if not selected_counts["train"] or not selected_counts["test"]:
        raise _invalid("SLR31 selection lacks train or held-out test speech")

    def build(root: Path) -> None:
        stores: dict[str, dict[str, Any]] = {}
        for split in ("train", "validation", "test"):
            items = generate_procedural_split(config, split)
            store = build_balanced_frame_store(
                items,
                max_frames=FRAME_BUDGETS[split],
                seed=FRAME_STORE_SEED,
            )
            tensors, metadata = _frame_store_payload(store)
            tensor_name = f"{split}.frames.npz"
            metadata_name = f"{split}.frames.json"
            _write_exclusive(root / tensor_name, tensors)
            _write_exclusive(root / metadata_name, metadata)
            stores[split] = {
                "frames": store.features.shape[0],
                "payload_sha256": store.payload_sha256,
                "tensor_archive_sha256": _hash_bytes(tensors),
                "metadata_sha256": _hash_bytes(metadata),
            }
            del items
        config_payload = canonical_json(config.to_document())
        corpus_manifest = {
            "schema_version": 1,
            "corpus": "wave-erf-mask-host-training",
            "sample_rate": SAMPLE_RATE,
            "procedural_config": config.to_document(),
            "procedural_config_sha256": _hash_bytes(config_payload),
            "slr31_manifest_sha256": _hash_path(slr_manifest_path),
            "slr31_source_page": slr_manifest.get("source_page"),
            "slr31_license": slr_manifest.get("license"),
            "slr31_selected_items": selected_counts,
            "natural_training_used": False,
            "natural_held_out_for_evaluation": True,
            "frame_store_seed": FRAME_STORE_SEED,
            "frame_budgets": dict(FRAME_BUDGETS),
            "frame_stores": stores,
            "private_inputs_used": False,
        }
        _write_exclusive(
            root / "corpus-manifest.json",
            canonical_json(corpus_manifest),
        )
        _write_exclusive(root / "procedural-config.json", config_payload)

    _atomic_directory(args.output, build)
    print(f"wrote deterministic frame stores to {args.output}")
    return 0


def _training_config_document(config: TrainingConfig) -> dict[str, Any]:
    return {
        "seed": config.seed,
        "batch_size": config.batch_size,
        "steps": config.steps,
        "learning_rate": config.learning_rate,
        "beta1": config.beta1,
        "beta2": config.beta2,
        "adam_epsilon": config.adam_epsilon,
        "validation_interval": config.validation_interval,
        "regularization": config.regularization,
    }


def _command_train(args: argparse.Namespace) -> int:
    train = _load_frame_store(args.examples, "train")
    validation = _load_frame_store(args.examples, "validation")
    corpus_payload = (args.examples / "corpus-manifest.json").read_bytes()
    if canonical_json(json.loads(corpus_payload)) != corpus_payload:
        raise _invalid("corpus manifest is not canonical JSON")
    config = TrainingConfig()
    result = train_erf_mask(train, validation, config)
    if not result.selected_validation_loss < result.initial_validation_loss:
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "trained checkpoint does not beat deterministic initialization",
        )

    def build(root: Path) -> None:
        weights = serialize_erf_mask_weights(result.slopes, result.biases)
        model = build_erf_mask_onnx(result.slopes, result.biases)
        training_manifest = canonical_json(
            {
                "schema_version": 1,
                "trainer": "wave-erf-mask-deterministic-adam",
                "trainer_version": 1,
                "configuration": _training_config_document(config),
                "train_frame_store_sha256": train.payload_sha256,
                "validation_frame_store_sha256": validation.payload_sha256,
                "selected_step": result.selected_step,
                "initial_validation_loss": result.initial_validation_loss,
                "selected_validation_loss": result.selected_validation_loss,
                "history": list(result.history),
                "result_payload_sha256": result.payload_sha256,
                "environment": {
                    "python": platform.python_version(),
                    "implementation": platform.python_implementation(),
                    "machine": platform.machine(),
                    "system": platform.system(),
                    "numpy": np.__version__,
                    "thread_variables": {
                        name: os.environ.get(name)
                        for name in (
                            "OMP_NUM_THREADS",
                            "OPENBLAS_NUM_THREADS",
                            "MKL_NUM_THREADS",
                            "VECLIB_MAXIMUM_THREADS",
                            "NUMEXPR_NUM_THREADS",
                        )
                    },
                },
            }
        )
        model_config = canonical_json(
            {
                "schema_version": 1,
                "model": "wave-erf-mask",
                "model_version": "0.1.0",
                "training": _training_config_document(config),
                "sample_rate": SAMPLE_RATE,
                "hop_samples": HOP_SAMPLES,
                "architecture": ("dfn3-cpu-or-learned-erf-causal-frame-selector"),
                "dfn": {
                    "implementation": "waved_npu.Dfn3CpuInfer",
                    "provider": "deep_filter_cpu",
                    "upstream_tag": "v0.5.6",
                    "attenuation_limit_db": DFN_ATTENUATION_LIMIT_DB,
                    "output_delay_hops": 1,
                },
                "selector": {
                    "decision": (
                        "choose_dfn_when_retained_frame_power_is_at_least_threshold"
                    ),
                    "dfn_clean_retain_db": DFN_CLEAN_RETAIN_DB,
                    "dfn_clean_retain_power_ratio": (DFN_CLEAN_RETAIN_POWER_RATIO),
                    "erf_partition_execution": "unconditional-per-frame",
                    "output_delay_hops": 1,
                },
                "public_output_delay_hops": 2,
            }
        )
        artifacts = {
            "weights.npz": weights,
            "model.onnx": model,
            "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
            "training-manifest.json": training_manifest,
            "corpus-manifest.json": corpus_payload,
            "config.json": model_config,
        }
        manifest = build_unsealed_model_manifest(artifacts)
        for name, payload in artifacts.items():
            _write_exclusive(root / name, payload)
        _write_exclusive(
            root / "model-manifest.json",
            canonical_json(manifest),
        )

    _atomic_directory(args.output, build)
    print(f"trained and exported model to {args.output}")
    return 0


def _load_model_package(
    root: Path,
) -> tuple[dict[str, Any], dict[str, bytes], np.ndarray, np.ndarray]:
    artifacts: dict[str, bytes] = {}
    try:
        for name in sorted(MODEL_ARTIFACTS):
            artifacts[name] = (root / name).read_bytes()
    except OSError as exc:
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID,
            f"cannot read model artifact: {exc}",
        ) from exc
    manifest = _load_json(root / "model-manifest.json")
    verified = verify_model_package(manifest, artifacts)
    try:
        with np.load(
            io.BytesIO(artifacts["weights.npz"]),
            allow_pickle=False,
        ) as archive:
            if archive.files != ["a", "b"]:
                raise ValueError("weight names differ")
            slopes = np.array(archive["a"], copy=True, order="C")
            biases = np.array(archive["b"], copy=True, order="C")
    except (OSError, ValueError) as exc:
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID,
            f"cannot load model weights: {exc}",
        ) from exc
    if serialize_erf_mask_weights(slopes, biases) != artifacts["weights.npz"]:
        raise HybridError(
            ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
            "weight archive is not in canonical form",
        )
    return verified, artifacts, slopes, biases


def _natural_seed(identity: str) -> int:
    digest = hashlib.sha256(f"wave-erf-mask|{identity}".encode()).digest()
    return int.from_bytes(digest[:8], "big") >> 1


def _config_from_corpus_manifest(
    corpus_manifest: Mapping[str, Any],
) -> ProceduralCorpusConfig:
    config_document = corpus_manifest.get("procedural_config")
    config_keys = {
        "schema_version",
        "sample_rate",
        "duration_samples",
        "train_seeds",
        "validation_seeds",
        "test_seeds",
        "clean_kinds",
        "noise_kinds",
        "snr_values_db",
    }
    if not isinstance(config_document, dict) or set(config_document) != config_keys:
        raise _invalid("model corpus manifest lacks procedural configuration")
    try:
        return ProceduralCorpusConfig(
            schema_version=config_document["schema_version"],
            sample_rate=config_document["sample_rate"],
            duration_samples=config_document["duration_samples"],
            train_seeds=tuple(config_document["train_seeds"]),
            validation_seeds=tuple(config_document["validation_seeds"]),
            test_seeds=tuple(config_document["test_seeds"]),
            clean_kinds=tuple(config_document["clean_kinds"]),
            noise_kinds=tuple(config_document["noise_kinds"]),
            snr_values_db=tuple(config_document["snr_values_db"]),
        )
    except (KeyError, TypeError) as exc:
        raise _invalid("invalid procedural configuration in model package") from exc


def _evaluation_items(
    config: ProceduralCorpusConfig,
    slr31_root: Path,
) -> Iterator[EvaluationAudioItem]:
    procedural = generate_procedural_split(config, "test")
    for item in procedural:
        category = {
            "mixture": "synthetic-mixture",
            "noise-only": "noise-only",
            "speech-only": "speech-only",
            "silence": "silence",
        }[item.category]
        yield EvaluationAudioItem(
            identity=f"procedural/{item.identity}",
            category=category,
            clean=item.clean,
            noise=item.noise,
            mixture=item.mixture,
            metadata={
                "tier": "procedural",
                "seed": item.seed,
                "clean_kind": item.clean_kind,
                "noise_kind": item.noise_kind,
                "snr_db": item.snr_db,
                "payload_sha256": item.payload_sha256,
            },
        )
    del procedural

    train_root, dev_root = _slr31_roots(slr31_root)
    selection = select_slr31_items(train_root, dev_root)
    noise_kinds = ("pink", "amplitude-modulated", "impulsive")
    snr_values = (0.0, -5.0, 5.0)
    for index, item in enumerate(selection["test"]):
        clean = load_and_resample_slr31_item(item)
        aligned = clean.size - clean.size % HOP_SAMPLES
        clean = np.ascontiguousarray(clean[:aligned], dtype=np.float32)
        kind = noise_kinds[index % len(noise_kinds)]
        snr = snr_values[index % len(snr_values)]
        noise = generate_noise_source(
            kind,
            clean.size,
            seed=_natural_seed(item.relative_path),
        )
        components = mix_at_snr(clean, noise, snr)
        common = {
            "tier": "slr31-test",
            "source_sha256": item.sha256,
            "relative_path": item.relative_path,
            "source_samples": item.source_samples,
        }
        yield EvaluationAudioItem(
            identity=f"slr31/test-mixture/{item.relative_path}",
            category="natural-mixture",
            clean=components.clean,
            noise=components.noise,
            mixture=components.mixture,
            metadata={
                **common,
                "noise_kind": kind,
                "snr_db": snr,
                "noise_seed": _natural_seed(item.relative_path),
            },
        )
        zeros = np.zeros_like(clean)
        yield EvaluationAudioItem(
            identity=f"slr31/test-clean/{item.relative_path}",
            category="speech-only",
            clean=clean,
            noise=zeros,
            mixture=clean,
            metadata=common,
        )


def _command_build_evaluation_stream(args: argparse.Namespace) -> int:
    config = load_procedural_config(args.procedural_config)
    slr_manifest = args.slr31 / "slr31-manifest.json"
    _load_json(slr_manifest)

    def build(root: Path) -> None:
        raw_path = root / "evaluation-input.f32"
        manifest = write_evaluation_stream(
            _evaluation_items(config, args.slr31),
            raw_path,
        )
        _write_exclusive(
            root / "stream-manifest.json",
            canonical_json(manifest),
        )
        _write_exclusive(
            root / "stream-provenance.json",
            canonical_json(
                {
                    "schema_version": 1,
                    "procedural_config_sha256": _hash_path(args.procedural_config),
                    "slr31_manifest_sha256": _hash_path(slr_manifest),
                    "private_inputs_used": False,
                }
            ),
        )

    _atomic_directory(args.output, build)
    print(f"built guarded DFN evaluation stream at {args.output}")
    return 0


def _command_evaluate(args: argparse.Namespace) -> int:
    _, artifacts, slopes, biases = _load_model_package(args.model)
    corpus_manifest = json.loads(artifacts["corpus-manifest.json"])
    config = _config_from_corpus_manifest(corpus_manifest)
    stream_manifest = _load_json(args.stream / "stream-manifest.json")
    verified_stream = verify_evaluation_stream(
        stream_manifest,
        args.stream / "evaluation-input.f32",
    )
    dfn_run = _load_json(args.dfn_run)
    if (
        set(dfn_run)
        != {
            "attenuation_limit_db",
            "backend",
            "hop_samples",
            "samples",
        }
        or dfn_run["attenuation_limit_db"] != DFN_ATTENUATION_LIMIT_DB
        or dfn_run["backend"] != "deep_filter_cpu"
        or dfn_run["hop_samples"] != HOP_SAMPLES
        or dfn_run["samples"] != verified_stream["total_samples"]
    ):
        raise _invalid("DFN run evidence violates the fixed host contract")
    if args.dfn_binary.is_symlink() or not args.dfn_binary.is_file():
        raise _invalid("DFN host binary must be a regular file")
    result = evaluate_dfn_erf_stream(
        _evaluation_items(config, args.slr31),
        verified_stream,
        args.dfn_output,
        slopes,
        biases,
    )
    result["model_manifest_sha256"] = _hash_path(args.model / "model-manifest.json")
    result["slr31_manifest_sha256"] = _hash_path(args.slr31 / "slr31-manifest.json")
    result["dfn"]["binary_sha256"] = _hash_path(args.dfn_binary)
    result["dfn"]["run_evidence_sha256"] = _hash_path(args.dfn_run)
    result["private_inputs_used"] = False

    def build(root: Path) -> None:
        _write_exclusive(
            root / "evaluation-summary.json",
            canonical_json(result),
        )

    _atomic_directory(args.output, build)
    if not result["passed"]:
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "held-out hybrid evaluation did not pass fixed thresholds",
            details={"summary": result["summary"]},
        )
    print(f"evaluated held-out hybrid quality at {args.output}")
    return 0


def _command_build_fixture(args: argparse.Namespace) -> int:
    train_root, dev_root = _slr31_roots(args.slr31)
    selection = select_slr31_items(train_root, dev_root)
    sources: list[FixtureSpeechItem] = []
    available = 0
    for item in sorted(
        selection["test"],
        key=lambda value: value.relative_path,
    ):
        audio = load_and_resample_slr31_item(item)
        sources.append(
            FixtureSpeechItem(
                identity=f"slr31/test/{item.relative_path}",
                source_sha256=item.sha256,
                samples=audio,
            )
        )
        available += audio.size
        if available >= SAMPLE_RATE * 8:
            break
    fixture = build_device_fixture(sources)

    def build(root: Path) -> None:
        _write_exclusive(
            root / "fixture.npz",
            deterministic_npz_bytes(
                {
                    "clean": fixture.clean,
                    "noise": fixture.noise,
                    "mixture": fixture.mixture,
                }
            ),
        )
        _write_exclusive(
            root / "fixture-manifest.json",
            canonical_json(fixture.manifest),
        )
        _write_exclusive(
            root / "fixture-mixture.wav",
            deterministic_wav_bytes(fixture.mixture),
        )
        _write_exclusive(
            root / "fixture-clean.wav",
            deterministic_wav_bytes(fixture.clean),
        )
        _write_exclusive(
            root / "fixture-noise.wav",
            deterministic_wav_bytes(fixture.noise),
        )

    _atomic_directory(args.output, build)
    print(f"built fixed 12-second device fixture at {args.output}")
    return 0


def _command_verify_package(args: argparse.Namespace) -> int:
    manifest, artifacts, _, _ = _load_model_package(args.model)
    result = {
        "schema_version": 1,
        "verified": True,
        "execution_approved": manifest["execution_approved"],
        "compiler_qualified": manifest["compiler_qualified"],
        "model_manifest_sha256": _hash_path(args.model / "model-manifest.json"),
        "artifact_sha256": {
            name: _hash_bytes(payload) for name, payload in sorted(artifacts.items())
        },
    }

    def build(root: Path) -> None:
        _write_exclusive(root / "verification.json", canonical_json(result))

    _atomic_directory(args.output, build)
    print(f"verified model package at {args.model}")
    return 0


def _verify_fixture(root: Path) -> dict[str, Any]:
    manifest = _load_json(root / "fixture-manifest.json")
    tensor_path = root / "fixture.npz"
    try:
        with np.load(tensor_path, allow_pickle=False) as archive:
            if archive.files != ["clean", "mixture", "noise"]:
                raise _invalid("fixture tensor names differ")
            arrays = {
                name: np.array(archive[name], copy=True, order="C")
                for name in archive.files
            }
    except (OSError, ValueError) as exc:
        raise _invalid(f"cannot load fixture tensors: {exc}") from exc
    if (
        set(arrays) != {"clean", "noise", "mixture"}
        or any(
            value.shape != (FIXTURE_TOTAL_SAMPLES,)
            or value.dtype != np.dtype(np.float32)
            or not np.isfinite(value).all()
            for value in arrays.values()
        )
        or not np.allclose(
            arrays["clean"] + arrays["noise"],
            arrays["mixture"],
            atol=2e-6,
            rtol=1e-6,
        )
    ):
        raise _invalid("fixture tensor contract differs")
    observed_hashes = {
        name: hashlib.sha256(
            np.ascontiguousarray(value, dtype="<f4").tobytes()
        ).hexdigest()
        for name, value in arrays.items()
    }
    if (
        manifest.get("schema_version") != 1
        or manifest.get("sample_rate") != SAMPLE_RATE
        or manifest.get("total_samples") != FIXTURE_TOTAL_SAMPLES
        or manifest.get("sha256") != observed_hashes
    ):
        raise _invalid("fixture manifest does not match tensor payloads")
    return manifest


def _command_qualify_host(args: argparse.Namespace) -> int:
    model, artifacts, _, _ = _load_model_package(args.model)
    evaluation = _load_json(args.evaluation)
    fixture = _verify_fixture(args.fixture)
    if type(evaluation.get("passed")) is not bool or not evaluation["passed"]:
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "host evaluation has not passed the fixed quality gates",
        )
    items_hash = evaluation.get("items_sha256")
    if type(items_hash) is not str or SHA256_RE.fullmatch(items_hash) is None:
        raise _invalid("evaluation item digest is missing or malformed")
    training = json.loads(artifacts["training-manifest.json"])
    if (
        not isinstance(training, dict)
        or not isinstance(training.get("initial_validation_loss"), (int, float))
        or not isinstance(training.get("selected_validation_loss"), (int, float))
        or not training["selected_validation_loss"]
        < training["initial_validation_loss"]
    ):
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "training ablation gate did not pass",
        )
    result = {
        "schema_version": 1,
        "poc": "13A",
        "qualified": True,
        "device_calls": 0,
        "execution_approved": model["execution_approved"],
        "compiler_qualified": model["compiler_qualified"],
        "model_manifest_sha256": _hash_path(args.model / "model-manifest.json"),
        "evaluation_sha256": _hash_path(args.evaluation),
        "evaluation_items_sha256": items_hash,
        "fixture_manifest_sha256": _hash_path(args.fixture / "fixture-manifest.json"),
        "fixture_mixture_sha256": fixture["sha256"]["mixture"],
        "private_inputs_used": False,
        "next_gate": "fixed compile-only Erf[481] capture",
        "non_claims": [
            "No device call was made by host qualification.",
            "No general compiler was qualified.",
            "No NPU numerical execution is approved by this result.",
        ],
    }

    def build(root: Path) -> None:
        _write_exclusive(
            root / "host-qualification.json",
            canonical_json(result),
        )

    _atomic_directory(args.output, build)
    print(f"qualified host PoC 13A at {args.output}")
    return 0


def _command_build_device_vectors(args: argparse.Namespace) -> int:
    _, _, slopes, biases = _load_model_package(args.model)
    fixture_manifest = _verify_fixture(args.fixture)
    try:
        with np.load(
            args.fixture / "fixture.npz",
            allow_pickle=False,
        ) as archive:
            mixture = np.array(archive["mixture"], copy=True, order="C")
    except (OSError, ValueError, KeyError) as exc:
        raise _invalid(f"cannot load fixture mixture: {exc}") from exc
    collection = build_qualification_vectors(slopes, biases, mixture)
    vector_pack = serialize_qualification_vectors(collection)
    vector_manifest = {
        **qualification_vector_manifest(collection),
        "pack": {
            "bytes": len(vector_pack),
            "sha256": _hash_bytes(vector_pack),
            "fnv1a64": f"0x{fnv1a64(vector_pack):016x}",
        },
        "model_manifest_sha256": _hash_path(args.model / "model-manifest.json"),
        "fixture_manifest_sha256": _hash_path(args.fixture / "fixture-manifest.json"),
        "fixture_mixture_sha256": fixture_manifest["sha256"]["mixture"],
        "private_inputs_used": False,
    }

    def build(root: Path) -> None:
        _write_exclusive(root / "qualification-vectors.bin", vector_pack)
        _write_exclusive(
            root / "qualification-vectors.json",
            canonical_json(vector_manifest),
        )

    _atomic_directory(args.output, build)
    print(f"built six-vector NPU qualification pack at {args.output}")
    return 0


def _command_build_device_stream(args: argparse.Namespace) -> int:
    _, _, slopes, biases = _load_model_package(args.model)
    fixture_manifest = _verify_fixture(args.fixture)
    try:
        with np.load(
            args.fixture / "fixture.npz",
            allow_pickle=False,
        ) as archive:
            mixture = np.array(archive["mixture"], copy=True, order="C")
    except (OSError, ValueError, KeyError) as exc:
        raise _invalid(f"cannot load fixture mixture: {exc}") from exc
    stream = capture_erf_inputs(slopes, biases, mixture)
    expected_frames = mixture.size // HOP_SAMPLES + 1
    if stream.count != expected_frames:
        raise _invalid("fixture Erf stream frame count differs")
    stream_pack = serialize_erf_stream(stream, magic=STREAM_INPUT_MAGIC)
    dfn_input = dfn_fixture_input_bytes(mixture)
    stream_manifest = {
        "schema_version": 1,
        "frame_count": stream.count,
        "width": ERF_BINS,
        "input_pack": {
            "bytes": len(stream_pack),
            "sha256": _hash_bytes(stream_pack),
            "fnv1a64": f"0x{fnv1a64(stream_pack):016x}",
        },
        "dfn_input": {
            "bytes": len(dfn_input),
            "samples": len(dfn_input) // np.dtype(np.float32).itemsize,
            "sha256": _hash_bytes(dfn_input),
            "guard_samples_before": HOP_SAMPLES,
            "guard_samples_after": HOP_SAMPLES,
            "output_delay_samples": HOP_SAMPLES,
        },
        "model_manifest_sha256": _hash_path(args.model / "model-manifest.json"),
        "fixture_manifest_sha256": _hash_path(args.fixture / "fixture-manifest.json"),
        "fixture_mixture_sha256": fixture_manifest["sha256"]["mixture"],
        "private_inputs_used": False,
    }

    def build(root: Path) -> None:
        _write_exclusive(root / "erf-stream-input.bin", stream_pack)
        _write_exclusive(root / "dfn-input.f32", dfn_input)
        _write_exclusive(
            root / "stream-manifest.json",
            canonical_json(stream_manifest),
        )

    _atomic_directory(args.output, build)
    print(f"built full-fixture NPU stream at {args.output}")
    return 0


def _command_reconstruct_device_waveform(
    args: argparse.Namespace,
) -> int:
    _, _, slopes, biases = _load_model_package(args.model)
    fixture_manifest = _verify_fixture(args.fixture)
    try:
        with np.load(
            args.fixture / "fixture.npz",
            allow_pickle=False,
        ) as archive:
            clean = np.array(archive["clean"], copy=True, order="C")
            mixture = np.array(archive["mixture"], copy=True, order="C")
    except (OSError, ValueError, KeyError) as exc:
        raise _invalid(f"cannot load fixture arrays: {exc}") from exc

    stream_manifest = _load_json(args.stream / "stream-manifest.json")
    expected_stream_keys = {
        "schema_version",
        "frame_count",
        "width",
        "input_pack",
        "dfn_input",
        "model_manifest_sha256",
        "fixture_manifest_sha256",
        "fixture_mixture_sha256",
        "private_inputs_used",
    }
    input_path = args.stream / "erf-stream-input.bin"
    try:
        input_payload = input_path.read_bytes()
    except OSError as exc:
        raise _invalid(f"cannot read device input stream: {exc}") from exc
    if (
        set(stream_manifest) != expected_stream_keys
        or stream_manifest["schema_version"] != 1
        or stream_manifest["frame_count"] != mixture.size // HOP_SAMPLES + 1
        or stream_manifest["width"] != ERF_BINS
        or stream_manifest["input_pack"]
        != {
            "bytes": len(input_payload),
            "sha256": _hash_bytes(input_payload),
            "fnv1a64": f"0x{fnv1a64(input_payload):016x}",
        }
        or stream_manifest["model_manifest_sha256"]
        != _hash_path(args.model / "model-manifest.json")
        or stream_manifest["fixture_manifest_sha256"]
        != _hash_path(args.fixture / "fixture-manifest.json")
        or stream_manifest["fixture_mixture_sha256"]
        != fixture_manifest["sha256"]["mixture"]
        or stream_manifest["private_inputs_used"] is not False
    ):
        raise _invalid("device stream manifest differs")
    input_stream = parse_erf_stream(
        input_payload,
        magic=STREAM_INPUT_MAGIC,
        expected_count=stream_manifest["frame_count"],
    )

    device_run = _load_json(args.device_run / "run.json")
    try:
        output_evidence = device_run["result"]["output"]
        numeric_evidence = output_evidence["numeric_match"]
    except (KeyError, TypeError) as exc:
        raise _invalid("device run evidence is incomplete") from exc
    output_path = args.device_run / "erf-stream-output.bin"
    try:
        output_payload = output_path.read_bytes()
    except OSError as exc:
        raise _invalid(f"cannot read device output stream: {exc}") from exc
    if (
        device_run.get("status") != "PASS"
        or device_run.get("execution_attempts") != 1
        or device_run.get("health_differences") != []
        or device_run.get("kernel_alerts") != []
        or numeric_evidence
        != {
            "passed": True,
            "compared_bytes": input_stream.count * ERF_BINS,
            "mismatched_bytes": 0,
            "max_abs_u8_error": 0,
        }
        or output_evidence.get("sha256") != _hash_bytes(output_payload)
        or output_evidence.get("bytes") != len(output_payload)
    ):
        raise _invalid("device full-stream evidence did not pass")
    output_stream = parse_erf_stream(
        output_payload,
        magic=STREAM_OUTPUT_MAGIC,
        expected_count=input_stream.count,
    )
    cpu_stream = cpu_output_stream(input_stream)
    if any(
        not np.array_equal(observed, expected)
        for observed, expected in zip(
            output_stream.frames,
            cpu_stream.frames,
            strict=True,
        )
    ):
        raise HybridError(
            ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
            "device Erf stream differs from the CPU oracle",
        )

    dfn_run = _load_json(args.dfn_run / "run.json")
    dfn_output_path = args.dfn_run / "dfn-output.f32"
    try:
        dfn_output_payload = dfn_output_path.read_bytes()
    except OSError as exc:
        raise _invalid(f"cannot read DFN fixture output: {exc}") from exc
    dfn_input = stream_manifest.get("dfn_input")
    if (
        not isinstance(dfn_input, dict)
        or dfn_run
        != {
            "attenuation_limit_db": 8.0,
            "backend": "deep_filter_cpu",
            "binary_sha256": (
                "2547fae5a5bffd96e2fb4baa94299ada9fc5d20becfba940a2185195a04a4bf1"
            ),
            "hop_samples": HOP_SAMPLES,
            "input_bytes": dfn_input.get("bytes"),
            "input_sha256": dfn_input.get("sha256"),
            "output_bytes": len(dfn_output_payload),
            "output_sha256": _hash_bytes(dfn_output_payload),
            "samples": dfn_input.get("samples"),
            "schema_version": 1,
        }
        or len(dfn_output_payload) != dfn_input.get("bytes")
    ):
        raise _invalid("DFN fixture evidence differs")
    base_enhanced = align_dfn_fixture_output(
        dfn_output_payload,
        mixture_samples=mixture.size,
    )

    replay = ReplayErfPartition(input_stream, output_stream)
    hybrid_partition = HybridErfPartition(replay)
    npu_selector = DfnErfFrameSelector(
        slopes,
        biases,
        partition=hybrid_partition,
    )
    npu_enhanced = npu_selector.process_audio(
        mixture,
        base_enhanced,
        compensate_delay=True,
    )
    if (
        not replay.complete
        or replay.calls != input_stream.count
        or hybrid_partition.npu_attempts != input_stream.count
        or hybrid_partition.npu_successes != input_stream.count
        or hybrid_partition.fallback_frames != 0
        or hybrid_partition.fallback_latched
    ):
        raise HybridError(
            ErrorCode.AUDIO_RECONSTRUCTION_MISMATCH,
            "NPU waveform reconstruction used fallback or missed frames",
        )

    cpu_selector = DfnErfFrameSelector(slopes, biases)
    cpu_enhanced = cpu_selector.process_audio(
        mixture,
        base_enhanced,
        compensate_delay=True,
    )
    if not np.array_equal(npu_enhanced, cpu_enhanced):
        maximum_error = float(
            np.max(
                np.abs(
                    npu_enhanced.astype(np.float64) - cpu_enhanced.astype(np.float64)
                )
            )
        )
        raise HybridError(
            ErrorCode.AUDIO_RECONSTRUCTION_MISMATCH,
            "NPU-derived and CPU waveforms differ",
            details={"maximum_abs_error": maximum_error},
        )

    segments: list[dict[str, Any]] = []
    mixture_improvements: list[float] = []
    mixture_stoi_deltas: list[float] = []
    noise_attenuation: float | None = None
    clean_rms_delta: float | None = None
    silence_rms_dbfs: float | None = None
    for segment in fixture_manifest["segments"]:
        start = segment["offset_samples"]
        stop = start + segment["samples"]
        clean_segment = clean[start:stop]
        mixture_segment = mixture[start:stop]
        enhanced_segment = npu_enhanced[start:stop]
        record: dict[str, Any] = {
            "index": segment["index"],
            "kind": segment["kind"],
            "samples": segment["samples"],
            "input_rms_dbfs": rms_db(mixture_segment),
            "enhanced_rms_dbfs": rms_db(enhanced_segment),
        }
        if segment["kind"] == "pink-noise-only":
            noise_attenuation = noise_attenuation_db(mixture_segment, enhanced_segment)
            record["noise_attenuation_db"] = noise_attenuation
        elif segment["kind"] == "clean-speech":
            clean_rms_delta = rms_db(enhanced_segment) - rms_db(clean_segment)
            record["clean_rms_delta_db"] = clean_rms_delta
            record["input_stoi"] = stoi_score(clean_segment, mixture_segment)
            record["enhanced_stoi"] = stoi_score(clean_segment, enhanced_segment)
        elif segment["snr_db"] is not None:
            input_si_sdr = si_sdr(clean_segment, mixture_segment)
            enhanced_si_sdr = si_sdr(clean_segment, enhanced_segment)
            if input_si_sdr is None or enhanced_si_sdr is None:
                raise _invalid("fixture mixture SI-SDR is undefined")
            improvement = enhanced_si_sdr - input_si_sdr
            input_stoi = stoi_score(clean_segment, mixture_segment)
            enhanced_stoi = stoi_score(clean_segment, enhanced_segment)
            stoi_delta = enhanced_stoi - input_stoi
            mixture_improvements.append(improvement)
            mixture_stoi_deltas.append(stoi_delta)
            record.update(
                {
                    "input_si_sdr_db": input_si_sdr,
                    "enhanced_si_sdr_db": enhanced_si_sdr,
                    "si_sdr_improvement_db": improvement,
                    "input_stoi": input_stoi,
                    "enhanced_stoi": enhanced_stoi,
                    "stoi_delta": stoi_delta,
                }
            )
        elif segment["kind"] == "silence":
            silence_rms_dbfs = record["enhanced_rms_dbfs"]
            record["maximum_abs_sample"] = float(np.max(np.abs(enhanced_segment)))
        segments.append(record)
    if (
        noise_attenuation is None
        or clean_rms_delta is None
        or silence_rms_dbfs is None
        or len(mixture_improvements) != 3
        or len(mixture_stoi_deltas) != 3
    ):
        raise _invalid("fixture quality segment coverage differs")
    quality = {
        "noise_attenuation_db": noise_attenuation,
        "clean_abs_rms_delta_db": abs(clean_rms_delta),
        "mixture_median_si_sdr_improvement_db": statistics.median(mixture_improvements),
        "mixture_median_stoi_delta": statistics.median(mixture_stoi_deltas),
        "silence_rms_dbfs": silence_rms_dbfs,
        "thresholds": {
            "noise_attenuation_db_min": 6.0,
            "clean_abs_rms_delta_db_max": (
                FIXTURE_TRANSITION_CLEAN_ABS_RMS_DELTA_DB_MAX
            ),
            "mixture_median_si_sdr_improvement_db_min": 1.0,
            "mixture_median_stoi_delta_min": -0.02,
            "silence_rms_dbfs_max": FIXTURE_SILENCE_RMS_DBFS_MAX,
            "clean_gate_context": (
                "transition fixture allowance; held-out 477-item "
                "speech-only gate remains 1.0 dB"
            ),
        },
    }
    quality["passed"] = (
        quality["noise_attenuation_db"] >= 6.0
        and quality["clean_abs_rms_delta_db"]
        <= FIXTURE_TRANSITION_CLEAN_ABS_RMS_DELTA_DB_MAX
        and quality["mixture_median_si_sdr_improvement_db"] >= 1.0
        and quality["mixture_median_stoi_delta"] >= -0.02
        and quality["silence_rms_dbfs"] <= FIXTURE_SILENCE_RMS_DBFS_MAX
    )

    def unavailable(_: np.ndarray) -> np.ndarray:
        raise RuntimeError("simulated NPU unavailable")

    fallback_probe = HybridErfPartition(unavailable)
    fallback_outputs = (
        fallback_probe(input_stream.frames[0]),
        fallback_probe(input_stream.frames[1]),
    )
    if (
        not np.array_equal(fallback_outputs[0], cpu_stream.frames[0])
        or not np.array_equal(fallback_outputs[1], cpu_stream.frames[1])
        or fallback_probe.npu_attempts != 1
        or fallback_probe.npu_successes != 0
        or fallback_probe.fallback_frames != 2
        or not fallback_probe.fallback_latched
    ):
        raise HybridError(
            ErrorCode.CPU_FALLBACK_FAILURE,
            "latched CPU fallback proof failed",
        )

    npu_f32 = np.ascontiguousarray(npu_enhanced, dtype="<f4").tobytes()
    cpu_f32 = np.ascontiguousarray(cpu_enhanced, dtype="<f4").tobytes()
    npu_wav = deterministic_wav_bytes(npu_enhanced)
    cpu_wav = deterministic_wav_bytes(cpu_enhanced)
    evidence = {
        "schema_version": 1,
        "poc": "13E",
        "working_noise_cancellation": bool(quality["passed"]),
        "architecture": ("dfn3-cpu-or-wave-npu-erf-causal-frame-selector"),
        "sample_rate": SAMPLE_RATE,
        "samples": int(npu_enhanced.size),
        "duration_seconds": npu_enhanced.size / SAMPLE_RATE,
        "npu": {
            "device_run_sha256": _hash_path(args.device_run / "run.json"),
            "output_stream_sha256": _hash_bytes(output_payload),
            "partition_frames": input_stream.count,
            "successful_frames": hybrid_partition.npu_successes,
            "fallback_frames": hybrid_partition.fallback_frames,
            "erf_selected_frames": npu_selector.erf_selected_frames,
            "dfn_selected_frames": npu_selector.dfn_selected_frames,
            "erf_selection_fraction": (npu_selector.erf_selection_fraction),
        },
        "waveform_equivalence": {
            "passed": True,
            "compared_samples": int(npu_enhanced.size),
            "mismatched_samples": 0,
            "maximum_abs_error": 0.0,
            "npu_f32_sha256": _hash_bytes(npu_f32),
            "cpu_f32_sha256": _hash_bytes(cpu_f32),
            "npu_wav_sha256": _hash_bytes(npu_wav),
            "cpu_wav_sha256": _hash_bytes(cpu_wav),
        },
        "quality": quality,
        "segments": segments,
        "cpu_fallback": {
            "passed": True,
            "policy": "latch-after-first-npu-failure",
            "npu_attempts": fallback_probe.npu_attempts,
            "npu_successes": fallback_probe.npu_successes,
            "fallback_frames": fallback_probe.fallback_frames,
            "fallback_latched": fallback_probe.fallback_latched,
            "first_failure": fallback_probe.first_failure,
            "byte_exact_cpu_outputs": True,
        },
        "dfn_run_sha256": _hash_path(args.dfn_run / "run.json"),
        "stream_manifest_sha256": _hash_path(args.stream / "stream-manifest.json"),
        "model_manifest_sha256": _hash_path(args.model / "model-manifest.json"),
        "fixture_manifest_sha256": _hash_path(args.fixture / "fixture-manifest.json"),
        "private_inputs_used": False,
    }

    def build(root: Path) -> None:
        _write_exclusive(root / "npu-enhanced.f32", npu_f32)
        _write_exclusive(root / "cpu-reference.f32", cpu_f32)
        _write_exclusive(root / "npu-enhanced.wav", npu_wav)
        _write_exclusive(root / "cpu-reference.wav", cpu_wav)
        _write_exclusive(
            root / "waveform-evidence.json",
            canonical_json(evidence),
        )

    _atomic_directory(args.output, build)
    if not quality["passed"]:
        raise HybridError(
            ErrorCode.AUDIO_QUALITY_GATE_FAILURE,
            "NPU-derived fixture waveform did not pass quality gates",
            details={"quality": quality},
        )
    print(f"reconstructed NPU-derived waveform at {args.output}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = _StrictParser(
        description="Host-only learned Erf-mask qualification pipeline"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    procedural = subparsers.add_parser("generate-procedural")
    procedural.add_argument("--config", required=True, type=Path)
    procedural.add_argument("--output", required=True, type=Path)
    procedural.set_defaults(handler=_command_generate_procedural)

    acquire = subparsers.add_parser("acquire-slr31")
    acquire.add_argument("--output", required=True, type=Path)
    acquire.add_argument("--archive-cache", type=Path)
    acquire.set_defaults(handler=_command_acquire_slr31)

    examples = subparsers.add_parser("build-examples")
    examples.add_argument("--procedural-config", required=True, type=Path)
    examples.add_argument("--slr31", required=True, type=Path)
    examples.add_argument("--output", required=True, type=Path)
    examples.set_defaults(handler=_command_build_examples)

    stream = subparsers.add_parser("build-evaluation-stream")
    stream.add_argument("--procedural-config", required=True, type=Path)
    stream.add_argument("--slr31", required=True, type=Path)
    stream.add_argument("--output", required=True, type=Path)
    stream.set_defaults(handler=_command_build_evaluation_stream)

    train = subparsers.add_parser("train")
    train.add_argument("--examples", required=True, type=Path)
    train.add_argument("--output", required=True, type=Path)
    train.set_defaults(handler=_command_train)

    evaluate = subparsers.add_parser("evaluate")
    evaluate.add_argument("--model", required=True, type=Path)
    evaluate.add_argument("--slr31", required=True, type=Path)
    evaluate.add_argument("--stream", required=True, type=Path)
    evaluate.add_argument("--dfn-output", required=True, type=Path)
    evaluate.add_argument("--dfn-run", required=True, type=Path)
    evaluate.add_argument("--dfn-binary", required=True, type=Path)
    evaluate.add_argument("--output", required=True, type=Path)
    evaluate.set_defaults(handler=_command_evaluate)

    fixture = subparsers.add_parser("build-fixture")
    fixture.add_argument("--slr31", required=True, type=Path)
    fixture.add_argument("--output", required=True, type=Path)
    fixture.set_defaults(handler=_command_build_fixture)

    verify = subparsers.add_parser("verify-package")
    verify.add_argument("--model", required=True, type=Path)
    verify.add_argument("--output", required=True, type=Path)
    verify.set_defaults(handler=_command_verify_package)

    qualify = subparsers.add_parser("qualify-host")
    qualify.add_argument("--model", required=True, type=Path)
    qualify.add_argument("--evaluation", required=True, type=Path)
    qualify.add_argument("--fixture", required=True, type=Path)
    qualify.add_argument("--output", required=True, type=Path)
    qualify.set_defaults(handler=_command_qualify_host)

    vectors = subparsers.add_parser("build-device-vectors")
    vectors.add_argument("--model", required=True, type=Path)
    vectors.add_argument("--fixture", required=True, type=Path)
    vectors.add_argument("--output", required=True, type=Path)
    vectors.set_defaults(handler=_command_build_device_vectors)

    device_stream = subparsers.add_parser("build-device-stream")
    device_stream.add_argument("--model", required=True, type=Path)
    device_stream.add_argument("--fixture", required=True, type=Path)
    device_stream.add_argument("--output", required=True, type=Path)
    device_stream.set_defaults(handler=_command_build_device_stream)

    waveform = subparsers.add_parser("reconstruct-device-waveform")
    waveform.add_argument("--model", required=True, type=Path)
    waveform.add_argument("--fixture", required=True, type=Path)
    waveform.add_argument("--stream", required=True, type=Path)
    waveform.add_argument("--device-run", required=True, type=Path)
    waveform.add_argument("--dfn-run", required=True, type=Path)
    waveform.add_argument("--output", required=True, type=Path)
    waveform.set_defaults(handler=_command_reconstruct_device_waveform)
    return parser


def _exit_code(code: ErrorCode) -> int:
    if code is ErrorCode.OUTPUT_EXISTS:
        return 3
    if code is ErrorCode.AUDIO_QUALITY_GATE_FAILURE:
        return 4
    return 2


def main(argv: Sequence[str] | None = None) -> int:
    try:
        parser = _build_parser()
        args = parser.parse_args(argv)
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
    except Exception as exc:
        failure = _invalid(f"unexpected host pipeline failure: {exc}")
        sys.stderr.write(
            json.dumps(
                {"error": failure.to_document()},
                ensure_ascii=False,
                sort_keys=True,
            )
            + "\n"
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
