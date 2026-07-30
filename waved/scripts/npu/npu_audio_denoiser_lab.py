#!/usr/bin/env python3
"""One-shot PoC 8B/8D harness for fixed audio-denoiser graphs."""

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
import npu_lab


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name(
    "audio_denoiser_execution_manifest.json"
)
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"

REMOTE_ROOT = "/tmp/wave-npu-audio8b"
REMOTE_EXECUTABLE = (
    f"{REMOTE_ROOT}/bin/wave-npu-run-audio-denoiser-v1"
)
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_ARGUMENT = "--execute-once"
EXPECTED_TIMEOUT_MS = 12000
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
FIXED_ARTIFACTS = {
    REMOTE_EXECUTABLE: (
        "7b0919b4576c5c4feb34bac71347a657"
        "65a111ad3d52ca5c2755bc266030adfc"
    ),
    REMOTE_TIM_VX: (
        "d2c8fc7aaf0ad0b86f02fcd54bb1cdca"
        "9f04967b5cd9efa61a211565b453375b"
    ),
    "/lib/libc.so.6": (
        "cfc6aa042a880a87e4f0adbfb74b55c9b"
        "a9607561905b20f58da96fcb2d3bb86"
    ),
    "/lib64/ld-2.31.so": (
        "912002c930f3096d98fc32e0897466049"
        "d6e5182cbb606b456b2ebcc99551366"
    ),
    "/usr/lib/libGAL.so": (
        "13b7d87eabb8924758d49fbd8f1fac41"
        "bb81f34069f6ceed949af764893061e6"
    ),
    "/usr/lib/libOpenVX.so": (
        "f9cd917d4232316ab53a443ec93cd1217"
        "893f4de96fc113811b800b9ae8ca0bb"
    ),
    "/usr/lib/libovxlib.so": (
        "09e740b4d4b0a759fb7f274ebf91456b"
        "dd6b702c96e47ab04a8a1655ffc65ba9"
    ),
}
EXPECTED_ABSENT = (
    "/lib/libCLC.so",
    f"{REMOTE_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
EXPECTED_IDENTITY = {
    "chip_model": "0x00009000",
    "chip_revision": "0x00009002",
    "chip_product_id": "0x05090001",
    "chip_eco_id": "0x04000000",
    "customer_id": "0x1000000e",
    "reserved": "0x00000000",
    "flags": "0x0000000000000000",
}
EXPECTED_RESULT = {
    "backend": "OPENVX",
    "kernel": "matrixmul",
    "input_energy": "16.000000",
    "output_energy": "0.125000",
    "max_error": "0.000000",
    "iterations": 1,
}
EXPECTED_STAGED_MODES = {
    REMOTE_EXECUTABLE: "-r-xr-xr-x",
    REMOTE_TIM_VX: "-r--r--r--",
}

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_AUDIO_EXIT_[0-9a-f]{32}__$")
AUDIO_RE = re.compile(
    r"^AUDIO status=PASS mode=direct-graph iterations=1 "
    r"input_energy=16\.000000 output_energy=0\.125000 "
    r"max_error=0\.000000$"
)


class AudioLabError(Exception):
    """Expected fixed audio-denoiser harness failure."""


class AudioManifestError(AudioLabError):
    """The source-controlled manifest violates fixed policy."""


class AudioPreflightError(AudioLabError):
    """The live device differs from the reviewed starting state."""


class AudioResultError(AudioLabError):
    """The fixed graph result is missing or malformed."""


@dataclass(frozen=True)
class AudioExecutionPolicy:
    poc_id: str
    experiment: str
    operation: str
    manifest_path: Path
    remote_root: str
    remote_executable: str
    remote_tim_vx: str
    fixed_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    staged_modes: Mapping[str, str]
    expected_result: Mapping[str, Any]
    result_mode: str
    result_re: re.Pattern[str]


POC8B_POLICY = AudioExecutionPolicy(
    poc_id="8B",
    experiment="wave-npu-poc8b-audio-denoiser-direct-execution",
    operation="Matmul",
    manifest_path=DEFAULT_MANIFEST,
    remote_root=REMOTE_ROOT,
    remote_executable=REMOTE_EXECUTABLE,
    remote_tim_vx=REMOTE_TIM_VX,
    fixed_artifacts=FIXED_ARTIFACTS,
    absent_artifacts=EXPECTED_ABSENT,
    staged_modes=EXPECTED_STAGED_MODES,
    expected_result=EXPECTED_RESULT,
    result_mode="direct-graph",
    result_re=AUDIO_RE,
)

POC8D_ROOT = "/tmp/wave-npu-audio8d"
POC8D_EXECUTABLE = (
    f"{POC8D_ROOT}/bin/wave-npu-run-conv-audio-denoiser-v1"
)
POC8D_TIM_VX = f"{POC8D_ROOT}/lib/libtim-vx.so"
POC8D_FIXED_ARTIFACTS = {
    **{
        path: digest
        for path, digest in FIXED_ARTIFACTS.items()
        if not path.startswith(f"{REMOTE_ROOT}/")
    },
    POC8D_EXECUTABLE: (
        "bf3e4716588055ed7c774135ed15a77a"
        "d068fddab52b362c0c0c03dfb05f2c9f"
    ),
    POC8D_TIM_VX: FIXED_ARTIFACTS[REMOTE_TIM_VX],
}
POC8D_ABSENT = (
    "/lib/libCLC.so",
    f"{POC8D_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
POC8D_STAGED_MODES = {
    POC8D_EXECUTABLE: "-r-xr-xr-x",
    POC8D_TIM_VX: "-r--r--r--",
}
POC8D_EXPECTED_RESULT = {
    **EXPECTED_RESULT,
    "kernel": "conv2d",
}
POC8D_AUDIO_RE = re.compile(
    r"^AUDIO status=PASS mode=conv-direct iterations=1 "
    r"input_energy=16\.000000 output_energy=0\.125000 "
    r"max_error=0\.000000$"
)
POC8D_POLICY = AudioExecutionPolicy(
    poc_id="8D",
    experiment="wave-npu-poc8d-convolutional-audio-denoiser-execution",
    operation="Conv2d",
    manifest_path=Path(__file__).with_name(
        "conv_audio_denoiser_execution_manifest.json"
    ),
    remote_root=POC8D_ROOT,
    remote_executable=POC8D_EXECUTABLE,
    remote_tim_vx=POC8D_TIM_VX,
    fixed_artifacts=POC8D_FIXED_ARTIFACTS,
    absent_artifacts=POC8D_ABSENT,
    staged_modes=POC8D_STAGED_MODES,
    expected_result=POC8D_EXPECTED_RESULT,
    result_mode="conv-direct",
    result_re=POC8D_AUDIO_RE,
)


@dataclass(frozen=True)
class AudioManifest:
    policy: AudioExecutionPolicy
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
    candidates: Mapping[str, Any]


def _without_duplicate_keys(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise AudioManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _keys(
    value: Any, expected: set[str], context: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AudioManifestError(f"{context} must be an object")
    missing = expected - set(value)
    unknown = set(value) - expected
    if missing or unknown:
        raise AudioManifestError(
            f"{context} key mismatch; missing={sorted(missing)}, "
            f"unknown={sorted(unknown)}"
        )
    return value


def _plain_string(value: Any, context: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or re.search(r"[\x00-\x1f\x7f]", value)
    ):
        raise AudioManifestError(
            f"{context} must be a nonempty plain string"
        )
    return value


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def load_manifest(
    path: Path | None = None,
    policy: AudioExecutionPolicy = POC8B_POLICY,
) -> AudioManifest:
    if path is None:
        path = policy.manifest_path
    try:
        document = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_without_duplicate_keys,
        )
    except AudioManifestError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AudioManifestError(f"cannot load {path}: {exc}") from exc

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
        raise AudioManifestError("unsupported schema_version")

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
    parameters = {
        key: _plain_string(value, f"device.npu_parameters.{key}")
        for key, value in parameters.items()
    }
    serial = _plain_string(device["serial"], "device.serial")
    kernel = _plain_string(device["kernel"], "device.kernel")
    galcore = _plain_string(
        device["galcore_major_minor"], "device.galcore_major_minor"
    )
    if (
        serial != "PSP_0000000000000000"
        or kernel != EXPECTED_KERNEL
        or galcore != "199:0"
        or parameters != EXPECTED_PARAMETERS
    ):
        raise AudioManifestError(
            f"device identity differs from fixed PoC {policy.poc_id} "
            "policy"
        )

    environment = _keys(
        root["environment"],
        {"LD_LIBRARY_PATH", "VIVANTE_SDK_DIR", "VSI_NN_LOG_LEVEL"},
        "environment",
    )
    environment = {
        key: _plain_string(value, f"environment.{key}")
        for key, value in environment.items()
    }
    expected_environment = {
        **EXPECTED_ENVIRONMENT,
        "LD_LIBRARY_PATH": f"{policy.remote_root}/lib:/usr/lib",
    }
    if environment != expected_environment:
        raise AudioManifestError(
            f"environment differs from fixed PoC {policy.poc_id} policy"
        )
    executable = _plain_string(
        root["remote_executable"], "remote_executable"
    )
    argument = _plain_string(root["remote_argument"], "remote_argument")
    if (
        executable != policy.remote_executable
        or argument != REMOTE_ARGUMENT
    ):
        raise AudioManifestError(
            f"remote invocation differs from fixed PoC {policy.poc_id} "
            "policy"
        )
    timeout_ms = root["timeout_ms"]
    if (
        isinstance(timeout_ms, bool)
        or not isinstance(timeout_ms, int)
        or timeout_ms != EXPECTED_TIMEOUT_MS
    ):
        raise AudioManifestError(
            f"timeout differs from fixed PoC {policy.poc_id} policy"
        )

    raw_artifacts = root["required_artifacts"]
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise AudioManifestError(
            "required_artifacts must be a nonempty object"
        )
    artifacts: dict[str, str] = {}
    for raw_path, raw_digest in raw_artifacts.items():
        artifact_path = _plain_string(raw_path, "artifact path")
        digest = _plain_string(
            raw_digest, f"artifact digest {artifact_path}"
        )
        if (
            not artifact_path.startswith("/")
            or SHA256_RE.fullmatch(digest) is None
        ):
            raise AudioManifestError(
                f"invalid artifact allowlist: {artifact_path}"
            )
        if (
            artifact_path.startswith("/tmp/")
            and not artifact_path.startswith(f"{policy.remote_root}/")
        ):
            raise AudioManifestError(
                f"temporary artifact is outside {policy.remote_root}"
            )
        artifacts[artifact_path] = digest
    for required_path, expected_digest in policy.fixed_artifacts.items():
        if artifacts.get(required_path) != expected_digest:
            raise AudioManifestError(
                f"fixed artifact mismatch: {required_path}"
            )

    raw_absent = root["absent_artifacts"]
    if (
        not isinstance(raw_absent, list)
        or any(not isinstance(path, str) for path in raw_absent)
        or len(set(raw_absent)) != len(raw_absent)
    ):
        raise AudioManifestError(
            "absent_artifacts must be a unique string array"
        )
    absent = tuple(sorted(raw_absent))
    if absent != tuple(sorted(policy.absent_artifacts)):
        raise AudioManifestError(
            "absent_artifacts differs from fixed CLC isolation policy"
        )
    expected = _keys(
        root["expected_result"],
        set(EXPECTED_RESULT),
        "expected_result",
    )
    if expected != policy.expected_result:
        raise AudioManifestError(
            f"expected result differs from fixed PoC {policy.poc_id} "
            "policy"
        )

    return AudioManifest(
        policy=policy,
        serial=serial,
        kernel=kernel,
        galcore_major_minor=galcore,
        npu_parameters=parameters,
        environment=environment,
        remote_executable=executable,
        remote_argument=argument,
        timeout_ms=timeout_ms,
        required_artifacts=artifacts,
        absent_artifacts=absent,
        expected_result=expected,
        sha256=_canonical_sha256(root),
        candidates={},
    )


def collect_snapshot(
    transport: Any, manifest: AudioManifest
) -> dict[str, Any]:
    snapshot = npu_compile_lab.collect_snapshot(transport, manifest)
    errors = snapshot["errors"]
    snapshot["absent_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in manifest.absent_artifacts
    }
    snapshot["staged_artifact_states"] = {
        path: npu_compile_lab._remote_file_state(transport, path, errors)
        for path in manifest.policy.staged_modes
    }
    return snapshot


def _competing_runner(
    manifest: AudioManifest, snapshot: Mapping[str, Any]
) -> bool:
    basename = manifest.remote_executable.rsplit("/", 1)[-1]
    return any(
        manifest.remote_executable in line or basename in line
        for line in snapshot.get("processes", [])
    )


def verify_preflight(
    manifest: AudioManifest, snapshot: Mapping[str, Any]
) -> None:
    if snapshot.get("errors"):
        raise AudioPreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise AudioPreflightError(
            "authorized device is not exclusively connected"
        )
    if snapshot.get("kernel") != manifest.kernel:
        raise AudioPreflightError("kernel identity mismatch")
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(boot_id) is None
    ):
        raise AudioPreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != dict(manifest.npu_parameters):
        raise AudioPreflightError(
            "NPU parameters differ from fixed baseline"
        )
    metadata = snapshot.get("galcore_device") or ""
    if re.search(r"\b199,\s*0\b", metadata) is None:
        raise AudioPreflightError(
            "/dev/galcore is not character device 199:0"
        )
    if snapshot.get("galcore_holders"):
        raise AudioPreflightError("/dev/galcore is already held")
    if _competing_runner(manifest, snapshot):
        raise AudioPreflightError(
            f"a PoC {manifest.policy.poc_id} runner process is already "
            "present"
        )
    for path, expected in manifest.required_artifacts.items():
        observed = snapshot.get("artifact_hashes", {}).get(path)
        if observed != expected:
            raise AudioPreflightError(
                f"artifact mismatch: {path}; expected {expected}, "
                f"observed {observed}"
            )
    for path, expected_mode in manifest.policy.staged_modes.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != expected_mode
        ):
            raise AudioPreflightError(
                f"staged artifact metadata mismatch: {path}"
            )
    for path in manifest.absent_artifacts:
        state = snapshot.get("absent_artifact_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise AudioPreflightError(
                f"forbidden compiler artifact is visible: {path}"
            )
    if not snapshot.get("dmesg_tail"):
        raise AudioPreflightError(
            "kernel diagnostic baseline is empty"
        )


def compare_health(
    manifest: AudioManifest,
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
    if _competing_runner(manifest, after):
        differences.append(
            f"the PoC {manifest.policy.poc_id} runner remains after "
            "execution"
        )
    return differences, alerts


def build_remote_command(
    manifest: AudioManifest, sentinel: str
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
        "wave_audio_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_audio_rc\"\n"
        "exit \"$wave_audio_rc\"\n"
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$",
        re.MULTILINE,
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise AudioResultError(
            f"expected one remote-exit sentinel, observed {len(matches)}"
        )
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise AudioResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def parse_result(
    stdout: str, manifest: AudioManifest
) -> dict[str, Any]:
    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    audio_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("AUDIO ")
    ]
    if len(identity_lines) != 1 or len(audio_lines) != 1:
        raise AudioResultError(
            "expected exactly one identity and one audio result line"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(
        identity_lines[0]
    )
    if (
        identity_match is None
        or manifest.policy.result_re.fullmatch(audio_lines[0]) is None
    ):
        raise AudioResultError(
            "identity or audio result line is malformed"
        )
    identity = identity_match.groupdict()
    if identity != EXPECTED_IDENTITY:
        raise AudioResultError("live identity differs from fixed policy")
    selectors = [
        match.groupdict()
        for match in npu_compile_lab.SELECTOR_RE.finditer(stdout)
        if match["kernel"] == manifest.expected_result["kernel"]
    ]
    if len(selectors) != 1:
        raise AudioResultError(
            f"expected exactly one "
            f"{manifest.expected_result['kernel']} selector result"
        )
    if selectors[0]["backend"] != manifest.expected_result["backend"]:
        raise AudioResultError(
            f"{manifest.expected_result['kernel']} selected "
            f"{selectors[0]['backend']}, "
            f"expected {manifest.expected_result['backend']}"
        )
    return {
        "identity": identity,
        "selector": selectors[0],
        "mode": manifest.policy.result_mode,
        "iterations": 1,
        "input_energy": 16.0,
        "output_energy": 0.125,
        "max_error": 0.0,
        "energy_ratio": 0.0078125,
    }


def run_once(
    transport: Any,
    manifest: AudioManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise AudioLabError(f"run ID must match {RUN_ID_RE.pattern}")
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": manifest.policy.experiment,
        "run_id": run_id,
        "started_at": npu_compile_lab._timestamp(),
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
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
    except AudioPreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = npu_compile_lab._timestamp()
        npu_compile_lab._write_json_exclusive(
            run_directory / "run.json", envelope
        )
        return envelope, run_directory

    sentinel = f"__WAVE_AUDIO_EXIT_{uuid.uuid4().hex}__"
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
                    "native audio-denoiser runner",
                )
            except npu_compile_lab.ResultError as exc:
                raise AudioResultError(str(exc)) from exc
            envelope["result"] = parse_result(
                cleaned_stdout, manifest
            )
        except AudioResultError as exc:
            envelope["errors"].append(str(exc))
    if differences:
        envelope["errors"].append("device health gate failed")

    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = npu_compile_lab._timestamp()
    npu_compile_lab._write_json_exclusive(
        run_directory / "run.json", envelope
    )
    return envelope, run_directory


def _parser(policy: AudioExecutionPolicy) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            f"Fixed Wave NPU PoC {policy.poc_id} one-shot direct "
            f"{policy.operation} denoiser; "
            "no model selector, retry, benchmark, or recovery interface"
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    run = subparsers.add_parser("run")
    run.add_argument("--run-id", required=True)
    subparsers.add_parser("manifest")
    return parser


def main(
    argv: Sequence[str] | None = None,
    policy: AudioExecutionPolicy = POC8B_POLICY,
) -> int:
    args = _parser(policy).parse_args(argv)
    try:
        manifest = load_manifest(policy=policy)
        if args.command == "manifest":
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "mode": policy.result_mode,
                        "operation": policy.operation,
                        "iterations": 1,
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
    except (AudioLabError, npu_lab.NpuLabError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
