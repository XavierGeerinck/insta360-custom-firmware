#!/usr/bin/env python3
"""Fixed read-only Wave OpenVX hardware-capability harness."""

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
DEFAULT_MANIFEST = Path(__file__).with_name("caps_manifest.json")
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"
REMOTE_EXECUTABLE = "/tmp/wave-npu-compile/bin/wave-npu-query-caps-v1"
EXPECTED_ENVIRONMENT = {"LD_LIBRARY_PATH": "/usr/lib"}
EXPECTED_TIMEOUT_MS = 12000
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
CAPS_EXIT_RE = re.compile(r"^__WAVE_CAPS_EXIT_[0-9a-f]{32}__$")
CAPS_RE = re.compile(
    r"^CAPS status=PASS mode=query-only "
    r"ext_bytes=(?P<ext_bytes>[0-9]+) "
    r"ext2_bytes=(?P<ext2_bytes>[0-9]+) "
    r"eco_id=(?P<eco_id>0x[0-9a-f]{8}) "
    r"customer_id=(?P<customer_id>0x[0-9a-f]{8}) "
    r"evis1=(?P<evis1>[0-9]+) "
    r"evis2=(?P<evis2>[0-9]+) "
    r"core_count=(?P<core_count>[0-9]+) "
    r"subgroup_size=(?P<subgroup_size>[0-9]+) "
    r"support_va40=(?P<support_va40>[0-9]+) "
    r"support_stream_processor=(?P<support_stream_processor>[0-9]+) "
    r"stream_processor_exec_count=(?P<stream_processor_exec_count>[0-9]+) "
    r"stream_processor_vector_size=(?P<stream_processor_vector_size>[0-9]+)$"
)


class CapsLabError(Exception):
    """Expected capability-lab failure."""


class CapsManifestError(CapsLabError):
    """The capability manifest violates fixed policy."""


class CapsPreflightError(CapsLabError):
    """The live device differs from the capability-query allowlist."""


class CapsResultError(CapsLabError):
    """The capability result is missing, malformed, or inconsistent."""


@dataclass(frozen=True)
class CapsManifest:
    serial: str
    kernel: str
    galcore_major_minor: str
    npu_parameters: Mapping[str, str]
    environment: Mapping[str, str]
    remote_executable: str
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    expected_identity: Mapping[str, str]
    sha256: str
    candidates: Mapping[str, Any]


