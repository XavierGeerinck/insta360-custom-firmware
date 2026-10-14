#!/usr/bin/env python3
"""Pinned stock-ORT CPU-only MarbleNet benchmark with owned staging/cleanup.

No NBG, TIM-VX, GAL dependency or chip-query claim is admitted. The controller
owns invocation; preparation and receipt validation remain entirely host-side.
"""

from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import time
import uuid
from pathlib import Path

import numpy as np
import onnxruntime as ort

import npu_marblenet_benchmark_lab as baseline
import npu_onnx_native_lab as lab

CPU_IDENTITY = {
    "ort_version": "1.15.1",
    "provider": "CPUExecutionProvider",
    "threads": 1,
}
ROOT_ITEMS = {
    "cpu.onnx",
    "cpu-input.bin",
    "cpu-reference.bin",
    "execute.cc",
    "preparation.json",
    "build",
}
BUILD_ITEMS = {"wave-npu-onnx-execute-v1", "execute.log", "execute.elf.txt"}


class CpuLabError(lab.NativeLabError):
    """A CPU-only artifact, output or lifecycle admission check failed."""


def _require(ok, message):
    if not ok:
        raise CpuLabError(message)


def _source():
    lab._host()
    from wave_onnx_hybrid import marblenet_cpu

    return marblenet_cpu


def validate_cpu_elf(report: str, root: str) -> None:
    _require(
        bool(lab.ROOT_RE.fullmatch(root)) and root.endswith("-execute"),
        "invalid CPU root",
    )
    for pattern in (
        r"Class:\s+ELF64",
        r"Data:\s+2's complement, little endian",
        r"Machine:\s+AArch64",
        r"Type:\s+DYN",
        r"GNU_RELRO",
        r"BIND_NOW",
        r"\bdlopen\b",
        r"\bdlsym\b",
    ):
        _require(
            bool(re.search(pattern, report)),
            "CPU ELF architecture/protection/runtime loading policy failed",
        )
    stack = next((line for line in report.splitlines() if "GNU_STACK" in line), "")
    _require(
        bool(re.search(r"\sRW\s", stack))
        and "RWE" not in stack
        and "(TEXTREL)" not in report
        and "(RPATH)" not in report,
        "CPU ELF stack/relocation policy failed",
    )
    needed = set(re.findall(r"\(NEEDED\)\s+Shared library: \[([^]]+)\]", report))
    allowed = {
        "libstdc++.so.6",
        "libgcc_s.so.1",
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "ld-linux-aarch64.so.1",
    }
    _require(
        {"libc.so.6", "libdl.so.2"} <= needed <= allowed,
        "CPU ELF dependency policy failed",
    )
    _require(
        not re.search(
            r"gcoHAL|\bvx[A-Z]\w*|vsi_nn|tim::|libGAL|libOpenVX|libtim-vx", report
        ),
        "CPU ELF contains vendor symbols/libraries",
    )
    _require(
        re.findall(r"\(RUNPATH\)\s+Library runpath: \[([^]]+)\]", report)
        == [root + "/lib:/usr/lib"],
        "CPU ELF RUNPATH changed",
    )
    limits = {
        "GLIBC": (2, 17),
        "GLIBCXX": (3, 4, 21),
        "CXXABI": (1, 3, 9),
        "GCC": (3, 0),
    }
    for namespace, version in re.findall(
        r"\b(GLIBCXX|GLIBC|CXXABI|GCC)_([0-9.]+)", report
    ):
        _require(
            tuple(map(int, version.split("."))) <= limits[namespace],
            "CPU ELF exceeds old device ABI floor",
        )


def compare_cpu_output(reference: bytes, observed: bytes) -> dict:
    _require(
        len(reference) == len(observed) == 1600, "CPU output requires400 FP32 elements"
    )
    expected = np.frombuffer(reference, dtype="<f4")
    actual = np.frombuffer(observed, dtype="<f4")
    _require(
        np.isfinite(expected).all()
        and np.isfinite(actual).all()
        and observed != b"\xa5" * 1600,
        "CPU output nonfinite/untouched",
    )
    error = np.abs(actual.astype(np.float64) - expected.astype(np.float64))
    bad = error > 0.0001 + 0.00001 * np.abs(expected.astype(np.float64))
    _require(not bad.any(), "CPU output exceeds frozen FP32 tolerance")
    return {
        "status": "PASS",
        "reference_sha256": lab._digest(reference),
        "observed_sha256": lab._digest(observed),
        "max_abs_error": float(error.max()),
        "elements": 400,
        "policy": {"atol": 0.0001, "rtol": 0.00001},
    }


