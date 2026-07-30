#!/usr/bin/env python3
"""Exactly-once full-fixture numerical execution of the sealed Erf NBG."""

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
import npu_lab


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name(
    "erf_mask_stream_execution_manifest.json"
)
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"
LOCAL_INPUT = (
    REPOSITORY_ROOT
    / "artifacts/npu/runs/poc13e-erf-mask-device-stream-20260729"
    / "erf-stream-input.bin"
)
EXPECTED_MANIFEST_FILE_SHA256 = (
    "9d030f1c5fe2dd17eb21ee7c3e8f2af1"
    "bf3332b24986d91cfa73074030c141c4"
)
REMOTE_ROOT = "/tmp/wave-npu-poc13e"
REMOTE_EXECUTABLE = (
    f"{REMOTE_ROOT}/bin/wave-npu-run-erf-mask-stream-v1"
)
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_NBG = f"{REMOTE_ROOT}/model/erf-mask.nbg"
REMOTE_INPUT = f"{REMOTE_ROOT}/input/erf-stream-input.bin"
REMOTE_OUTPUT = f"{REMOTE_ROOT}/output/erf-stream-output.bin"
REMOTE_PARTIAL_OUTPUT = (
    f"{REMOTE_ROOT}/output/.erf-stream-output.bin.partial"
)
REMOTE_ARGUMENT = "--fixture-stream"
EXPECTED_TIMEOUT_MS = 40000
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
        "3f4be43ee71e8340fbab7aba375a67fc6"
        "855bb0beff5c8a810117a5589a67b38"
    ),
    REMOTE_TIM_VX: (
        "d2c8fc7aaf0ad0b86f02fcd54bb1cdca"
        "9f04967b5cd9efa61a211565b453375b"
    ),
    REMOTE_NBG: (
        "96447d6afd63a7c349bbd80f0af99150"
        "c89ef24ed7f1ef66ab285075362e6c94"
    ),
    REMOTE_INPUT: (
        "a409686c2482b576288b6f46ecf77d3b"
        "99b019c9c436e0f042cb1ac39dacc3e9"
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
    REMOTE_INPUT: "-r--r--r--",
}
EXPECTED_STAGED_BYTES = {
    REMOTE_NBG: 5840,
    REMOTE_INPUT: 577697,
}
FRAME_COUNT = 1201
FRAME_WIDTH = 481
STREAM_HEADER = struct.Struct("<4sIII")
STREAM_BYTES = STREAM_HEADER.size + FRAME_COUNT * FRAME_WIDTH
ERF_TABLE_SHA256 = (
    "c9525963e022d4a4aea0814845119b374"
    "88f3727ec8d96684f52e1e2ad776296"
)
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_ERF_STREAM_EXIT_[0-9a-f]{32}__$")
SUMMARY_RE = re.compile(
    r"^ERF_STREAM status=PASS mode=fixture-stream frames=1201 "
    r"bytes_per_frame=481 compile_count=1 run_count=1201 "
    r"canaries=PASS total_run_us=(?P<total>[0-9]+) "
    r"min_run_us=(?P<minimum>[0-9]+) "
    r"max_run_us=(?P<maximum>[0-9]+) output_bytes=577697 "
    r"output_fnv1a64=(?P<fnv>0x[0-9a-f]{16}) "
    r"output=/tmp/wave-npu-poc13e/output/erf-stream-output\.bin$"
)


class StreamExecutionError(Exception):
    """Expected PoC 13E harness failure."""


class ManifestError(StreamExecutionError):
    """The fixed stream manifest differs."""


class PreflightError(StreamExecutionError):
    """The device baseline differs."""


class ResultError(StreamExecutionError):
    """The full-stream result is invalid."""


@dataclass(frozen=True)
class ExecutionPolicy:
    absent_artifacts: tuple[str, ...]


@dataclass(frozen=True)
class StreamManifest:
    serial: str
    kernel: str
    galcore_major_minor: str
    npu_parameters: Mapping[str, str]
    environment: Mapping[str, str]
    remote_executable: str
    remote_argument: str
    remote_output: str
    remote_partial_output: str
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    expected_result: Mapping[str, Any]
    sha256: str
    policy: ExecutionPolicy
    candidates: Mapping[str, Any]


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


