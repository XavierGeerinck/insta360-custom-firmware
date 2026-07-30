#!/usr/bin/env python3
"""Exactly-once Wave stock CTC CPU-versus-NPU benchmark controller."""

from __future__ import annotations

import argparse
import functools
import hashlib
import json
import math
import re
import shlex
import statistics
import struct
import sys
import time
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import npu_compile_lab
import npu_lab


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name("ctc_cpu_npu_benchmark_manifest.json")
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"

REMOTE_ROOT = "/tmp/wave-npu-poc15a-r2"
REMOTE_EXECUTABLE = f"{REMOTE_ROOT}/bin/wave-npu-benchmark-ctc-cpu-vs-npu-v2"
REMOTE_ONNX = f"{REMOTE_ROOT}/model/ctc.onnx"
REMOTE_NBG = f"{REMOTE_ROOT}/model/ctc_online.nb"
REMOTE_MANIFEST = f"{REMOTE_ROOT}/meta/manifest.json"
REMOTE_TIMING = f"{REMOTE_ROOT}/result/ctc-cpu-npu-timings-v2.bin"
REMOTE_PARTIAL_TIMING = f"{REMOTE_ROOT}/result/ctc-cpu-npu-timings-v2.partial"
REMOTE_ARGUMENT = "--benchmark"
EXPECTED_TIMEOUT_MS = 120_000
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
EXPECTED_ARTIFACTS = {
    REMOTE_EXECUTABLE: (
        "d5316b2b45d49edbbc83daf368f32776388580a4e7db3667d0fe4ac88ebccf52"
    ),
    REMOTE_ONNX: ("8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219"),
    REMOTE_NBG: ("3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5"),
    "/usr/lib/libArchModelSw.so": (
        "2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f"
    ),
    "/usr/lib/libGAL.so": (
        "13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6"
    ),
    "/usr/lib/libNNArchPerf.so": (
        "253167677e9839d09815de93a86423824f26f56f097c100d59c03170adecffa9"
    ),
    "/usr/lib/libOpenVX.so": (
        "f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb"
    ),
    "/usr/lib/libVSC.so": (
        "e9a39fe3b522b3926ff34f3fb380c73983ad8d48c5f40ffd6e6d8024adc2ed18"
    ),
    "/usr/lib/libnnsdk.so": (
        "36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe"
    ),
    "/usr/lib/libonnxruntime.so.1.15.1": (
        "457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639"
    ),
    "/usr/lib/libovxlib.so": (
        "09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9"
    ),
}
EXPECTED_ABSENT_ARTIFACTS = (
    "/lib/libCLC.so",
    f"{REMOTE_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
EXPECTED_STAGED = {
    REMOTE_EXECUTABLE: ("-r-xr-xr-x", 49_848),
    REMOTE_ONNX: ("-r--r--r--", 10_258_790),
    REMOTE_NBG: ("-r--r--r--", 5_263_360),
}
EXPECTED_LOCAL_ARTIFACTS = {
    "binary": {
        "path": ("artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v2"),
        "sha256": EXPECTED_ARTIFACTS[REMOTE_EXECUTABLE],
        "bytes": 49_848,
    },
    "build_script": {
        "path": ("waved/tools/npu-compiler/build-device-ctc-cpu-npu-benchmark.sh"),
        "sha256": ("06e2fb0852c6043459988d6e6889663e22080a08a3002f6ef3292b407c37fbf5"),
    },
    "onnxruntime_header": {
        "path": (
            "artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/onnxruntime_c_api.h"
        ),
        "sha256": ("cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91"),
        "bytes": 185_543,
    },
    "policy_script": {
        "path": (
            "waved/tools/npu-compiler/verify-device-ctc-cpu-npu-benchmark-policy.sh"
        ),
        "sha256": ("728eff19e5ce08432118766eba58db71e6ba834bd2751cf94a882d7317f3ef57"),
    },
    "source": {
        "path": ("waved/tools/npu-compiler/wave_npu_benchmark_ctc_cpu_vs_npu.cc"),
        "sha256": ("6d924c2779f2636e50aab49656d015165dab487a058c73691b857c14692b7d27"),
    },
}

INPUT_ELEMENTS = 16 * 512
OUTPUT_ELEMENTS = 16 * 4999
FIXTURE_COUNT = 4
COLD_PAIRS = 1
WARMUP_PAIRS = 20
MEASURED_PAIRS = 200
QUALIFICATION_SEED = 0x573315A1
BENCHMARK_SEED = 0x573315B1
FINAL_QUALIFICATION_STATE = 0x474C5D42
FINAL_BENCHMARK_STATE = 0x98D70268
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3
MEAN_ABSOLUTE_ERROR_MAX = 0.025
MAXIMUM_ABSOLUTE_ERROR_MAX = 0.25
COSINE_SIMILARITY_MIN = 0.999
BRIDGE_RATIO_OF_MEANS_MAX = 0.9
BRIDGE_PAIRED_RATIO_P50_MAX = 0.9
BRIDGE_PAIRED_RATIO_P95_MAX_EXCLUSIVE = 1.0

TIMING_MAGIC = b"CCN2"
TIMING_SCHEMA_VERSION = 2
HEADER_STRUCT = struct.Struct("<4s15I")
QUALIFICATION_STRUCT = struct.Struct("<IIIIQQQQddddd")
TIMING_STRUCT = struct.Struct("<II16Q")
TIMING_BYTES = (
    HEADER_STRUCT.size
    + FIXTURE_COUNT * QUALIFICATION_STRUCT.size
    + (COLD_PAIRS + MEASURED_PAIRS) * TIMING_STRUCT.size
)

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
FNV_RE = re.compile(r"^0x[0-9a-f]{16}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_CTC_BENCH_EXIT_[0-9a-f]{32}__$")
QUALIFICATION_RE = re.compile(
    r"^CTC_QUAL fixture=(?P<name>zero|impulse|stripe|xorshift) "
    r"fixture_id=(?P<id>[0-3]) status=PASS top1=16 finite=79984 "
    r"mae=(?P<mae>[0-9.eE+-]+) max_abs=(?P<max>[0-9.eE+-]+) "
    r"cosine=(?P<cosine>[0-9.eE+-]+) repeat=PASS$"
)
SUMMARY_RE = re.compile(
    r"^CTC_CPU_NPU_BENCH status=PASS mode=benchmark "
    r"cold_pairs=1 qualification_fixtures=4 warmup_pairs=20 "
    r"measured_pairs=200 cpu_run_count=225 npu_run_count=229 "
    r"module_create_count=1 session_create_count=1 "
    r"canaries=PASS semantics=PASS timing_bytes=27752 "
    r"timing_fnv1a64=(?P<timing>0x[0-9a-f]{16}) "
    r"measured_input_fnv1a64=(?P<input>0x[0-9a-f]{16}) "
    r"cpu_output_fnv1a64=(?P<cpu>0x[0-9a-f]{16}) "
    r"npu_output_fnv1a64=(?P<npu>0x[0-9a-f]{16}) "
    r"final_qualification_state=(?P<qualification>0x[0-9a-f]{8}) "
    r"final_benchmark_state=(?P<benchmark>0x[0-9a-f]{8}) "
    r"process_cpu_ns=(?P<process_cpu>[0-9]+) "
    r"process_wall_ns=(?P<process_wall>[0-9]+)$"
)


class BenchmarkError(Exception):
    """Expected PoC 15A-R2 failure."""


class ManifestError(BenchmarkError):
    """The fixed manifest differs from policy."""


class PreflightError(BenchmarkError):
    """The staged device baseline is not eligible."""


class ResultError(BenchmarkError):
    """Native or binary evidence is malformed."""


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
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    sha256: str
    file_sha256: str
    policy: ExecutionPolicy
    candidates: Mapping[str, Any]


@dataclass(frozen=True)
class QualificationRecord:
    fixture_id: int
    top1_matches: int
    finite_count: int
    reserved: int
    cpu_float_fnv: int
    cpu_half_fnv: int
    npu_half_fnv: int
    npu_repeat_fnv: int
    sum_absolute_error: float
    maximum_absolute_error: float
    cosine_dot_product: float
    cosine_cpu_squared: float
    cosine_npu_squared: float


@dataclass(frozen=True)
class TimingRecord:
    sequence: int
    order: int
    cpu_wall_ns: int
    cpu_thread_ns: int
    convert_in_wall_ns: int
    convert_in_thread_ns: int
    input_set_wall_ns: int
    input_set_thread_ns: int
    run_wall_ns: int
    run_thread_ns: int
    output_get_wall_ns: int
    output_get_thread_ns: int
    convert_out_wall_ns: int
    convert_out_thread_ns: int
    native_total_wall_ns: int
    native_total_thread_ns: int
    bridge_total_wall_ns: int
    bridge_total_thread_ns: int


@dataclass(frozen=True)
class TimingPayload:
    qualifications: tuple[QualificationRecord, ...]
    cold: TimingRecord
    measured: tuple[TimingRecord, ...]


def _duplicate_safe(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65_536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(path: Path = DEFAULT_MANIFEST) -> BenchmarkManifest:
    try:
        raw = path.read_bytes()
        document = json.loads(raw.decode("utf-8"), object_pairs_hook=_duplicate_safe)
    except ManifestError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot load manifest: {exc}") from exc
    expected_keys = {
        "schema_version",
        "experiment",
        "device",
        "environment",
        "invocation",
        "local_artifacts",
        "required_artifacts",
        "absent_artifacts",
        "expected_result",
    }
    if not isinstance(document, dict) or set(document) != expected_keys:
        raise ManifestError("manifest root keys differ")
    expected_device = {
        "serial": "PSP_0000000000000000",
        "kernel": EXPECTED_KERNEL,
        "galcore_major_minor": "199:0",
        "npu_parameters": EXPECTED_PARAMETERS,
    }
    expected_invocation = {
        "remote_root": REMOTE_ROOT,
        "executable": REMOTE_EXECUTABLE,
        "argument": REMOTE_ARGUMENT,
        "timing": REMOTE_TIMING,
        "partial_timing": REMOTE_PARTIAL_TIMING,
        "timeout_ms": EXPECTED_TIMEOUT_MS,
    }
    expected_result = {
        "benchmark_seed": "0x573315b1",
        "cold_pairs": COLD_PAIRS,
        "cosine_similarity_min": COSINE_SIMILARITY_MIN,
        "cpu_run_count": 225,
        "final_benchmark_state": "0x98d70268",
        "final_qualification_state": "0x474c5d42",
        "fixture_count": FIXTURE_COUNT,
        "fp32_bridge_ratio_of_means_max": BRIDGE_RATIO_OF_MEANS_MAX,
        "fp32_bridge_ratio_p50_max": BRIDGE_PAIRED_RATIO_P50_MAX,
        "fp32_bridge_ratio_p95_max_exclusive": (BRIDGE_PAIRED_RATIO_P95_MAX_EXCLUSIVE),
        "impulse_linear_index": 0,
        "maximum_absolute_error_max": MAXIMUM_ABSOLUTE_ERROR_MAX,
        "mean_absolute_error_max": MEAN_ABSOLUTE_ERROR_MAX,
        "measured_pairs": MEASURED_PAIRS,
        "npu_run_count": 229,
        "performance_semantics": "finite-materialized-output-only",
        "qualification_before_timing": True,
        "qualification_record_bytes": QUALIFICATION_STRUCT.size,
        "qualification_seed": "0x573315a1",
        "random_mapping": "(xorshift32&255)-128 over 256",
        "timing_bytes": TIMING_BYTES,
        "timing_record_bytes": TIMING_STRUCT.size,
        "timing_schema": "CCN2-v2",
        "top1_matches_required": 64,
        "warmup_pairs": WARMUP_PAIRS,
    }
    if (
        document["schema_version"] != 1
        or document["experiment"] != "wave-npu-poc15a-r2-ctc-cpu-vs-npu"
        or document["device"] != expected_device
        or document["environment"] != EXPECTED_ENVIRONMENT
        or document["invocation"] != expected_invocation
        or document["expected_result"] != expected_result
        or document["required_artifacts"] != EXPECTED_ARTIFACTS
        or tuple(document["absent_artifacts"]) != EXPECTED_ABSENT_ARTIFACTS
        or document["local_artifacts"] != EXPECTED_LOCAL_ARTIFACTS
    ):
        raise ManifestError("manifest fixed policy differs")
    for record in EXPECTED_LOCAL_ARTIFACTS.values():
        local_path = REPOSITORY_ROOT / record["path"]
        if (
            not local_path.is_file()
            or local_path.is_symlink()
            or _sha256(local_path) != record["sha256"]
            or ("bytes" in record and local_path.stat().st_size != record["bytes"])
        ):
            raise ManifestError(f"local artifact differs: {record['path']}")
    return BenchmarkManifest(
        serial=expected_device["serial"],
        kernel=EXPECTED_KERNEL,
        galcore_major_minor="199:0",
        npu_parameters=EXPECTED_PARAMETERS,
        environment=EXPECTED_ENVIRONMENT,
        remote_executable=REMOTE_EXECUTABLE,
        remote_argument=REMOTE_ARGUMENT,
        remote_timing=REMOTE_TIMING,
        remote_partial_timing=REMOTE_PARTIAL_TIMING,
        timeout_ms=EXPECTED_TIMEOUT_MS,
        required_artifacts=EXPECTED_ARTIFACTS,
        absent_artifacts=EXPECTED_ABSENT_ARTIFACTS,
        sha256=_canonical_sha256(document),
        file_sha256=hashlib.sha256(raw).hexdigest(),
        policy=ExecutionPolicy(EXPECTED_ABSENT_ARTIFACTS),
        candidates={},
    )


def _update_fnv(value: int, payload: bytes) -> int:
    for byte in payload:
        value ^= byte
        value = (value * FNV64_PRIME) & 0xFFFFFFFFFFFFFFFF
    return value


def _next_xorshift32(state: int) -> int:
    state ^= (state << 13) & 0xFFFFFFFF
    state ^= state >> 17
    state ^= (state << 5) & 0xFFFFFFFF
    return state & 0xFFFFFFFF


@functools.lru_cache(maxsize=1)
def expected_measured_input_fnv() -> str:
    state = BENCHMARK_SEED
    digest = FNV64_OFFSET_BASIS
    total_pairs = COLD_PAIRS + WARMUP_PAIRS + MEASURED_PAIRS
    for pair in range(total_pairs):
        payload = bytearray(INPUT_ELEMENTS * 4)
        for index in range(INPUT_ELEMENTS):
            state = _next_xorshift32(state)
            quantized = (state & 0xFF) - 128
            struct.pack_into("<f", payload, index * 4, quantized / 256.0)
        if pair >= COLD_PAIRS + WARMUP_PAIRS:
            digest = _update_fnv(digest, payload)
    if state != FINAL_BENCHMARK_STATE:
        raise AssertionError(f"host generator ended at 0x{state:08x}")
    return f"0x{digest:016x}"


def _qualification_metrics(record: QualificationRecord) -> dict[str, float]:
    if record.finite_count <= 0:
        raise ResultError("qualification finite count is zero")
    denominator = math.sqrt(record.cosine_cpu_squared * record.cosine_npu_squared)
    if denominator <= 0 or not math.isfinite(denominator):
        raise ResultError("qualification cosine denominator is invalid")
    return {
        "mean_absolute_error": (record.sum_absolute_error / record.finite_count),
        "maximum_absolute_error": record.maximum_absolute_error,
        "cosine_similarity": record.cosine_dot_product / denominator,
    }


def _validate_qualification(record: QualificationRecord, expected_id: int) -> None:
    numeric = (
        record.sum_absolute_error,
        record.maximum_absolute_error,
        record.cosine_dot_product,
        record.cosine_cpu_squared,
        record.cosine_npu_squared,
    )
    if (
        record.fixture_id != expected_id
        or record.top1_matches != 16
        or record.finite_count != OUTPUT_ELEMENTS
        or record.reserved != 0
        or record.npu_half_fnv != record.npu_repeat_fnv
        or any(
            value == 0
            for value in (
                record.cpu_float_fnv,
                record.cpu_half_fnv,
                record.npu_half_fnv,
            )
        )
        or not all(math.isfinite(value) for value in numeric)
    ):
        raise ResultError(f"qualification record {expected_id} is malformed")
    metrics = _qualification_metrics(record)
    if (
        metrics["mean_absolute_error"] > MEAN_ABSOLUTE_ERROR_MAX
        or metrics["maximum_absolute_error"] > MAXIMUM_ABSOLUTE_ERROR_MAX
        or metrics["cosine_similarity"] < COSINE_SIMILARITY_MIN
        or metrics["cosine_similarity"] > 1.000001
    ):
        raise ResultError(f"qualification record {expected_id} failed gates")


def _validate_timing(record: TimingRecord, sequence: int, order: int) -> None:
    if record.sequence != sequence or record.order != order:
        raise ResultError("timing sequence/order differs")
    durations = tuple(asdict(record).values())[2:]
    if any(value <= 0 for value in durations):
        raise ResultError("timing record contains a zero duration")
    stage_wall = (
        record.input_set_wall_ns + record.run_wall_ns + record.output_get_wall_ns
    )
    bridge_wall = (
        record.convert_in_wall_ns
        + record.native_total_wall_ns
        + record.convert_out_wall_ns
    )
    if (
        record.native_total_wall_ns < stage_wall
        or record.bridge_total_wall_ns < bridge_wall
    ):
        raise ResultError("timing total is smaller than its measured stages")


def parse_timing_payload(payload: bytes) -> TimingPayload:
    if len(payload) != TIMING_BYTES:
        raise ResultError(
            f"timing byte count differs: {len(payload)} != {TIMING_BYTES}"
        )
    header = HEADER_STRUCT.unpack_from(payload)
    expected_header = (
        TIMING_MAGIC,
        TIMING_SCHEMA_VERSION,
        FIXTURE_COUNT,
        MEASURED_PAIRS,
        INPUT_ELEMENTS,
        OUTPUT_ELEMENTS,
        INPUT_ELEMENTS * 4,
        INPUT_ELEMENTS * 2,
        OUTPUT_ELEMENTS * 4,
        OUTPUT_ELEMENTS * 2,
        COLD_PAIRS,
        WARMUP_PAIRS,
        QUALIFICATION_SEED,
        BENCHMARK_SEED,
        TIMING_STRUCT.size,
        QUALIFICATION_STRUCT.size,
    )
    if header != expected_header:
        raise ResultError("timing header differs from CCN2-v2")
    offset = HEADER_STRUCT.size
    qualifications: list[QualificationRecord] = []
    for fixture_id in range(FIXTURE_COUNT):
        record = QualificationRecord(*QUALIFICATION_STRUCT.unpack_from(payload, offset))
        offset += QUALIFICATION_STRUCT.size
        _validate_qualification(record, fixture_id)
        qualifications.append(record)
    cold = TimingRecord(*TIMING_STRUCT.unpack_from(payload, offset))
    offset += TIMING_STRUCT.size
    _validate_timing(cold, 0, 0)
    measured: list[TimingRecord] = []
    for sequence in range(MEASURED_PAIRS):
        record = TimingRecord(*TIMING_STRUCT.unpack_from(payload, offset))
        offset += TIMING_STRUCT.size
        _validate_timing(record, sequence, sequence & 1)
        measured.append(record)
    if offset != len(payload):
        raise ResultError("timing parser did not consume the payload")
    return TimingPayload(tuple(qualifications), cold, tuple(measured))


def _nearest_rank(values: Sequence[int | float], quantile: float) -> int | float:
    ordered = sorted(values)
    rank = max(1, math.ceil(quantile * len(ordered)))
    return ordered[rank - 1]


def _statistics(values: Sequence[int | float]) -> dict[str, int | float]:
    ordered = sorted(values)
    return {
        "count": len(ordered),
        "mean": statistics.fmean(ordered),
        "minimum": ordered[0],
        "p50": _nearest_rank(ordered, 0.50),
        "p95": _nearest_rank(ordered, 0.95),
        "p99": _nearest_rank(ordered, 0.99),
        "maximum": ordered[-1],
        "standard_deviation": statistics.pstdev(ordered),
    }


def summarize_timings(timing: TimingPayload) -> dict[str, Any]:
    fields = {
        name: tuple(getattr(record, name) for record in timing.measured)
        for name in (
            "cpu_wall_ns",
            "cpu_thread_ns",
            "convert_in_wall_ns",
            "input_set_wall_ns",
            "run_wall_ns",
            "output_get_wall_ns",
            "convert_out_wall_ns",
            "native_total_wall_ns",
            "bridge_total_wall_ns",
            "bridge_total_thread_ns",
        )
    }
    summary: dict[str, Any] = {
        name: _statistics(values) for name, values in fields.items()
    }
    cpu_mean = float(summary["cpu_wall_ns"]["mean"])
    native_mean = float(summary["native_total_wall_ns"]["mean"])
    bridge_mean = float(summary["bridge_total_wall_ns"]["mean"])
    paired_native = tuple(
        record.native_total_wall_ns / record.cpu_wall_ns for record in timing.measured
    )
    paired_bridge = tuple(
        record.bridge_total_wall_ns / record.cpu_wall_ns for record in timing.measured
    )
    ratio_of_means = bridge_mean / cpu_mean
    bridge_p50 = float(_nearest_rank(paired_bridge, 0.50))
    bridge_p95 = float(_nearest_rank(paired_bridge, 0.95))
    performance_pass = (
        ratio_of_means <= BRIDGE_RATIO_OF_MEANS_MAX
        and bridge_p50 <= BRIDGE_PAIRED_RATIO_P50_MAX
        and bridge_p95 < BRIDGE_PAIRED_RATIO_P95_MAX_EXCLUSIVE
    )
    summary["ratios"] = {
        "native_total_over_cpu_ratio_of_means": native_mean / cpu_mean,
        "bridge_total_over_cpu_ratio_of_means": ratio_of_means,
        "paired_native_total_over_cpu": _statistics(paired_native),
        "paired_bridge_total_over_cpu": _statistics(paired_bridge),
    }
    summary["throughput_inferences_per_second"] = {
        "cpu": 1_000_000_000.0 / cpu_mean,
        "npu_native": 1_000_000_000.0 / native_mean,
        "npu_fp32_bridge": 1_000_000_000.0 / bridge_mean,
    }
    summary["cold"] = asdict(timing.cold)
    summary["qualification"] = [
        {
            **asdict(record),
            **_qualification_metrics(record),
            "cpu_float_fnv": f"0x{record.cpu_float_fnv:016x}",
            "cpu_half_fnv": f"0x{record.cpu_half_fnv:016x}",
            "npu_half_fnv": f"0x{record.npu_half_fnv:016x}",
            "npu_repeat_fnv": f"0x{record.npu_repeat_fnv:016x}",
        }
        for record in timing.qualifications
    ]
    summary["performance_gate"] = {
        "ratio_of_means": ratio_of_means,
        "ratio_of_means_max": BRIDGE_RATIO_OF_MEANS_MAX,
        "paired_p50": bridge_p50,
        "paired_p50_max": BRIDGE_PAIRED_RATIO_P50_MAX,
        "paired_p95": bridge_p95,
        "paired_p95_max_exclusive": (BRIDGE_PAIRED_RATIO_P95_MAX_EXCLUSIVE),
        "pass": performance_pass,
    }
    summary["decision"] = "npu-faster" if performance_pass else "cpu-faster"
    return summary


def parse_native_result(stdout: str, manifest: BenchmarkManifest) -> dict[str, Any]:
    identities = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    qualification_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("CTC_QUAL ")
    ]
    summaries = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("CTC_CPU_NPU_BENCH ")
    ]
    if (
        len(identities) != 1
        or len(qualification_lines) != FIXTURE_COUNT
        or len(summaries) != 1
    ):
        raise ResultError("native result framing differs")
    identity = npu_compile_lab.IDENTITY_RE.fullmatch(identities[0])
    summary = SUMMARY_RE.fullmatch(summaries[0])
    qualifications = [QUALIFICATION_RE.fullmatch(line) for line in qualification_lines]
    if (
        identity is None
        or summary is None
        or any(match is None for match in qualifications)
    ):
        raise ResultError("native result line is malformed")
    expected_identity = {
        "chip_model": "0x00009000",
        "chip_revision": "0x00009002",
        "chip_product_id": "0x05090001",
        "chip_eco_id": "0x04000000",
        "customer_id": "0x1000000e",
        "reserved": "0x00000000",
        "flags": "0x0000000000000000",
    }
    if identity.groupdict() != expected_identity:
        raise ResultError("live chip identity differs")
    for expected_id, match in enumerate(qualifications):
        assert match is not None
        numeric = tuple(float(match[name]) for name in ("mae", "max", "cosine"))
        if (
            int(match["id"]) != expected_id
            or not all(math.isfinite(value) for value in numeric)
            or numeric[0] > MEAN_ABSOLUTE_ERROR_MAX
            or numeric[1] > MAXIMUM_ABSOLUTE_ERROR_MAX
            or numeric[2] < COSINE_SIMILARITY_MIN
        ):
            raise ResultError("native qualification summary failed")
    observed = summary.groupdict()
    if (
        observed["input"] != expected_measured_input_fnv()
        or observed["qualification"] != f"0x{FINAL_QUALIFICATION_STATE:08x}"
        or observed["benchmark"] != f"0x{FINAL_BENCHMARK_STATE:08x}"
        or FNV_RE.fullmatch(observed["cpu"]) is None
        or FNV_RE.fullmatch(observed["npu"]) is None
        or int(observed["process_cpu"]) <= 0
        or int(observed["process_wall"]) <= 0
    ):
        raise ResultError("native digest/state/process result differs")
    return {
        "identity": identity.groupdict(),
        "qualification_lines": qualification_lines,
        "timing_fnv1a64": observed["timing"],
        "measured_input_fnv1a64": observed["input"],
        "cpu_output_fnv1a64": observed["cpu"],
        "npu_output_fnv1a64": observed["npu"],
        "process_cpu_ns": int(observed["process_cpu"]),
        "process_wall_ns": int(observed["process_wall"]),
        "cpu_run_count": 225,
        "npu_run_count": 229,
        "manifest_sha256": manifest.sha256,
    }