def _vad(reference: bytes, observed: bytes) -> dict:
    compare_cpu_output(reference, observed)

    def probability(payload):
        logits = (
            np.frombuffer(payload, dtype="<f4").reshape(1, 2, 200).astype(np.float64)
        )
        values = np.exp(logits - logits.max(axis=1, keepdims=True))
        return (values / values.sum(axis=1, keepdims=True))[0, 1]

    expected, actual = probability(reference), probability(observed)
    error = float(np.max(np.abs(expected - actual)))
    disagreements = int(np.count_nonzero((expected >= 0.5) != (actual >= 0.5)))
    _require(
        error <= 0.02 and disagreements <= 2,
        "CPU VAD gate failed against original FP32",
    )
    return {
        "status": "PASS",
        "max_abs_error": error,
        "decision_agreement": 1 - disagreements / 200,
        "disagreements": disagreements,
        "original_speech_frames": int((expected >= 0.5).sum()),
        "cpu_speech_frames": int((actual >= 0.5).sum()),
        "frames": 200,
        "policy": {"max_abs_error": 0.02, "minimum_decision_agreement": 0.99},
    }


def _baseline(package_dir: Path, prepared: Path):
    baseline._preparation(prepared)
    package = lab.load_package(package_dir, require_qualification=False)
    _require(
        package["record"]["source_model_sha256"] == baseline.FULL_MODEL_SHA256,
        "CPU runner requires the frozen full model",
    )
    model = baseline.ir8_model(lab._regular(prepared / "marblenet_folded_fp32.onnx"))
    features = np.load(prepared / "speech_features_fp32.npy", allow_pickle=False)
    _require(
        features.shape == (1, 80, 400)
        and features.dtype == np.float32
        and np.isfinite(features).all(),
        "CPU input feature contract differs",
    )
    options = ort.SessionOptions()
    options.intra_op_num_threads = options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    session = ort.InferenceSession(model, options, providers=["CPUExecutionProvider"])
    _require(
        session.get_providers() == ["CPUExecutionProvider"]
        and ort.__version__ == "1.27.0",
        "host CPU reference provider/version differs",
    )
    output = session.run(["output"], {"input": features})[0]
    _require(
        output.shape == (1, 2, 200)
        and output.dtype == np.float32
        and np.isfinite(output).all(),
        "host CPU output contract differs",
    )
    reference = np.ascontiguousarray(output, dtype="<f4").tobytes()
    original = lab._regular(prepared / "18d_speech_original_fp32.bin", 1600)
    _vad(original, reference)
    return (
        package,
        {
            "cpu.onnx": model,
            "cpu-input.bin": np.ascontiguousarray(features, dtype="<f4").tobytes(),
            "cpu-reference.bin": reference,
        },
        original,
    )


def _directory(path: Path):
    _require(not path.is_symlink(), "CPU directory is linked")
    path = path.resolve()
    allowed = (lab.REPOSITORY_ROOT / "artifacts/npu/poc18").resolve()
    _require(allowed in path.parents, "CPU artifacts must remain under PoC18")
    return path


