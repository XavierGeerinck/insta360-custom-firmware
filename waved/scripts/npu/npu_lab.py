#!/usr/bin/env python3
"""Safe, reproducible host harness for allowlisted Insta360 Wave NPU probes.

This module deliberately exposes no arbitrary remote-command option. Every
executable, argument, environment entry, timeout, artifact hash, and expected
result comes from the adjacent source-controlled ``experiments.json`` file.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping, Sequence


SCHEMA_VERSION = 1
DEFAULT_EXPERIMENTS_PATH = Path(__file__).with_name("experiments.json")

ALLOWED_ENVIRONMENT_KEYS = frozenset({"LD_LIBRARY_PATH", "VIVANTE_SDK_DIR"})
GRANDFATHERED_EXECUTABLE = "/data/waved/waved-npu-probe.poc"
EXPERIMENT_EXECUTABLE_ROOT = "/data/waved/npu-lab/"

MAX_CAPTURE_BYTES = 1024 * 1024
MAX_SNAPSHOT_LINES = 32
MAX_DIFFERENCES = 64
SNAPSHOT_COMMAND_TIMEOUT_MS = 5000
MAX_EXPERIMENT_TIMEOUT_MS = 300_000

RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
EXPERIMENT_ID_RE = RUN_ID_RE
CONTROL_CHARACTER_RE = re.compile(r"[\x00-\x1f\x7f]")
ANSI_CSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")

ROOT_KEYS = frozenset({"schema_version", "experiments"})
EXPERIMENT_KEYS = frozenset(
    {
        "description",
        "remote_argv",
        "environment",
        "timeout_ms",
        "expected_remote_exit",
        "required_artifacts",
        "expected_result",
    }
)
EXPECTED_RESULT_KEYS = frozenset(
    {
        "status",
        "iterations",
        "output_bytes",
        "output_sha256",
        "cleanup",
    }
)

RESULT_LINE_RE = re.compile(
    r"^RESULT: (?P<status>PASS|FAIL) — NPU executed "
    r"(?P<iterations>[0-9]+) firmware-graph iterations; "
    r"output_bytes=(?P<output_bytes>[0-9]+), "
    r"output_sha256=(?P<output_sha256>[0-9a-f]{64}), "
    r"average=(?P<average_ms>[0-9]+(?:\.[0-9]+)?) ms, "
    r"cleanup=(?P<cleanup>-?[0-9]+)\.$"
)


class NpuLabError(Exception):
    """Base class for expected NPU lab errors."""


class SchemaError(NpuLabError):
    """An experiment definition is malformed or outside policy."""


class ExperimentNotFound(NpuLabError):
    """The selected source-controlled experiment does not exist."""


class RunDirectoryError(NpuLabError):
    """A run ID or output directory violates the non-overwrite policy."""


class ResultParseError(NpuLabError):
    """Machine-readable native output is missing or malformed."""


class PreflightError(NpuLabError):
    """Device identity or required artifacts do not match the manifest."""


@dataclass(frozen=True)
class ExpectedResult:
    status: str
    iterations: int
    output_bytes: int
    output_sha256: str
    cleanup: int

    def as_dict(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "iterations": self.iterations,
            "output_bytes": self.output_bytes,
            "output_sha256": self.output_sha256,
            "cleanup": self.cleanup,
        }


@dataclass(frozen=True)
class Experiment:
    experiment_id: str
    description: str
    remote_argv: tuple[str, ...]
    environment_items: tuple[tuple[str, str], ...]
    timeout_ms: int
    expected_remote_exit: int
    required_artifact_items: tuple[tuple[str, str], ...]
    expected_result: ExpectedResult

    @property
    def environment(self) -> dict[str, str]:
        return dict(self.environment_items)

    @property
    def required_artifacts(self) -> dict[str, str]:
        return dict(self.required_artifact_items)

    def definition_dict(self) -> dict[str, Any]:
        return {
            "description": self.description,
            "remote_argv": list(self.remote_argv),
            "environment": self.environment,
            "timeout_ms": self.timeout_ms,
            "expected_remote_exit": self.expected_remote_exit,
            "required_artifacts": self.required_artifacts,
            "expected_result": self.expected_result.as_dict(),
        }


@dataclass(frozen=True)
class ExperimentRegistry:
    schema_version: int
    experiments: Mapping[str, Experiment]
    definition_hashes: Mapping[str, str]

    def get(self, experiment_id: str) -> Experiment:
        try:
            return self.experiments[experiment_id]
        except KeyError as exc:
            available = ", ".join(sorted(self.experiments))
            raise ExperimentNotFound(
                f"unknown experiment {experiment_id!r}; available: {available}"
            ) from exc

    def definition_sha256(self, experiment_id: str) -> str:
        self.get(experiment_id)
        return self.definition_hashes[experiment_id]


@dataclass(frozen=True)
class CommandResult:
    stdout: str
    stderr: str
    returncode: int
    duration_ms: int
    timed_out: bool = False


def _json_object_without_duplicates(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise SchemaError(f"duplicate JSON key: {key!r}")
        result[key] = value
    return result


def _strict_keys(
    value: Mapping[str, Any], expected: frozenset[str], context: str
) -> None:
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing:
        raise SchemaError(f"{context} missing keys: {', '.join(missing)}")
    if unknown:
        raise SchemaError(f"{context} has unknown keys: {', '.join(unknown)}")


def _require_dict(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise SchemaError(f"{context} must be a JSON object")
    return value


def _require_string(value: Any, context: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str):
        raise SchemaError(f"{context} must be a string")
    if nonempty and not value:
        raise SchemaError(f"{context} must not be empty")
    if CONTROL_CHARACTER_RE.search(value):
        raise SchemaError(f"{context} contains a control character")
    return value


def _require_int(
    value: Any,
    context: str,
    *,
    minimum: int | None = None,
    maximum: int | None = None,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise SchemaError(f"{context} must be an integer")
    if minimum is not None and value < minimum:
        raise SchemaError(f"{context} must be at least {minimum}")
    if maximum is not None and value > maximum:
        raise SchemaError(f"{context} must be at most {maximum}")
    return value


def _require_sha256(value: Any, context: str) -> str:
    digest = _require_string(value, context)
    if SHA256_RE.fullmatch(digest) is None:
        raise SchemaError(f"{context} must be a lowercase SHA-256 digest")
    return digest


def _validate_executable(path: str) -> None:
    if path == GRANDFATHERED_EXECUTABLE:
        return
    if path.startswith(EXPERIMENT_EXECUTABLE_ROOT):
        relative = path[len(EXPERIMENT_EXECUTABLE_ROOT) :]
        if relative and not relative.startswith("/") and "/../" not in f"/{relative}/":
            return
    raise SchemaError(
        "experiment executable must be the grandfathered probe or reside "
        f"under {EXPERIMENT_EXECUTABLE_ROOT}"
    )


def _parse_expected_result(value: Any, context: str) -> ExpectedResult:
    raw = _require_dict(value, context)
    _strict_keys(raw, EXPECTED_RESULT_KEYS, context)
    status = _require_string(raw["status"], f"{context}.status")
    if status not in {"PASS", "FAIL"}:
        raise SchemaError(f"{context}.status must be PASS or FAIL")
    return ExpectedResult(
        status=status,
        iterations=_require_int(
            raw["iterations"], f"{context}.iterations", minimum=0
        ),
        output_bytes=_require_int(
            raw["output_bytes"], f"{context}.output_bytes", minimum=0
        ),
        output_sha256=_require_sha256(
            raw["output_sha256"], f"{context}.output_sha256"
        ),
        cleanup=_require_int(raw["cleanup"], f"{context}.cleanup"),
    )


def _parse_experiment(experiment_id: str, value: Any) -> Experiment:
    if EXPERIMENT_ID_RE.fullmatch(experiment_id) is None:
        raise SchemaError(
            f"experiment ID {experiment_id!r} must match {RUN_ID_RE.pattern}"
        )

    context = f"experiments.{experiment_id}"
    raw = _require_dict(value, context)
    _strict_keys(raw, EXPERIMENT_KEYS, context)

    argv_value = raw["remote_argv"]
    if not isinstance(argv_value, list) or not argv_value:
        raise SchemaError(f"{context}.remote_argv must be a nonempty array")
    argv = tuple(
        _require_string(token, f"{context}.remote_argv[{index}]")
        for index, token in enumerate(argv_value)
    )
    _validate_executable(argv[0])

    environment_value = _require_dict(raw["environment"], f"{context}.environment")
    unknown_environment = sorted(
        set(environment_value) - ALLOWED_ENVIRONMENT_KEYS
    )
    if unknown_environment:
        raise SchemaError(
            f"{context}.environment has non-allowlisted keys: "
            + ", ".join(unknown_environment)
        )
    environment = tuple(
        sorted(
            (
                key,
                _require_string(
                    environment_value[key], f"{context}.environment.{key}"
                ),
            )
            for key in environment_value
        )
    )

    artifact_value = _require_dict(
        raw["required_artifacts"], f"{context}.required_artifacts"
    )
    if not artifact_value:
        raise SchemaError(f"{context}.required_artifacts must not be empty")
    artifact_items: list[tuple[str, str]] = []
    for path, digest_value in artifact_value.items():
        artifact_path = _require_string(
            path, f"{context}.required_artifacts path"
        )
        if not artifact_path.startswith("/"):
            raise SchemaError(
                f"{context}.required_artifacts path must be absolute: "
                f"{artifact_path!r}"
            )
        artifact_items.append(
            (
                artifact_path,
                _require_sha256(
                    digest_value,
                    f"{context}.required_artifacts[{artifact_path!r}]",
                ),
            )
        )

    return Experiment(
        experiment_id=experiment_id,
        description=_require_string(raw["description"], f"{context}.description"),
        remote_argv=argv,
        environment_items=environment,
        timeout_ms=_require_int(
            raw["timeout_ms"],
            f"{context}.timeout_ms",
            minimum=1,
            maximum=MAX_EXPERIMENT_TIMEOUT_MS,
        ),
        expected_remote_exit=_require_int(
            raw["expected_remote_exit"],
            f"{context}.expected_remote_exit",
            minimum=0,
            maximum=255,
        ),
        required_artifact_items=tuple(sorted(artifact_items)),
        expected_result=_parse_expected_result(
            raw["expected_result"], f"{context}.expected_result"
        ),
    )


def _canonical_json_sha256(value: Any) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def load_registry(path: Path = DEFAULT_EXPERIMENTS_PATH) -> ExperimentRegistry:
    """Load and validate all source-controlled experiment definitions."""

    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise SchemaError(f"cannot read experiment definitions at {path}: {exc}") from exc

    try:
        document = json.loads(text, object_pairs_hook=_json_object_without_duplicates)
    except SchemaError:
        raise
    except (json.JSONDecodeError, UnicodeError) as exc:
        raise SchemaError(f"invalid experiment JSON at {path}: {exc}") from exc

    root = _require_dict(document, "root")
    _strict_keys(root, ROOT_KEYS, "root")
    schema_version = _require_int(root["schema_version"], "schema_version")
    if schema_version != SCHEMA_VERSION:
        raise SchemaError(
            f"unsupported schema_version {schema_version}; expected {SCHEMA_VERSION}"
        )

    raw_experiments = _require_dict(root["experiments"], "experiments")
    if not raw_experiments:
        raise SchemaError("experiments must not be empty")

    experiments: dict[str, Experiment] = {}
    definition_hashes: dict[str, str] = {}
    for experiment_id, raw_value in raw_experiments.items():
        experiment = _parse_experiment(experiment_id, raw_value)
        experiments[experiment_id] = experiment
        definition_hashes[experiment_id] = _canonical_json_sha256(
            {
                "experiment_id": experiment_id,
                **experiment.definition_dict(),
            }
        )

    return ExperimentRegistry(
        schema_version=schema_version,
        experiments=MappingProxyType(experiments),
        definition_hashes=MappingProxyType(definition_hashes),
    )


def bounded_text(text: str, limit: int = MAX_CAPTURE_BYTES) -> str:
    """Retain bounded UTF-8 head/tail evidence and mark truncation."""

    if limit < 1:
        raise ValueError("limit must be positive")
    encoded = text.encode("utf-8", errors="replace")
    if len(encoded) <= limit:
        return text
    head_size = max(1, limit // 2)
    tail_size = max(0, limit - head_size)
    head = encoded[:head_size].decode("utf-8", errors="replace")
    tail = encoded[-tail_size:].decode("utf-8", errors="replace") if tail_size else ""
    omitted = len(encoded) - head_size - tail_size
    return f"{head}\n... [TRUNCATED {omitted} BYTES] ...\n{tail}"


def _read_bounded_file(handle: Any, limit: int = MAX_CAPTURE_BYTES) -> str:
    handle.flush()
    handle.seek(0, os.SEEK_END)
    size = handle.tell()
    if size <= limit:
        handle.seek(0)
        payload = handle.read()
    else:
        head_size = max(1, limit // 2)
        tail_size = max(0, limit - head_size)
        handle.seek(0)
        head = handle.read(head_size)
        handle.seek(-tail_size, os.SEEK_END) if tail_size else None
        tail = handle.read(tail_size) if tail_size else b""
        marker = (
            f"\n... [TRUNCATED {size - head_size - tail_size} BYTES] ...\n"
        ).encode("ascii")
        payload = head + marker + tail
    return payload.decode("utf-8", errors="replace")


class AdbTransport:
    """Production ADB boundary with no shell=True and bounded host capture."""

    def __init__(self, adb_path: str | None = None):
        self.adb_path = adb_path or shutil.which("adb") or "adb"

    def run(self, args: Sequence[str], timeout_ms: int) -> CommandResult:
        if not args or any(not isinstance(arg, str) for arg in args):
            raise ValueError("ADB arguments must be a nonempty string sequence")
        if timeout_ms <= 0:
            raise ValueError("timeout_ms must be positive")

        command = [self.adb_path, *args]
        started = time.monotonic()
        with tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file:
            try:
                process = subprocess.Popen(
                    command,
                    stdin=subprocess.DEVNULL,
                    stdout=stdout_file,
                    stderr=stderr_file,
                    shell=False,
                )
            except OSError as exc:
                duration_ms = int((time.monotonic() - started) * 1000)
                return CommandResult(
                    stdout="",
                    stderr=bounded_text(f"cannot execute adb: {exc}\n"),
                    returncode=127,
                    duration_ms=duration_ms,
                    timed_out=False,
                )

            timed_out = False
            try:
                process.wait(timeout=timeout_ms / 1000)
            except subprocess.TimeoutExpired:
                timed_out = True
                # This terminates only the host adb client. It does not attempt
                # an unsafe remote kill or driver reset.
                process.kill()
                process.wait()

            duration_ms = int((time.monotonic() - started) * 1000)
            stdout = _read_bounded_file(stdout_file)
            stderr = _read_bounded_file(stderr_file)
            if timed_out:
                stderr = bounded_text(
                    stderr
                    + (
                        "\nNPU_LAB: host ADB timeout; no automatic remote "
                        "recovery was attempted.\n"
                    )
                )
            return CommandResult(
                stdout=stdout,
                stderr=stderr,
                returncode=process.returncode,
                duration_ms=duration_ms,
                timed_out=timed_out,
            )


def _bounded_lines(text: str, maximum: int = MAX_SNAPSHOT_LINES) -> list[str]:
    lines = [line.rstrip() for line in text.splitlines() if line.strip()]
    return lines[:maximum]


def _shell_value(
    transport: Any,
    command: str,
    *,
    optional: bool,
    errors: list[str],
    label: str,
) -> str | None:
    response = transport.run(("shell", command), SNAPSHOT_COMMAND_TIMEOUT_MS)
    if response.timed_out:
        if not optional:
            errors.append(f"{label}: timed out")
        return None
    if response.returncode != 0:
        if not optional:
            errors.append(f"{label}: adb exit {response.returncode}")
        return None
    return ANSI_CSI_RE.sub("", response.stdout.strip())


def _parameter_snapshot(
    transport: Any,
    name: str,
    errors: list[str],
    *,
    optional: bool,
) -> dict[str, Any]:
    path = f"/sys/module/galcore/parameters/{name}"
    command = (
        f"if [ -r {shlex.quote(path)} ]; then cat {shlex.quote(path)}; "
        "else printf '__UNAVAILABLE__\\n'; fi"
    )
    value = _shell_value(
        transport,
        command,
        optional=optional,
        errors=errors,
        label=f"NPU parameter {name}",
    )
    if value is None or value == "__UNAVAILABLE__":
        if value == "__UNAVAILABLE__" and not optional:
            errors.append(f"NPU parameter {name}: unavailable")
        return {"status": "unavailable", "value": None}
    return {"status": "observed", "value": bounded_text(value, 128)}


def _holder_snapshot(
    transport: Any, errors: list[str]
) -> list[dict[str, int]]:
    command = """count=0