def _without_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CapsManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _keys(value: Any, expected: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CapsManifestError(f"{context} must be an object")
    missing = expected - set(value)
    unknown = set(value) - expected
    if missing or unknown:
        raise CapsManifestError(
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
        raise CapsManifestError(f"{context} must be a nonempty plain string")
    return value


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def load_manifest(path: Path = DEFAULT_MANIFEST) -> CapsManifest:
    try:
        document = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_without_duplicate_keys,
        )
    except CapsManifestError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise CapsManifestError(f"cannot load {path}: {exc}") from exc

    root = _keys(
        document,
        {
            "schema_version",
            "device",
            "environment",
            "remote_executable",
            "timeout_ms",
            "required_artifacts",
            "expected_identity",
        },
        "manifest",
    )
    if root["schema_version"] != SCHEMA_VERSION:
        raise CapsManifestError("unsupported schema_version")

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
    environment = _keys(
        root["environment"], {"LD_LIBRARY_PATH"}, "environment"
    )
    environment = {
        key: _plain_string(value, f"environment.{key}")
        for key, value in environment.items()
    }
    if environment != EXPECTED_ENVIRONMENT:
        raise CapsManifestError("environment differs from fixed policy")

    executable = _plain_string(root["remote_executable"], "remote_executable")
    if executable != REMOTE_EXECUTABLE:
        raise CapsManifestError("remote executable differs from fixed policy")
    timeout_ms = root["timeout_ms"]
    if (
        isinstance(timeout_ms, bool)
        or not isinstance(timeout_ms, int)
        or timeout_ms != EXPECTED_TIMEOUT_MS
    ):
        raise CapsManifestError("timeout_ms differs from fixed policy")

    raw_artifacts = root["required_artifacts"]
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise CapsManifestError("required_artifacts must be a nonempty object")
    artifacts: dict[str, str] = {}
    for raw_path, raw_digest in raw_artifacts.items():
        artifact_path = _plain_string(raw_path, "artifact path")
        digest = _plain_string(raw_digest, f"artifact digest {artifact_path}")
        if not artifact_path.startswith("/") or SHA256_RE.fullmatch(digest) is None:
            raise CapsManifestError(f"invalid artifact allowlist: {artifact_path}")
        artifacts[artifact_path] = digest
    if artifacts.get(REMOTE_EXECUTABLE) is None:
        raise CapsManifestError("probe executable is not hash-pinned")

    identity = _keys(
        root["expected_identity"],
        {"chip_model", "chip_product_id", "chip_eco_id", "customer_id"},
        "expected_identity",
    )
    expected_identity = {
        key: _plain_string(value, f"expected_identity.{key}")
        for key, value in identity.items()
    }
    fixed_identity = {
        "chip_model": "0x00009000",
        "chip_product_id": "0x05090001",
        "chip_eco_id": "0x04000000",
        "customer_id": "0x1000000e",
    }
    if expected_identity != fixed_identity:
        raise CapsManifestError("expected identity differs from Wave policy")

    serial = _plain_string(device["serial"], "device.serial")
    kernel = _plain_string(device["kernel"], "device.kernel")
    galcore = _plain_string(
        device["galcore_major_minor"], "device.galcore_major_minor"
    )
    if serial != "PSP_0000000000000000" or galcore != "199:0":
        raise CapsManifestError("device identity differs from fixed policy")

    return CapsManifest(
        serial=serial,
        kernel=kernel,
        galcore_major_minor=galcore,
        npu_parameters=parameters,
        environment=environment,
        remote_executable=executable,
        timeout_ms=timeout_ms,
        required_artifacts=artifacts,
        expected_identity=expected_identity,
        sha256=_canonical_sha256(root),
        candidates={},
    )


def verify_preflight(
    manifest: CapsManifest, snapshot: Mapping[str, Any]
) -> None:
    if snapshot.get("errors"):
        raise CapsPreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise CapsPreflightError("authorized device is not exclusively connected")
    if snapshot.get("kernel") != manifest.kernel:
        raise CapsPreflightError("kernel identity mismatch")
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or npu_compile_lab.BOOT_ID_RE.fullmatch(boot_id) is None
    ):
        raise CapsPreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != dict(manifest.npu_parameters):
        raise CapsPreflightError("NPU parameters differ from baseline")
    metadata = snapshot.get("galcore_device") or ""
    if re.search(r"\b199,\s*0\b", metadata) is None:
        raise CapsPreflightError("/dev/galcore is not character device 199:0")
    if snapshot.get("galcore_holders"):
        raise CapsPreflightError("/dev/galcore is already held")
    for path, expected in manifest.required_artifacts.items():
        observed = snapshot.get("artifact_hashes", {}).get(path)
        if observed != expected:
            raise CapsPreflightError(
                f"artifact mismatch: {path}; expected {expected}, "
                f"observed {observed}"
            )
    if not snapshot.get("dmesg_tail"):
        raise CapsPreflightError("kernel diagnostic baseline is empty")