def load_manifest(path: Path = DEFAULT_MANIFEST) -> StreamManifest:
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
            raw.decode("utf-8"), object_pairs_hook=_duplicate_safe
        )
    except ManifestError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot parse manifest: {exc}") from exc
    expected_root_keys = {
        "schema_version",
        "device",
        "environment",
        "remote_executable",
        "remote_argument",
        "remote_output",
        "remote_partial_output",
        "timeout_ms",
        "required_artifacts",
        "absent_artifacts",
        "expected_result",
    }
    if not isinstance(document, dict) or set(document) != expected_root_keys:
        raise ManifestError("manifest root keys differ")
    device = document["device"]
    if (
        document["schema_version"] != 1
        or not isinstance(device, dict)
        or set(device)
        != {"serial", "kernel", "galcore_major_minor", "npu_parameters"}
        or device["serial"] != "PSP_0000000000000000"
        or device["kernel"] != EXPECTED_KERNEL
        or device["galcore_major_minor"] != "199:0"
        or device["npu_parameters"] != EXPECTED_PARAMETERS
    ):
        raise ManifestError("device identity differs from fixed policy")
    if document["environment"] != EXPECTED_ENVIRONMENT:
        raise ManifestError("environment differs from fixed policy")
    if (
        document["remote_executable"] != REMOTE_EXECUTABLE
        or document["remote_argument"] != REMOTE_ARGUMENT
        or document["remote_output"] != REMOTE_OUTPUT
        or document["remote_partial_output"] != REMOTE_PARTIAL_OUTPUT
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
        if (
            artifact_path.startswith("/tmp/")
            and not artifact_path.startswith(f"{REMOTE_ROOT}/")
        ):
            raise ManifestError("temporary artifact escapes fixed root")
    for artifact_path, digest in EXPECTED_ARTIFACTS.items():
        if artifacts.get(artifact_path) != digest:
            raise ManifestError(f"fixed artifact mismatch: {artifact_path}")

    absent = document["absent_artifacts"]
    if (
        not isinstance(absent, list)
        or tuple(sorted(absent))
        != tuple(sorted(EXPECTED_ABSENT_ARTIFACTS))
    ):
        raise ManifestError("absent_artifacts differs from CLC policy")
    expected = document["expected_result"]
    if (
        not isinstance(expected, dict)
        or set(expected)
        != {
            "identity",
            "input_sha256",
            "input_fnv1a64",
            "frame_count",
            "width",
            "stream_bytes",
            "compile_count",
            "run_count",
        }
        or expected["identity"] != EXPECTED_IDENTITY
        or expected["input_sha256"] != EXPECTED_ARTIFACTS[REMOTE_INPUT]
        or expected["input_fnv1a64"] != "0xe2c6f1eda5f3eaea"
        or expected["frame_count"] != FRAME_COUNT
        or expected["width"] != FRAME_WIDTH
        or expected["stream_bytes"] != STREAM_BYTES
        or expected["compile_count"] != 1
        or expected["run_count"] != FRAME_COUNT
    ):
        raise ManifestError("expected stream result differs")
    absent_tuple = tuple(sorted(absent))
    return StreamManifest(
        serial=device["serial"],
        kernel=device["kernel"],
        galcore_major_minor=device["galcore_major_minor"],
        npu_parameters=device["npu_parameters"],
        environment=document["environment"],
        remote_executable=document["remote_executable"],
        remote_argument=document["remote_argument"],
        remote_output=document["remote_output"],
        remote_partial_output=document["remote_partial_output"],
        timeout_ms=document["timeout_ms"],
        required_artifacts=artifacts,
        absent_artifacts=absent_tuple,
        expected_result=expected,
        sha256=_canonical_sha256(document),
        policy=ExecutionPolicy(absent_tuple),
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


def collect_snapshot(
    transport: Any, manifest: StreamManifest
) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in EXPECTED_STAGED_MODES
    }
    snapshot["stream_output_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in (REMOTE_OUTPUT, REMOTE_PARTIAL_OUTPUT)
    }
    return snapshot


def _competing_executor(
    manifest: StreamManifest, snapshot: Mapping[str, Any]
) -> bool:
    basename = manifest.remote_executable.rsplit("/", 1)[-1]
    return any(
        manifest.remote_executable in line or basename in line
        for line in snapshot.get("processes", [])
    )


def verify_preflight(
    manifest: StreamManifest, snapshot: Mapping[str, Any]
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
    if re.search(
        r"\b199,\s*0\b", snapshot.get("galcore_device") or ""
    ) is None:
        raise PreflightError(
            "/dev/galcore is not character device 199:0"
        )
    if snapshot.get("galcore_holders"):
        raise PreflightError("/dev/galcore is already held")
    if _competing_executor(manifest, snapshot):
        raise PreflightError("the fixed stream executor is already present")
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
            raise PreflightError(
                f"staged artifact metadata mismatch: {path}"
            )
    for path in manifest.absent_artifacts:
        if (
            snapshot.get("absent_artifact_states", {})
            .get(path, {})
            .get("status")
            != "absent"
        ):
            raise PreflightError(f"forbidden CLC artifact visible: {path}")
    if any(
        state.get("status") != "absent"
        for state in snapshot.get("stream_output_states", {}).values()
    ):
        raise PreflightError("fixed stream output is not absent")
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")


def compare_health(
    manifest: StreamManifest,
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
        differences.append("the fixed stream executor remains")
    partial = after.get("stream_output_states", {}).get(
        REMOTE_PARTIAL_OUTPUT, {}
    )
    if partial.get("status") != "absent":
        differences.append("partial stream output remains")
    return differences, alerts


def build_remote_command(
    manifest: StreamManifest, sentinel: str
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
        "wave_erf_stream_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_erf_stream_rc\"\n"
        "exit \"$wave_erf_stream_rc\"\n"
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


def parse_native_result(
    stdout: str, manifest: StreamManifest
) -> dict[str, Any]:
    identities = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    summaries = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("ERF_STREAM ")
    ]
    if len(identities) != 1 or len(summaries) != 1:
        raise ResultError("expected one identity and stream summary")
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(identities[0])
    summary_match = SUMMARY_RE.fullmatch(summaries[0])
    if identity_match is None or summary_match is None:
        raise ResultError("identity or stream summary is malformed")
    identity = identity_match.groupdict()
    if identity != manifest.expected_result["identity"]:
        raise ResultError("live identity differs from fixed policy")
    timing = summary_match.groupdict()
    minimum = int(timing["minimum"])
    maximum = int(timing["maximum"])
    total = int(timing["total"])
    if minimum > maximum or total < minimum * FRAME_COUNT:
        raise ResultError("native timing summary is inconsistent")
    return {
        "identity": identity,
        "total_run_us": total,
        "minimum_run_us": minimum,
        "maximum_run_us": maximum,
        "output_fnv1a64": timing["fnv"],
    }


def verify_pulled_output(
    output_path: Path,
    native_result: Mapping[str, Any],
) -> dict[str, Any]:
    try:
        input_payload = LOCAL_INPUT.read_bytes()
        output_payload = output_path.read_bytes()
    except OSError as exc:
        raise ResultError(f"cannot read stream payload: {exc}") from exc
    if (
        len(input_payload) != STREAM_BYTES
        or hashlib.sha256(input_payload).hexdigest()
        != EXPECTED_ARTIFACTS[REMOTE_INPUT]
        or len(output_payload) != STREAM_BYTES
    ):
        raise ResultError("stream byte count or sealed input hash differs")
    input_header = STREAM_HEADER.unpack_from(input_payload)
    output_header = STREAM_HEADER.unpack_from(output_payload)
    if (
        input_header != (b"ERI1", 1, FRAME_COUNT, FRAME_WIDTH)
        or output_header != (b"ERO1", 1, FRAME_COUNT, FRAME_WIDTH)
    ):
        raise ResultError("input/output stream header differs")
    output_fnv = f"0x{fnv1a64(output_payload):016x}"
    if output_fnv != native_result["output_fnv1a64"]:
        raise ResultError("pulled output FNV differs from native result")

    table = build_independent_erf_table()
    input_frames = input_payload[STREAM_HEADER.size :]
    observed = output_payload[STREAM_HEADER.size :]
    expected = input_frames.translate(table)
    mismatches = sum(
        left != right
        for left, right in zip(observed, expected, strict=True)
    )
    if mismatches:
        raise ResultError(
            f"NPU/CPU full-stream mismatch: {mismatches} bytes"
        )
    total_run_us = int(native_result["total_run_us"])
    return {
        "path": output_path.relative_to(REPOSITORY_ROOT).as_posix(),
        "bytes": len(output_payload),
        "sha256": hashlib.sha256(output_payload).hexdigest(),
        "fnv1a64": output_fnv,
        "frame_count": FRAME_COUNT,
        "width": FRAME_WIDTH,
        "numeric_match": {
            "passed": True,
            "compared_bytes": len(observed),
            "mismatched_bytes": 0,
            "max_abs_u8_error": 0,
        },
        "output_unique": len(set(observed)),
        "latency_us": {
            "total": total_run_us,
            "mean": total_run_us / FRAME_COUNT,
            "minimum": native_result["minimum_run_us"],
            "maximum": native_result["maximum_run_us"],
            "audio_hop_budget": 10000,
            "graph_run_realtime_fraction": (
                total_run_us / (FRAME_COUNT * 10000)
            ),
        },
        "cpu_oracle": {
            "implementation": "python-math.erf-independent-lut",
            "table_sha256": ERF_TABLE_SHA256,
        },
    }


def run_once(
    transport: Any,
    manifest: StreamManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise StreamExecutionError(
            f"run ID must match {RUN_ID_RE.pattern}"
        )
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc13e-erf-mask-fixture-stream",
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

    sentinel = f"__WAVE_ERF_STREAM_EXIT_{uuid.uuid4().hex}__"
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
            cleaned, remote_exit = parse_remote_exit(
                command_result.stdout, sentinel
            )
            envelope["transport"]["remote_exit"] = remote_exit
            try:
                npu_compile_lab.verify_transport_status(
                    command_result.returncode,
                    remote_exit,
                    "native full-stream executor",
                )
            except npu_compile_lab.ResultError as exc:
                raise ResultError(str(exc)) from exc
            native = parse_native_result(cleaned, manifest)
            output_state = postflight["stream_output_states"][
                REMOTE_OUTPUT
            ]
            if (
                output_state.get("status") != "present"
                or output_state.get("permissions") != "-r--r--r--"
                or output_state.get("bytes") != STREAM_BYTES
            ):
                raise ResultError("remote fixed output metadata differs")
            local_output = run_directory / "erf-stream-output.bin"
            pull = transport.run(
                ("pull", REMOTE_OUTPUT, str(local_output)), 15000
            )
            npu_compile_lab._write_text_exclusive(
                run_directory / "pull.stdout.txt", pull.stdout
            )
            npu_compile_lab._write_text_exclusive(
                run_directory / "pull.stderr.txt", pull.stderr
            )
            if (
                pull.timed_out
                or pull.returncode != 0
                or not local_output.is_file()
            ):
                raise ResultError("ADB output pull failed")
            if (
                hashlib.sha256(local_output.read_bytes()).hexdigest()
                != output_state.get("sha256")
            ):
                raise ResultError("pulled/remote output hashes differ")
            envelope["result"] = {
                "native": native,
                "output": verify_pulled_output(local_output, native),
                "compile_count": 1,
                "run_count": FRAME_COUNT,
                "canaries": "PASS",
            }
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
            "Fixed Wave NPU PoC 13E full-fixture Erf stream; "
            "no arbitrary inputs, retry, benchmark, or recovery"
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
                        "manifest_file_sha256": (
                            EXPECTED_MANIFEST_FILE_SHA256
                        ),
                        "input_sha256": hashlib.sha256(
                            LOCAL_INPUT.read_bytes()
                        ).hexdigest(),
                        "mode": "fixture-stream",
                        "frame_count": FRAME_COUNT,
                        "width": FRAME_WIDTH,
                        "compile_count": 1,
                        "run_count": FRAME_COUNT,
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
    except (StreamExecutionError, npu_lab.NpuLabError, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
