#!/usr/bin/env python3
"""Fixed PoC18 VAD evidence gate and same-device prepared-feature benchmark."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from dataclasses import replace
from pathlib import Path

import numpy as np

import npu_onnx_native_lab as lab

PREPARATION_SHA256 = "4fcc00ccc0f1983f0a43c88053586da7ec3b5154ff13e4829f55972912db38cb"
FULL_MODEL_SHA256 = "f5d1cdfe2c1f3ae8b3feb3ffa7568403839a7c59e8ff363846e3fde46cfad46a"
ORT_PATH = "/usr/lib/libonnxruntime.so.1.15.1"
ORT_SHA256 = "457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639"
HEADER = (
    lab.REPOSITORY_ROOT
    / "artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/onnxruntime_c_api.h"
)
HEADER_SHA256 = "cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91"
CLIPS = ("speech", "silence", "noise", "noisy-speech")


class BenchmarkError(lab.NativeLabError):
    """A fixed reference, live result or benchmark admission check failed."""


def _preparation(directory: Path) -> dict:
    path = directory / "preparation.json"
    if lab._digest(lab._regular(path)) != PREPARATION_SHA256:
        raise BenchmarkError("frozen preparation receipt changed")
    receipt = lab._json(path)
    for name, identity in receipt["files"].items():
        if Path(name).name != name:
            raise BenchmarkError("invalid prepared artifact name")
        payload = lab._regular(directory / name)
        if (
            len(payload) != identity["bytes"]
            or lab._digest(payload) != identity["sha256"]
        ):
            raise BenchmarkError(f"prepared artifact changed: {name}")
    return receipt


def compare_vad(reference: bytes, observed: bytes) -> dict:
    if len(reference) != 1600 or len(observed) != 800:
        raise BenchmarkError("VAD logits must be FP32/FP16 [1,2,200]")
    original = np.frombuffer(reference, dtype="<f4").reshape(1, 2, 200)
    native = np.frombuffer(observed, dtype="<f2").reshape(1, 2, 200).astype(np.float64)
    if not np.all(np.isfinite(original)) or not np.all(np.isfinite(native)):
        raise BenchmarkError("VAD logits contain nonfinite values")

    def probabilities(logits):
        logits = logits.astype(np.float64)
        values = np.exp(logits - np.max(logits, axis=1, keepdims=True))
        return (values / np.sum(values, axis=1, keepdims=True))[0, 1]

    cpu_probabilities, native_probabilities = (
        probabilities(original),
        probabilities(native),
    )
    error = float(np.max(np.abs(cpu_probabilities - native_probabilities)))
    disagreements = int(
        np.count_nonzero((cpu_probabilities >= 0.5) != (native_probabilities >= 0.5))
    )
    agreement = 1 - disagreements / 200
    result = {
        "status": "PASS" if error <= 0.02 and agreement >= 0.99 else "FAIL",
        "max_abs_error": error,
        "decision_agreement": agreement,
        "disagreements": disagreements,
        "frames": 200,
        "threshold": 0.5,
        "original_speech_frames": int(np.count_nonzero(cpu_probabilities >= 0.5)),
        "native_speech_frames": int(np.count_nonzero(native_probabilities >= 0.5)),
        "original_output_sha256": lab._digest(reference),
        "native_output_sha256": lab._digest(observed),
        "policy": {"max_abs_error": 0.02, "minimum_decision_agreement": 0.99},
    }
    if result["status"] != "PASS":
        raise BenchmarkError(
            "native speech probability/decision gate failed: " + json.dumps(result)
        )
    return result


def vad_gate(package_dir: Path, prepared: Path, runs: dict[str, Path]) -> dict:
    _preparation(prepared)
    package = lab.load_package(package_dir, require_qualification=True)
    if package["record"]["source_model_sha256"] != FULL_MODEL_SHA256 or set(
        runs
    ) != set(CLIPS):
        raise BenchmarkError("four frozen clips and qualified full model required")
    clips = {}
    for clip in CLIPS:
        directory = runs[clip]
        envelope = lab._json(directory / "run.json")
        lab._successful_run(envelope)
        if envelope["execution_attempts"] != 1 or envelope["result"]["iterations"] != 1:
            raise BenchmarkError("clip requires exactly one native Run")
        identity = lab._json(directory / "native-identity.json")
        with np.load(prepared / f"18d_{clip}_input.npz", allow_pickle=False) as archive:
            inputs = {"input": np.array(archive["input"], copy=True, order="C")}
            input_bytes = inputs["input"].tobytes()
        _, independent = lab._cpu_expected(package, inputs, verify_onnx_runtime=False)
        stage_dir = (
            Path(package["directory"]) / "evidence" / (envelope["run_id"] + "-stage")
        )
        if lab._regular(stage_dir / "input.bin") != input_bytes:
            raise BenchmarkError("clip staged input changed")
        manifest = lab._execute_manifest(
            package, stage_dir / "input.bin", independent, stage_dir, create=False
        )
        if envelope["manifest_sha256"] != manifest.sha256:
            raise BenchmarkError("clip execution manifest changed")
        before = lab._json(directory / "preflight.json")
        after = lab._json(directory / "postflight.json")
        lab.execution.verify_preflight(manifest, before)
        differences, alerts = lab.health(manifest, before, after)
        if differences or alerts:
            raise BenchmarkError("clip raw device health check failed")
        observed = lab._regular(directory / "output.bin", 800)
        state = after["output_states"][manifest.candidates["output"].output]
        if state != {
            "status": "present",
            "bytes": 800,
            "permissions": "-r--r--r--",
            "sha256": lab._digest(observed),
        }:
            raise BenchmarkError("clip raw output state differs")
        stdout = lab._regular(directory / "stdout.txt").decode()
        sentinels = re.findall(r"__WAVE_EXEC_EXIT_[0-9a-f]{32}__", stdout)
        if len(sentinels) != 1:
            raise BenchmarkError("clip raw exit sentinel differs")
        cleaned, remote_exit = lab.execution.parse_remote_exit(stdout, sentinels[0])
        if (
            remote_exit != 0
            or lab.execution.parse_result(cleaned, manifest) != envelope["result"]
        ):
            raise BenchmarkError("clip raw result differs")
        expected_identity = {
            "package_sha256": package["package_sha256"],
            "source_model_sha256": FULL_MODEL_SHA256,
            "lowering_sha256": package["record"]["lowering_sha256"],
            "nbg_sha256": package["compiled"]["nbg_sha256"],
            "input_sha256": lab._digest(input_bytes),
            "output_sha256": lab._digest(observed),
        }
        if (
            identity != expected_identity
            or envelope["result"]["output_hex"] != observed.hex()
        ):
            raise BenchmarkError("clip native identity/readback differs")
        cleanup = lab._json(directory / "cleanup.json")
        if (
            cleanup.get("status") != "CLEAN"
            or cleanup.get("health_differences")
            or cleanup.get("kernel_alerts")
        ):
            raise BenchmarkError("clip cleanup is not clean")
        comparison = lab._json(directory / "numerical-comparison.json")
        if comparison != lab._compare_output(package["record"], independent, observed):
            raise BenchmarkError("clip numerical evidence differs")
        reference = lab._regular(prepared / f"18d_{clip}_original_fp32.bin", 1600)
        clips[clip] = {
            **compare_vad(reference, observed),
            "run_id": envelope["run_id"],
            "run_sha256": lab._digest(lab._regular(directory / "run.json")),
            "cleanup_sha256": lab._digest(lab._regular(directory / "cleanup.json")),
            "identity_sha256": lab._digest(
                lab._regular(directory / "native-identity.json")
            ),
        }
    return {
        "schema_version": 1,
        "status": "PASS",
        "preparation_sha256": PREPARATION_SHA256,
        "package_sha256": package["package_sha256"],
        "clips": clips,
    }


def ir8_model(payload: bytes) -> bytes:
    import onnx

    model = onnx.load_model_from_string(payload)
    original_ir = model.ir_version
    if original_ir not in (8, 10):
        raise BenchmarkError("expected an IR8 or IR10 export")
    model.ir_version = 8
    result = model.SerializeToString()
    model.ir_version = original_ir
    if model.SerializeToString() != payload:
        raise BenchmarkError("IR compatibility conversion changed other model fields")
    return result


def _build(package: dict, directory: Path) -> dict:
    lab._verify_runtime()
    if lab._digest(lab._regular(HEADER)) != HEADER_SHA256:
        raise BenchmarkError("ORT API15 header changed")
    output = directory / "build"
    output.mkdir()
    remote = lab.remote_root(package["record"], "execute")
    script = r"""export LC_ALL=C SOURCE_DATE_EPOCH=1785196800