def _build(directory: Path, remote: str) -> dict:
    _require(
        lab._digest(lab._regular(baseline.HEADER)) == baseline.HEADER_SHA256,
        "pinned ORT API15 header changed",
    )
    output = directory / "build"
    output.mkdir()
    script = r"""export LC_ALL=C SOURCE_DATE_EPOCH=1785196800
aarch64-linux-gnu-g++ -std=c++17 -O2 -g0 -Wall -Wextra -Werror -Wformat=2 -Wshadow \
 -D_FORTIFY_SOURCE=2 -fPIE -fstack-protector-strong \
 -I /work/artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include /cpu/execute.cc \
 -Wl,--as-needed -Wl,--build-id=sha1 -Wl,--enable-new-dtags "-Wl,-rpath,$1/lib:/usr/lib" \
 -Wl,-z,noexecstack -Wl,-z,now -Wl,-z,relro -pie -ldl -lpthread -lm \
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
        f"{directory}:/cpu:ro",
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
    _require(result.returncode == 0, "offline CPU build failed; log retained")
    validate_cpu_elf(result.stdout, remote)
    lab._write(output / "execute.elf.txt", result.stdout.encode())
    return {
        "image_id": lab.IMAGE,
        "ort_header_sha256": baseline.HEADER_SHA256,
        "network": "none",
        "repository_mount": "read-only",
        **{
            key: lab._digest(lab._regular(directory / name))
            for name, key in [
                ("execute.cc", "source_sha256"),
                ("build/wave-npu-onnx-execute-v1", "binary_sha256"),
                ("build/execute.elf.txt", "inspection_sha256"),
                ("build/execute.log", "log_sha256"),
            ]
        },
    }


def prepare_cpu_benchmark(package_dir: Path, prepared: Path, directory: Path) -> dict:
    package, files, original = _baseline(package_dir, prepared)
    directory = _directory(directory)
    _require(
        directory != Path(package["directory"]).resolve()
        and directory != prepared.resolve()
        and Path(package["directory"]).resolve() not in directory.parents
        and prepared.resolve() not in directory.parents,
        "CPU preparation aliases package/prepared inputs",
    )
    directory.mkdir(parents=True, exist_ok=False)
    for name, payload in files.items():
        lab._write(directory / name, payload)
    remote = lab.remote_root(package["record"], "execute")
    source = _source().generate_cpu_source(
        package["record"], remote, len(files["cpu.onnx"])
    )
    lab._write(directory / "execute.cc", source.encode())
    build = _build(directory, remote)
    receipt = {
        "schema_version": 1,
        "operation": "fp32-marblenet-cpu",
        "package_sha256": package["package_sha256"],
        "full_model_sha256": baseline.FULL_MODEL_SHA256,
        "frozen_preparation_sha256": baseline.PREPARATION_SHA256,
        "ort_library_sha256": baseline.ORT_SHA256,
        "host_ort_version": ort.__version__,
        "native_runs": 0,
        "cpu_iterations": 36,
        "warmup_iterations": 4,
        "measured_iterations": 32,
        "files": {
            name: {"sha256": lab._digest(payload), "bytes": len(payload)}
            for name, payload in files.items()
        },
        "host_original_vad": _vad(original, files["cpu-reference.bin"]),
        "build": build,
    }
    lab._write_json(directory / "preparation.json", receipt)
    return receipt


def _admit(package_dir: Path, prepared: Path, directory: Path, expected_sha: str):
    _require(
        bool(re.fullmatch(r"[0-9a-f]{64}", expected_sha)),
        "reviewed preparation SHA is mandatory",
    )
    directory = _directory(directory)
    _require(
        lab._digest(lab._regular(directory / "preparation.json")) == expected_sha,
        "reviewed CPU preparation changed",
    )
    _require(
        {p.name for p in directory.iterdir()} == ROOT_ITEMS
        and {p.name for p in (directory / "build").iterdir()} == BUILD_ITEMS,
        "CPU local file inventory changed",
    )
    package, files, original = _baseline(package_dir, prepared)
    receipt = lab._json(directory / "preparation.json")
    _require(
        set(receipt)
        == {
            "schema_version",
            "operation",
            "package_sha256",
            "full_model_sha256",
            "frozen_preparation_sha256",
            "ort_library_sha256",
            "host_ort_version",
            "native_runs",
            "cpu_iterations",
            "warmup_iterations",
            "measured_iterations",
            "files",
            "host_original_vad",
            "build",
        },
        "CPU preparation key set changed",
    )
    for field, value in {
        "schema_version": 1,
        "native_runs": 0,
        "cpu_iterations": 36,
        "warmup_iterations": 4,
        "measured_iterations": 32,
    }.items():
        _require(
            type(receipt[field]) is int and receipt[field] == value,
            "CPU fixed iteration/schema policy changed",
        )
    _require(
        receipt["operation"] == "fp32-marblenet-cpu"
        and receipt["host_ort_version"] == "1.27.0"
        and receipt["host_original_vad"] == _vad(original, files["cpu-reference.bin"]),
        "CPU reference policy/metrics changed",
    )
    _require(
        set(receipt["build"])
        == {
            "image_id",
            "ort_header_sha256",
            "network",
            "repository_mount",
            "source_sha256",
            "binary_sha256",
            "inspection_sha256",
            "log_sha256",
        },
        "CPU build provenance key set changed",
    )

    _require(
        receipt["package_sha256"] == package["package_sha256"]
        and receipt["frozen_preparation_sha256"] == baseline.PREPARATION_SHA256
        and receipt["full_model_sha256"] == baseline.FULL_MODEL_SHA256
        and receipt["ort_library_sha256"] == baseline.ORT_SHA256,
        "CPU baseline bindings changed",
    )
    _require(set(receipt["files"]) == set(files), "CPU receipt file set changed")
    for name, payload in files.items():
        _require(
            lab._regular(directory / name) == payload
            and receipt["files"][name]
            == {"sha256": lab._digest(payload), "bytes": len(payload)},
            "CPU independent model/input/reference changed",
        )
    for name, key in [
        ("execute.cc", "source_sha256"),
        ("build/wave-npu-onnx-execute-v1", "binary_sha256"),
        ("build/execute.elf.txt", "inspection_sha256"),
        ("build/execute.log", "log_sha256"),
    ]:
        _require(
            lab._digest(lab._regular(directory / name)) == receipt["build"][key],
            "CPU build artifact changed",
        )
    _require(
        receipt["build"]["image_id"] == lab.IMAGE
        and receipt["build"]["ort_header_sha256"] == baseline.HEADER_SHA256
        and receipt["build"]["network"] == "none"
        and receipt["build"]["repository_mount"] == "read-only",
        "CPU build provenance changed",
    )
    remote = lab.remote_root(package["record"], "execute")
    _source().validate_cpu_source(
        package["record"],
        remote,
        len(files["cpu.onnx"]),
        lab._regular(directory / "execute.cc").decode(),
    )
    validate_cpu_elf(lab._regular(directory / "build/execute.elf.txt").decode(), remote)
    return package, receipt, original


def _manifest(package: dict, directory: Path, receipt: dict):
    remote = lab.remote_root(package["record"], "execute")
    payloads = {
        remote + "/model/cpu.onnx": (directory / "cpu.onnx", "0444"),
        remote + "/input/cpu-input.bin": (directory / "cpu-input.bin", "0444"),
        remote + "/input/cpu-reference.bin": (directory / "cpu-reference.bin", "0444"),
        remote + "/bin/wave-npu-onnx-execute-v1": (
            directory / "build/wave-npu-onnx-execute-v1",
            "0555",
        ),
    }
    artifacts = {
        **lab.SYSTEM_HASHES,
        baseline.ORT_PATH: baseline.ORT_SHA256,
        **{
            remote: lab._digest(lab._regular(local))
            for remote, (local, _) in payloads.items()
        },
    }
    absent = (
        "/lib/libCLC.so",
        "/usr/lib/libCLC.so",
        "/usr/lib64/libCLC.so",
        remote + "/lib/libCLC.so",
    )
    policy = lab.execution.ExecutionPolicy(
        poc_id="18CPU",
        experiment="wave-poc18-stock-ort-cpu-benchmark",
        manifest_path=directory / "execution-manifest.json",
        remote_root=remote,
        remote_executable=remote + "/bin/wave-npu-onnx-execute-v1",
        remote_tim_vx=baseline.ORT_PATH,
        remote_nbg=remote + "/model/cpu.onnx",
        fixed_artifacts=artifacts,
        absent_artifacts=absent,
        staged_modes={
            p: "-r-xr-xr-x" if mode == "0555" else "-r--r--r--"
            for p, (_, mode) in payloads.items()
        },
        nbg_bytes=receipt["files"]["cpu.onnx"]["bytes"],
        input_values=(),
        output_values=(),
        execution_re=re.compile("CPU_EXECUTION"),
        expected_iterations=36,
    )
    # The inherited field aliases above serve generic file preflight only;
    # there is no NBG/TIM payload and no synthetic live chip identity.
    document = {
        "schema_version": 1,
        "operation": "CPU",
        "device_baseline": lab.DEVICE,
        "environment": {"LD_LIBRARY_PATH": remote + "/lib:/usr/lib"},
        "timeout_ms": 12000,
        "required_artifacts": artifacts,
        "absent_artifacts": list(absent),
        "actual_result_identity": CPU_IDENTITY,
        "remote_executable": policy.remote_executable,
        "remote_argument": "--execute-once",
    }
    lab._write_json(policy.manifest_path, document)
    candidate = lab.compile_lab.Candidate(
        "output",
        "--execute-once",
        remote + "/output/output.bin",
        remote + "/output/.output.bin.partial",
    )
    device = lab.DEVICE
    manifest = lab.execution.ExecutionManifest(
        policy=policy,
        serial=device["serial"],
        kernel=device["kernel"],
        galcore_major_minor=device["galcore_major_minor"],
        npu_parameters=device["npu_parameters"],
        environment=document["environment"],
        remote_executable=policy.remote_executable,
        remote_argument="--execute-once",
        timeout_ms=12000,
        required_artifacts=artifacts,
        absent_artifacts=absent,
        expected_result={"identity": CPU_IDENTITY},
        sha256=lab._digest(lab._canonical(document)),
        candidates={"output": candidate},
    )
    return manifest, payloads


def parse_cpu_result(stdout: str) -> dict:
    _require(
        isinstance(stdout, str) and len(stdout) <= 1024 * 1024,
        "CPU stdout exceeds bound",
    )
    identities = [s for s in stdout.splitlines() if s.startswith("CPU_IDENTITY")]
    expected = "CPU_IDENTITY ort_version=1.15.1 provider=CPUExecutionProvider threads=1"
    executions = [s for s in stdout.splitlines() if s.startswith("CPU_EXECUTION")]
    _require(
        identities == [expected] and len(executions) == 1,
        "CPU identity/PASS count differs",
    )
    match = re.fullmatch(
        r"CPU_EXECUTION status=PASS mode=execute-once operation=fp32-marblenet-cpu iterations=36 input_bytes=128000 output_bytes=1600 graph_us=([0-9]+) output_hex=([0-9a-f]{3200}) cleanup=0",
        executions[0],
    )
    _require(match is not None, "CPU execution line malformed")
    _source().parse_cpu_samples(stdout)
    observed = bytes.fromhex(match[2])
    _require(
        np.isfinite(np.frombuffer(observed, dtype="<f4")).all(),
        "CPU stdout output nonfinite",
    )
    return {
        "identity": dict(CPU_IDENTITY),
        "mode": "execute-once",
        "iterations": 36,
        "input_bytes": 128000,
        "output_bytes": 1600,
        "graph_us": int(match[1]),
        "output_hex": observed.hex(),
        "output": list(observed),
        "cpu_samples": _source().parse_cpu_samples(stdout),
    }


def _run_once(transport, manifest, run_id):
    directory = lab.npu_lab.create_run_directory(lab.execution.DEFAULT_RUN_ROOT, run_id)
    before = lab.execution.collect_snapshot(transport, manifest)
    lab._write_json(directory / "preflight.json", before)
    envelope = {
        "schema_version": 1,
        "experiment": manifest.policy.experiment,
        "run_id": run_id,
        "manifest_sha256": manifest.sha256,
        "status": "INCOMPLETE",
        "errors": [],
        "health_differences": [],
        "kernel_alerts": [],
        "execution_attempts": 0,
        "native_runs": 0,
        "result": None,
    }
    try:
        lab.execution.verify_preflight(manifest, before)
    except Exception as error:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(error))
        lab._write_json(directory / "run.json", envelope)
        return envelope, directory
    sentinel = f"__WAVE_EXEC_EXIT_{uuid.uuid4().hex}__"
    command = " ".join(
        shlex.quote(s)
        for s in [
            "env",
            "-i",
            "PATH=/bin:/usr/bin",
            "LC_ALL=C",
            "LD_LIBRARY_PATH=" + manifest.environment["LD_LIBRARY_PATH"],
            manifest.remote_executable,
            "--execute-once",
        ]
    )
    command += (
        '\nwave_cpu_rc=$?\nprintf "\\n'
        + sentinel
        + ':%s\\n" "$wave_cpu_rc"\nexit "$wave_cpu_rc"\n'
    )
    envelope["execution_attempts"] = 1
    started = time.monotonic()
    result = transport.run(("shell", command), 12000)
    lab._write(directory / "stdout.txt", result.stdout.encode())
    lab._write(directory / "stderr.txt", result.stderr.encode())
    after = lab.execution.collect_snapshot(transport, manifest)
    lab.compile_lab.align_native_diagnostics(transport, manifest, before, after)
    lab._write_json(directory / "postflight.json", after)
    differences, alerts = lab.health(manifest, before, after)
    envelope["health_differences"] = differences
    envelope["kernel_alerts"] = alerts
    envelope["transport"] = {
        "adb_returncode": result.returncode,
        "timed_out": result.timed_out,
        "duration_ms": int((time.monotonic() - started) * 1000),
    }
    if result.timed_out:
        envelope["errors"].append("CPU host deadline expired; no retry")
    else:
        try:
            stdout, exit_code = lab.execution.parse_remote_exit(result.stdout, sentinel)
            envelope["transport"]["remote_exit"] = exit_code
            lab.compile_lab.verify_transport_status(
                result.returncode, exit_code, "CPU-only executor"
            )
            _require(exit_code == 0, "CPU executor failed")
            envelope["result"] = parse_cpu_result(stdout)
        except Exception as error:
            envelope["errors"].append(str(error))
    if differences or alerts:
        envelope["errors"].append("CPU health gate failed")
    envelope["status"] = "FAIL" if envelope["errors"] else "PASS"
    lab._write_json(directory / "run.json", envelope)
    return envelope, directory


def run_cpu_benchmark(
    package_dir: Path,
    prepared: Path,
    directory: Path,
    run_id: str,
    expected_preparation_sha256: str,
) -> dict:
    lab._run_id(run_id)
    directory = _directory(directory)
    package, receipt, original = _admit(
        package_dir, prepared, directory, expected_preparation_sha256
    )
    lab._write_json(
        directory / "attempt.json",
        {"run_id": run_id, "reviewed_preparation_sha256": expected_preparation_sha256},
    )
    manifest, payloads = _manifest(package, directory, receipt)
    stage = directory / "stage"
    stage.mkdir(exist_ok=False)
    lab._stage(manifest, payloads, manifest.policy.remote_root, stage, execute=True)
    envelope, run_directory = _run_once(lab.npu_lab.AdbTransport(), manifest, run_id)
    lab._successful_run(envelope)
    after = lab._json(run_directory / "postflight.json")
    state = after["output_states"][manifest.candidates["output"].output]
    _require(
        state.get("status") == "present"
        and state.get("bytes") == 1600
        and state.get("permissions") == "-r--r--r--",
        "CPU output file contract differs",
    )
    lab._adb(
        manifest,
        "pull",
        manifest.candidates["output"].output,
        str(run_directory / "output.bin"),
    )
    observed = lab._regular(run_directory / "output.bin", 1600)
    _require(
        envelope["result"]["output_hex"] == observed.hex()
        and lab._digest(observed) == state.get("sha256"),
        "CPU actual output identity differs",
    )
    lab._cleanup(
        manifest,
        payloads,
        manifest.policy.remote_root,
        envelope,
        run_directory,
        execute=True,
    )
    metric = compare_cpu_output(
        lab._regular(directory / "cpu-reference.bin", 1600), observed
    )
    vad = _vad(original, observed)
    samples = envelope["result"]["cpu_samples"]
    timings = {}
    for key in (
        "boundary_ns",
        "thread_ns",
        "input_copy_ns",
        "run_ns",
        "postprocess_ns",
    ):
        values = np.array([sample[key] for sample in samples], np.float64) / 1e6
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
        "native_runs": 0,
        "cpu_runs": 36,
        "cpu_identity": CPU_IDENTITY,
        "prepared_features_only": True,
        "feature_extraction_excluded": True,
        "floating_point_type": "FLOAT32",
        "comparison": metric,
        "vad_against_original_fp32": vad,
        "samples": samples,
        "timings": timings,
        "run_sha256": lab._digest(lab._regular(run_directory / "run.json")),
        "cleanup_sha256": lab._digest(lab._regular(run_directory / "cleanup.json")),
        "output_sha256": lab._digest(observed),
        "reviewed_preparation_sha256": expected_preparation_sha256,
    }
    lab._write_json(directory / "results.json", result)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "run"])
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--run-id")
    parser.add_argument("--expected-preparation-sha256")
    args = parser.parse_args(argv)
    if args.command == "prepare":
        result = prepare_cpu_benchmark(args.package, args.prepared, args.directory)
    else:
        if not args.run_id or not args.expected_preparation_sha256:
            parser.error("run requires run ID and reviewed preparation SHA")
        result = run_cpu_benchmark(
            args.package,
            args.prepared,
            args.directory,
            args.run_id,
            args.expected_preparation_sha256,
        )
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
