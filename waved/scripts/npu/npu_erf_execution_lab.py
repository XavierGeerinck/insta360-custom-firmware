#!/usr/bin/env python3
"""One-shot PoC 6 harness for the generated Wave Erf NBG.

The command, model, tensors, input, expected output, environment, deadline,
and artifact identities are fixed in source control. This module deliberately
has no model selector, arbitrary command, iteration count, retry, or recovery
interface.
"""

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
DEFAULT_MANIFEST = Path(__file__).with_name("erf_execution_manifest.json")
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"

REMOTE_ROOT = "/tmp/wave-npu-exec"
REMOTE_EXECUTABLE = (
    f"{REMOTE_ROOT}/bin/wave-npu-execute-erf-zero-v1"
)
REMOTE_TIM_VX = f"{REMOTE_ROOT}/lib/libtim-vx.so"
REMOTE_NBG = f"{REMOTE_ROOT}/model/wave-erf-openvx.nbg"
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
        "ed6fa998e20e51cb785b928d1a2bb398"
        "f39770a7c95190a857627bee5d16e630"
    ),
    REMOTE_TIM_VX: (
        "d2c8fc7aaf0ad0b86f02fcd54bb1cdca"
        "9f04967b5cd9efa61a211565b453375b"
    ),
    REMOTE_NBG: (
        "6b2ac998909fecfdf8243e0b1e2b283e"
        "5abb485d09d579ae818fbc59824de3ee"
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
EXPECTED_ABSENT_ARTIFACTS = (
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
EXPECTED_STAGED_MODES = {
    REMOTE_EXECUTABLE: "-r-xr-xr-x",
    REMOTE_TIM_VX: "-r--r--r--",
    REMOTE_NBG: "-r--r--r--",
}

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
EXIT_RE = re.compile(r"^__WAVE_EXEC_EXIT_[0-9a-f]{32}__$")
EXECUTION_RE = re.compile(
    r"^EXECUTION status=PASS mode=execute-once iterations=1 "
    r"input=0,0,0,0 output=0,0,0,0 bytes=4$"
)


class ExecutionLabError(Exception):
    """Expected PoC 6 harness failure."""


class ExecutionManifestError(ExecutionLabError):
    """The source-controlled execution manifest violates fixed policy."""


class ExecutionPreflightError(ExecutionLabError):
    """The live device differs from the reviewed starting state."""


class ExecutionResultError(ExecutionLabError):
    """The one-shot native execution result is missing or malformed."""


@dataclass(frozen=True)
class ExecutionPolicy:
    poc_id: str
    experiment: str
    manifest_path: Path
    remote_root: str
    remote_executable: str
    remote_tim_vx: str
    remote_nbg: str
    fixed_artifacts: Mapping[str, str]
    absent_artifacts: tuple[str, ...]
    staged_modes: Mapping[str, str]
    nbg_bytes: int
    input_values: tuple[int, ...]
    output_values: tuple[int, ...]
    execution_re: re.Pattern[str]


POC6_POLICY = ExecutionPolicy(
    poc_id="6",
    experiment="wave-npu-poc6-first-custom-nbg-execution",
    manifest_path=DEFAULT_MANIFEST,
    remote_root=REMOTE_ROOT,
    remote_executable=REMOTE_EXECUTABLE,
    remote_tim_vx=REMOTE_TIM_VX,
    remote_nbg=REMOTE_NBG,
    fixed_artifacts=FIXED_ARTIFACTS,
    absent_artifacts=EXPECTED_ABSENT_ARTIFACTS,
    staged_modes=EXPECTED_STAGED_MODES,
    nbg_bytes=5840,
    input_values=(0, 0, 0, 0),
    output_values=(0, 0, 0, 0),
    execution_re=EXECUTION_RE,
)

POC7_ROOT = "/tmp/wave-npu-exec7"
POC7_EXECUTABLE = (
    f"{POC7_ROOT}/bin/wave-npu-execute-erf-nonzero-v1"
)
POC7_TIM_VX = f"{POC7_ROOT}/lib/libtim-vx.so"
POC7_NBG = f"{POC7_ROOT}/model/wave-erf-openvx.nbg"
POC7_FIXED_ARTIFACTS = {
    **{
        path: digest
        for path, digest in FIXED_ARTIFACTS.items()
        if not path.startswith(f"{REMOTE_ROOT}/")
    },
    POC7_EXECUTABLE: (
        "df354003e0bb89458d8d425718ca91db0"
        "ed344e1c7c792e1c7ea11fa2c779e58"
    ),
    POC7_TIM_VX: FIXED_ARTIFACTS[REMOTE_TIM_VX],
    POC7_NBG: FIXED_ARTIFACTS[REMOTE_NBG],
}
POC7_ABSENT_ARTIFACTS = (
    "/lib/libCLC.so",
    f"{POC7_ROOT}/lib/libCLC.so",
    "/usr/lib/libCLC.so",
)
POC7_STAGED_MODES = {
    POC7_EXECUTABLE: "-r-xr-xr-x",
    POC7_TIM_VX: "-r--r--r--",
    POC7_NBG: "-r--r--r--",
}
POC7_EXECUTION_RE = re.compile(
    r"^EXECUTION status=PASS mode=execute-once iterations=1 "
    r"input=0,1,2,255 output=0,1,1,1 bytes=4$"
)
POC7_POLICY = ExecutionPolicy(
    poc_id="7",
    experiment="wave-npu-poc7-nonzero-erf-characterization",
    manifest_path=Path(__file__).with_name(
        "erf_nonzero_execution_manifest.json"
    ),
    remote_root=POC7_ROOT,
    remote_executable=POC7_EXECUTABLE,
    remote_tim_vx=POC7_TIM_VX,
    remote_nbg=POC7_NBG,
    fixed_artifacts=POC7_FIXED_ARTIFACTS,
    absent_artifacts=POC7_ABSENT_ARTIFACTS,
    staged_modes=POC7_STAGED_MODES,
    nbg_bytes=5840,
    input_values=(0, 1, 2, 255),
    output_values=(0, 1, 1, 1),
    execution_re=POC7_EXECUTION_RE,
)


@dataclass(frozen=True)
class ExecutionManifest:
    policy: ExecutionPolicy
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
            raise ExecutionManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _keys(
    value: Any, expected: set[str], context: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ExecutionManifestError(f"{context} must be an object")
    missing = expected - set(value)
    unknown = set(value) - expected
    if missing or unknown:
        raise ExecutionManifestError(
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
        raise ExecutionManifestError(
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
    policy: ExecutionPolicy = POC6_POLICY,
) -> ExecutionManifest:
    if path is None:
        path = policy.manifest_path
    try:
        document = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_without_duplicate_keys,
        )
    except ExecutionManifestError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ExecutionManifestError(f"cannot load {path}: {exc}") from exc

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
        raise ExecutionManifestError("unsupported schema_version")

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
        raise ExecutionManifestError(
            "device identity differs from fixed PoC 6 policy"
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
        raise ExecutionManifestError(
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
        raise ExecutionManifestError(
            f"remote invocation differs from fixed PoC {policy.poc_id} "
            "policy"
        )
    timeout_ms = root["timeout_ms"]
    if (
        isinstance(timeout_ms, bool)
        or not isinstance(timeout_ms, int)
        or timeout_ms != EXPECTED_TIMEOUT_MS
    ):
        raise ExecutionManifestError(
            f"timeout_ms differs from fixed PoC {policy.poc_id} policy"
        )

    raw_artifacts = root["required_artifacts"]
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise ExecutionManifestError(
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
            raise ExecutionManifestError(
                f"invalid artifact allowlist: {artifact_path}"
            )
        if (
            artifact_path.startswith("/tmp/")
            and not artifact_path.startswith(f"{policy.remote_root}/")
        ):
            raise ExecutionManifestError(
                f"temporary artifact is outside {policy.remote_root}"
            )
        artifacts[artifact_path] = digest
    for required_path, expected_digest in policy.fixed_artifacts.items():
        if artifacts.get(required_path) != expected_digest:
            raise ExecutionManifestError(
                f"fixed artifact mismatch: {required_path}"
            )

    raw_absent = root["absent_artifacts"]
    if (
        not isinstance(raw_absent, list)
        or any(not isinstance(path, str) for path in raw_absent)
        or len(set(raw_absent)) != len(raw_absent)
    ):
        raise ExecutionManifestError(
            "absent_artifacts must be a unique string array"
        )
    absent = tuple(sorted(raw_absent))
    if absent != tuple(sorted(policy.absent_artifacts)):
        raise ExecutionManifestError(
            "absent_artifacts differs from fixed CLC isolation policy"
        )

    expected = _keys(
        root["expected_result"],
        {"identity", "input", "output", "bytes", "iterations"},
        "expected_result",
    )
    identity = _keys(
        expected["identity"], set(EXPECTED_IDENTITY), "expected_result.identity"
    )
    if identity != EXPECTED_IDENTITY:
        raise ExecutionManifestError(
            "expected identity differs from fixed PoC 6 policy"
        )
    if (
        expected["input"] != list(policy.input_values)
        or expected["output"] != list(policy.output_values)
        or expected["bytes"] != len(policy.output_values)
        or expected["iterations"] != 1
    ):
        raise ExecutionManifestError(
            "expected result differs from fixed PoC 6 policy"
        )

    return ExecutionManifest(
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
    transport: Any, manifest: ExecutionManifest
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
        raise ExecutionPreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise ExecutionPreflightError(
            "authorized device is not exclusively connected"
        )
    if snapshot.get("kernel") != manifest.kernel:
        raise ExecutionPreflightError("kernel identity mismatch")
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(boot_id) is None
    ):
        raise ExecutionPreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != dict(manifest.npu_parameters):
        raise ExecutionPreflightError(
            "NPU parameters differ from fixed baseline"
        )
    metadata = snapshot.get("galcore_device") or ""
    if re.search(r"\b199,\s*0\b", metadata) is None:
        raise ExecutionPreflightError(
            "/dev/galcore is not character device 199:0"
        )
    if snapshot.get("galcore_holders"):
        raise ExecutionPreflightError("/dev/galcore is already held")
    if _competing_executor(manifest, snapshot):
        raise ExecutionPreflightError(
            "a PoC 6 executor process is already present"
        )
    for path, expected in manifest.required_artifacts.items():
        observed = snapshot.get("artifact_hashes", {}).get(path)
        if observed != expected:
            raise ExecutionPreflightError(
                f"artifact mismatch: {path}; expected {expected}, "
                f"observed {observed}"
            )
    for path, expected_mode in manifest.policy.staged_modes.items():
        state = snapshot.get("staged_artifact_states", {}).get(path, {})
        if (
            state.get("status") != "present"
            or state.get("permissions") != expected_mode
        ):
            raise ExecutionPreflightError(
                f"staged artifact metadata mismatch: {path}"
            )
    nbg_state = snapshot["staged_artifact_states"][
        manifest.policy.remote_nbg
    ]
    if nbg_state.get("bytes") != manifest.policy.nbg_bytes:
        raise ExecutionPreflightError("fixed NBG byte count mismatch")
    for path in manifest.absent_artifacts:
        state = snapshot.get("absent_artifact_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise ExecutionPreflightError(
                f"forbidden compiler artifact is visible: {path}"
            )
    if not snapshot.get("dmesg_tail"):
        raise ExecutionPreflightError(
            "kernel diagnostic baseline is empty"
        )


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
    if (
        before.get("absent_artifact_states")
        != after.get("absent_artifact_states")
    ):
        differences.append("CLC isolation state changed")
    if any(
        state.get("status") != "absent"
        for state in after.get("absent_artifact_states", {}).values()
    ):
        differences.append("a forbidden compiler artifact is visible")
    if _competing_executor(manifest, after):
        differences.append("the PoC 6 executor remains after execution")
    if manifest.policy.poc_id in ("16C", "16D"):
        def services(snapshot):
            return sorted(
                (line.split()[0], re.sub(r"\s+", " ", line).split(" ", 2)[-1])
                for line in snapshot.get("processes", [])
                if re.search(r"(?:/|\b)(?:waved|S60waved|audioserver|pspapp)(?:\s|$)", line)
            )
        if services(before) != services(after):
            differences.append("service process identities changed during execution")
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
        "wave_exec_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_exec_rc\"\n"
        "exit \"$wave_exec_rc\"\n"
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$",
        re.MULTILINE,
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ExecutionResultError(
            f"expected one remote-exit sentinel, observed {len(matches)}"
        )
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise ExecutionResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def parse_result(
    stdout: str, manifest: ExecutionManifest
) -> dict[str, Any]:
    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    execution_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("EXECUTION ")
    ]
    if len(identity_lines) != 1 or len(execution_lines) != 1:
        raise ExecutionResultError(
            "expected exactly one identity and one execution line"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(
        identity_lines[0]
    )
    execution_match = manifest.policy.execution_re.fullmatch(execution_lines[0])
    if identity_match is None or execution_match is None:
        raise ExecutionResultError(
            "identity or execution result line is malformed"
        )
    identity = identity_match.groupdict()
    if identity != manifest.expected_result["identity"]:
        raise ExecutionResultError("live identity differs from fixed policy")
    result = {
        "identity": identity,
        "mode": "execute-once",
        "iterations": 1,
        "input": list(manifest.policy.input_values),
        "output": list(manifest.policy.output_values),
        "bytes": len(manifest.policy.output_values),
    }
    measured = execution_match.groupdict()
    if "graph_us" in measured:
        result["graph_us"] = int(measured["graph_us"])
        result["output_hex"] = measured["output_hex"]
    return result


def run_once(
    transport: Any,
    manifest: ExecutionManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise ExecutionLabError(f"run ID must match {RUN_ID_RE.pattern}")
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
    except ExecutionPreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = npu_compile_lab._timestamp()
        npu_compile_lab._write_json_exclusive(
            run_directory / "run.json", envelope
        )
        return envelope, run_directory

    sentinel = f"__WAVE_EXEC_EXIT_{uuid.uuid4().hex}__"
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
                    "native one-shot executor",
                )
            except npu_compile_lab.ResultError as exc:
                raise ExecutionResultError(str(exc)) from exc
            envelope["result"] = parse_result(cleaned_stdout, manifest)
        except ExecutionResultError as exc:
            envelope["errors"].append(str(exc))
    if differences:
        envelope["errors"].append("device health gate failed")

    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = npu_compile_lab._timestamp()
    npu_compile_lab._write_json_exclusive(
        run_directory / "run.json", envelope
    )
    return envelope, run_directory


def _parser(policy: ExecutionPolicy) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            f"Fixed Wave NPU PoC {policy.poc_id} one-shot Erf execution; "
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
    policy: ExecutionPolicy = POC6_POLICY,
) -> int:
    args = _parser(policy).parse_args(argv)
    try:
        manifest = load_manifest(policy=policy)
        if args.command == "manifest":
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "mode": "execute-once",
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
    except (ExecutionLabError, npu_lab.NpuLabError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