def build_remote_command(manifest: CapsManifest, sentinel: str) -> str:
    if CAPS_EXIT_RE.fullmatch(sentinel) is None:
        raise ValueError("invalid internal sentinel")
    invocation = [
        "env",
        "-i",
        "PATH=/bin:/usr/bin",
        "LC_ALL=C",
        f"LD_LIBRARY_PATH={manifest.environment['LD_LIBRARY_PATH']}",
        manifest.remote_executable,
        "--query",
    ]
    command = " ".join(shlex.quote(token) for token in invocation)
    return (
        f"{command}\n"
        "wave_caps_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_caps_rc\"\n"
        "exit \"$wave_caps_rc\"\n"
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(
        rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$", re.MULTILINE
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise CapsResultError(
            f"expected one remote-exit sentinel, observed {len(matches)}"
        )
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise CapsResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    return (stdout[:start] + stdout[end:]).strip(), remote_exit


def parse_result(
    stdout: str, manifest: CapsManifest
) -> dict[str, Mapping[str, Any]]:
    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    caps_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("CAPS ")
    ]
    if len(identity_lines) != 1 or len(caps_lines) != 1:
        raise CapsResultError(
            "expected exactly one identity and one capability line"
        )
    identity_match = npu_compile_lab.IDENTITY_RE.fullmatch(identity_lines[0])
    caps_match = CAPS_RE.fullmatch(caps_lines[0])
    if identity_match is None or caps_match is None:
        raise CapsResultError("identity or capability line is malformed")
    identity = identity_match.groupdict()
    caps_raw = caps_match.groupdict()
    for key, expected in manifest.expected_identity.items():
        if identity[key] != expected:
            raise CapsResultError(f"live identity mismatch: {key}")
    if caps_raw["eco_id"] != manifest.expected_identity["chip_eco_id"]:
        raise CapsResultError("capability ECO ID mismatch")
    if caps_raw["customer_id"] != manifest.expected_identity["customer_id"]:
        raise CapsResultError("capability customer ID mismatch")

    integer_fields = {
        key: int(value)
        for key, value in caps_raw.items()
        if key not in {"eco_id", "customer_id"}
    }
    if integer_fields["ext_bytes"] != 32 or integer_fields["ext2_bytes"] != 40:
        raise CapsResultError("capability ABI size mismatch")
    for key in ("evis1", "evis2", "support_va40", "support_stream_processor"):
        if integer_fields[key] not in {0, 1}:
            raise CapsResultError(f"non-boolean capability field: {key}")
    bounded = {
        "core_count": 64,
        "subgroup_size": 4096,
        "stream_processor_exec_count": 65536,
        "stream_processor_vector_size": 1 << 24,
    }
    for key, maximum in bounded.items():
        if not 0 <= integer_fields[key] <= maximum:
            raise CapsResultError(f"capability field outside bound: {key}")
    return {
        "identity": identity,
        "capabilities": {
            **integer_fields,
            "eco_id": caps_raw["eco_id"],
            "customer_id": caps_raw["customer_id"],
            "shader_selector_eligible": (
                integer_fields["subgroup_size"] > 0
                and (integer_fields["evis1"] ^ integer_fields["evis2"]) == 1
            ),
        },
    }


def run_query(
    transport: Any,
    manifest: CapsManifest,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise CapsLabError(f"run ID must match {RUN_ID_RE.pattern}")
    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": "wave-npu-poc5d-hardware-caps-query-only",
        "run_id": run_id,
        "started_at": npu_compile_lab._timestamp(),
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
        "git_commit": npu_compile_lab._git_commit(),
        "status": "INCOMPLETE",
        "errors": [],
        "health_differences": [],
        "kernel_alerts": [],
        "command": {
            "remote_executable": manifest.remote_executable,
            "argument": "--query",
            "timeout_ms": manifest.timeout_ms,
        },
        "result": None,
    }

    preflight = npu_compile_lab.collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(
        run_directory / "preflight.json", preflight
    )
    try:
        verify_preflight(manifest, preflight)
    except CapsPreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = npu_compile_lab._timestamp()
        npu_compile_lab._write_json_exclusive(
            run_directory / "run.json", envelope
        )
        return envelope, run_directory

    sentinel = f"__WAVE_CAPS_EXIT_{uuid.uuid4().hex}__"
    remote_command = build_remote_command(manifest, sentinel)
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

    postflight = npu_compile_lab.collect_snapshot(transport, manifest)
    npu_compile_lab._write_json_exclusive(
        run_directory / "postflight.json", postflight
    )
    differences, alerts = npu_compile_lab.compare_health(
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
            if command_result.returncode != remote_exit:
                raise CapsResultError(
                    "ADB return code differs from recovered remote exit"
                )
            if remote_exit != 0:
                raise CapsResultError(f"native capability probe exited {remote_exit}")
            envelope["result"] = parse_result(cleaned_stdout, manifest)
        except CapsResultError as exc:
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
            "Fixed read-only Wave NPU PoC 5D capability query; "
            "never creates or executes a graph"
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
                        "mode": "query-only",
                        "creates_graph": False,
                        "executes_graph": False,
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        if args.command == "run":
            envelope, run_directory = run_query(
                npu_lab.AdbTransport(), manifest, args.run_id
            )
            print(
                json.dumps(
                    {
                        "status": envelope["status"],
                        "run_directory": run_directory.relative_to(
                            REPOSITORY_ROOT
                        ).as_posix(),
                        "errors": envelope["errors"],
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0 if envelope["status"] == "PASS" else 1
        raise AssertionError(args.command)
    except CapsLabError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
