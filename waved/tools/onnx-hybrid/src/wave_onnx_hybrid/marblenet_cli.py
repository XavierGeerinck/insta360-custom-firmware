"""File-oriented MarbleNet VAD with CPU features and postprocessing.

Torch is imported only for actual feature preparation. Strict inference delegates
one neural dispatch to NativeSession and never constructs a CPU neural session.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import os
import stat
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import numpy as np
import soundfile as sf

from .audit import canonical_json
from .cli import _write_exclusive
from .errors import ErrorCode, HybridError
from .fixtures import deterministic_npz_bytes
from .native_backend import NativeSession, native_bridge, validate_native_run_id
from .runtime import ExecutionMode

CHECKPOINT_SHA256 = "84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706"
FULL_MODEL_SHA256 = "f5d1cdfe2c1f3ae8b3feb3ffa7568403839a7c59e8ff363846e3fde46cfad46a"
NEMO_REVISION = "ceeafa4f2b8cece8a0fd47b29051ad659e55b76c"
SAMPLES = 63840


def _require(ok: bool, message: str, code=ErrorCode.MODEL_PACKAGE_INVALID) -> None:
    if not ok:
        raise HybridError(code, message)


def _sha(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _model_helper() -> Any:
    try:
        return importlib.import_module("wave_onnx_hybrid.marblenet_model")
    except ImportError as exc:
        raise HybridError(
            ErrorCode.MODEL_STATE_VERSION_MISMATCH,
            "MarbleNet features require the isolated PoC18 Torch2.5.1 environment",
        ) from exc


def _hash_handle(handle: Any) -> str:
    result = hashlib.sha256()
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        result.update(block)
    return result.hexdigest()


def _hash_file(path: Path) -> str:
    with path.open("rb") as handle:
        _require(
            stat.S_ISREG(os.fstat(handle.fileno()).st_mode),
            "input must be a regular file",
        )
        return _hash_handle(handle)


def _paths(
    checkpoint: Path, package: Path, audio: Path, outputs: Path, evidence: Path
) -> tuple[Path, ...]:
    for output in (outputs, evidence):
        if Path(output).is_symlink():
            raise HybridError(ErrorCode.OUTPUT_EXISTS, f"output is linked: {output}")
    values = tuple(
        Path(p).resolve() for p in (checkpoint, package, audio, outputs, evidence)
    )
    checkpoint, package, audio, outputs, evidence = values
    _require(
        len(set(values)) == len(values),
        "input, package and output paths must differ",
        ErrorCode.POLICY_INVALID,
    )
    for output in (outputs, evidence):
        _require(
            package not in output.parents,
            "outputs must be outside the native package",
            ErrorCode.POLICY_INVALID,
        )
        if output.exists() or output.is_symlink():
            raise HybridError(
                ErrorCode.OUTPUT_EXISTS, f"output already exists: {output}"
            )
    _require(
        checkpoint.is_file() and audio.is_file() and package.is_dir(),
        "checkpoint/audio/package is missing",
    )
    return values


def _audio_window(path: Path, offset: int) -> tuple[np.ndarray, dict[str, Any]]:
    _require(
        type(offset) is int and offset >= 0,
        "offset must be a nonnegative sample index",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    try:
        with path.open("rb") as handle:
            before = os.fstat(handle.fileno())
            _require(
                stat.S_ISREG(before.st_mode),
                "audio must be a regular file",
                ErrorCode.ONNX_INPUT_INVALID,
            )
            digest = _hash_handle(handle)
            handle.seek(0)
            with sf.SoundFile(handle) as source:
                _require(
                    source.format in {"WAV", "WAVEX"}
                    and source.samplerate == 16000
                    and source.channels == 1,
                    "audio requires a mono16kHz WAV; resampling is not implicit",
                    ErrorCode.ONNX_INPUT_INVALID,
                )
                frames = source.frames
                _require(
                    frames >= offset + SAMPLES,
                    "audio is shorter than the selected63840-sample window",
                    ErrorCode.ONNX_INPUT_INVALID,
                )
                source.seek(offset)
                wave = source.read(SAMPLES, dtype="float32", always_2d=False)
            after = os.fstat(handle.fileno())
            _require(
                (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns),
                "audio changed while reading",
                ErrorCode.ONNX_INPUT_INVALID,
            )
    except HybridError:
        raise
    except (OSError, ValueError, RuntimeError) as exc:
        raise HybridError(
            ErrorCode.ONNX_INPUT_INVALID, f"cannot read audio: {exc}"
        ) from exc
    wave = np.ascontiguousarray(wave, dtype=np.float32)
    _require(
        wave.shape == (SAMPLES,) and np.isfinite(wave).all(),
        "audio window must contain finite mono samples",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    return wave, {
        "path": str(path),
        "file_sha256": digest,
        "selected_samples_sha256": _sha(wave.tobytes()),
        "selection": {
            "offset_samples": offset,
            "samples": SAMPLES,
            "sample_rate": 16000,
            "channels": 1,
            "source_frames": frames,
            "origin_seconds": offset / 16000,
            "end_seconds": (offset + SAMPLES) / 16000,
        },
    }


def _full_package(
    package: Path, loader: Callable[..., dict[str, Any]], *, require_qualification: bool
) -> dict[str, Any]:
    try:
        loaded = loader(package, require_qualification=require_qualification)
    except HybridError:
        raise
    except Exception as exc:
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID, f"qualified package loading failed: {exc}"
        ) from exc
    _require(
        isinstance(loaded, dict)
        and Path(loaded.get("directory", "/invalid")).resolve() == package,
        "package directory identity mismatch",
    )
    record = loaded.get("record", {})
    _require(
        record.get("profile_id") == "fp16-marblenet-v1"
        and record.get("source_model_sha256") == FULL_MODEL_SHA256
        and record.get("onnx_input")
        == {
            "name": "input",
            "dtype": "FLOAT16",
            "shape": [1, 80, 400],
            "byte_size": 64000,
        }
        and record.get("onnx_output")
        == {
            "name": "output",
            "dtype": "FLOAT16",
            "shape": [1, 2, 200],
            "byte_size": 800,
        },
        "frontend requires the exact trained full MarbleNet graph",
    )
    compiled = loaded.get("compiled")
    qualification = loaded.get("qualification")
    if compiled is not None:
        _require(
            isinstance(compiled, dict) and compiled.get("status") == "PASS",
            "compiled MarbleNet evidence is invalid",
        )
    if qualification is not None:
        _require(
            isinstance(compiled, dict)
            and isinstance(qualification, dict)
            and qualification.get("status") == "PASS"
            and compiled.get("nbg_sha256") == qualification.get("nbg_sha256"),
            "MarbleNet qualification is invalid",
        )
    if require_qualification:
        _require(qualification is not None, "full MarbleNet package is not qualified")
    _require(
        _hash_file(package / "model.onnx") == FULL_MODEL_SHA256,
        "full ONNX model identity changed",
        ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
    )
    return loaded


def run_marblenet(
    *,
    checkpoint: Path,
    package: Path,
    audio: Path,
    offset_samples: int,
    run_id: str,
    mode: str,
    outputs: Path,
    evidence: Path,
    package_loader: Callable[..., dict[str, Any]] | None = None,
    session_factory: Callable[..., Any] | None = None,
    model_helper: Any | None = None,
) -> dict[str, Any]:
    """Read one selected window and dispatch one native or fallback neural run.

    Injected collaborators are for host tests. Strict mode requires qualification.
    Hybrid mode allows the exact unqualified full graph on CPU and delegates
    latched preflight/execution fallback to NativeSession.
    """
    start = time.perf_counter()
    validate_native_run_id(run_id)
    _require(
        mode in {"strict", "hybrid"},
        "frontend requires strict or hybrid mode",
        ErrorCode.POLICY_INVALID,
    )
    checkpoint, package, audio, outputs, evidence = _paths(
        checkpoint, package, audio, outputs, evidence
    )
    audio_start = time.perf_counter()
    wave, audio_evidence = _audio_window(audio, offset_samples)
    audio_seconds = time.perf_counter() - audio_start
    loader = package_loader or native_bridge().load_package
    validation_start = time.perf_counter()
    loaded = _full_package(package, loader, require_qualification=mode == "strict")
    qualification_initial_sha = (
        _hash_file(package / "qualification.json")
        if loaded["qualification"] is not None
        else None
    )
    loaded_identity = canonical_json(
        {
            "record": loaded["record"],
            "package_sha256": loaded["package_sha256"],
            "compiled": loaded["compiled"],
            "qualification": loaded["qualification"],
        }
    )
    validation_seconds = time.perf_counter() - validation_start
    feature_start = time.perf_counter()
    _require(
        _hash_file(checkpoint) == CHECKPOINT_SHA256,
        "checkpoint identity mismatch",
        ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
    )
    helper = model_helper or _model_helper()
    _require(
        str(helper.torch.__version__).split("+")[0] == "2.5.1",
        "features require pinned Torch2.5.1",
        ErrorCode.MODEL_STATE_VERSION_MISMATCH,
    )
    checkpoint_data = helper.load_checkpoint(checkpoint)
    _require(
        checkpoint_data.get("checkpoint_sha256") == CHECKPOINT_SHA256,
        "loaded checkpoint identity mismatch",
        ErrorCode.MODEL_WEIGHT_HASH_MISMATCH,
    )
    features = helper.preprocess_audio(wave, checkpoint_data["state"])
    _require(
        isinstance(features, np.ndarray)
        and features.dtype == np.float32
        and features.shape == (1, 80, 400)
        and np.isfinite(features).all(),
        "feature contract mismatch",
        ErrorCode.ONNX_INPUT_INVALID,
    )
    half = np.ascontiguousarray(features, dtype="<f2")
    _require(
        np.isfinite(half).all(), "nonfinite FP16 features", ErrorCode.ONNX_INPUT_INVALID
    )
    feature_seconds = time.perf_counter() - feature_start
    dispatch_start = time.perf_counter()
    factory = session_factory or NativeSession
    session = factory(
        package / "model.onnx", package, run_id=run_id, mode=ExecutionMode(mode)
    )
    result = session.run({"input": half})
    # Recheck host evidence after the single dispatch. This performs no ADB/ORT.
    current = _full_package(package, loader, require_qualification=mode == "strict")
    current_identity = canonical_json(
        {
            "record": current["record"],
            "package_sha256": current["package_sha256"],
            "compiled": current["compiled"],
            "qualification": current["qualification"],
        }
    )
    _require(
        current_identity == loaded_identity,
        "package changed during dispatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    qualification_sha = (
        _hash_file(package / "qualification.json")
        if current["qualification"] is not None
        else None
    )
    _require(
        qualification_sha == qualification_initial_sha,
        "qualification changed during dispatch",
        ErrorCode.EVIDENCE_INVALID,
    )
    dispatch_seconds = time.perf_counter() - dispatch_start
    post_start = time.perf_counter()
    _require(
        isinstance(result, dict) and set(result) == {"output"},
        "backend output names differ",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    logits = result["output"]
    _require(
        isinstance(logits, np.ndarray)
        and logits.dtype == np.dtype("<f2")
        and logits.shape == (1, 2, 200)
        and np.isfinite(logits).all(),
        "backend logits contract mismatch",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    logits = np.ascontiguousarray(logits)
    probability = helper.probabilities(logits)
    _require(
        isinstance(probability, np.ndarray)
        and probability.dtype == np.float32
        and probability.shape == (200,)
        and np.isfinite(probability).all()
        and (probability >= 0).all()
        and (probability <= 1).all(),
        "speech probability contract mismatch",
        ErrorCode.ONNX_OUTPUT_INVALID,
    )
    probability = np.ascontiguousarray(probability)
    speech = probability >= np.float32(0.5)
    execution = session.evidence
    _require(
        isinstance(execution, dict)
        and execution.get("providers") == session.providers
        and execution.get("providers")
        in [["WaveNativeBackend"], ["CPUExecutionProvider"]],
        "backend provider evidence differs",
        ErrorCode.EVIDENCE_INVALID,
    )
    _require(
        execution.get("model_sha256") == FULL_MODEL_SHA256,
        "backend model identity differs",
        ErrorCode.EVIDENCE_INVALID,
    )
    nbg_sha = (loaded["compiled"] or {}).get("nbg_sha256")
    if execution["providers"] == ["WaveNativeBackend"]:
        _require(
            loaded["qualification"] is not None
            and execution.get("package_sha256") == loaded["package_sha256"]
            and execution.get("nbg_sha256") == nbg_sha,
            "native backend qualification identity differs",
            ErrorCode.EVIDENCE_INVALID,
        )
    else:
        _require(
            mode == "hybrid"
            and type(execution.get("cpu_sessions")) is int
            and execution["cpu_sessions"] == 1,
            "CPU fallback is allowed only in hybrid mode",
            ErrorCode.EVIDENCE_INVALID,
        )
        if execution.get("package_sha256") is None:
            _require(
                execution.get("nbg_sha256") is None
                and execution.get("classification") == "cpu-native-preflight-fallback"
                and execution.get("native_attempts") == 0
                and execution.get("native_calls") == 0
                and bool(execution.get("npu_error")),
                "CPU preflight fallback evidence differs",
                ErrorCode.EVIDENCE_INVALID,
            )
        else:
            _require(
                execution.get("package_sha256") == loaded["package_sha256"]
                and execution.get("nbg_sha256") == nbg_sha,
                "CPU fallback package identity differs",
                ErrorCode.EVIDENCE_INVALID,
            )
    if mode == "strict":
        _require(
            execution.get("providers") == ["WaveNativeBackend"]
            and type(execution.get("cpu_sessions")) is int
            and execution.get("cpu_sessions") == 0
            and type(execution.get("native_calls")) is int
            and execution.get("native_calls") == 1
            and type(execution.get("native_attempts")) is int
            and execution.get("native_attempts") == 1
            and execution.get("classification") == "npu-strict",
            "strict frontend did not complete one native neural run",
            ErrorCode.PARTITION_NOT_NPU_RESIDENT,
        )
    payload = deterministic_npz_bytes(
        {"logits": logits, "speech_probability": probability, "speech": speech}
    )
    post_seconds = time.perf_counter() - post_start
    record = loaded["record"]
    document = {
        "schema_version": 1,
        "operation": "offline-marblenet-vad",
        "audio": audio_evidence,
        "checkpoint": {
            "path": str(checkpoint),
            "sha256": CHECKPOINT_SHA256,
            "nemo_source_revision": NEMO_REVISION,
        },
        "model": {
            "sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
            "graph_signature": record["graph_signature"],
            "profile_id": record["profile_id"],
        },
        "package": {
            "directory": str(package),
            "sha256": loaded["package_sha256"],
            "nbg_sha256": nbg_sha,
            "qualification_sha256": qualification_sha,
            "qualification_status": "PASS"
            if loaded["qualification"] is not None
            else "UNQUALIFIED",
            "compiled_status": (loaded["compiled"] or {}).get("status"),
        },
        "input": {
            "features_fp32_sha256": _sha(np.ascontiguousarray(features).tobytes()),
            "features_fp16_sha256": _sha(half.tobytes()),
            "shape": [1, 80, 400],
        },
        "execution": execution,
        "outputs": {
            "path": str(outputs),
            "npz_sha256": _sha(payload),
            "logits_sha256": _sha(logits.tobytes()),
            "speech_probability_sha256": _sha(probability.tobytes()),
            "speech_sha256": _sha(speech.tobytes()),
            "speech_frames": int(speech.sum()),
            "frames": 200,
            "decision_threshold": 0.5,
            "decision_operator": ">=",
        },
        "context": {
            "mode": "offline-fixed-window",
            "causal_latency_qualified": False,
            "prediction_spacing_seconds": 0.02,
            "receptive_field_feature_frames": 291,
            "future_feature_frames": 145,
            "future_seconds_without_stft": 1.45,
            "future_seconds_with_stft_approx": 1.4625,
            "edge_context": "symmetric zero context at convolution boundaries",
        },
        "timing": {
            "audio_read_hash_decode_wall_seconds": audio_seconds,
            "initial_package_validation_wall_seconds": validation_seconds,
            "cpu_feature_preparation_wall_seconds": feature_seconds,
            "host_dispatch_including_validation_wall_seconds": dispatch_seconds,
            "cpu_postprocessing_serialization_wall_seconds": post_seconds,
            "host_before_output_write_wall_seconds": time.perf_counter() - start,
            "scope": (
                "host wall times; dispatch includes validation, CPU oracle, "
                "transport and device work; native graph_us is not end-to-end"
            ),
        },
    }
    _write_exclusive(outputs, payload)
    try:
        _write_exclusive(evidence, canonical_json(document))
    except BaseException:
        outputs.unlink(missing_ok=True)
        raise
    return document


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    run = sub.add_parser(
        "run", help="run one full-model window, with explicit strict or hybrid policy"
    )
    run.add_argument("--checkpoint", type=Path, required=True)
    run.add_argument("--package", type=Path, required=True)
    run.add_argument("--audio", type=Path, required=True)
    run.add_argument("--offset-samples", type=int, default=0)
    run.add_argument("--run-id", required=True)
    run.add_argument("--mode", choices=["strict", "hybrid"], default="strict")
    run.add_argument("--outputs", type=Path, required=True)
    run.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        document = run_marblenet(
            checkpoint=args.checkpoint,
            package=args.package,
            audio=args.audio,
            offset_samples=args.offset_samples,
            run_id=args.run_id,
            mode=args.mode,
            outputs=args.outputs,
            evidence=args.evidence,
        )
    except HybridError as exc:
        sys.stderr.write(canonical_json(exc.to_document()).decode())
        return 2
    except (OSError, ValueError) as exc:
        error = HybridError(ErrorCode.MODEL_PACKAGE_INVALID, str(exc))
        sys.stderr.write(canonical_json(error.to_document()).decode())
        return 2
    print(f"wrote {args.outputs} ({document['execution']['providers'][0]})")
    print(f"wrote {args.evidence}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