def collect_snapshot(transport: Any, manifest: BenchmarkManifest) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in (*EXPECTED_STAGED, REMOTE_MANIFEST)
    }
    snapshot["timing_output_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in (REMOTE_TIMING, REMOTE_PARTIAL_TIMING)
    }
    process_text = npu_compile_lab._shell_value(
        transport,
        (
            "ps 2>/dev/null | "
            "grep '[w]ave-npu-benchmark-ctc-cpu-vs-npu-v2' | "
            "head -n 8 || true"
        ),
        "benchmark process scan",
        errors,
    )
    snapshot["benchmark_processes"] = npu_compile_lab._bounded_lines(
        process_text or "", 8
    )
    context_commands = {
        "cpu_online": "cat /sys/devices/system/cpu/online",
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


def verify_preflight(manifest: BenchmarkManifest, snapshot: Mapping[str, Any]) -> None:
    if snapshot.get("errors"):
        raise PreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise PreflightError("authorized Wave is not exclusively connected")
    if snapshot.get("kernel") != manifest.kernel:
        raise PreflightError("kernel identity differs")
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(boot_id) is None
    ):
        raise PreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != EXPECTED_PARAMETERS:
        raise PreflightError("NPU parameters differ")
    if re.search(r"\b199,\s*0\b", snapshot.get("galcore_device") or "") is None:
        raise PreflightError("/dev/galcore is not character device 199:0")
    if snapshot.get("galcore_holders"):
        raise PreflightError("/dev/galcore already has a holder")
    if snapshot.get("benchmark_processes"):
        raise PreflightError("PoC 15A-R2 process is already running")
    for path, expected in EXPECTED_ARTIFACTS.items():
        if snapshot.get("artifact_hashes", {}).get(path) != expected:
            raise PreflightError(f"artifact differs: {path}")
    for path in EXPECTED_ABSENT_ARTIFACTS:
        state = snapshot.get("absent_artifact_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise PreflightError(f"forbidden CLC is visible: {path}")
    for path, (mode, byte_count) in EXPECTED_STAGED.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != mode
            or state.get("bytes") != byte_count
        ):
            raise PreflightError(f"staged metadata differs: {path}")
    manifest_state = snapshot.get("staged_artifact_states", {}).get(REMOTE_MANIFEST, {})
    if (
        manifest_state.get("status") != "present"
        or manifest_state.get("permissions") != "-r--r--r--"
        or manifest_state.get("sha256") != manifest.file_sha256
    ):
        raise PreflightError("staged manifest differs")
    if any(
        state.get("status") != "absent"
        for state in snapshot.get("timing_output_states", {}).values()
    ):
        raise PreflightError("timing output already exists")
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")
    context = snapshot.get("system_context", {})
    if not context.get("cpu_online") or not context.get("proc_stat"):
        raise PreflightError("CPU context is incomplete")


