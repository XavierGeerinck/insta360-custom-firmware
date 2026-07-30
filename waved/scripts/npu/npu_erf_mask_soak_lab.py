#!/usr/bin/env python3
"""Exactly-once 10,000-frame numerical and health soak for the Wave Erf NBG."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shlex
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
DEFAULT_MANIFEST = Path(__file__).with_name("erf_mask_soak_manifest.json")
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"
EXPECTED_MANIFEST_FILE_SHA256 = (
    "ed64868e2c28b057066513c4b1f390d3"
    "686f2fd16c47f47fa9a3b469bb6acd74"
)
REMOTE_ROOT = "/tmp/wave-npu-poc13f"
REMOTE_EXECUTABLE = f"{REMOTE_ROOT}/bin/wave-npu-soak-erf-mask-v1"
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_NBG = f"{REMOTE_ROOT}/model/erf-mask.nbg"
REMOTE_ARGUMENT = "--soak-10000"
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
EXPECTED_IDENTITY = stream_lab.EXPECTED_IDENTITY
EXPECTED_ARTIFACTS = {
    REMOTE_EXECUTABLE: (
        "ef0d4ece1d44141b1254581aa5b05e5f"
        "d371f99f819cfac32b478cbcaaaf4cff"
    ),
    REMOTE_TIM_VX: stream_lab.EXPECTED_ARTIFACTS[
        stream_lab.REMOTE_TIM_VX
    ],
    REMOTE_NBG: stream_lab.EXPECTED_ARTIFACTS[
        stream_lab.REMOTE_NBG
    ],
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
}
SOAK_FRAMES = 10000
CHECKPOINT_FRAMES = 1000
FRAME_WIDTH = 481
INPUT_SEED = 0x5733130F
FNV64_OFFSET_BASIS = 0xCBF29CE484222325
FNV64_PRIME = 0x100000001B3

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_ERF_SOAK_EXIT_[0-9a-f]{32}__$")
CHECKPOINT_RE = re.compile(
    r"^CHECKPOINT frames=(?P<frames>[0-9]+) "
    r"input_fnv1a64=(?P<input>0x[0-9a-f]{16}) "
    r"output_fnv1a64=(?P<output>0x[0-9a-f]{16})$"
)
SUMMARY_RE = re.compile(
    r"^ERF_SOAK status=PASS mode=soak-10000 frames=10000 "
    r"bytes_per_frame=481 compile_count=1 run_count=10000 "
    r"checkpoints=10 canaries=PASS "
    r"total_run_us=(?P<total>[0-9]+) "
    r"min_run_us=(?P<minimum>[0-9]+) "
    r"max_run_us=(?P<maximum>[0-9]+) "
    r"input_fnv1a64=(?P<input>0x[0-9a-f]{16}) "
    r"output_fnv1a64=(?P<output>0x[0-9a-f]{16})$"
)


class SoakError(Exception):
    """Expected PoC 13F failure."""


class ManifestError(SoakError):
    """The soak manifest differs from fixed policy."""


class PreflightError(SoakError):
    """The device baseline differs."""


class ResultError(SoakError):
    """The soak output is malformed or numerically incorrect."""


@dataclass(frozen=True)
class ExecutionPolicy:
    absent_artifacts: tuple[str, ...]


@dataclass(frozen=True)
class SoakManifest:
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


def _duplicate_safe(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_manifest(path: Path = DEFAULT_MANIFEST) -> SoakManifest:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise ManifestError(f"cannot read manifest: {exc}") from exc
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
    if not isinstance(document, dict) or set(document) != {
        "schema_version",
        "device",
        "environment",
        "remote_executable",
        "remote_argument",
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
    if expected != {
        "identity": EXPECTED_IDENTITY,
        "frame_count": SOAK_FRAMES,
        "width": FRAME_WIDTH,
        "seed": f"0x{INPUT_SEED:08x}",
        "checkpoint_frames": CHECKPOINT_FRAMES,
        "checkpoint_count": SOAK_FRAMES // CHECKPOINT_FRAMES,
        "compile_count": 1,
        "run_count": SOAK_FRAMES,
    }:
        raise ManifestError("expected soak result differs")
    absent_tuple = tuple(sorted(absent))
    canonical = json.dumps(
        document,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode()
    return SoakManifest(
        serial=device["serial"],
        kernel=device["kernel"],
        galcore_major_minor=device["galcore_major_minor"],
        npu_parameters=device["npu_parameters"],
        environment=document["environment"],
        remote_executable=document["remote_executable"],
        remote_argument=document["remote_argument"],
        timeout_ms=document["timeout_ms"],
        required_artifacts=artifacts,
        absent_artifacts=absent_tuple,
        expected_result=expected,
        sha256=hashlib.sha256(canonical).hexdigest(),
        policy=ExecutionPolicy(absent_tuple),
        candidates={},
    )


def _update_fnv(value: int, payload: bytes) -> int:
    for byte in payload:
        value ^= byte
        value = (value * FNV64_PRIME) & 0xFFFFFFFFFFFFFFFF
    return value


def expected_checkpoints() -> tuple[dict[str, Any], ...]:
    table = stream_lab.build_independent_erf_table()
    state = INPUT_SEED
    input_fnv = FNV64_OFFSET_BASIS
    output_fnv = FNV64_OFFSET_BASIS
    checkpoints: list[dict[str, Any]] = []
    for frame_index in range(SOAK_FRAMES):
        frame = bytearray(FRAME_WIDTH)
        for index in range(FRAME_WIDTH):
            state ^= (state << 13) & 0xFFFFFFFF
            state ^= state >> 17
            state ^= (state << 5) & 0xFFFFFFFF
            state &= 0xFFFFFFFF
            frame[index] = state & 0xFF
        input_payload = bytes(frame)
        output_payload = input_payload.translate(table)
        input_fnv = _update_fnv(input_fnv, input_payload)
        output_fnv = _update_fnv(output_fnv, output_payload)
        frames = frame_index + 1
        if frames % CHECKPOINT_FRAMES == 0:
            checkpoints.append(
                {
                    "frames": frames,
                    "input_fnv1a64": f"0x{input_fnv:016x}",
                    "output_fnv1a64": f"0x{output_fnv:016x}",
                }
            )
    return tuple(checkpoints)


def collect_snapshot(
    transport: Any, manifest: SoakManifest
) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in EXPECTED_STAGED_MODES
    }
    return snapshot


def _competing_executor(
    manifest: SoakManifest, snapshot: Mapping[str, Any]
) -> bool:
    basename = manifest.remote_executable.rsplit("/", 1)[-1]
    return any(
        manifest.remote_executable in line or basename in line
        for line in snapshot.get("processes", [])
    )


def verify_preflight(
    manifest: SoakManifest, snapshot: Mapping[str, Any]
) -> None:
    if snapshot.get("errors"):
        raise PreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise PreflightError("authorized device is not exclusively connected")
    if snapshot.get("kernel") != manifest.kernel:
        raise PreflightError("kernel identity mismatch")
    if (
        not isinstance(snapshot.get("boot_id"), str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(snapshot["boot_id"])
        is None
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
        raise PreflightError("the fixed soak executor is already present")
    for path, expected in manifest.required_artifacts.items():
        if snapshot.get("artifact_hashes", {}).get(path) != expected:
            raise PreflightError(f"artifact mismatch: {path}")
    for path, expected_mode in EXPECTED_STAGED_MODES.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != expected_mode
            or (path == REMOTE_NBG and state.get("bytes") != 5840)
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
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")


def compare_health(
    manifest: SoakManifest,
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
        differences.append("the fixed soak executor remains")
    return differences, alerts


def build_remote_command(
    manifest: SoakManifest, sentinel: str
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
        "wave_erf_soak_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_erf_soak_rc\"\n"
        "exit \"$wave_erf_soak_rc\"\n"
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
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def parse_result(
    stdout: str,
    manifest: SoakManifest,
    expected: tuple[dict[str, Any], ...],
) -> dict[str, Any]:
    identities = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    checkpoint_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("CHECKPOINT ")
    ]
    summaries = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("ERF_SOAK ")
    ]
    if (
        len(identities) != 1
        or len(checkpoint_lines) != 10
        or len(summaries) != 1
    ):
        raise ResultError(
            "expected one identity, ten checkpoints, and one summary"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(identities[0])
    if (
        identity_match is None
        or identity_match.groupdict()
        != manifest.expected_result["identity"]
    ):
        raise ResultError("live identity differs")
    observed_checkpoints: list[dict[str, Any]] = []
    for line in checkpoint_lines:
        match = CHECKPOINT_RE.fullmatch(line)
        if match is None:
            raise ResultError("checkpoint line is malformed")
        fields = match.groupdict()
        observed_checkpoints.append(
            {
                "frames": int(fields["frames"]),
                "input_fnv1a64": fields["input"],
                "output_fnv1a64": fields["output"],
            }
        )
    if tuple(observed_checkpoints) != expected:
        raise ResultError("NPU soak checkpoint digest mismatch")
    summary_match = SUMMARY_RE.fullmatch(summaries[0])
    if summary_match is None:
        raise ResultError("soak summary is malformed")
    summary = summary_match.groupdict()
    if (
        summary["input"] != expected[-1]["input_fnv1a64"]
        or summary["output"] != expected[-1]["output_fnv1a64"]
    ):
        raise ResultError("soak final digest differs")
    total = int(summary["total"])
    minimum = int(summary["minimum"])
    maximum = int(summary["maximum"])
    if minimum > maximum or total < minimum * SOAK_FRAMES:
        raise ResultError("soak timing summary is inconsistent")
    return {
        "identity": identity_match.groupdict(),
        "frames": SOAK_FRAMES,
        "compared_bytes": SOAK_FRAMES * FRAME_WIDTH,
        "checkpoints": observed_checkpoints,
        "numeric_digest_match": True,
        "canaries": "PASS",
        "latency_us": {
            "total": total,
            "mean": total / SOAK_FRAMES,
            "minimum": minimum,
            "maximum": maximum,
            "audio_hop_budget": 10000,
            "graph_run_realtime_fraction": (
                total / (SOAK_FRAMES * 10000)
            ),
        },
    }


def run_once(
    transport: Any,
    manifest: SoakManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise SoakError(f"run ID must match {RUN_ID_RE.pattern}")
    expected = expected_checkpoints()
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc13f-erf-mask-10k-soak",
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

    sentinel = f"__WAVE_ERF_SOAK_EXIT_{uuid.uuid4().hex}__"
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
                    "native 10k soak executor",
                )
            except npu_compile_lab.ResultError as exc:
                raise ResultError(str(exc)) from exc
            envelope["result"] = parse_result(
                cleaned, manifest, expected
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
            "Fixed Wave NPU PoC 13F 10k-frame Erf soak; "
            "no arbitrary seed, inputs, retry, benchmark, or recovery"
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
            expected = expected_checkpoints()
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "manifest_file_sha256": (
                            EXPECTED_MANIFEST_FILE_SHA256
                        ),
                        "mode": "soak-10000",
                        "frames": SOAK_FRAMES,
                        "width": FRAME_WIDTH,
                        "final_input_fnv1a64": (
                            expected[-1]["input_fnv1a64"]
                        ),
                        "final_output_fnv1a64": (
                            expected[-1]["output_fnv1a64"]
                        ),
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
    except (SoakError, npu_lab.NpuLabError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
