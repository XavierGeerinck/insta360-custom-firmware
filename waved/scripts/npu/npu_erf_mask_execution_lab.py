#!/usr/bin/env python3
"""Sealed six-vector numerical qualification for the Wave Erf-mask NBG.

The device invocation, graph, model, vector pack, tensor contract, timeout,
and artifact identities are fixed. The host independently evaluates quantized
Erf with :mod:`math`; device output is accepted only on a byte-exact match.
There is no arbitrary command, path, input, retry, recovery, or benchmark
interface.
"""

from __future__ import annotations

import argparse
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
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import npu_compile_lab
import npu_lab


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name(
    "erf_mask_execution_manifest.json"
)
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"
LOCAL_VECTOR_PACK = (
    REPOSITORY_ROOT
    / "artifacts/npu/runs/poc13d-erf-mask-vectors-20260729"
    / "qualification-vectors.bin"
)
EXPECTED_MANIFEST_FILE_SHA256 = (
    "10565c58589afdf7e9514220863fd03a7"
    "d52b20235ea38be669b4f773d5f873c"
)

REMOTE_ROOT = "/tmp/wave-npu-poc13d"
REMOTE_EXECUTABLE = (
    f"{REMOTE_ROOT}/bin/wave-npu-run-erf-mask-qualification-v1"
)
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_NBG = f"{REMOTE_ROOT}/model/erf-mask.nbg"
REMOTE_VECTORS = f"{REMOTE_ROOT}/input/qualification-vectors.bin"
REMOTE_ARGUMENT = "--qualify-six"
EXPECTED_TIMEOUT_MS = 30000
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
    "VSI_NN_LOG_LEVEL": "5",
}
EXPECTED_IDENTITY = {
    "chip_model": "0x00009000",
    "chip_revision": "0x00009002",
    "chip_product_id": "0x05090001",
    "chip_eco_id": "0x04000000",
    "customer_id": "0x1000000e",
    "reserved": "0x00000000",
    "flags": "0x0000000000000000",
}
EXPECTED_ARTIFACTS = {
    REMOTE_EXECUTABLE: (
        "99d51397b8c6b21c360755b6a66cc62e"
        "ba3897287fb12a215468bd8f7e2f62d3"
    ),
    REMOTE_TIM_VX: (
        "d2c8fc7aaf0ad0b86f02fcd54bb1cdca"
        "9f04967b5cd9efa61a211565b453375b"
    ),
    REMOTE_NBG: (
        "96447d6afd63a7c349bbd80f0af99150"
        "c89ef24ed7f1ef66ab285075362e6c94"
    ),
    REMOTE_VECTORS: (
        "3374b928722c7d504e9ae8d8e1e99c1d"
        "c7b3542ee08cbede60b2f8bb8f93a232"
    ),
}
EXPECTED_ABSENT_ARTIFACTS = (
    "/lib/libCLC.so",
    f"{REMOTE_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
EXPECTED_STAGED_MODES = {
    REMOTE_EXECUTABLE: "-r-xr-xr-x",
    REMOTE_TIM_VX: "-r--r--r--",
    REMOTE_NBG: "-r--r--r--",
    REMOTE_VECTORS: "-r--r--r--",
}
EXPECTED_STAGED_BYTES = {
    REMOTE_NBG: 5840,
    REMOTE_VECTORS: 2902,
}
VECTOR_IDS = (
    "zero-point",
    "minimum",
    "maximum",
    "repeated-ramp",
    "seeded-random",
    "learned-fixture",
)
VECTOR_COUNT = len(VECTOR_IDS)
VECTOR_WIDTH = 481
VECTOR_PACK_HEADER = struct.Struct("<4sIII")
VECTOR_PACK_FNV1A64 = 0x8F90178B58CA8AFB
ERF_TABLE_SHA256 = (
    "c9525963e022d4a4aea0814845119b374"
    "88f3727ec8d96684f52e1e2ad776296"
)
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_ERF_MASK_EXIT_[0-9a-f]{32}__$")
VECTOR_RE = re.compile(
    r"^VECTOR index=(?P<index>[0-5]) "
    r"id=(?P<id>[a-z][a-z-]{0,31}) "
    r"duration_us=(?P<duration_us>[0-9]+) "
    r"input_fnv1a64=(?P<input_fnv>0x[0-9a-f]{16}) "
    r"output_fnv1a64=(?P<output_fnv>0x[0-9a-f]{16}) "
    r"output_hex=(?P<output_hex>[0-9a-f]{962})$"
)
SUMMARY_LINE = (
    "ERF_MASK status=PASS mode=qualify-six vectors=6 "
    "bytes_per_vector=481 compile_count=1 run_count=6 canaries=PASS"
)


class ErfMaskExecutionError(Exception):
    """Expected PoC 13D harness failure."""


class ManifestError(ErfMaskExecutionError):
    """The sealed manifest differs from fixed policy."""


class PreflightError(ErfMaskExecutionError):
    """The live device differs from the reviewed baseline."""


class ResultError(ErfMaskExecutionError):
    """The native result is malformed or numerically incorrect."""


@dataclass(frozen=True)
class ExecutionPolicy:
    absent_artifacts: tuple[str, ...]


@dataclass(frozen=True)
class ExecutionManifest:
    serial: str
    kernel: str
    galcore_major_minor: str
    npu_parameters: Mapping[str, str]
    environment: Mapping[str, str]
    remote_executable: str
    remote_argument: str
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    expected_result: Mapping[str, Any]
    sha256: str
    policy: ExecutionPolicy
    candidates: Mapping[str, Any]


def _without_duplicate_keys(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _keys(
    value: Any, expected: set[str], context: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ManifestError(f"{context} must be an object")
    missing = expected - set(value)
    unknown = set(value) - expected
    if missing or unknown:
        raise ManifestError(
            f"{context} key mismatch; missing={sorted(missing)}, "
            f"unknown={sorted(unknown)}"
        )
    return value


def _plain_string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value or "\x00" in value:
        raise ManifestError(f"{context} must be a nonempty plain string")
    return value


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(path: Path = DEFAULT_MANIFEST) -> ExecutionManifest:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise ManifestError(f"cannot read {path}: {exc}") from exc
    if (
        path.resolve() == DEFAULT_MANIFEST.resolve()
        and hashlib.sha256(raw).hexdigest()
        != EXPECTED_MANIFEST_FILE_SHA256
    ):
        raise ManifestError("source-controlled manifest file hash differs")
    try:
        document = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_without_duplicate_keys,
        )
    except ManifestError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot parse {path}: {exc}") from exc

    root = _keys(
        document,
        {
            "schema_version",
            "device",
            "environment",
            "remote_executable",
            "remote_argument",
            "timeout_ms",
            "required_artifacts",
            "absent_artifacts",
            "expected_result",
        },
        "manifest",
    )
    if root["schema_version"] != SCHEMA_VERSION:
        raise ManifestError("unsupported schema_version")

    device = _keys(
        root["device"],
        {"serial", "kernel", "galcore_major_minor", "npu_parameters"},
        "device",
    )
    parameters = _keys(
        device["npu_parameters"],
        {"nanoqFreq", "enableNN", "recovery"},
        "device.npu_parameters",
    )
    if (
        device["serial"] != "PSP_0000000000000000"
        or device["kernel"] != EXPECTED_KERNEL
        or device["galcore_major_minor"] != "199:0"
        or parameters != EXPECTED_PARAMETERS
    ):
        raise ManifestError("device identity differs from fixed policy")

    environment = _keys(
        root["environment"],
        {"LD_LIBRARY_PATH", "VIVANTE_SDK_DIR", "VSI_NN_LOG_LEVEL"},
        "environment",
    )
    if environment != EXPECTED_ENVIRONMENT:
        raise ManifestError("environment differs from fixed policy")
    if (
        root["remote_executable"] != REMOTE_EXECUTABLE
        or root["remote_argument"] != REMOTE_ARGUMENT
        or root["timeout_ms"] != EXPECTED_TIMEOUT_MS
    ):
        raise ManifestError("remote invocation differs from fixed policy")

    raw_artifacts = root["required_artifacts"]
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise ManifestError("required_artifacts must be nonempty")
    artifacts: dict[str, str] = {}
    for raw_path, raw_digest in raw_artifacts.items():
        artifact_path = _plain_string(raw_path, "artifact path")
        digest = _plain_string(raw_digest, f"digest for {artifact_path}")
        if (
            not artifact_path.startswith("/")
            or SHA256_RE.fullmatch(digest) is None
        ):
            raise ManifestError(f"invalid artifact allowlist: {artifact_path}")
        if (
            artifact_path.startswith("/tmp/")
            and not artifact_path.startswith(f"{REMOTE_ROOT}/")
        ):
            raise ManifestError(
                f"temporary artifact is outside {REMOTE_ROOT}"
            )
        artifacts[artifact_path] = digest
    for artifact_path, digest in EXPECTED_ARTIFACTS.items():
        if artifacts.get(artifact_path) != digest:
            raise ManifestError(f"fixed artifact mismatch: {artifact_path}")

    absent = root["absent_artifacts"]
    if (
        not isinstance(absent, list)
        or any(not isinstance(item, str) for item in absent)
        or len(set(absent)) != len(absent)
        or tuple(sorted(absent))
        != tuple(sorted(EXPECTED_ABSENT_ARTIFACTS))
    ):
        raise ManifestError("absent_artifacts differs from fixed CLC policy")

    expected = _keys(
        root["expected_result"],
        {
            "identity",
            "model_sha256",
            "vector_pack_sha256",
            "vector_pack_fnv1a64",
            "vector_count",
            "width",
            "vector_ids",
            "input_sha256",
            "output_sha256",
        },
        "expected_result",
    )
    if expected["identity"] != EXPECTED_IDENTITY:
        raise ManifestError("expected device identity differs")
    if (
        expected["model_sha256"] != EXPECTED_ARTIFACTS[REMOTE_NBG]
        or expected["vector_pack_sha256"]
        != EXPECTED_ARTIFACTS[REMOTE_VECTORS]
        or expected["vector_pack_fnv1a64"]
        != f"0x{VECTOR_PACK_FNV1A64:016x}"
        or expected["vector_count"] != VECTOR_COUNT
        or expected["width"] != VECTOR_WIDTH
        or expected["vector_ids"] != list(VECTOR_IDS)
    ):
        raise ManifestError("expected tensor/vector contract differs")
    for key in ("input_sha256", "output_sha256"):
        digests = expected[key]
        if (
            not isinstance(digests, list)
            or len(digests) != VECTOR_COUNT
            or any(
                not isinstance(digest, str)
                or SHA256_RE.fullmatch(digest) is None
                for digest in digests
            )
        ):
            raise ManifestError(f"expected_result.{key} is malformed")

    return ExecutionManifest(
        serial=device["serial"],
        kernel=device["kernel"],
        galcore_major_minor=device["galcore_major_minor"],
        npu_parameters=parameters,
        environment=environment,
        remote_executable=root["remote_executable"],
        remote_argument=root["remote_argument"],
        timeout_ms=root["timeout_ms"],
        required_artifacts=artifacts,
        absent_artifacts=tuple(sorted(absent)),
        expected_result=expected,
        sha256=_canonical_sha256(root),
        policy=ExecutionPolicy(tuple(sorted(absent))),
        candidates={},
    )


def fnv1a64(payload: bytes) -> int:
    value = FNV64_OFFSET_BASIS
    for byte in payload:
        value ^= byte
        value = (value * FNV64_PRIME) & 0xFFFFFFFFFFFFFFFF
    return value


def build_independent_erf_table() -> bytes:
    table = bytes(
        max(
            0,
            min(
                255,
                round(math.erf((value - 128) / 32.0) * 128.0) + 128,
            ),
        )
        for value in range(256)
    )
    if hashlib.sha256(table).hexdigest() != ERF_TABLE_SHA256:
        raise ResultError("independent quantized-Erf table hash differs")
    return table


def load_qualification_vectors(
    manifest: ExecutionManifest,
    path: Path = LOCAL_VECTOR_PACK,
) -> tuple[bytes, ...]:
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise ResultError(f"cannot read sealed vector pack: {exc}") from exc
    if (
        len(payload) != EXPECTED_STAGED_BYTES[REMOTE_VECTORS]
        or hashlib.sha256(payload).hexdigest()
        != manifest.expected_result["vector_pack_sha256"]
        or fnv1a64(payload) != VECTOR_PACK_FNV1A64
    ):
        raise ResultError("sealed qualification vector pack differs")
    magic, schema, count, width = VECTOR_PACK_HEADER.unpack_from(payload)
    if (
        magic != b"ERF1"
        or schema != 1
        or count != VECTOR_COUNT
        or width != VECTOR_WIDTH
    ):
        raise ResultError("qualification vector header differs")
    vectors = tuple(
        payload[
            VECTOR_PACK_HEADER.size + index * VECTOR_WIDTH :
            VECTOR_PACK_HEADER.size + (index + 1) * VECTOR_WIDTH
        ]
        for index in range(VECTOR_COUNT)
    )
    table = build_independent_erf_table()
    for index, vector in enumerate(vectors):
        expected_output = vector.translate(table)
        if (
            hashlib.sha256(vector).hexdigest()
            != manifest.expected_result["input_sha256"][index]
            or hashlib.sha256(expected_output).hexdigest()
            != manifest.expected_result["output_sha256"][index]
        ):
            raise ResultError(
                f"qualification vector/oracle hash differs at {index}"
            )
    return vectors


def collect_snapshot(
    transport: Any, manifest: ExecutionManifest
) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in EXPECTED_STAGED_MODES
    }
    return snapshot


def _competing_executor(
    manifest: ExecutionManifest, snapshot: Mapping[str, Any]
) -> bool:
    basename = manifest.remote_executable.rsplit("/", 1)[-1]
    return any(
        manifest.remote_executable in line or basename in line
        for line in snapshot.get("processes", [])
    )


def verify_preflight(
    manifest: ExecutionManifest, snapshot: Mapping[str, Any]
) -> None:
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
    metadata = snapshot.get("galcore_device") or ""
    if re.search(r"\b199,\s*0\b", metadata) is None:
        raise PreflightError(
            "/dev/galcore is not character device 199:0"
        )
    if snapshot.get("galcore_holders"):
        raise PreflightError("/dev/galcore is already held")
    if _competing_executor(manifest, snapshot):
        raise PreflightError("the fixed PoC 13D executor is already present")
    for path, expected in manifest.required_artifacts.items():
        if snapshot.get("artifact_hashes", {}).get(path) != expected:
            raise PreflightError(f"artifact mismatch: {path}")
    for path, expected_mode in EXPECTED_STAGED_MODES.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != expected_mode
        ):
            raise PreflightError(
                f"staged artifact metadata mismatch: {path}"
            )
        expected_bytes = EXPECTED_STAGED_BYTES.get(path)
        if (
            expected_bytes is not None
            and state.get("bytes") != expected_bytes
        ):
            raise PreflightError(
                f"staged artifact byte count mismatch: {path}"
            )
    for path in manifest.absent_artifacts:
        state = snapshot.get("absent_artifact_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise PreflightError(f"forbidden CLC artifact visible: {path}")
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")


def compare_health(
    manifest: ExecutionManifest,
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> tuple[list[str], list[str]]:
    differences, alerts = npu_compile_lab.compare_health(
        manifest, before, after
    )
    if (
        before.get("staged_artifact_states")
        != after.get("staged_artifact_states")
    ):
        differences.append("staged artifact metadata changed")
    if _competing_executor(manifest, after):
        differences.append("the fixed PoC 13D executor remains")
    return differences, alerts


def build_remote_command(
    manifest: ExecutionManifest, sentinel: str
) -> str:
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
        "wave_erf_mask_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_erf_mask_rc\"\n"
        "exit \"$wave_erf_mask_rc\"\n"
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$",
        re.MULTILINE,
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultError(
            f"expected one remote-exit sentinel, observed {len(matches)}"
        )
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise ResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def parse_result(
    stdout: str,
    manifest: ExecutionManifest,
    vectors: tuple[bytes, ...],
) -> dict[str, Any]:
    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    vector_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("VECTOR ")
    ]
    summary_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("ERF_MASK ")
    ]
    if (
        len(identity_lines) != 1
        or len(vector_lines) != VECTOR_COUNT
        or summary_lines != [SUMMARY_LINE]
    ):
        raise ResultError(
            "expected exactly one identity, six vectors, and one summary"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(
        identity_lines[0]
    )
    if identity_match is None:
        raise ResultError("identity line is malformed")
    identity = identity_match.groupdict()
    if identity != manifest.expected_result["identity"]:
        raise ResultError("live identity differs from fixed policy")

    table = build_independent_erf_table()
    records: list[dict[str, Any]] = []
    durations: list[int] = []
    for expected_index, line in enumerate(vector_lines):
        match = VECTOR_RE.fullmatch(line)
        if match is None:
            raise ResultError(
                f"vector line {expected_index} is malformed"
            )
        fields = match.groupdict()
        if (
            int(fields["index"]) != expected_index
            or fields["id"] != VECTOR_IDS[expected_index]
        ):
            raise ResultError("vector order or ID differs")
        input_bytes = vectors[expected_index]
        expected_output = input_bytes.translate(table)
        observed_output = bytes.fromhex(fields["output_hex"])
        input_fnv = f"0x{fnv1a64(input_bytes):016x}"
        output_fnv = f"0x{fnv1a64(observed_output):016x}"
        if fields["input_fnv"] != input_fnv:
            raise ResultError(
                f"native input checksum differs at {expected_index}"
            )
        if fields["output_fnv"] != output_fnv:
            raise ResultError(
                f"native output checksum differs at {expected_index}"
            )
        if observed_output != expected_output:
            mismatch_count = sum(
                left != right
                for left, right in zip(
                    observed_output, expected_output, strict=True
                )
            )
            raise ResultError(
                f"NPU/CPU byte mismatch at vector {expected_index}: "
                f"{mismatch_count} of {VECTOR_WIDTH}"
            )
        output_sha256 = hashlib.sha256(observed_output).hexdigest()
        if (
            output_sha256
            != manifest.expected_result["output_sha256"][expected_index]
        ):
            raise ResultError(
                f"output hash differs at vector {expected_index}"
            )
        duration_us = int(fields["duration_us"])
        durations.append(duration_us)
        records.append(
            {
                "index": expected_index,
                "id": VECTOR_IDS[expected_index],
                "duration_us": duration_us,
                "input_fnv1a64": input_fnv,
                "output_fnv1a64": output_fnv,
                "input_sha256": hashlib.sha256(input_bytes).hexdigest(),
                "output_sha256": output_sha256,
                "output_unique": len(set(observed_output)),
                "mismatched_bytes": 0,
                "max_abs_u8_error": 0,
            }
        )

    ramp = records[3]
    ramp_output = vectors[3].translate(table)
    if (
        list(ramp_output[:256]) != sorted(ramp_output[:256])
        or list(ramp_output[256:]) != sorted(ramp_output[256:])
    ):
        raise ResultError("repeated-ramp output is not monotonic per ramp")
    learned_output = vectors[5].translate(table)
    if len(set(learned_output)) < 2 or learned_output == vectors[5]:
        raise ResultError("learned-fixture output is degenerate")
    if (
        records[0]["output_unique"] != 1
        or records[1]["output_unique"] != 1
        or records[2]["output_unique"] != 1
        or ramp["output_unique"] < 2
    ):
        raise ResultError("boundary/ramp qualification is degenerate")

    return {
        "identity": identity,
        "mode": "qualify-six",
        "vector_count": VECTOR_COUNT,
        "width": VECTOR_WIDTH,
        "compile_count": 1,
        "run_count": VECTOR_COUNT,
        "canaries": "PASS",
        "cpu_oracle": {
            "implementation": "python-math.erf-independent-lut",
            "table_sha256": ERF_TABLE_SHA256,
        },
        "numeric_match": {
            "passed": True,
            "compared_bytes": VECTOR_COUNT * VECTOR_WIDTH,
            "mismatched_bytes": 0,
            "max_abs_u8_error": 0,
        },
        "latency_us": {
            "minimum": min(durations),
            "median": statistics.median(durations),
            "maximum": max(durations),
            "per_vector": durations,
        },
        "vectors": records,
    }


def run_once(
    transport: Any,
    manifest: ExecutionManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise ErfMaskExecutionError(
            f"run ID must match {RUN_ID_RE.pattern}"
        )
    vectors = load_qualification_vectors(manifest)
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc13d-erf-mask-six-vector-execution",
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
        "command": {
            "remote_executable": manifest.remote_executable,
            "argument": manifest.remote_argument,
            "timeout_ms": manifest.timeout_ms,
        },
        "result": None,
    }

    preflight = collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(
        run_directory / "preflight.json", preflight
    )
    try:
        verify_preflight(manifest, preflight)
    except PreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = npu_compile_lab._timestamp()
        npu_compile_lab._write_json_exclusive(
            run_directory / "run.json", envelope
        )
        return envelope, run_directory

    sentinel = f"__WAVE_ERF_MASK_EXIT_{uuid.uuid4().hex}__"
    remote_command = build_remote_command(manifest, sentinel)
    envelope["execution_attempts"] = 1
    started = time.monotonic()
    command_result = transport.run(
        ("shell", remote_command), manifest.timeout_ms
    )
    duration_ms = int((time.monotonic() - started) * 1000)
    npu_compile_lab._write_text_exclusive(
        run_directory / "stdout.txt", command_result.stdout
    )
    npu_compile_lab._write_text_exclusive(
        run_directory / "stderr.txt", command_result.stderr
    )

    postflight = collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(
        run_directory / "postflight.json", postflight
    )
    differences, alerts = compare_health(
        manifest, preflight, postflight
    )
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
            cleaned_stdout, remote_exit = parse_remote_exit(
                command_result.stdout, sentinel
            )
            envelope["transport"]["remote_exit"] = remote_exit
            try:
                npu_compile_lab.verify_transport_status(
                    command_result.returncode,
                    remote_exit,
                    "native six-vector executor",
                )
            except npu_compile_lab.ResultError as exc:
                raise ResultError(str(exc)) from exc
            envelope["result"] = parse_result(
                cleaned_stdout, manifest, vectors
            )
        except ResultError as exc:
            envelope["errors"].append(str(exc))
    if differences:
        envelope["errors"].append("device health gate failed")

    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = npu_compile_lab._timestamp()
    npu_compile_lab._write_json_exclusive(
        run_directory / "run.json", envelope
    )
    return envelope, run_directory


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Fixed Wave NPU PoC 13D six-vector numerical Erf-mask "
            "qualification; no arbitrary inputs, retry, or recovery"
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
            vectors = load_qualification_vectors(manifest)
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "manifest_file_sha256": (
                            EXPECTED_MANIFEST_FILE_SHA256
                        ),
                        "vector_pack_sha256": _file_sha256(
                            LOCAL_VECTOR_PACK
                        ),
                        "vector_count": len(vectors),
                        "width": VECTOR_WIDTH,
                        "mode": "qualify-six",
                        "compile_count": 1,
                        "run_count": 6,
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
                        "execution_attempts": envelope[
                            "execution_attempts"
                        ],
                        "errors": envelope["errors"],
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0 if envelope["status"] == "PASS" else 1
        raise AssertionError(args.command)
    except (ErfMaskExecutionError, npu_lab.NpuLabError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