for fd in /proc/[0-9]*/fd/*; do
    target=$(readlink "$fd" 2>/dev/null) || continue
    [ "$target" = "/dev/galcore" ] || continue
    pid=${fd#/proc/}
    pid=${pid%%/*}
    number=${fd##*/}
    printf '%s:%s\\n' "$pid" "$number"
    count=$((count + 1))
    [ "$count" -ge 32 ] && break
done"""
    raw = _shell_value(
        transport,
        command,
        optional=True,
        errors=errors,
        label="galcore holder scan",
    )
    holders: set[tuple[int, int]] = set()
    for line in _bounded_lines(raw or ""):
        match = re.fullmatch(r"([0-9]+):([0-9]+)", line.strip())
        if match is not None:
            holders.add((int(match.group(1)), int(match.group(2))))
    return [{"pid": pid, "fd": fd} for pid, fd in sorted(holders)]


def collect_snapshot(transport: Any, experiment: Experiment) -> dict[str, Any]:
    """Collect the bounded read-only device state needed for one experiment."""

    errors: list[str] = []
    snapshot: dict[str, Any] = {
        "adb_state": "unavailable",
        "kernel": None,
        "uptime_seconds": None,
        "npu_parameters": {
            "nanoqFreq": {"status": "unavailable", "value": None},
            "enableNN": {"status": "unavailable", "value": None},
            "recovery": {"status": "unavailable", "value": None},
        },
        "galcore_device": {"status": "unavailable", "metadata": None},
        "processes": [],
        "galcore_holders": [],
        "artifact_hashes": {
            path: None for path in experiment.required_artifacts
        },
        "errors": errors,
    }

    state_result = transport.run(("get-state",), SNAPSHOT_COMMAND_TIMEOUT_MS)
    if state_result.timed_out:
        errors.append("adb get-state timed out")
        return snapshot
    if state_result.returncode != 0:
        errors.append(f"adb get-state exit {state_result.returncode}")
        return snapshot
    state = state_result.stdout.strip()
    snapshot["adb_state"] = bounded_text(state, 64) if state else "unavailable"
    if state != "device":
        errors.append(f"adb state is {state or 'empty'}, expected device")
        return snapshot

    snapshot["kernel"] = _shell_value(
        transport,
        "uname -a",
        optional=False,
        errors=errors,
        label="kernel",
    )

    uptime = _shell_value(
        transport,
        "cat /proc/uptime",
        optional=False,
        errors=errors,
        label="uptime",
    )
    if uptime:
        try:
            snapshot["uptime_seconds"] = float(uptime.split()[0])
        except (IndexError, ValueError):
            errors.append("uptime: malformed response")

    snapshot["npu_parameters"] = {
        "nanoqFreq": _parameter_snapshot(
            transport, "nanoqFreq", errors, optional=False
        ),
        "enableNN": _parameter_snapshot(
            transport, "enableNN", errors, optional=False
        ),
        "recovery": _parameter_snapshot(
            transport, "recovery", errors, optional=True
        ),
    }

    galcore_metadata = _shell_value(
        transport,
        (
            "if [ -e /dev/galcore ]; then ls -ln /dev/galcore; "
            "else printf '__UNAVAILABLE__\\n'; fi"
        ),
        optional=False,
        errors=errors,
        label="/dev/galcore",
    )
    if galcore_metadata and galcore_metadata != "__UNAVAILABLE__":
        snapshot["galcore_device"] = {
            "status": "observed",
            "metadata": bounded_text(galcore_metadata, 512),
        }
    else:
        errors.append("/dev/galcore: unavailable")

    process_output = _shell_value(
        transport,
        (
            "ps 2>/dev/null | "
            "grep -E '(^|[ /{])(waved|audioserver|pspapp)([ }/]|$)' | "
            "head -n 32 || true"
        ),
        optional=True,
        errors=errors,
        label="selected process scan",
    )
    snapshot["processes"] = _bounded_lines(process_output or "")
    snapshot["galcore_holders"] = _holder_snapshot(transport, errors)

    artifact_hashes: dict[str, str | None] = {}
    for path in experiment.required_artifacts:
        command = (
            f"sha256sum {shlex.quote(path)} 2>/dev/null "
            "|| printf '__MISSING__\\n'"
        )
        output = _shell_value(
            transport,
            command,
            optional=False,
            errors=errors,
            label=f"artifact hash {path}",
        )
        if output is None or output == "__MISSING__":
            artifact_hashes[path] = None
            continue
        first_line = output.splitlines()[0] if output.splitlines() else ""
        digest = first_line.split()[0] if first_line.split() else ""
        if SHA256_RE.fullmatch(digest):
            artifact_hashes[path] = digest
        else:
            artifact_hashes[path] = None
            errors.append(f"artifact hash {path}: malformed response")
    snapshot["artifact_hashes"] = artifact_hashes
    return snapshot


def _snapshot_required_fields_complete(snapshot: Mapping[str, Any]) -> bool:
    if snapshot.get("adb_state") != "device":
        return False
    if not snapshot.get("kernel"):
        return False
    parameters = snapshot.get("npu_parameters", {})
    for key in ("nanoqFreq", "enableNN"):
        if parameters.get(key, {}).get("status") != "observed":
            return False
    if snapshot.get("galcore_device", {}).get("status") != "observed":
        return False
    hashes = snapshot.get("artifact_hashes", {})
    return bool(hashes) and all(
        isinstance(value, str) and SHA256_RE.fullmatch(value)
        for value in hashes.values()
    )


def verify_preflight(
    experiment: Experiment, snapshot: Mapping[str, Any]
) -> dict[str, str]:
    """Refuse execution unless device state and all hashes are exact."""

    if snapshot.get("adb_state") != "device":
        raise PreflightError("adb device is unavailable")
    if not snapshot.get("kernel"):
        raise PreflightError("kernel identity is unavailable")
    parameters = snapshot.get("npu_parameters", {})
    for key in ("nanoqFreq", "enableNN"):
        if parameters.get(key, {}).get("status") != "observed":
            raise PreflightError(f"required NPU parameter {key} is unavailable")
    if snapshot.get("galcore_device", {}).get("status") != "observed":
        raise PreflightError("/dev/galcore is unavailable")

    actual_hashes = snapshot.get("artifact_hashes", {})
    missing: list[str] = []
    mismatches: list[str] = []
    verified: dict[str, str] = {}
    for path, expected_digest in experiment.required_artifacts.items():
        actual_digest = actual_hashes.get(path)
        if actual_digest is None:
            missing.append(path)
        elif actual_digest != expected_digest:
            mismatches.append(
                f"{path} (expected {expected_digest}, observed {actual_digest})"
            )
        else:
            verified[path] = actual_digest
    if missing:
        raise PreflightError("missing required artifacts: " + ", ".join(missing))
    if mismatches:
        raise PreflightError("artifact hash mismatch: " + "; ".join(mismatches))
    return verified


def build_remote_command(experiment: Experiment, sentinel: str) -> str:
    """Build the fixed remote shell wrapper from validated definition tokens."""

    if re.fullmatch(r"__NPU_LAB_REMOTE_EXIT_[0-9a-f]+__", sentinel) is None:
        raise ValueError("invalid internal exit sentinel")
    environment = " ".join(
        shlex.quote(f"{key}={value}")
        for key, value in experiment.environment_items
    )
    command = " ".join(shlex.quote(token) for token in experiment.remote_argv)
    invocation = f"env {environment} {command}" if environment else command
    return (
        f"{invocation}\n"
        "npu_lab_remote_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$npu_lab_remote_rc\"\n"
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    sentinel_pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$", re.MULTILINE
    )
    matches = list(sentinel_pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultParseError(
            f"expected exactly one remote exit sentinel, found {len(matches)}"
        )
    remote_exit = int(matches[0].group("exit"))
    if not 0 <= remote_exit <= 255:
        raise ResultParseError(f"remote exit is outside 0..255: {remote_exit}")
    start, end = matches[0].span()
    if end < len(stdout) and stdout[end : end + 1] == "\n":
        end += 1
    cleaned = stdout[:start] + stdout[end:]
    return cleaned, remote_exit


def parse_stock_result(stdout: str) -> dict[str, Any]:
    result_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("RESULT:")
    ]
    if len(result_lines) != 1:
        raise ResultParseError(
            f"expected exactly one RESULT line, found {len(result_lines)}"
        )
    match = RESULT_LINE_RE.fullmatch(result_lines[0])
    if match is None:
        raise ResultParseError("stock CTC RESULT line is malformed")
    groups = match.groupdict()
    return {
        "status": groups["status"],
        "iterations": int(groups["iterations"]),
        "output_bytes": int(groups["output_bytes"]),
        "output_sha256": groups["output_sha256"],
        "average_ms": float(groups["average_ms"]),
        "cleanup": int(groups["cleanup"]),
    }


def validate_run_id(run_id: str) -> str:
    if not isinstance(run_id, str) or RUN_ID_RE.fullmatch(run_id) is None:
        raise RunDirectoryError(f"run ID must match {RUN_ID_RE.pattern}")
    return run_id


def create_run_directory(artifact_root: Path, run_id: str) -> Path:
    validate_run_id(run_id)
    try:
        artifact_root.mkdir(parents=True, exist_ok=True)
        run_directory = artifact_root / run_id
        run_directory.mkdir(mode=0o750, exist_ok=False)
    except FileExistsError as exc:
        raise RunDirectoryError(
            f"run directory already exists; refusing overwrite: "
            f"{artifact_root / run_id}"
        ) from exc
    except OSError as exc:
        raise RunDirectoryError(f"cannot create run directory: {exc}") from exc
    return run_directory


def _write_text(path: Path, text: str) -> None:
    bounded = bounded_text(text)
    with path.open("x", encoding="utf-8", newline="") as handle:
        handle.write(bounded)


def _write_json_atomic(path: Path, value: Any, *, overwrite: bool = False) -> None:
    if path.exists() and not overwrite:
        raise RunDirectoryError(f"refusing to overwrite existing file: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary_path = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        if path.exists() and not overwrite:
            raise RunDirectoryError(f"refusing to overwrite existing file: {path}")
        os.replace(temporary_path, path)
    finally:
        try:
            temporary_path.unlink()
        except FileNotFoundError:
            pass


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _utc_timestamp() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace(
        "+00:00", "Z"
    )


def _git_identity(repository: Path) -> str:
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repository,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=True,
            timeout=5,
            text=True,
        ).stdout.strip()
        dirty = subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=repository,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=True,
            timeout=5,
            text=True,
        ).stdout
        return f"{commit}+dirty" if dirty else commit
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def collect_host_metadata(transport: Any) -> dict[str, str]:
    adb_result = transport.run(("version",), SNAPSHOT_COMMAND_TIMEOUT_MS)
    if adb_result.returncode == 0 and not adb_result.timed_out:
        adb_line = next(
            (line for line in adb_result.stdout.splitlines() if line.strip()),
            "unknown",
        )
    else:
        adb_line = "unavailable"
    return {
        "platform": platform.platform(),
        "python": sys.version.splitlines()[0],
        "adb": bounded_text(adb_line, 256),
    }


def _health_summary(
    before: Mapping[str, Any],
    after: Mapping[str, Any],
    *,
    timed_out: bool,
) -> dict[str, Any]:
    before_parameters = before.get("npu_parameters")
    after_parameters = after.get("npu_parameters")
    before_hashes = before.get("artifact_hashes")
    after_hashes = after.get("artifact_hashes")
    return {
        "adb_responsive": after.get("adb_state") == "device",
        "post_snapshot_complete": _snapshot_required_fields_complete(after),
        "npu_parameters_unchanged": (
            bool(before_parameters) and before_parameters == after_parameters
        ),
        "required_artifacts_unchanged": (
            bool(before_hashes) and before_hashes == after_hashes
        ),
        "galcore_present": (
            after.get("galcore_device", {}).get("status") == "observed"
        ),
        "timeout_requires_manual_holder_review": timed_out,
    }


def _semantic_matches(
    expected: ExpectedResult, observed: Mapping[str, Any]
) -> tuple[bool, str | None]:
    expected_dict = expected.as_dict()
    differences = [
        f"{key}: expected {expected_value!r}, observed {observed.get(key)!r}"
        for key, expected_value in expected_dict.items()
        if observed.get(key) != expected_value
    ]
    return (not differences, "; ".join(differences) if differences else None)


def run_experiment(
    *,
    registry: ExperimentRegistry,
    experiment_id: str,
    transport: Any,
    artifact_root: Path,
    run_id: str,
    git_commit: str | None = None,
    host_metadata: Mapping[str, str] | None = None,
) -> tuple[dict[str, Any], Path]:
    """Run one allowlisted experiment and persist its complete result envelope."""

    experiment = registry.get(experiment_id)
    run_directory = create_run_directory(artifact_root, run_id)

    stdout = ""
    stderr = ""
    before: dict[str, Any] = {}
    after: dict[str, Any] = {}
    preflight_hashes: dict[str, Any] = {}
    semantic_result: dict[str, Any] = {}
    execution: dict[str, Any] = {
        "classification": "internal-error",
        "adb_exit": None,
        "remote_exit": None,
        "duration_ms": 0,
        "timed_out": False,
        "reason": "run did not reach execution",
    }
    health: dict[str, Any] = {
        "adb_responsive": False,
        "post_snapshot_complete": False,
        "npu_parameters_unchanged": False,
        "required_artifacts_unchanged": False,
        "galcore_present": False,
        "timeout_requires_manual_holder_review": False,
    }

    try:
        before = collect_snapshot(transport, experiment)
        preflight_hashes = dict(before.get("artifact_hashes", {}))

        if before.get("adb_state") != "device":
            execution.update(
                classification="adb-unavailable",
                reason="ADB device was not available for the pre-run snapshot",
            )
        else:
            try:
                verify_preflight(experiment, before)
            except PreflightError as exc:
                execution.update(
                    classification="preflight-rejected",
                    reason=str(exc),
                )
            else:
                sentinel = (
                    f"__NPU_LAB_REMOTE_EXIT_{uuid.uuid4().hex}__"
                )
                remote_command = build_remote_command(experiment, sentinel)
                native = transport.run(
                    ("shell", remote_command), experiment.timeout_ms
                )
                stdout = native.stdout
                stderr = native.stderr
                execution.update(
                    adb_exit=native.returncode,
                    duration_ms=native.duration_ms,
                    timed_out=native.timed_out,
                )

                if native.timed_out:
                    execution.update(
                        classification="native-timeout",
                        reason=(
                            "host ADB timeout; no remote kill, NPU reset, or "
                            "driver recovery was attempted"
                        ),
                    )
                elif native.returncode != 0:
                    execution.update(
                        classification="adb-failure",
                        reason=f"adb shell exited {native.returncode}",
                    )
                else:
                    try:
                        stdout, remote_exit = parse_remote_exit(stdout, sentinel)
                    except ResultParseError as exc:
                        execution.update(
                            classification="adb-failure",
                            reason=str(exc),
                        )
                    else:
                        execution["remote_exit"] = remote_exit
                        if remote_exit != experiment.expected_remote_exit:
                            execution.update(
                                classification="native-failure",
                                reason=(
                                    f"remote exit {remote_exit}, expected "
                                    f"{experiment.expected_remote_exit}"
                                ),
                            )
                        else:
                            try:
                                semantic_result = parse_stock_result(stdout)
                            except ResultParseError as exc:
                                execution.update(
                                    classification="result-parse-failure",
                                    reason=str(exc),
                                )
                            else:
                                matches, mismatch_reason = _semantic_matches(
                                    experiment.expected_result, semantic_result
                                )
                                if matches:
                                    execution.update(
                                        classification="success", reason=None
                                    )
                                else:
                                    execution.update(
                                        classification="native-failure",
                                        reason=mismatch_reason,
                                    )

                # Always take a second bounded snapshot after native execution,
                # including failures and host timeouts.
                after = collect_snapshot(transport, experiment)
                health = _health_summary(
                    before,
                    after,
                    timed_out=bool(execution["timed_out"]),
                )
                if (
                    execution["classification"] == "success"
                    and not health["post_snapshot_complete"]
                ):
                    execution.update(
                        classification="post-snapshot-failure",
                        reason="required post-run health evidence is incomplete",
                    )

            if not after:
                # Preflight rejection did not execute native code, but a second
                # read-only snapshot proves that rejection left state intact.
                after = collect_snapshot(transport, experiment)
                health = _health_summary(before, after, timed_out=False)

    except Exception as exc:  # Envelope unexpected host-side failures.
        execution.update(
            classification="internal-error",
            reason=f"{type(exc).__name__}: {exc}",
        )
        if before and not after:
            try:
                after = collect_snapshot(transport, experiment)
                health = _health_summary(
                    before,
                    after,
                    timed_out=bool(execution.get("timed_out")),
                )
            except Exception as snapshot_exc:
                after = {
                    "adb_state": "unavailable",
                    "errors": [
                        f"post-snapshot internal error: "
                        f"{type(snapshot_exc).__name__}: {snapshot_exc}"
                    ],
                }

    stdout_path = run_directory / "stdout.txt"
    stderr_path = run_directory / "stderr.txt"
    _write_text(stdout_path, stdout)
    _write_text(stderr_path, stderr)

    repository = Path(__file__).resolve().parents[3]
    envelope: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "experiment_id": experiment_id,
        "run_id": run_id,
        "timestamp_utc": _utc_timestamp(),
        "git_commit": git_commit if git_commit is not None else _git_identity(repository),
        "host": (
            dict(host_metadata)
            if host_metadata is not None
            else collect_host_metadata(transport)
        ),
        "definition_sha256": registry.definition_sha256(experiment_id),
        "required_artifacts": experiment.required_artifacts,
        "preflight_hashes": preflight_hashes,
        "command": {
            "argv": list(experiment.remote_argv),
            "environment_allowlist": experiment.environment,
            "timeout_ms": experiment.timeout_ms,
        },
        "device_before": before,
        "execution": execution,
        "semantic_result": semantic_result,
        "device_after": after,
        "health": health,
        "artifacts": [
            {
                "kind": "stdout",
                "path": "stdout.txt",
                "bytes": stdout_path.stat().st_size,
                "sha256": _sha256_file(stdout_path),
            },
            {
                "kind": "stderr",
                "path": "stderr.txt",
                "bytes": stderr_path.stat().st_size,
                "sha256": _sha256_file(stderr_path),
            },
        ],
    }
    _write_json_atomic(run_directory / "envelope.json", envelope)
    return envelope, run_directory


def _projection(envelope: Mapping[str, Any]) -> dict[str, Any]:
    before = envelope.get("device_before", {})
    after = envelope.get("device_after", {})
    execution = envelope.get("execution", {})
    semantic = envelope.get("semantic_result", {})
    health = envelope.get("health", {})
    semantic_keys = (
        "status",
        "iterations",
        "output_bytes",
        "output_sha256",
        "cleanup",
    )
    health_keys = (
        "adb_responsive",
        "post_snapshot_complete",
        "npu_parameters_unchanged",
        "required_artifacts_unchanged",
        "galcore_present",
    )
    return {
        "experiment_id": envelope.get("experiment_id"),
        "definition_sha256": envelope.get("definition_sha256"),
        "required_artifacts": envelope.get("required_artifacts"),
        "preflight_hashes": envelope.get("preflight_hashes"),
        "device_before": {
            "kernel": before.get("kernel"),
            "npu_parameters": before.get("npu_parameters"),
        },
        "execution": {
            "classification": execution.get("classification"),
            "remote_exit": execution.get("remote_exit"),
        },
        "semantic_result": {key: semantic.get(key) for key in semantic_keys},
        "device_after": {
            "kernel": after.get("kernel"),
            "npu_parameters": after.get("npu_parameters"),
        },
        "health": {key: health.get(key) for key in health_keys},
    }


def _diff_values(
    left: Any,
    right: Any,
    path: str,
    differences: list[str],
) -> None:
    if len(differences) >= MAX_DIFFERENCES:
        return
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            child_path = f"{path}.{key}" if path else key
            if key not in left:
                differences.append(f"{child_path}: missing on left")
            elif key not in right:
                differences.append(f"{child_path}: missing on right")
            else:
                _diff_values(left[key], right[key], child_path, differences)
            if len(differences) >= MAX_DIFFERENCES:
                return
    elif left != right:
        differences.append(f"{path}: {left!r} != {right!r}")


def compare_envelopes(
    left: Mapping[str, Any], right: Mapping[str, Any]
) -> list[str]:
    differences: list[str] = []
    _diff_values(_projection(left), _projection(right), "", differences)
    return differences


def _read_envelope(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError) as exc:
        raise NpuLabError(f"cannot read envelope {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise NpuLabError(f"envelope {path} is not a JSON object")
    return value


def _default_run_id(experiment_id: str) -> str:
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dt%H%M%S%fz")
    return validate_run_id(f"{timestamp}-{experiment_id}"[:64])


def _build_argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run source-controlled, hash-pinned Wave NPU experiments. "
            "Arbitrary remote commands are intentionally unsupported."
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("list", help="list allowlisted experiments")

    snapshot = subparsers.add_parser(
        "snapshot", help="capture bounded read-only device evidence"
    )
    snapshot.add_argument("--experiment", required=True)
    snapshot.add_argument("--output", type=Path, required=True)

    run = subparsers.add_parser("run", help="run one allowlisted experiment")
    run.add_argument("experiment")
    run.add_argument(
        "--artifact-root", type=Path, default=Path("artifacts/npu")
    )
    run.add_argument("--run-id")

    compare = subparsers.add_parser(
        "compare", help="compare two envelopes semantically"
    )
    compare.add_argument("left", type=Path)
    compare.add_argument("right", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_argument_parser()
    args = parser.parse_args(argv)

    try:
        registry = load_registry()
        if args.command == "list":
            for experiment_id in sorted(registry.experiments):
                experiment = registry.get(experiment_id)
                print(
                    f"{experiment_id}\t{experiment.timeout_ms} ms\t"
                    f"{experiment.description}"
                )
            return 0

        if args.command == "snapshot":
            experiment = registry.get(args.experiment)
            snapshot = collect_snapshot(AdbTransport(), experiment)
            payload = {
                "schema_version": SCHEMA_VERSION,
                "experiment_id": args.experiment,
                "timestamp_utc": _utc_timestamp(),
                "definition_sha256": registry.definition_sha256(args.experiment),
                "snapshot": snapshot,
            }
            _write_json_atomic(args.output, payload)
            print(args.output)
            return 0 if snapshot.get("adb_state") == "device" else 2

        if args.command == "run":
            run_id = args.run_id or _default_run_id(args.experiment)
            envelope, run_directory = run_experiment(
                registry=registry,
                experiment_id=args.experiment,
                transport=AdbTransport(),
                artifact_root=args.artifact_root,
                run_id=run_id,
            )
            classification = envelope["execution"]["classification"]
            print(f"{classification}: {run_directory / 'envelope.json'}")
            return 0 if classification == "success" else 2

        if args.command == "compare":
            left = _read_envelope(args.left)
            right = _read_envelope(args.right)
            differences = compare_envelopes(left, right)
            if not differences:
                print("equivalent")
                return 0
            print(f"not equivalent ({len(differences)} difference(s)):")
            for difference in differences:
                print(f"- {difference}")
            return 1

        parser.error(f"unsupported command: {args.command}")
        return 2
    except NpuLabError as exc:
        print(f"npu-lab: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
