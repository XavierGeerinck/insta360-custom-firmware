"""Deterministic stream bridge between DFN3 and the learned Erf selector."""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

from .audit import SHA256_RE, canonical_json
from .erf_mask_inference import (
    DFN_CLEAN_RETAIN_DB,
    DFN_CLEAN_RETAIN_POWER_RATIO,
    DfnErfFrameSelector,
)
from .erf_mask_metrics import (
    EVALUATION_CATEGORIES,
    aggregate_evaluation,
    evaluate_audio_item,
)
from .erf_mask_signal import HOP_SAMPLES, SAMPLE_RATE
from .errors import ErrorCode, HybridError

DFN_ATTENUATION_LIMIT_DB = 8.0
DFN_OUTPUT_DELAY_SAMPLES = HOP_SAMPLES
STREAM_GUARD_SAMPLES = SAMPLE_RATE * 2
STREAM_SCHEMA_VERSION = 1

_STREAM_KEYS = frozenset(
    {
        "schema_version",
        "sample_rate",
        "hop_samples",
        "sample_dtype",
        "guard_samples",
        "item_count",
        "total_samples",
        "raw_bytes",
        "raw_sha256",
        "items",
    }
)
_ITEM_KEYS = frozenset(
    {
        "index",
        "identity",
        "category",
        "stream_offset_samples",
        "samples",
        "sha256",
        "metadata",
    }
)


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _readonly_f32(samples: np.ndarray, *, name: str) -> np.ndarray:
    values = np.asarray(samples)
    if (
        values.ndim != 1
        or values.size == 0
        or values.size % HOP_SAMPLES != 0
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid(f"{name} must be a nonempty hop-aligned finite float vector")
    result = np.ascontiguousarray(values, dtype=np.float32)
    result.flags.writeable = False
    return result


def _f32_bytes(samples: np.ndarray) -> bytes:
    return np.ascontiguousarray(samples, dtype="<f4").tobytes()


def _f32_sha256(samples: np.ndarray) -> str:
    return hashlib.sha256(_f32_bytes(samples)).hexdigest()


@dataclass(frozen=True, slots=True)
class EvaluationAudioItem:
    identity: str
    category: str
    clean: np.ndarray = field(repr=False)
    noise: np.ndarray = field(repr=False)
    mixture: np.ndarray = field(repr=False)
    metadata: Mapping[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        clean = _readonly_f32(self.clean, name="evaluation clean")
        noise = _readonly_f32(self.noise, name="evaluation noise")
        mixture = _readonly_f32(self.mixture, name="evaluation mixture")
        if (
            not self.identity
            or self.category not in EVALUATION_CATEGORIES
            or clean.shape != noise.shape
            or clean.shape != mixture.shape
            or not np.allclose(
                clean.astype(np.float64) + noise.astype(np.float64),
                mixture.astype(np.float64),
                atol=2e-6,
                rtol=1e-6,
            )
        ):
            raise _invalid("invalid evaluation audio item contract")
        try:
            metadata = json.loads(canonical_json(self.metadata))
        except (HybridError, TypeError, ValueError) as exc:
            raise _invalid("evaluation item metadata is not strict JSON") from exc
        if not isinstance(metadata, dict):
            raise _invalid("evaluation item metadata must be an object")
        object.__setattr__(self, "clean", clean)
        object.__setattr__(self, "noise", noise)
        object.__setattr__(self, "mixture", mixture)
        object.__setattr__(self, "metadata", metadata)


def _validate_item_again(item: EvaluationAudioItem) -> None:
    for name in ("clean", "noise", "mixture"):
        values = np.asarray(getattr(item, name))
        if (
            values.dtype != np.dtype(np.float32)
            or values.ndim != 1
            or values.size == 0
            or values.size % HOP_SAMPLES != 0
            or not np.isfinite(values).all()
        ):
            raise _invalid(f"evaluation item {item.identity} changed after validation")
    if not np.allclose(
        item.clean.astype(np.float64) + item.noise.astype(np.float64),
        item.mixture.astype(np.float64),
        atol=2e-6,
        rtol=1e-6,
    ):
        raise _invalid(f"evaluation item {item.identity} mixture changed")


def write_evaluation_stream(
    items: Iterable[EvaluationAudioItem],
    output_path: Path | str,
    *,
    guard_samples: int = STREAM_GUARD_SAMPLES,
) -> dict[str, Any]:
    """Write a guarded raw-f32 stream without retaining the corpus in RAM."""

    if (
        type(guard_samples) is not int
        or guard_samples < DFN_OUTPUT_DELAY_SAMPLES
        or guard_samples % HOP_SAMPLES != 0
    ):
        raise _invalid("stream guard must cover delay and align to one hop")
    path = Path(output_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, 0o644)
    except FileExistsError as exc:
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"evaluation stream already exists: {path}",
        ) from exc

    digest = hashlib.sha256()
    zero_payload = np.zeros(guard_samples, dtype="<f4").tobytes()
    documents: list[dict[str, Any]] = []
    identities: set[str] = set()
    offset = 0
    try:
        with os.fdopen(descriptor, "wb") as output:
            for index, item in enumerate(items):
                if not isinstance(item, EvaluationAudioItem):
                    raise _invalid("evaluation stream item has the wrong type")
                _validate_item_again(item)
                if item.identity in identities:
                    raise _invalid(f"duplicate evaluation identity: {item.identity}")
                identities.add(item.identity)
                output.write(zero_payload)
                digest.update(zero_payload)
                offset += guard_samples
                mixture_payload = _f32_bytes(item.mixture)
                output.write(mixture_payload)
                digest.update(mixture_payload)
                documents.append(
                    {
                        "index": index,
                        "identity": item.identity,
                        "category": item.category,
                        "stream_offset_samples": offset,
                        "samples": item.mixture.size,
                        "sha256": {
                            "clean": _f32_sha256(item.clean),
                            "noise": _f32_sha256(item.noise),
                            "mixture": _f32_sha256(item.mixture),
                        },
                        "metadata": dict(item.metadata),
                    }
                )
                offset += item.mixture.size
            if not documents:
                raise _invalid("evaluation stream cannot be empty")
            output.write(zero_payload)
            digest.update(zero_payload)
            offset += guard_samples
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    return {
        "schema_version": STREAM_SCHEMA_VERSION,
        "sample_rate": SAMPLE_RATE,
        "hop_samples": HOP_SAMPLES,
        "sample_dtype": "FLOAT32_LE",
        "guard_samples": guard_samples,
        "item_count": len(documents),
        "total_samples": offset,
        "raw_bytes": offset * np.dtype(np.float32).itemsize,
        "raw_sha256": digest.hexdigest(),
        "items": documents,
    }


def _hash_file(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    total = 0
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                total += len(chunk)
                digest.update(chunk)
    except OSError as exc:
        raise _invalid(f"cannot hash stream file {path}: {exc}") from exc
    return total, digest.hexdigest()


def verify_evaluation_stream(
    manifest: Mapping[str, Any],
    raw_path: Path | str,
) -> dict[str, Any]:
    """Verify the exact stream schema, item layout, byte count, and hash."""

    if not isinstance(manifest, Mapping) or set(manifest) != _STREAM_KEYS:
        raise _invalid("evaluation stream manifest keys differ")
    try:
        document = json.loads(canonical_json(manifest))
    except HybridError as exc:
        raise _invalid("evaluation stream manifest is not strict JSON") from exc
    path = Path(raw_path)
    try:
        metadata = path.lstat()
    except OSError as exc:
        raise _invalid(f"cannot inspect evaluation stream: {exc}") from exc
    if path.is_symlink() or not path.is_file():
        raise _invalid("evaluation stream must be a regular file")
    scalar_valid = (
        document["schema_version"] == STREAM_SCHEMA_VERSION
        and document["sample_rate"] == SAMPLE_RATE
        and document["hop_samples"] == HOP_SAMPLES
        and document["sample_dtype"] == "FLOAT32_LE"
        and type(document["guard_samples"]) is int
        and document["guard_samples"] >= DFN_OUTPUT_DELAY_SAMPLES
        and document["guard_samples"] % HOP_SAMPLES == 0
        and type(document["item_count"]) is int
        and document["item_count"] > 0
        and type(document["total_samples"]) is int
        and document["total_samples"] > 0
        and type(document["raw_bytes"]) is int
        and document["raw_bytes"] == document["total_samples"] * 4
        and type(document["raw_sha256"]) is str
        and SHA256_RE.fullmatch(document["raw_sha256"]) is not None
        and isinstance(document["items"], list)
        and len(document["items"]) == document["item_count"]
    )
    if not scalar_valid or metadata.st_size != document["raw_bytes"]:
        raise _invalid("evaluation stream scalar contract differs")
    expected_offset = 0
    identities: set[str] = set()
    for index, item in enumerate(document["items"]):
        if (
            not isinstance(item, dict)
            or set(item) != _ITEM_KEYS
            or item["index"] != index
            or type(item["identity"]) is not str
            or not item["identity"]
            or item["identity"] in identities
            or item["category"] not in EVALUATION_CATEGORIES
            or type(item["samples"]) is not int
            or item["samples"] <= 0
            or item["samples"] % HOP_SAMPLES != 0
            or not isinstance(item["sha256"], dict)
            or set(item["sha256"]) != {"clean", "noise", "mixture"}
            or any(
                type(value) is not str or SHA256_RE.fullmatch(value) is None
                for value in item["sha256"].values()
            )
            or not isinstance(item["metadata"], dict)
        ):
            raise _invalid(f"evaluation stream item {index} is invalid")
        identities.add(item["identity"])
        expected_offset += document["guard_samples"]
        if item["stream_offset_samples"] != expected_offset:
            raise _invalid(f"evaluation stream item {index} offset differs")
        expected_offset += item["samples"]
    expected_offset += document["guard_samples"]
    if expected_offset != document["total_samples"]:
        raise _invalid("evaluation stream total layout differs")
    observed_bytes, observed_hash = _hash_file(path)
    if (
        observed_bytes != document["raw_bytes"]
        or observed_hash != document["raw_sha256"]
    ):
        raise _invalid("evaluation stream raw payload hash differs")
    return document


def evaluate_dfn_erf_stream(
    items: Iterable[EvaluationAudioItem],
    stream_manifest: Mapping[str, Any],
    dfn_output_path: Path | str,
    slopes: np.ndarray,
    biases: np.ndarray,
    *,
    feature_partition: Callable[[np.ndarray], np.ndarray] | None = None,
) -> dict[str, Any]:
    """Evaluate aligned DFN output through the learned Erf frame selector."""

    manifest = json.loads(canonical_json(stream_manifest))
    if not isinstance(manifest, dict) or set(manifest) != _STREAM_KEYS:
        raise _invalid("evaluation items do not match the stream manifest")
    output_path = Path(dfn_output_path)
    try:
        output_metadata = output_path.lstat()
    except OSError as exc:
        raise _invalid(f"cannot inspect DFN output stream: {exc}") from exc
    if output_path.is_symlink() or not output_path.is_file():
        raise _invalid("DFN output stream must be a regular file")
    if output_metadata.st_size != manifest["raw_bytes"]:
        raise _invalid("DFN output stream byte count differs")
    output_bytes, output_hash = _hash_file(output_path)
    if output_bytes != manifest["raw_bytes"]:
        raise _invalid("DFN output stream size changed while hashing")
    output = np.memmap(
        output_path,
        mode="r",
        dtype="<f4",
        shape=(manifest["total_samples"],),
    )
    evaluations: list[dict[str, Any]] = []
    partition_calls = 0
    erf_selected_frames = 0
    for item, item_document in zip(items, manifest["items"], strict=True):
        _validate_item_again(item)
        if (
            item.identity != item_document["identity"]
            or item.category != item_document["category"]
            or item.mixture.size != item_document["samples"]
            or {
                "clean": _f32_sha256(item.clean),
                "noise": _f32_sha256(item.noise),
                "mixture": _f32_sha256(item.mixture),
            }
            != item_document["sha256"]
            or dict(item.metadata) != item_document["metadata"]
        ):
            raise _invalid(f"evaluation item changed: {item.identity}")
        start = item_document["stream_offset_samples"] + DFN_OUTPUT_DELAY_SAMPLES
        stop = start + item.mixture.size
        if stop > output.size:
            raise _invalid(f"DFN output is truncated for {item.identity}")
        base = np.asarray(output[start:stop], dtype=np.float32)
        if not np.isfinite(base).all():
            raise _invalid(f"DFN output is non-finite for {item.identity}")
        selector = DfnErfFrameSelector(
            slopes,
            biases,
            feature_partition=feature_partition,
        )
        enhanced = selector.process_audio(
            item.mixture,
            base,
            compensate_delay=True,
        )
        partition_calls += selector.partition_calls
        erf_selected_frames += selector.erf_selected_frames
        evaluation = evaluate_audio_item(
            identity=item.identity,
            category=item.category,
            clean=item.clean,
            noise=item.noise,
            mixture=item.mixture,
            enhanced=enhanced,
        )
        evaluation["base_enhanced_sha256"] = _f32_sha256(base)
        evaluation["metadata"] = dict(item.metadata)
        evaluation["selector"] = {
            "partition_calls": selector.partition_calls,
            "erf_selected_frames": selector.erf_selected_frames,
            "dfn_selected_frames": selector.dfn_selected_frames,
            "erf_selection_fraction": selector.erf_selection_fraction,
        }
        evaluations.append(evaluation)
    del output
    result = aggregate_evaluation(evaluations)
    if feature_partition is None:
        result["architecture"] = "dfn3-cpu-or-learned-erf-causal-frame-selector"
        partition_scope = "quantized-erf-only"
    else:
        result["architecture"] = (
            "dfn3-cpu-or-learned-feature-partition-causal-frame-selector"
        )
        partition_scope = "float-feature-to-u8-mask"
    result["dfn"] = {
        "provider": "deep_filter_cpu",
        "attenuation_limit_db": DFN_ATTENUATION_LIMIT_DB,
        "output_delay_samples": DFN_OUTPUT_DELAY_SAMPLES,
        "output_bytes": output_bytes,
        "output_sha256": output_hash,
    }
    result["selector"] = {
        "decision": "choose_dfn_when_retained_frame_power_is_at_least_threshold",
        "dfn_clean_retain_db": DFN_CLEAN_RETAIN_DB,
        "dfn_clean_retain_power_ratio": DFN_CLEAN_RETAIN_POWER_RATIO,
        "partition_calls": partition_calls,
        "erf_selected_frames": erf_selected_frames,
        "dfn_selected_frames": partition_calls - erf_selected_frames,
        "erf_selection_fraction": (
            erf_selected_frames / partition_calls if partition_calls else 0.0
        ),
    }
    if feature_partition is not None:
        result["selector"]["partition_scope"] = partition_scope
    result["evaluation_stream_sha256"] = manifest["raw_sha256"]
    return result