def compare_health(
    manifest: BenchmarkManifest,
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> tuple[list[str], list[str]]:
    differences, alerts = npu_compile_lab.compare_health(manifest, before, after)
    if before.get("staged_artifact_states") != after.get("staged_artifact_states"):
        differences.append("staged artifact metadata changed")
    if after.get("benchmark_processes"):
        differences.append("benchmark process remains")
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
            f"{name}={manifest.environment[name]}"
            for name in (
                "LD_LIBRARY_PATH",
                "VIVANTE_SDK_DIR",
                "VSI_NN_LOG_LEVEL",
            )
        ),
        manifest.remote_executable,
        manifest.remote_argument,
    ]
    command = " ".join(shlex.quote(value) for value in invocation)
    return (
        f"{command}\n"
        "wave_ctc_bench_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_ctc_bench_rc\"\n"
        'exit "$wave_ctc_bench_rc"\n'
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$", re.MULTILINE)
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultError("remote exit sentinel count differs")
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise ResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def _report_markdown(result: Mapping[str, Any]) -> str:
    timings = result["timings"]
    gate = timings["performance_gate"]
    cpu = timings["cpu_wall_ns"]
    native = timings["native_total_wall_ns"]
    bridge = timings["bridge_total_wall_ns"]
    return (
        "# Wave PoC 15A-R2 — stock CTC CPU versus NPU\n\n"
        f"Decision: **{timings['decision']}**.\n\n"
        "| Boundary | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) |\n"
        "|---|---:|---:|---:|---:|\n"
        f"| Stock ORT CPU | {cpu['mean'] / 1e6:.6f} | "
        f"{cpu['p50'] / 1e6:.6f} | {cpu['p95'] / 1e6:.6f} | "
        f"{cpu['p99'] / 1e6:.6f} |\n"
        f"| NPU native | {native['mean'] / 1e6:.6f} | "
        f"{native['p50'] / 1e6:.6f} | {native['p95'] / 1e6:.6f} | "
        f"{native['p99'] / 1e6:.6f} |\n"
        f"| NPU FP32 bridge | {bridge['mean'] / 1e6:.6f} | "
        f"{bridge['p50'] / 1e6:.6f} | {bridge['p95'] / 1e6:.6f} | "
        f"{bridge['p99'] / 1e6:.6f} |\n\n"
        "The custom-model prerequisite requires all three bridge gates. "
        f"Observed ratio of means `{gate['ratio_of_means']:.6f}`, "
        f"paired p50 `{gate['paired_p50']:.6f}`, and paired p95 "
        f"`{gate['paired_p95']:.6f}`; pass is `{gate['pass']}`.\n"
    )


def run_once(
    transport: Any, manifest: BenchmarkManifest, run_id: str
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise BenchmarkError(f"run ID must match {RUN_ID_RE.pattern}")
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc15a-r2-ctc-cpu-vs-npu",
        "run_id": run_id,
        "started_at": npu_compile_lab._timestamp(),
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
        "manifest_file_sha256": manifest.file_sha256,
        "git_commit": npu_compile_lab._git_commit(),
        "status": "INCOMPLETE",
        "execution_attempts": 0,
        "errors": [],
        "health_differences": [],
        "kernel_alerts": [],
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

    sentinel = f"__WAVE_CTC_BENCH_EXIT_{uuid.uuid4().hex}__"
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
            "host deadline expired; no retry or recovery was attempted"
        )
    else:
        try:
            cleaned, remote_exit = parse_remote_exit(command_result.stdout, sentinel)
            envelope["transport"]["remote_exit"] = remote_exit
            try:
                npu_compile_lab.verify_transport_status(
                    command_result.returncode,
                    remote_exit,
                    "native CTC CPU/NPU benchmark",
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
                raise ResultError("remote timing metadata differs")
            local_timing = run_directory / "ctc-cpu-npu-timings-v2.bin"
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
            payload_sha256 = hashlib.sha256(payload).hexdigest()
            if payload_sha256 != output_state.get("sha256"):
                raise ResultError("pulled/remote timing hashes differ")
            timing_fnv = f"0x{_update_fnv(FNV64_OFFSET_BASIS, payload):016x}"
            if timing_fnv != native["timing_fnv1a64"]:
                raise ResultError("pulled/native timing FNV differs")
            parsed = parse_timing_payload(payload)
            timing_summary = summarize_timings(parsed)
            envelope["result"] = {
                "native": native,
                "timing_artifact": {
                    "path": local_timing.relative_to(REPOSITORY_ROOT).as_posix(),
                    "bytes": len(payload),
                    "sha256": payload_sha256,
                    "fnv1a64": timing_fnv,
                    "schema": "CCN2-v2",
                    "qualification_records": len(parsed.qualifications),
                    "cold_records": 1,
                    "measured_records": len(parsed.measured),
                },
                "timings": timing_summary,
            }
        except (OSError, ResultError) as exc:
            envelope["errors"].append(str(exc))
    if differences:
        envelope["errors"].append("device health gate failed")
    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = npu_compile_lab._timestamp()
    if envelope["result"] is not None:
        npu_compile_lab._write_text_exclusive(
            run_directory / "report.md",
            _report_markdown(envelope["result"]),
        )
    npu_compile_lab._write_json_exclusive(run_directory / "run.json", envelope)
    return envelope, run_directory


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Fixed Wave PoC 15A-R2 stock CTC CPU/NPU benchmark; "
            "no arbitrary inputs, retry, recovery, or target mutation"
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("manifest")
    subparsers.add_parser("preflight")
    analyze = subparsers.add_parser("analyze")
    analyze.add_argument("timing", type=Path)
    run = subparsers.add_parser("run")
    run.add_argument("--run-id", required=True)
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
                        "manifest_file_sha256": manifest.file_sha256,
                        "timing_schema": "CCN2-v2",
                        "timing_bytes": TIMING_BYTES,
                        "expected_measured_input_fnv": (expected_measured_input_fnv()),
                        "execution_process_cap": 1,
                        "has_retry_interface": False,
                        "has_recovery_interface": False,
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        if args.command == "preflight":
            snapshot = collect_snapshot(npu_lab.AdbTransport(), manifest)
            verify_preflight(manifest, snapshot)
            print(json.dumps(snapshot, indent=2, sort_keys=True))
            return 0
        if args.command == "analyze":
            payload = args.timing.read_bytes()
            print(
                json.dumps(
                    summarize_timings(parse_timing_payload(payload)),
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
                        "decision": (
                            envelope["result"]["timings"]["decision"]
                            if envelope["result"] is not None
                            else None
                        ),
                        "errors": envelope["errors"],
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0 if envelope["status"] == "PASS" else 1
        raise AssertionError(args.command)
    except (BenchmarkError, npu_lab.NpuLabError, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