aarch64-linux-gnu-g++ -std=c++17 -O2 -g0 -Wall -Wextra -Werror -Wformat=2 -Wshadow \
 -D_FORTIFY_SOURCE=2 -fPIE -fstack-protector-strong \
 -I /work/waved/third_party/TIM-VX/include \
 -I /work/artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include /benchmark/execute.cc \
 -L /work/artifacts/npu/build/poc16/runtime -L /work/waved/third_party/device-sdk/lib \
 -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib -Wl,--allow-shlib-undefined \
 -Wl,--as-needed -Wl,--build-id=sha1 -Wl,--enable-new-dtags "-Wl,-rpath,$1/lib:/usr/lib" \
 -Wl,-z,noexecstack -Wl,-z,now -Wl,-z,relro -pie -ltim-vx -lGAL -ldl -lpthread \
 -o /output/wave-npu-onnx-execute-v1
aarch64-linux-gnu-readelf -W -h -l -d -V /output/wave-npu-onnx-execute-v1
aarch64-linux-gnu-nm -D --undefined-only /output/wave-npu-onnx-execute-v1 | c++filt
"""
    command = [
        "docker",
        "run",
        "--rm",
        "--pull=never",
        "--network=none",
        "--platform=linux/amd64",
        "--volume",
        f"{lab.REPOSITORY_ROOT}:/work:ro",
        "--volume",
        f"{directory}:/benchmark:ro",
        "--volume",
        f"{output}:/output",
        "--workdir",
        "/work",
        lab.IMAGE,
        "bash",
        "-euo",
        "pipefail",
        "-c",
        script,
        "bash",
        remote,
    ]
    result = subprocess.run(command, capture_output=True, text=True, timeout=90)
    lab._write(output / "execute.log", (result.stdout + result.stderr).encode())
    if result.returncode:
        raise BenchmarkError("offline benchmark build failed; log retained")
    lab.validate_elf(result.stdout, remote)
    lab._write(output / "execute.elf.txt", result.stdout.encode())
    receipt = {
        "image_id": lab.IMAGE,
        "tim_vx_commit": lab.TIM_COMMIT,
        "wrapper_sha256": lab.WRAPPER_SHA256,
        "ort_header_sha256": HEADER_SHA256,
        "source_sha256": lab._digest(lab._regular(directory / "execute.cc")),
        "binary_sha256": lab._digest(lab._regular(output / "wave-npu-onnx-execute-v1")),
        "inspection_sha256": lab._digest(lab._regular(output / "execute.elf.txt")),
        "log_sha256": lab._digest(lab._regular(output / "execute.log")),
        "network": "none",
        "source_sdk_mount": "read-only",
    }
    lab._write_json(directory / "build.json", receipt)
    return receipt


def prepare_benchmark(
    package_dir: Path, prepared: Path, gate_path: Path, directory: Path
) -> dict:
    from wave_onnx_hybrid.marblenet_benchmark import generate_benchmark_source
    import onnx
    import onnxruntime as ort

    package = lab.load_package(package_dir, require_qualification=True)
    _preparation(prepared)
    gate = lab._json(gate_path)
    runs = {
        clip: lab.compile_lab.DEFAULT_RUN_ROOT / gate["clips"][clip]["run_id"]
        for clip in CLIPS
    }
    if gate != vad_gate(package_dir, prepared, runs):
        raise BenchmarkError("four-clip gate changed")
    directory.mkdir(parents=True, exist_ok=False)
    cpu_model = ir8_model(lab._regular(prepared / "marblenet_folded_fp32.onnx"))
    lab._write(directory / "cpu.onnx", cpu_model)
    features = np.load(prepared / "speech_features_fp32.npy", allow_pickle=False)
    if (
        features.shape != (1, 80, 400)
        or features.dtype != np.float32
        or not np.all(np.isfinite(features))
    ):
        raise BenchmarkError("CPU feature contract differs")
    options = ort.SessionOptions()
    options.intra_op_num_threads = options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    original = ort.InferenceSession(
        str(prepared / "marblenet_folded_fp32.onnx"),
        options,
        providers=["CPUExecutionProvider"],
    )
    derived = ort.InferenceSession(
        cpu_model, options, providers=["CPUExecutionProvider"]
    )
    original_output = original.run(["output"], {"input": features})[0]
    cpu_output = derived.run(["output"], {"input": features})[0]
    if not np.array_equal(original_output, cpu_output) or not np.all(
        np.isfinite(cpu_output)
    ):
        raise BenchmarkError("IR8 host CPU output changed")
    with np.load(prepared / "18d_speech_input.npz", allow_pickle=False) as archive:
        native_input = np.ascontiguousarray(archive["input"]).tobytes()
    if features.astype("<f2").tobytes() != native_input:
        raise BenchmarkError("CPU/NPU feature precision conversion differs")
    native_reference = bytes.fromhex(package["qualification"]["output_hex"])
    for name, payload in {
        "input.bin": native_input,
        "cpu-input.bin": features.astype("<f4").tobytes(),
        "cpu-reference.bin": cpu_output.astype("<f4").tobytes(),
        "native-reference.bin": native_reference,
    }.items():
        lab._write(directory / name, payload)
    source = generate_benchmark_source(
        package["record"],
        lab.remote_root(package["record"], "execute"),
        package["compiled"]["nbg_bytes"],
        len(cpu_model),
    )
    lab._write(directory / "execute.cc", source.encode())
    build = _build(package, directory)
    files = {
        name: {
            "sha256": lab._digest(lab._regular(directory / name)),
            "bytes": (directory / name).stat().st_size,
        }
        for name in (
            "cpu.onnx",
            "input.bin",
            "cpu-input.bin",
            "cpu-reference.bin",
            "native-reference.bin",
        )
    }
    receipt = {
        "schema_version": 1,
        "package_sha256": package["package_sha256"],
        "preparation_sha256": PREPARATION_SHA256,
        "vad_gate_sha256": lab._digest(lab._regular(gate_path)),
        "cpu_model_original_sha256": lab._digest(
            lab._regular(prepared / "marblenet_folded_fp32.onnx")
        ),
        "cpu_ir_change": {
            "from": onnx.load_model_from_string(
                lab._regular(prepared / "marblenet_folded_fp32.onnx")
            ).ir_version,
            "to": 8,
            "other_fields_identical": True,
            "host_output_exact": True,
        },
        "host_ort_version": ort.__version__,
        "cpu_fp32_policy": {"atol": 0.0001, "rtol": 0.00001},
        "native_output_exact_qualified": True,
        "files": files,
        "build": build,
    }
    lab._write_json(directory / "preparation.json", receipt)
    return receipt


def _manifest(package: dict, directory: Path, receipt: dict):
    remote = lab.remote_root(package["record"], "execute")
    locations = {
        "model/cpu.onnx": "cpu.onnx",
        "input/input.bin": "input.bin",
        "input/cpu-input.bin": "cpu-input.bin",
        "input/cpu-reference.bin": "cpu-reference.bin",
        "input/native-reference.bin": "native-reference.bin",
    }
    payloads = {
        remote + "/" + relative: (directory / local, "0444")
        for relative, local in locations.items()
    }
    payloads.update(
        {
            remote + "/bin/wave-npu-onnx-execute-v1": (
                directory / "build/wave-npu-onnx-execute-v1",
                "0555",
            ),
            remote + "/lib/libtim-vx.so": (lab.RUNTIME, "0444"),
            remote + "/model/model.nbg": (
                Path(package["directory"]) / "model.nbg",
                "0444",
            ),
        }
    )
    artifacts = {
        **lab.SYSTEM_HASHES,
        ORT_PATH: ORT_SHA256,
        **{
            path: lab._digest(lab._regular(local))
            for path, (local, _) in payloads.items()
        },
    }
    expected = lab._regular(directory / "native-reference.bin", 800)
    input_bytes = lab._regular(directory / "input.bin", 64000)
    policy = lab.execution.ExecutionPolicy(
        poc_id="18",
        experiment="wave-npu-poc18-marblenet-benchmark",
        manifest_path=directory / "execution-manifest.json",
        remote_root=remote,
        remote_executable=remote + "/bin/wave-npu-onnx-execute-v1",
        remote_tim_vx=remote + "/lib/libtim-vx.so",
        remote_nbg=remote + "/model/model.nbg",
        fixed_artifacts=artifacts,
        absent_artifacts=(
            "/lib/libCLC.so",
            "/usr/lib/libCLC.so",
            "/usr/lib64/libCLC.so",
            remote + "/lib/libCLC.so",
        ),
        staged_modes={
            path: "-r-xr-xr-x" if mode == "0555" else "-r--r--r--"
            for path, (_, mode) in payloads.items()
        },
        nbg_bytes=package["compiled"]["nbg_bytes"],
        input_values=tuple(input_bytes),
        output_values=tuple(expected),
        expected_iterations=36,
        execution_re=re.compile(
            r"^EXECUTION status=PASS mode=execute-once target=wave operation=fp16-marblenet-benchmark "
            r"iterations=36 input_bytes=64000 output_bytes=800 graph_us=(?P<graph_us>[0-9]+) "
            r"output_hex=(?P<output_hex>" + expected.hex() + r") cleanup=0$"
        ),
    )
    document = {
        "schema_version": 1,
        "device": lab.DEVICE,
        "environment": {
            "LD_LIBRARY_PATH": remote + "/lib:/usr/lib",
            "VIVANTE_SDK_DIR": "/usr",
            "VSI_NN_LOG_LEVEL": "5",
        },
        "remote_executable": policy.remote_executable,
        "remote_argument": "--execute-once",
        "timeout_ms": 12000,
        "required_artifacts": artifacts,
        "absent_artifacts": list(policy.absent_artifacts),
        "expected_result": {
            "identity": lab.execution.EXPECTED_IDENTITY,
            "input": list(input_bytes),
            "output": list(expected),
            "bytes": 800,
            "iterations": 36,
        },
    }
    if policy.manifest_path.exists():
        if lab._json(policy.manifest_path) != document:
            raise BenchmarkError("benchmark manifest changed")
    else:
        lab._write_json(policy.manifest_path, document)
    manifest = lab.execution.load_manifest(policy=policy)
    candidate = lab.compile_lab.Candidate(
        "output",
        "--execute-once",
        remote + "/output/output.bin",
        remote + "/output/.output.bin.partial",
    )
    return replace(manifest, candidates={"output": candidate}), payloads


def run_benchmark(
    package_dir: Path,
    prepared: Path,
    gate_path: Path,
    directory: Path,
    run_id: str,
    expected_receipt_sha256: str,
) -> dict:
    from wave_onnx_hybrid.marblenet_benchmark import (
        parse_benchmark_samples,
        validate_benchmark_source,
    )

    lab._run_id(run_id)
    package = lab.load_package(package_dir, require_qualification=True)
    _preparation(prepared)
    gate = lab._json(gate_path)
    runs = {
        clip: lab.compile_lab.DEFAULT_RUN_ROOT / gate["clips"][clip]["run_id"]
        for clip in CLIPS
    }
    if gate != vad_gate(package_dir, prepared, runs):
        raise BenchmarkError("four-clip gate changed")
    if (
        not re.fullmatch(r"[0-9a-f]{64}", expected_receipt_sha256)
        or lab._digest(lab._regular(directory / "preparation.json"))
        != expected_receipt_sha256
    ):
        raise BenchmarkError("reviewed benchmark preparation changed")
    receipt = lab._json(directory / "preparation.json")
    if receipt["package_sha256"] != package["package_sha256"] or receipt[
        "vad_gate_sha256"
    ] != lab._digest(lab._regular(gate_path)):
        raise BenchmarkError("benchmark qualification binding changed")
    if set(receipt["files"]) != {
        "cpu.onnx",
        "input.bin",
        "cpu-input.bin",
        "cpu-reference.bin",
        "native-reference.bin",
    }:
        raise BenchmarkError("benchmark file inventory changed")
    if lab._regular(directory / "cpu.onnx") != ir8_model(
        lab._regular(prepared / "marblenet_folded_fp32.onnx")
    ):
        raise BenchmarkError("benchmark CPU graph changed")
    features = np.load(prepared / "speech_features_fp32.npy", allow_pickle=False)
    if (
        lab._regular(directory / "cpu-input.bin") != features.astype("<f4").tobytes()
        or lab._regular(directory / "input.bin") != features.astype("<f2").tobytes()
    ):
        raise BenchmarkError("benchmark speech features changed")
    if lab._regular(directory / "native-reference.bin") != bytes.fromhex(
        package["qualification"]["output_hex"]
    ):
        raise BenchmarkError("benchmark native reference changed")
    for name, identity in receipt["files"].items():
        payload = lab._regular(directory / name)
        if (
            len(payload) != identity["bytes"]
            or lab._digest(payload) != identity["sha256"]
        ):
            raise BenchmarkError("benchmark input changed")
    for name, key in (
        ("execute.cc", "source_sha256"),
        ("build/wave-npu-onnx-execute-v1", "binary_sha256"),
        ("build/execute.elf.txt", "inspection_sha256"),
        ("build/execute.log", "log_sha256"),
    ):
        if lab._digest(lab._regular(directory / name)) != receipt["build"][key]:
            raise BenchmarkError("benchmark build changed")
    validate_benchmark_source(
        package["record"],
        lab.remote_root(package["record"], "execute"),
        package["compiled"]["nbg_bytes"],
        receipt["files"]["cpu.onnx"]["bytes"],
        lab._regular(directory / "execute.cc").decode(),
    )
    lab.validate_elf(
        lab._regular(directory / "build/execute.elf.txt").decode(),
        lab.remote_root(package["record"], "execute"),
    )
    attempt = directory / "attempt.json"
    lab._write_json(
        attempt,
        {
            "run_id": run_id,
            "preparation_sha256": lab._digest(
                lab._regular(directory / "preparation.json")
            ),
        },
    )
    manifest, payloads = _manifest(package, directory, receipt)
    stage_dir = directory / "stage"
    stage_dir.mkdir(exist_ok=False)
    remote = manifest.policy.remote_root
    lab._stage(manifest, payloads, remote, stage_dir, execute=True)
    envelope, run_directory = lab.execution.run_once(
        lab.npu_lab.AdbTransport(), manifest, run_id
    )
    lab._successful_run(envelope)
    before, after = (
        lab._json(run_directory / "preflight.json"),
        lab._json(run_directory / "postflight.json"),
    )
    differences, alerts = lab.health(manifest, before, after)
    if differences or alerts:
        raise BenchmarkError("benchmark changed device health")
    state = after["output_states"][manifest.candidates["output"].output]
    if (
        state.get("status") != "present"
        or state.get("bytes") != 800
        or state.get("permissions") != "-r--r--r--"
    ):
        raise BenchmarkError("benchmark output contract differs")
    lab._adb(
        manifest,
        "pull",
        manifest.candidates["output"].output,
        str(run_directory / "output.bin"),
    )
    actual = lab._regular(run_directory / "output.bin", 800)
    if (
        actual != lab._regular(directory / "native-reference.bin", 800)
        or lab._digest(actual) != state.get("sha256")
        or envelope["result"]["output_hex"] != actual.hex()
    ):
        raise BenchmarkError("benchmark output differs from qualified output")
    # Native PASS is safe to clean even if host sample interpretation refuses.
    lab._cleanup(manifest, payloads, remote, envelope, run_directory, execute=True)
    samples = parse_benchmark_samples(
        lab._regular(run_directory / "stdout.txt").decode()
    )
    timings = {}
    for key in samples[0]:
        if key.endswith("_ns"):
            values = (
                np.array([sample[key] for sample in samples], dtype=np.float64) / 1e6
            )
            timings[key.removesuffix("_ns") + "_ms"] = {
                "median": float(np.median(values)),
                "p95": float(np.percentile(values, 95)),
                "min": float(values.min()),
                "max": float(values.max()),
            }
    result = {
        "schema_version": 1,
        "status": "PASS",
        "run_id": run_id,
        "native_runs": 36,
        "warmup_pairs": 4,
        "measured_pairs": 32,
        "cpu_provider": "CPUExecutionProvider",
        "cpu_ort_version": "1.15.1",
        "feature_extraction_excluded": True,
        "float32_to_float16_input_conversion_excluded": True,
        "cpu_model_precision": "FLOAT32",
        "native_model_precision": "FLOAT16",
        "run_sha256": lab._digest(lab._regular(run_directory / "run.json")),
        "stdout_sha256": lab._digest(lab._regular(run_directory / "stdout.txt")),
        "cleanup_sha256": lab._digest(lab._regular(run_directory / "cleanup.json")),
        "samples": samples,
        "timings": timings,
        "boundary_speedup": timings["cpu_boundary_ms"]["median"]
        / timings["npu_boundary_ms"]["median"],
        "thread_cpu_reduction_fraction": 1
        - timings["npu_thread_ms"]["median"] / timings["cpu_thread_ms"]["median"],
    }
    lab._write_json(directory / "results.json", result)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "run"))
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--gate", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--run-id")
    parser.add_argument("--expected-preparation-sha256")
    args = parser.parse_args(argv)
    if args.command == "prepare":
        prepare_benchmark(args.package, args.prepared, args.gate, args.directory)
    else:
        if not args.run_id or not args.expected_preparation_sha256:
            parser.error("run requires --run-id and --expected-preparation-sha256")
        run_benchmark(
            args.package,
            args.prepared,
            args.gate,
            args.directory,
            args.run_id,
            args.expected_preparation_sha256,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
