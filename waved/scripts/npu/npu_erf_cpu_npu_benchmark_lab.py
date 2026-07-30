#!/usr/bin/env python3
"""Exactly-once Wave Cortex-A55 versus NPU Erf inference benchmark."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import shlex
import struct
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import npu_compile_lab
import npu_erf_mask_stream_execution_lab as stream_lab
import npu_lab


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name("erf_cpu_npu_benchmark_manifest.json")
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"
EXPECTED_MANIFEST_FILE_SHA256 = (
    "54cdb3350a10d1e435a62081a648acebef9f82f711b5a789a67c6c88573f167a"
)

REMOTE_ROOT = "/tmp/wave-npu-poc13g-r2"
REMOTE_EXECUTABLE = f"{REMOTE_ROOT}/bin/wave-npu-benchmark-erf-cpu-vs-npu-v2"
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_NBG = f"{REMOTE_ROOT}/model/erf-mask.nbg"
REMOTE_TIMING = f"{REMOTE_ROOT}/result/erf-cpu-npu-timings-v1.bin"
REMOTE_PARTIAL_TIMING = f"{REMOTE_ROOT}/result/erf-cpu-npu-timings-v1.partial"
REMOTE_ARGUMENT = "--benchmark"
EXPECTED_TIMEOUT_MS = 60_000

EXPECTED_KERNEL = (
    "Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT "
    "Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
)
EXPECTED_PARAMETERS = {
    "enableNN": "255",
    "nanoqFreq": "838860800",
    "recovery": "1",
}
EXPECTED_ENVIRONMENT = {
    "LD_LIBRARY_PATH": f"{REMOTE_ROOT}/lib:/usr/lib",
    "VIVANTE_SDK_DIR": "/usr",
    "VSI_NN_LOG_LEVEL": "2",
}
EXPECTED_IDENTITY = stream_lab.EXPECTED_IDENTITY
EXPECTED_ARTIFACTS = {
    REMOTE_EXECUTABLE: (
        "cc9154961833124c138553e15e572442033bd3c40b26ca302cd32aed40715799"
    ),
    REMOTE_TIM_VX: stream_lab.EXPECTED_ARTIFACTS[stream_lab.REMOTE_TIM_VX],
    REMOTE_NBG: stream_lab.EXPECTED_ARTIFACTS[stream_lab.REMOTE_NBG],
}
EXPECTED_SOURCE_SHA256 = (
    "07feee816c2c5958b38f4965a076d061ae7bca9fb8d381f5d1010fe109c8be91"
)
EXPECTED_CPU_TABLE_SHA256 = stream_lab.ERF_TABLE_SHA256
EXPECTED_ABSENT_ARTIFACTS = (
    "/lib/libCLC.so",
    f"{REMOTE_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
EXPECTED_STAGED_MODES = {
    REMOTE_EXECUTABLE: "-r-xr-xr-x",
    REMOTE_TIM_VX: "-r--r--r--",
    REMOTE_NBG: "-r--r--r--",
}
EXPECTED_STAGED_BYTES = {REMOTE_NBG: 5_840}

COLD_FRAMES = 1
WARMUP_FRAMES = 100
MEASURED_FRAMES = 10_000
TOTAL_FRAMES = COLD_FRAMES + WARMUP_FRAMES + MEASURED_FRAMES
FRAME_WIDTH = 481
INPUT_SEED = 0x5733130F
CALIBRATION_SAMPLES = 100_000
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3

TIMING_MAGIC = b"ECN1"
TIMING_SCHEMA_VERSION = 1
HEADER_STRUCT = struct.Struct("<4sIIIIQQ")
RECORD_STRUCT = struct.Struct("<IIQQQQQQQ")
TIMING_BYTES = HEADER_STRUCT.size + MEASURED_FRAMES * RECORD_STRUCT.size

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
FNV_RE = re.compile(r"^0x[0-9a-f]{16}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_ERF_BENCH_EXIT_[0-9a-f]{32}__$")
COLD_RE = re.compile(
    r"^COLD cpu_wall_ns=(?P<cpu>[0-9]+) "
    r"npu_copy_in_ns=(?P<copy_in>[0-9]+) "
    r"npu_run_ns=(?P<run>[0-9]+) "
    r"npu_copy_out_ns=(?P<copy_out>[0-9]+) "
    r"npu_total_ns=(?P<total>[0-9]+)$"
)
PROCESS_RE = re.compile(r"^PROCESS_CPU frames=10101 total_ns=(?P<total>[0-9]+)$")
SUMMARY_RE = re.compile(
    r"^ERF_CPU_NPU_BENCH status=PASS mode=benchmark "
    r"cold_frames=1 warmup_frames=100 measured_frames=10000 "
    r"cpu_count=10101 npu_run_count=10101 compile_count=1 "
    r"canaries=PASS timing_bytes=640036 "
    r"cpu_thread_zero_count=(?P<cpu_thread_zeros>[0-9]+) "
    r"timing_fnv1a64=(?P<timing>0x[0-9a-f]{16}) "
    r"input_fnv1a64=(?P<input>0x[0-9a-f]{16}) "
    r"cpu_output_fnv1a64=(?P<cpu>0x[0-9a-f]{16}) "
    r"npu_output_fnv1a64=(?P<npu>0x[0-9a-f]{16}) "
    r"final_state=(?P<state>0x[0-9a-f]{8})$"
)


class BenchmarkError(Exception):
    """Expected PoC 13G failure."""


class ManifestError(BenchmarkError):
    """The benchmark manifest differs from fixed policy."""


class PreflightError(BenchmarkError):
    """The device baseline is unsafe or differs from fixed policy."""


class ResultError(BenchmarkError):
    """The benchmark result is malformed or numerically incorrect."""


@dataclass(frozen=True)
class ExecutionPolicy:
    absent_artifacts: tuple[str, ...]


@dataclass(frozen=True)
class BenchmarkManifest:
    serial: str
    kernel: str
    galcore_major_minor: str
    npu_parameters: Mapping[str, str]
    environment: Mapping[str, str]
    remote_executable: str
    remote_argument: str
    remote_timing: str
    remote_partial_timing: str
    source_sha256: str
    cpu_table_sha256: str
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    expected_result: Mapping[str, Any]
    sha256: str
    policy: ExecutionPolicy
    candidates: Mapping[str, Any]


@dataclass(frozen=True)
class TimingRecord:
    sequence: int
    order: int
    cpu_wall_ns: int
    cpu_thread_ns: int
    npu_copy_in_ns: int
    npu_run_ns: int
    npu_copy_out_ns: int
    npu_total_wall_ns: int
    npu_thread_ns: int


@dataclass(frozen=True)
class TimingPayload:
    calibration_samples: int
    calibration_total_wall_ns: int
    calibration_min_wall_ns: int
    records: tuple[TimingRecord, ...]


def _duplicate_safe(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _canonical_sha256(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode()
    ).hexdigest()


def _fixed_expected_result() -> dict[str, Any]:
    return {
        "identity": EXPECTED_IDENTITY,
        "cold_frames": COLD_FRAMES,
        "warmup_frames": WARMUP_FRAMES,
        "measured_frames": MEASURED_FRAMES,
        "total_frames": TOTAL_FRAMES,
        "width": FRAME_WIDTH,
        "seed": f"0x{INPUT_SEED:08x}",
        "calibration_samples": CALIBRATION_SAMPLES,
        "timing_bytes": TIMING_BYTES,
        "compile_count": 1,
        "cpu_count": TOTAL_FRAMES,
        "npu_run_count": TOTAL_FRAMES,
    }


def load_manifest(path: Path = DEFAULT_MANIFEST) -> BenchmarkManifest:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise ManifestError(f"cannot read manifest: {exc}") from exc
    if (
        path.resolve() == DEFAULT_MANIFEST.resolve()
        and hashlib.sha256(raw).hexdigest() != EXPECTED_MANIFEST_FILE_SHA256
    ):
        raise ManifestError("source-controlled manifest file hash differs")
    try:
        document = json.loads(raw.decode("utf-8"), object_pairs_hook=_duplicate_safe)
    except ManifestError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot parse manifest: {exc}") from exc

    if not isinstance(document, dict) or set(document) != {
        "schema_version",
        "device",
        "environment",
        "remote_executable",
        "remote_argument",
        "remote_timing",
        "remote_partial_timing",
        "source_sha256",
        "cpu_table_sha256",
        "timeout_ms",
        "required_artifacts",
        "absent_artifacts",
        "expected_result",
    }:
        raise ManifestError("manifest root keys differ")
    device = document["device"]
    if (
        document["schema_version"] != 1
        or not isinstance(device, dict)
        or device
        != {
            "serial": "PSP_0000000000000000",
            "kernel": EXPECTED_KERNEL,
            "galcore_major_minor": "199:0",
            "npu_parameters": EXPECTED_PARAMETERS,
        }
    ):
        raise ManifestError("device identity differs from fixed policy")
    if document["environment"] != EXPECTED_ENVIRONMENT:
        raise ManifestError("environment differs from fixed policy")
    if (
        document["remote_executable"] != REMOTE_EXECUTABLE
        or document["remote_argument"] != REMOTE_ARGUMENT
        or document["remote_timing"] != REMOTE_TIMING
        or document["remote_partial_timing"] != REMOTE_PARTIAL_TIMING
        or document["source_sha256"] != EXPECTED_SOURCE_SHA256
        or document["cpu_table_sha256"] != EXPECTED_CPU_TABLE_SHA256
        or document["timeout_ms"] != EXPECTED_TIMEOUT_MS
    ):
        raise ManifestError("remote invocation differs from fixed policy")

    artifacts = document["required_artifacts"]
    if not isinstance(artifacts, dict) or not artifacts:
        raise ManifestError("required_artifacts must be nonempty")
    for artifact_path, digest in artifacts.items():
        if (
            not isinstance(artifact_path, str)
            or not artifact_path.startswith("/")
            or not isinstance(digest, str)
            or SHA256_RE.fullmatch(digest) is None
        ):
            raise ManifestError("artifact allowlist is malformed")
        if artifact_path.startswith("/tmp/") and not artifact_path.startswith(
            f"{REMOTE_ROOT}/"
        ):
            raise ManifestError("temporary artifact escapes fixed root")
    for artifact_path, digest in EXPECTED_ARTIFACTS.items():
        if artifacts.get(artifact_path) != digest:
            raise ManifestError(f"fixed artifact mismatch: {artifact_path}")

    absent = document["absent_artifacts"]
    if not isinstance(absent, list) or tuple(sorted(absent)) != tuple(
        sorted(EXPECTED_ABSENT_ARTIFACTS)
    ):
        raise ManifestError("absent_artifacts differs from CLC policy")
    expected = document["expected_result"]
    if expected != _fixed_expected_result():
        raise ManifestError("expected benchmark result differs")

    absent_tuple = tuple(sorted(absent))
    return BenchmarkManifest(
        serial=device["serial"],
        kernel=device["kernel"],
        galcore_major_minor=device["galcore_major_minor"],
        npu_parameters=device["npu_parameters"],
        environment=document["environment"],
        remote_executable=document["remote_executable"],
        remote_argument=document["remote_argument"],
        remote_timing=document["remote_timing"],
        remote_partial_timing=document["remote_partial_timing"],
        source_sha256=document["source_sha256"],
        cpu_table_sha256=document["cpu_table_sha256"],
        timeout_ms=document["timeout_ms"],
        required_artifacts=artifacts,
        absent_artifacts=absent_tuple,
        expected_result=expected,
        sha256=_canonical_sha256(document),
        policy=ExecutionPolicy(absent_tuple),
        candidates={},
    )


def _update_fnv(value: int, payload: bytes) -> int:
    for byte in payload:
        value ^= byte
        value = (value * FNV64_PRIME) & 0xFFFFFFFFFFFFFFFF
    return value


def expected_digests() -> dict[str, str]:
    """Generate the fixed input/output oracle independently of native code."""
    table = stream_lab.build_independent_erf_table()
    state = INPUT_SEED
    input_fnv = FNV64_OFFSET_BASIS
    output_fnv = FNV64_OFFSET_BASIS
    for _ in range(TOTAL_FRAMES):
        frame = bytearray(FRAME_WIDTH)
        for index in range(FRAME_WIDTH):
            state ^= (state << 13) & 0xFFFFFFFF
            state ^= state >> 17
            state ^= (state << 5) & 0xFFFFFFFF
            state &= 0xFFFFFFFF
            frame[index] = state & 0xFF
        payload = bytes(frame)
        input_fnv = _update_fnv(input_fnv, payload)
        output_fnv = _update_fnv(output_fnv, payload.translate(table))
    return {
        "input_fnv1a64": f"0x{input_fnv:016x}",
        "output_fnv1a64": f"0x{output_fnv:016x}",
        "final_state": f"0x{state:08x}",
    }


def parse_timing_payload(payload: bytes) -> TimingPayload:
    if len(payload) != TIMING_BYTES:
        raise ResultError(
            f"timing byte count differs: {len(payload)} != {TIMING_BYTES}"
        )
    (
        magic,
        schema,
        measured_count,
        record_size,
        calibration_samples,
        calibration_total,
        calibration_min,
    ) = HEADER_STRUCT.unpack_from(payload)
    if (
        magic != TIMING_MAGIC
        or schema != TIMING_SCHEMA_VERSION
        or measured_count != MEASURED_FRAMES
        or record_size != RECORD_STRUCT.size
    ):
        raise ResultError("timing header differs from fixed schema")
    if (
        calibration_samples != CALIBRATION_SAMPLES
        or calibration_total <= 0
        or calibration_min <= 0
        or calibration_total < calibration_min
    ):
        raise ResultError("timing calibration is malformed")

    records: list[TimingRecord] = []
    offset = HEADER_STRUCT.size
    for expected_sequence in range(MEASURED_FRAMES):
        fields = RECORD_STRUCT.unpack_from(payload, offset)
        offset += RECORD_STRUCT.size
        record = TimingRecord(*fields)
        if record.sequence != expected_sequence:
            raise ResultError("timing sequence is not contiguous")
        if record.order != (expected_sequence & 1):
            raise ResultError("timing alternating order differs")
        mandatory_durations = (
            record.cpu_wall_ns,
            record.npu_copy_in_ns,
            record.npu_run_ns,
            record.npu_copy_out_ns,
            record.npu_total_wall_ns,
            record.npu_thread_ns,
        )
        if any(duration == 0 for duration in mandatory_durations):
            raise ResultError("timing record contains zero duration")
        if record.npu_total_wall_ns < max(
            record.npu_copy_in_ns,
            record.npu_run_ns,
            record.npu_copy_out_ns,
        ):
            raise ResultError("NPU total timing is inconsistent")
        records.append(record)
    return TimingPayload(
        calibration_samples=calibration_samples,
        calibration_total_wall_ns=calibration_total,
        calibration_min_wall_ns=calibration_min,
        records=tuple(records),
    )


def _nearest_rank(sorted_values: list[int | float], quantile: float) -> int | float:
    rank = max(1, math.ceil(quantile * len(sorted_values)))
    return sorted_values[rank - 1]


def _statistics(
    values: Sequence[int | float],
) -> dict[str, int | float]:
    ordered = sorted(values)
    total = sum(ordered)
    return {
        "count": len(ordered),
        "resolution_limited_zeros": ordered.count(0),
        "total": total,
        "mean": total / len(ordered),
        "minimum": ordered[0],
        "p50": _nearest_rank(ordered, 0.50),
        "p90": _nearest_rank(ordered, 0.90),
        "p95": _nearest_rank(ordered, 0.95),
        "p99": _nearest_rank(ordered, 0.99),
        "p99_9": _nearest_rank(ordered, 0.999),
        "maximum": ordered[-1],
    }


def summarize_timings(timing: TimingPayload) -> dict[str, Any]:
    fields = {
        "cpu_wall_ns": tuple(r.cpu_wall_ns for r in timing.records),
        "cpu_thread_ns": tuple(r.cpu_thread_ns for r in timing.records),
        "npu_copy_in_ns": tuple(r.npu_copy_in_ns for r in timing.records),
        "npu_run_ns": tuple(r.npu_run_ns for r in timing.records),
        "npu_copy_out_ns": tuple(r.npu_copy_out_ns for r in timing.records),
        "npu_total_wall_ns": tuple(r.npu_total_wall_ns for r in timing.records),
        "npu_thread_ns": tuple(r.npu_thread_ns for r in timing.records),
    }
    summary = {name: _statistics(values) for name, values in fields.items()}
    cpu_mean = float(summary["cpu_wall_ns"]["mean"])
    npu_total_mean = float(summary["npu_total_wall_ns"]["mean"])
    npu_run_mean = float(summary["npu_run_ns"]["mean"])
    summary["ratios"] = {
        "npu_total_over_cpu_wall": npu_total_mean / cpu_mean,
        "npu_run_over_cpu_wall": npu_run_mean / cpu_mean,
        "cpu_over_npu_total_wall": cpu_mean / npu_total_mean,
        "cpu_over_npu_run": cpu_mean / npu_run_mean,
    }
    summary["throughput_frames_per_second"] = {
        "cpu_wall": 1_000_000_000.0 / cpu_mean,
        "npu_total_wall": 1_000_000_000.0 / npu_total_mean,
        "npu_run": 1_000_000_000.0 / npu_run_mean,
    }
    summary["audio_hop_fraction"] = {
        "hop_ns": 10_000_000,
        "cpu_wall": cpu_mean / 10_000_000.0,
        "npu_total_wall": npu_total_mean / 10_000_000.0,
        "npu_run": npu_run_mean / 10_000_000.0,
    }
    summary["timer_calibration"] = {
        "samples": timing.calibration_samples,
        "total_wall_ns": timing.calibration_total_wall_ns,
        "mean_pair_ns": (timing.calibration_total_wall_ns / timing.calibration_samples),
        "minimum_pair_ns": timing.calibration_min_wall_ns,
    }
    summary["paired_ratios"] = {
        "npu_run_over_cpu_wall": _statistics(
            tuple(r.npu_run_ns / r.cpu_wall_ns for r in timing.records)
        ),
        "npu_total_over_cpu_wall": _statistics(
            tuple(r.npu_total_wall_ns / r.cpu_wall_ns for r in timing.records)
        ),
    }
    order_subsets: dict[str, Any] = {}
    for order, name in ((0, "cpu-first"), (1, "npu-first")):
        records = tuple(r for r in timing.records if r.order == order)
        cpu = _statistics(tuple(r.cpu_wall_ns for r in records))
        npu_run = _statistics(tuple(r.npu_run_ns for r in records))
        npu_total = _statistics(tuple(r.npu_total_wall_ns for r in records))
        order_subsets[name] = {
            "count": len(records),
            "cpu_wall_ns": cpu,
            "npu_run_ns": npu_run,
            "npu_total_wall_ns": npu_total,
            "ratios_of_means": {
                "npu_run_over_cpu_wall": (float(npu_run["mean"]) / float(cpu["mean"])),
                "npu_total_over_cpu_wall": (
                    float(npu_total["mean"]) / float(cpu["mean"])
                ),
            },
        }
    summary["order_subsets"] = order_subsets
    calibration_mean = timing.calibration_total_wall_ns / timing.calibration_samples
    noise_ns = max(float(timing.calibration_min_wall_ns), calibration_mean) * 2.0
    if abs(npu_total_mean - cpu_mean) <= noise_ns:
        classification = "within-measurement-noise"
    elif npu_total_mean < cpu_mean:
        classification = "npu-total-faster"
    elif npu_run_mean < cpu_mean:
        classification = "npu-run-faster-but-total-slower"
    else:
        classification = "cpu-faster"
    summary["classification"] = classification
    summary["classification_noise_ns"] = noise_ns
    return summary


def parse_native_result(stdout: str, manifest: BenchmarkManifest) -> dict[str, Any]:
    identities = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    cold_lines = [
        line.rstrip("\r") for line in stdout.splitlines() if line.startswith("COLD ")
    ]
    process_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("PROCESS_CPU ")
    ]
    summaries = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("ERF_CPU_NPU_BENCH ")
    ]
    if (
        len(identities) != 1
        or len(cold_lines) != 1
        or len(process_lines) != 1
        or len(summaries) != 1
    ):
        raise ResultError(
            "expected one identity, cold result, process result, and summary"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(identities[0])
    cold_match = COLD_RE.fullmatch(cold_lines[0])
    process_match = PROCESS_RE.fullmatch(process_lines[0])
    summary_match = SUMMARY_RE.fullmatch(summaries[0])
    if (
        identity_match is None
        or cold_match is None
        or process_match is None
        or summary_match is None
    ):
        raise ResultError("identity, cold, process, or summary result is malformed")
    identity = identity_match.groupdict()
    if identity != manifest.expected_result["identity"]:
        raise ResultError("live identity differs from fixed policy")
    cold_values = {
        "cpu_wall_ns": int(cold_match["cpu"]),
        "npu_copy_in_ns": int(cold_match["copy_in"]),
        "npu_run_ns": int(cold_match["run"]),
        "npu_copy_out_ns": int(cold_match["copy_out"]),
        "npu_total_ns": int(cold_match["total"]),
    }
    if any(value <= 0 for value in cold_values.values()):
        raise ResultError("cold timing contains zero duration")
    if cold_values["npu_total_ns"] < max(
        cold_values["npu_copy_in_ns"],
        cold_values["npu_run_ns"],
        cold_values["npu_copy_out_ns"],
    ):
        raise ResultError("cold NPU total timing is inconsistent")
    process_cpu_total_ns = int(process_match["total"])
    if process_cpu_total_ns <= 0:
        raise ResultError("process CPU timing contains zero duration")

    observed = summary_match.groupdict()
    expected = expected_digests()
    if (
        observed["input"] != expected["input_fnv1a64"]
        or observed["cpu"] != expected["output_fnv1a64"]
        or observed["npu"] != expected["output_fnv1a64"]
        or observed["state"] != expected["final_state"]
    ):
        raise ResultError("native CPU/NPU digest differs from host oracle")
    return {
        "identity": identity,
        "cold": cold_values,
        "process_cpu_total_ns": process_cpu_total_ns,
        "cpu_thread_zero_count": int(observed["cpu_thread_zeros"]),
        "timing_fnv1a64": observed["timing"],
        "input_fnv1a64": observed["input"],
        "cpu_output_fnv1a64": observed["cpu"],
        "npu_output_fnv1a64": observed["npu"],
        "final_state": observed["state"],
        "numeric_digest_match": True,
        "compile_count": 1,
        "cpu_count": TOTAL_FRAMES,
        "npu_run_count": TOTAL_FRAMES,
        "run_count": TOTAL_FRAMES,
        "canaries": "PASS",
    }


def collect_snapshot(transport: Any, manifest: BenchmarkManifest) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in EXPECTED_STAGED_MODES
    }
    snapshot["timing_output_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in (REMOTE_TIMING, REMOTE_PARTIAL_TIMING)
    }
    benchmark_processes = npu_compile_lab._shell_value(
        transport,
        (
            "ps 2>/dev/null | "
            "grep '[w]ave-npu-benchmark-erf-cpu-vs-npu-v2' | "
            "head -n 8 || true"
        ),
        "benchmark process scan",
        errors,
    )
    snapshot["processes"] = [
        *snapshot.get("processes", []),
        *npu_compile_lab._bounded_lines(benchmark_processes or "", 8),
    ]
    context_commands = {
        "cpu_online": "cat /sys/devices/system/cpu/online",
        "cpu_model": (
            "sed -n '/^processor[[:space:]]*:/p;"
            "/^model name[[:space:]]*:/p;"
            "/^CPU architecture[[:space:]]*:/p;"
            "/^CPU part[[:space:]]*:/p' /proc/cpuinfo | head -n 32"
        ),
        "cpu_frequency": (
            "for directory in /sys/devices/system/cpu/cpufreq/policy*; do "
            '[ -d "$directory" ] || continue; '
            "printf '%s ' \"${directory##*/}\"; "
            "for name in affected_cpus scaling_cur_freq "
            "scaling_min_freq scaling_max_freq scaling_governor; do "
            "printf '%s=' \"$name\"; "
            'cat "$directory/$name" 2>/dev/null || printf unavailable; '
            "done; done"
        ),
        "thermal": (
            "for directory in /sys/class/thermal/thermal_zone*; do "
            '[ -d "$directory" ] || continue; '
            "printf '%s type=' \"${directory##*/}\"; "
            'cat "$directory/type" 2>/dev/null || printf unavailable; '
            "printf ' temp='; "
            'cat "$directory/temp" 2>/dev/null || printf unavailable; '
            "done"
        ),
        "proc_stat": "sed -n '1p' /proc/stat",
    }
    snapshot["system_context"] = {
        name: npu_compile_lab._shell_value(transport, command, name, errors)
        for name, command in context_commands.items()
    }
    return snapshot


def _competing_executor(
    manifest: BenchmarkManifest, snapshot: Mapping[str, Any]
) -> bool:
    basename = manifest.remote_executable.rsplit("/", 1)[-1]
    return any(
        manifest.remote_executable in line or basename in line
        for line in snapshot.get("processes", [])
    )


def verify_preflight(manifest: BenchmarkManifest, snapshot: Mapping[str, Any]) -> None:
    if snapshot.get("errors"):
        raise PreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise PreflightError("authorized device is not exclusively connected")
    if snapshot.get("kernel") != manifest.kernel:
        raise PreflightError("kernel identity mismatch")
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(boot_id) is None
    ):
        raise PreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != dict(manifest.npu_parameters):
        raise PreflightError("NPU parameters differ from fixed baseline")
    if re.search(r"\b199,\s*0\b", snapshot.get("galcore_device") or "") is None:
        raise PreflightError("/dev/galcore is not character device 199:0")
    if snapshot.get("galcore_holders"):
        raise PreflightError("/dev/galcore is already held")
    if _competing_executor(manifest, snapshot):
        raise PreflightError("the fixed benchmark executor is already present")
    for path, expected in manifest.required_artifacts.items():
        if snapshot.get("artifact_hashes", {}).get(path) != expected:
            raise PreflightError(f"artifact mismatch: {path}")
    for path, expected_mode in EXPECTED_STAGED_MODES.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != expected_mode
            or (
                path in EXPECTED_STAGED_BYTES
                and state.get("bytes") != EXPECTED_STAGED_BYTES[path]
            )
        ):
            raise PreflightError(f"staged artifact metadata mismatch: {path}")
    for path in manifest.absent_artifacts:
        if (
            snapshot.get("absent_artifact_states", {}).get(path, {}).get("status")
            != "absent"
        ):
            raise PreflightError(f"forbidden CLC artifact visible: {path}")
    if any(
        state.get("status") != "absent"
        for state in snapshot.get("timing_output_states", {}).values()
    ):
        raise PreflightError("fixed timing output is not absent")
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")
    system_context = snapshot.get("system_context", {})
    if (
        not system_context.get("cpu_online")
        or not system_context.get("cpu_model")
        or not system_context.get("proc_stat")
    ):
        raise PreflightError("CPU scheduling context is incomplete")


def compare_health(
    manifest: BenchmarkManifest,
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> tuple[list[str], list[str]]:
    differences, alerts = npu_compile_lab.compare_health(manifest, before, after)
    if before.get("staged_artifact_states") != after.get("staged_artifact_states"):
        differences.append("staged artifact metadata changed")
    if _competing_executor(manifest, after):
        differences.append("the fixed benchmark executor remains")
    partial = after.get("timing_output_states", {}).get(REMOTE_PARTIAL_TIMING, {})
    if partial.get("status") != "absent":
        differences.append("partial timing output remains")
    return differences, alerts


def build_remote_command(manifest: BenchmarkManifest, sentinel: str) -> str:
    if EXIT_RE.fullmatch(sentinel) is None:
        raise ValueError("invalid internal sentinel")
    invocation = [
        "env",
        "-i",
        "PATH=/bin:/usr/bin",
        "LC_ALL=C",
        *(
            f"{key}={manifest.environment[key]}"
            for key in (
                "LD_LIBRARY_PATH",
                "VIVANTE_SDK_DIR",
                "VSI_NN_LOG_LEVEL",
            )
        ),
        manifest.remote_executable,
        manifest.remote_argument,
    ]
    command = " ".join(shlex.quote(token) for token in invocation)
    return (
        f"{command}\n"
        "wave_erf_bench_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_erf_bench_rc\"\n"
        'exit "$wave_erf_bench_rc"\n'
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$",
        re.MULTILINE,
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultError(f"expected one remote-exit sentinel, observed {len(matches)}")
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise ResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def run_once(
    transport: Any,
    manifest: BenchmarkManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise BenchmarkError(f"run ID must match {RUN_ID_RE.pattern}")
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc13g-r2-erf-cpu-vs-npu",
        "run_id": run_id,
        "started_at": npu_compile_lab._timestamp(),
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
        "manifest_file_sha256": EXPECTED_MANIFEST_FILE_SHA256,
        "git_commit": npu_compile_lab._git_commit(),
        "status": "INCOMPLETE",
        "errors": [],
        "health_differences": [],
        "kernel_alerts": [],
        "execution_attempts": 0,
        "result": None,
    }
    preflight = collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(run_directory / "preflight.json", preflight)
    try:
        verify_preflight(manifest, preflight)
    except PreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = npu_compile_lab._timestamp()
        npu_compile_lab._write_json_exclusive(run_directory / "run.json", envelope)
        return envelope, run_directory

    sentinel = f"__WAVE_ERF_BENCH_EXIT_{uuid.uuid4().hex}__"
    envelope["execution_attempts"] = 1
    started = time.monotonic()
    command_result = transport.run(
        ("shell", build_remote_command(manifest, sentinel)),
        manifest.timeout_ms,
    )
    duration_ms = int((time.monotonic() - started) * 1000)
    npu_compile_lab._write_text_exclusive(
        run_directory / "stdout.txt", command_result.stdout
    )
    npu_compile_lab._write_text_exclusive(
        run_directory / "stderr.txt", command_result.stderr
    )
    postflight = collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(run_directory / "postflight.json", postflight)
    differences, alerts = compare_health(manifest, preflight, postflight)
    envelope["health_differences"] = differences
    envelope["kernel_alerts"] = alerts
    envelope["transport"] = {
        "adb_returncode": command_result.returncode,
        "duration_ms": duration_ms,
        "timed_out": command_result.timed_out,
    }

    if command_result.timed_out:
        envelope["errors"].append(
            "host ADB deadline expired; no retry or recovery was attempted"
        )
    else:
        try:
            cleaned, remote_exit = parse_remote_exit(command_result.stdout, sentinel)
            envelope["transport"]["remote_exit"] = remote_exit
            try:
                npu_compile_lab.verify_transport_status(
                    command_result.returncode,
                    remote_exit,
                    "native CPU-versus-NPU benchmark",
                )
            except npu_compile_lab.ResultError as exc:
                raise ResultError(str(exc)) from exc
            native = parse_native_result(cleaned, manifest)
            output_state = postflight["timing_output_states"][REMOTE_TIMING]
            if (
                output_state.get("status") != "present"
                or output_state.get("permissions") != "-r--r--r--"
                or output_state.get("bytes") != TIMING_BYTES
            ):
                raise ResultError("remote fixed timing metadata differs")
            local_timing = run_directory / "erf-cpu-npu-timings-v1.bin"
            pull = transport.run(("pull", REMOTE_TIMING, str(local_timing)), 15_000)
            npu_compile_lab._write_text_exclusive(
                run_directory / "pull.stdout.txt", pull.stdout
            )
            npu_compile_lab._write_text_exclusive(
                run_directory / "pull.stderr.txt", pull.stderr
            )
            if pull.timed_out or pull.returncode != 0 or not local_timing.is_file():
                raise ResultError("ADB timing pull failed")
            payload = local_timing.read_bytes()
            if hashlib.sha256(payload).hexdigest() != output_state.get("sha256"):
                raise ResultError("pulled/remote timing hashes differ")
            timing_fnv = f"0x{stream_lab.fnv1a64(payload):016x}"
            if timing_fnv != native["timing_fnv1a64"]:
                raise ResultError("pulled/native timing FNV differs")
            timing = parse_timing_payload(payload)
            timing_summary = summarize_timings(timing)
            if (
                timing_summary["cpu_thread_ns"]["resolution_limited_zeros"]
                != native["cpu_thread_zero_count"]
            ):
                raise ResultError("native/raw CPU thread zero counts differ")
            envelope["result"] = {
                "native": native,
                "timing_artifact": {
                    "path": local_timing.relative_to(REPOSITORY_ROOT).as_posix(),
                    "bytes": len(payload),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                    "fnv1a64": timing_fnv,
                    "schema": TIMING_SCHEMA_VERSION,
                    "records": len(timing.records),
                },
                "timings": timing_summary,
                "compile_count": 1,
                "cpu_count": TOTAL_FRAMES,
                "npu_run_count": TOTAL_FRAMES,
                "canaries": "PASS",
            }
        except (OSError, ResultError) as exc:
            envelope["errors"].append(str(exc))
    if differences:
        envelope["errors"].append("device health gate failed")
    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = npu_compile_lab._timestamp()
    npu_compile_lab._write_json_exclusive(run_directory / "run.json", envelope)
    return envelope, run_directory


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Fixed Wave PoC 13G Cortex-A55 versus NPU benchmark; "
            "no arbitrary inputs, retry, recovery, or target mutation"
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    run = subparsers.add_parser("run")
    run.add_argument("--run-id", required=True)
    subparsers.add_parser("manifest")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        manifest = load_manifest()
        if args.command == "manifest":
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "manifest_file_sha256": (EXPECTED_MANIFEST_FILE_SHA256),
                        "mode": "benchmark",
                        "cold_frames": COLD_FRAMES,
                        "warmup_frames": WARMUP_FRAMES,
                        "measured_frames": MEASURED_FRAMES,
                        "total_frames": TOTAL_FRAMES,
                        "width": FRAME_WIDTH,
                        "expected_digests": expected_digests(),
                        "timing_schema": TIMING_SCHEMA_VERSION,
                        "timing_bytes": TIMING_BYTES,
                        "executes_graph": True,
                        "has_retry_interface": False,
                        "has_recovery_interface": False,
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        if args.command == "run":
            envelope, run_directory = run_once(
                npu_lab.AdbTransport(), manifest, args.run_id
            )
            print(
                json.dumps(
                    {
                        "status": envelope["status"],
                        "run_directory": run_directory.relative_to(
                            REPOSITORY_ROOT
                        ).as_posix(),
                        "execution_attempts": envelope["execution_attempts"],
                        "errors": envelope["errors"],
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0 if envelope["status"] == "PASS" else 1
        raise AssertionError(args.command)
    except (BenchmarkError, npu_lab.NpuLabError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
