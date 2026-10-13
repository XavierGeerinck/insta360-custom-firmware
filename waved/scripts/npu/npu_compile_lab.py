#!/usr/bin/env python3
"""Allowlisted compile-only harness core for Wave NPU compiler gates.

This module has no arbitrary remote-command, executable, model, output-path,
environment, or retry interface. Each entry point selects one immutable
source-controlled policy and its adjacent manifest.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shlex
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

import nbg_format
import npu_lab


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
MANIFEST_DIRECTORY = Path(__file__).resolve().parent
DEFAULT_RUN_ROOT = REPOSITORY_ROOT / "artifacts/npu/runs"

REMOTE_ROOT = "/tmp/wave-npu-compile"
EXPECTED_ENVIRONMENT = {
    "LD_LIBRARY_PATH": f"{REMOTE_ROOT}/lib:/usr/lib",
    "VIVANTE_SDK_DIR": f"{REMOTE_ROOT}/sdk",
    "VSI_NN_LOG_LEVEL": "5",
}

SNAPSHOT_TIMEOUT_MS = 5000
PULL_TIMEOUT_MS = 10000
MAX_DMESG_LINES = 200
MAX_SNAPSHOT_LINES = 32
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
BOOT_ID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-"
    r"[0-9a-f]{4}-[0-9a-f]{12}$"
)
RUN_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
KERNEL_ALERT_RE = re.compile(
    r"(warning|error|fault|timeout|timed out|(?<![a-z])hang|recover|exception|mmu|panic|oops|BUG:)",
    re.IGNORECASE,
)

IDENTITY_RE = re.compile(
    r"^IDENTITY "
    r"chip_model=(?P<chip_model>0x[0-9a-f]{8}) "
    r"chip_revision=(?P<chip_revision>0x[0-9a-f]{8}) "
    r"chip_product_id=(?P<chip_product_id>0x[0-9a-f]{8}) "
    r"chip_eco_id=(?P<chip_eco_id>0x[0-9a-f]{8}) "
    r"customer_id=(?P<customer_id>0x[0-9a-f]{8}) "
    r"reserved=(?P<reserved>0x[0-9a-f]{8}) "
    r"flags=(?P<flags>0x[0-9a-f]{16})$"
)
RESULT_RE = re.compile(
    r"^RESULT status=PASS mode=compile-only target=wave "
    r"candidate=(?P<candidate>[a-z0-9][a-z0-9-]{0,31}) "
    r"bytes=(?P<bytes>[0-9]+) "
    r"version=(?P<version>0x[0-9a-f]{8}) "
    r"product=(?P<product>0x[0-9a-f]{8}) "
    r"output=(?P<output>/tmp/wave-npu-[a-z0-9-]+/output/"
    r"[a-z0-9][a-z0-9.-]{0,95}\.nbg)$"
)
SELECTOR_RE = re.compile(
    r"Instance (?P<backend>CPU|EVIS|CL|OPENVX|SP) node with kernel "
    r'"(?P<kernel>[a-z0-9_]+)"'
)


class CompileLabError(Exception):
    """Expected compile-lab failure."""


class ManifestError(CompileLabError):
    """The source-controlled manifest violates policy."""


class PreflightError(CompileLabError):
    """The device is not in the exact allowlisted starting state."""


class ResultError(CompileLabError):
    """The native compiler result is missing, malformed, or inconsistent."""


@dataclass(frozen=True)
class Candidate:
    candidate_id: str
    argument: str
    output: str
    partial_output: str


@dataclass(frozen=True)
class TensorExpectation:
    dimensions: tuple[int, ...]
    data_format_name: str
    byte_size: int
    quantization_formats: tuple[str, ...]
    quantization_scale: float | None = None
    quantization_zero_point: int | None = None


@dataclass(frozen=True)
class CompilePolicy:
    poc_id: str
    experiment: str
    operation: str
    manifest_path: Path
    remote_executable: str
    candidates: Mapping[str, Mapping[str, str]]
    inputs: tuple[TensorExpectation, ...]
    outputs: tuple[TensorExpectation, ...]
    selector_backend: str | None
    selector_kernel: str | None
    absent_artifacts: tuple[str, ...] = ()

    @property
    def input_count(self) -> int:
        return len(self.inputs)

    @property
    def output_count(self) -> int:
        return len(self.outputs)

    @property
    def remote_root(self) -> str:
        marker = "/bin/"
        if marker not in self.remote_executable:
            raise ValueError("fixed compiler executable has no /bin/ root")
        return self.remote_executable.split(marker, 1)[0]

    @property
    def expected_environment(self) -> dict[str, str]:
        return {
            "LD_LIBRARY_PATH": f"{self.remote_root}/lib:/usr/lib",
            "VIVANTE_SDK_DIR": f"{self.remote_root}/sdk",
            "VSI_NN_LOG_LEVEL": "5",
        }


def _fixed_candidates(
    operation_slug: str, remote_root: str = REMOTE_ROOT
) -> dict[str, dict[str, str]]:
    return {
        "candidate-1": {
            "argument": "--candidate-1",
            "output": (f"{remote_root}/output/wave-{operation_slug}-candidate-1.nbg"),
            "partial_output": (
                f"{remote_root}/output/.wave-{operation_slug}-candidate-1.nbg.partial"
            ),
        },
        "candidate-2": {
            "argument": "--candidate-2",
            "output": (f"{remote_root}/output/wave-{operation_slug}-candidate-2.nbg"),
            "partial_output": (
                f"{remote_root}/output/.wave-{operation_slug}-candidate-2.nbg.partial"
            ),
        },
    }


ADD_POLICY = CompilePolicy(
    poc_id="5B",
    experiment="wave-npu-poc5b-compile-only",
    operation="Add",
    manifest_path=MANIFEST_DIRECTORY / "compile_manifest.json",
    remote_executable=f"{REMOTE_ROOT}/bin/wave-npu-compile-add-v2",
    candidates=_fixed_candidates("add"),
    inputs=(
        TensorExpectation((2,), "uint8", 2, ("affine-asymmetric",)),
        TensorExpectation((2,), "uint8", 2, ("affine-asymmetric",)),
    ),
    outputs=(TensorExpectation((2,), "uint8", 2, ("affine-asymmetric",)),),
    selector_backend=None,
    selector_kernel=None,
)
ERF_POLICY = CompilePolicy(
    poc_id="5C",
    experiment="wave-npu-poc5c-embedded-erf-compile-only",
    operation="Erf",
    manifest_path=MANIFEST_DIRECTORY / "compile_erf_manifest.json",
    remote_executable=f"{REMOTE_ROOT}/bin/wave-npu-compile-erf-v1",
    candidates=_fixed_candidates("erf"),
    inputs=(TensorExpectation((4,), "uint8", 4, ("affine-asymmetric",)),),
    outputs=(TensorExpectation((4,), "uint8", 4, ("affine-asymmetric",)),),
    selector_backend="OPENVX",
    selector_kernel="erf",
)
SIGNAL_FRAME_POLICY = CompilePolicy(
    poc_id="5E",
    experiment="wave-npu-poc5e-signal-frame-evis-compile-only",
    operation="SignalFrame",
    manifest_path=MANIFEST_DIRECTORY / "compile_signal_frame_manifest.json",
    remote_executable=(f"{REMOTE_ROOT}/bin/wave-npu-compile-signal-frame-v1"),
    candidates=_fixed_candidates("signal-frame"),
    inputs=(TensorExpectation((32,), "fp16", 64, ("none",)),),
    outputs=(TensorExpectation((8, 7), "fp16", 112, ("none",)),),
    selector_backend="EVIS",
    selector_kernel="signal_frame",
)
SIGNAL_FRAME_CLC_POLICY = CompilePolicy(
    poc_id="5F-B",
    experiment="wave-npu-poc5f-b-signal-frame-clc-compile-only",
    operation="SignalFrame",
    manifest_path=(MANIFEST_DIRECTORY / "compile_signal_frame_clc_manifest.json"),
    remote_executable=(f"{REMOTE_ROOT}/bin/wave-npu-compile-signal-frame-clc-v1"),
    candidates=_fixed_candidates("signal-frame-clc"),
    inputs=(TensorExpectation((32,), "fp16", 64, ("none",)),),
    outputs=(TensorExpectation((8, 7), "fp16", 112, ("none",)),),
    selector_backend="EVIS",
    selector_kernel="signal_frame",
)
AUDIO_DENOISER_ROOT = "/tmp/wave-npu-compile8"
AUDIO_DENOISER_CANDIDATES = {
    "candidate-1": _fixed_candidates("audio-denoiser", AUDIO_DENOISER_ROOT)[
        "candidate-1"
    ]
}
AUDIO_DENOISER_POLICY = CompilePolicy(
    poc_id="8A",
    experiment="wave-npu-poc8a-audio-denoiser-compile-only",
    operation="MatmulAudioDenoiser",
    manifest_path=MANIFEST_DIRECTORY / "audio_denoiser_manifest.json",
    remote_executable=(f"{AUDIO_DENOISER_ROOT}/bin/wave-npu-compile-audio-denoiser-v1"),
    candidates=AUDIO_DENOISER_CANDIDATES,
    inputs=(TensorExpectation((16, 1), "fp32", 64, ("none",)),),
    outputs=(TensorExpectation((16, 1), "fp32", 64, ("none",)),),
    selector_backend="OPENVX",
    selector_kernel="matrixmul",
    absent_artifacts=(
        "/lib/libCLC.so",
        f"{AUDIO_DENOISER_ROOT}/lib/libCLC.so",
        "/usr/lib/libCLC.so",
    ),
)
CONV_AUDIO_DENOISER_ROOT = "/tmp/wave-npu-compile8c"
CONV_AUDIO_DENOISER_CANDIDATES = {
    "candidate-1": _fixed_candidates("conv-audio-denoiser", CONV_AUDIO_DENOISER_ROOT)[
        "candidate-1"
    ]
}
CONV_AUDIO_DENOISER_POLICY = CompilePolicy(
    poc_id="8C",
    experiment="wave-npu-poc8c-conv-audio-denoiser-compile-only",
    operation="Conv2dAudioDenoiser",
    manifest_path=(MANIFEST_DIRECTORY / "conv_audio_denoiser_manifest.json"),
    remote_executable=(
        f"{CONV_AUDIO_DENOISER_ROOT}/bin/wave-npu-compile-conv-audio-denoiser-v1"
    ),
    candidates=CONV_AUDIO_DENOISER_CANDIDATES,
    inputs=(TensorExpectation((16, 1, 1, 1), "fp32", 64, ("none",)),),
    outputs=(TensorExpectation((16, 1, 1, 1), "fp32", 64, ("none",)),),
    selector_backend="OPENVX",
    selector_kernel="conv2d",
    absent_artifacts=(
        "/lib/libCLC.so",
        f"{CONV_AUDIO_DENOISER_ROOT}/lib/libCLC.so",
        "/usr/lib/libCLC.so",
    ),
)
ERF_MASK_ROOT = "/tmp/wave-npu-poc13b"
ERF_MASK_CANDIDATES = {
    "a1": {
        "argument": "--a1",
        "output": f"{ERF_MASK_ROOT}/output/erf-mask-a1.nbg",
        "partial_output": f"{ERF_MASK_ROOT}/output/.erf-mask-a1.nbg.partial",
    },
    "a2": {
        "argument": "--a2",
        "output": f"{ERF_MASK_ROOT}/output/erf-mask-a2.nbg",
        "partial_output": f"{ERF_MASK_ROOT}/output/.erf-mask-a2.nbg.partial",
    },
}
ERF_MASK_POLICY = CompilePolicy(
    poc_id="13B",
    experiment="wave-npu-poc13b-erf-mask-compile-only",
    operation="ErfMask481",
    manifest_path=MANIFEST_DIRECTORY / "erf_mask_compile_manifest.json",
    remote_executable=(f"{ERF_MASK_ROOT}/bin/wave-npu-compile-erf-mask-v1"),
    candidates=ERF_MASK_CANDIDATES,
    inputs=(
        TensorExpectation(
            (481,),
            "uint8",
            481,
            ("affine-asymmetric",),
            0.03125,
            128,
        ),
    ),
    outputs=(
        TensorExpectation(
            (481,),
            "uint8",
            481,
            ("affine-asymmetric",),
            0.0078125,
            128,
        ),
    ),
    selector_backend="OPENVX",
    selector_kernel="erf",
    absent_artifacts=(
        "/lib/libCLC.so",
        f"{ERF_MASK_ROOT}/lib/libCLC.so",
        "/usr/lib/libCLC.so",
    ),
)
ERF_AFFINE_ROOT = "/tmp/wave-npu-poc14b"
ERF_AFFINE_CANDIDATES = {
    candidate: {
        "argument": f"--{candidate}",
        "output": (f"{ERF_AFFINE_ROOT}/output/erf-affine-{candidate}.nbg"),
        "partial_output": (
            f"{ERF_AFFINE_ROOT}/output/.erf-affine-{candidate}.nbg.partial"
        ),
    }
    for candidate in ("a1", "a2", "b1", "b2", "c1", "c2")
}
ERF_AFFINE_POLICY = CompilePolicy(
    poc_id="14B",
    experiment="wave-npu-poc14b-erf-affine-compile-only",
    operation="AffineErf481",
    manifest_path=MANIFEST_DIRECTORY / "erf_affine_compile_manifest.json",
    remote_executable=(f"{ERF_AFFINE_ROOT}/bin/wave-npu-compile-erf-affine-v1"),
    candidates=ERF_AFFINE_CANDIDATES,
    inputs=(
        TensorExpectation(
            (481,),
            "uint8",
            481,
            ("affine-asymmetric",),
            0.0625,
            128,
        ),
    ),
    outputs=(
        TensorExpectation(
            (481,),
            "uint8",
            481,
            ("affine-asymmetric",),
            0.0078125,
            128,
        ),
    ),
    selector_backend="OPENVX",
    selector_kernel="erf",
    absent_artifacts=(
        "/lib/libCLC.so",
        f"{ERF_AFFINE_ROOT}/lib/libCLC.so",
        "/usr/lib/libCLC.so",
    ),
)

# Backward-compatible names identify the original PoC 5B entry point. The
# PoC 5C wrapper passes ERF_POLICY explicitly and exposes no policy selector.
DEFAULT_MANIFEST = ADD_POLICY.manifest_path
REMOTE_EXECUTABLE = ADD_POLICY.remote_executable
ALLOWED_CANDIDATES = ADD_POLICY.candidates


@dataclass(frozen=True)
class CompileManifest:
    policy: CompilePolicy
    serial: str
    kernel: str
    galcore_major_minor: str
    npu_parameters: Mapping[str, str]
    environment: Mapping[str, str]
    remote_executable: str
    timeout_ms: int
    required_artifacts: Mapping[str, str]
    candidates: Mapping[str, Candidate]
    expected_nbg: Mapping[str, Any]
    sha256: str


def _without_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _keys(value: Any, expected: set[str], context: str) -> dict[str, Any]:
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


def _string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value or re.search(r"[\x00-\x1f]", value):
        raise ManifestError(f"{context} must be a nonempty plain string")
    return value


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def load_manifest(
    path: Path | None = None,
    policy: CompilePolicy = ADD_POLICY,
) -> CompileManifest:
    if path is None:
        path = policy.manifest_path
    try:
        document = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_without_duplicate_keys,
        )
    except ManifestError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot load {path}: {exc}") from exc

    root = _keys(
        document,
        {
            "schema_version",
            "device",
            "environment",
            "remote_executable",
            "timeout_ms",
            "required_artifacts",
            "candidates",
            "expected_nbg",
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
    parameters = {
        name: _string(value, f"device.npu_parameters.{name}")
        for name, value in parameters.items()
    }

    environment = _keys(
        root["environment"],
        set(policy.expected_environment),
        "environment",
    )
    environment = {
        key: _string(value, f"environment.{key}") for key, value in environment.items()
    }
    if environment != policy.expected_environment:
        raise ManifestError(
            f"environment differs from the fixed PoC {policy.poc_id} policy"
        )

    executable = _string(root["remote_executable"], "remote_executable")
    if executable != policy.remote_executable:
        raise ManifestError("remote executable differs from fixed policy")

    timeout_ms = root["timeout_ms"]
    if (
        isinstance(timeout_ms, bool)
        or not isinstance(timeout_ms, int)
        or timeout_ms != 20000
    ):
        raise ManifestError("timeout_ms must be exactly 20000")

    raw_artifacts = root["required_artifacts"]
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise ManifestError("required_artifacts must be a nonempty object")
    artifacts: dict[str, str] = {}
    for raw_path, raw_digest in raw_artifacts.items():
        artifact_path = _string(raw_path, "required_artifacts path")
        digest = _string(raw_digest, f"required_artifacts[{artifact_path!r}]")
        if not artifact_path.startswith("/") or SHA256_RE.fullmatch(digest) is None:
            raise ManifestError(f"invalid artifact allowlist entry: {artifact_path}")
        if artifact_path.startswith("/tmp/") and not artifact_path.startswith(
            f"{policy.remote_root}/"
        ):
            raise ManifestError(f"temporary artifact is outside {policy.remote_root}")
        artifacts[artifact_path] = digest

    raw_candidates = _keys(root["candidates"], set(policy.candidates), "candidates")
    candidates: dict[str, Candidate] = {}
    for candidate_id, fixed in policy.candidates.items():
        raw = _keys(
            raw_candidates[candidate_id],
            {"argument", "output", "partial_output"},
            f"candidates.{candidate_id}",
        )
        observed = {
            key: _string(raw[key], f"candidates.{candidate_id}.{key}") for key in fixed
        }
        if observed != fixed:
            raise ManifestError(f"{candidate_id} differs from fixed policy")
        candidates[candidate_id] = Candidate(candidate_id, **observed)

    expected_nbg = _keys(
        root["expected_nbg"],
        {
            "magic",
            "minimum_bytes",
            "maximum_bytes",
            "version",
            "product",
            "chip_model",
        },
        "expected_nbg",
    )
    fixed_nbg = {
        "magic": "VPMN",
        "minimum_bytes": 513,
        "maximum_bytes": 64 * 1024 * 1024,
        "version": "0x00020000",
        "product": "0x1000000e",
        "chip_model": "0x00009000",
    }
    if expected_nbg != fixed_nbg:
        raise ManifestError(
            f"expected_nbg differs from fixed PoC {policy.poc_id} policy"
        )

    serial = _string(device["serial"], "device.serial")
    kernel = _string(device["kernel"], "device.kernel")
    galcore = _string(device["galcore_major_minor"], "device.galcore_major_minor")
    if galcore != "199:0":
        raise ManifestError("galcore major/minor must be 199:0")

    return CompileManifest(
        policy=policy,
        serial=serial,
        kernel=kernel,
        galcore_major_minor=galcore,
        npu_parameters=parameters,
        environment=environment,
        remote_executable=executable,
        timeout_ms=timeout_ms,
        required_artifacts=artifacts,
        candidates=candidates,
        expected_nbg=expected_nbg,
        sha256=_canonical_sha256(root),
    )


def _shell_value(
    transport: Any,
    command: str,
    label: str,
    errors: list[str],
) -> str | None:
    result = transport.run(("shell", command), SNAPSHOT_TIMEOUT_MS)
    if result.timed_out:
        errors.append(f"{label}: timed out")
        return None
    if result.returncode != 0:
        errors.append(f"{label}: adb exit {result.returncode}")
        return None
    return result.stdout.strip()


def _bounded_lines(text: str, limit: int) -> list[str]:
    return [line.rstrip() for line in text.splitlines() if line.strip()][-limit:]


def _device_serials(transport: Any, errors: list[str]) -> list[str]:
    result = transport.run(("devices", "-l"), SNAPSHOT_TIMEOUT_MS)
    if result.timed_out or result.returncode != 0:
        errors.append("adb devices failed")
        return []
    serials: list[str] = []
    for line in result.stdout.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 2 and fields[1] == "device":
            serials.append(fields[0])
    return serials


def _holder_snapshot(transport: Any, errors: list[str]) -> list[dict[str, int]]:
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
    output = _shell_value(transport, command, "galcore holder scan", errors)
    holders: list[dict[str, int]] = []
    for line in _bounded_lines(output or "", MAX_SNAPSHOT_LINES):
        match = re.fullmatch(r"([0-9]+):([0-9]+)", line)
        if match:
            holders.append({"pid": int(match[1]), "fd": int(match[2])})
    return holders


def _remote_file_state(transport: Any, path: str, errors: list[str]) -> dict[str, Any]:
    quoted = shlex.quote(path)
    command = (
        f"if [ -e {quoted} ]; then "
        f"bytes=$(wc -c < {quoted}) || exit 21; "
        f"permissions=$(ls -ldn {quoted} | awk '{{print $1}}') || exit 22; "
        f"digest=$(sha256sum {quoted} | cut -d ' ' -f 1) || exit 23; "
        "printf 'PRESENT %s %s %s\\n' "
        '"$bytes" "$permissions" "$digest"; '
        "else printf 'ABSENT\\n'; fi"
    )
    output = _shell_value(transport, command, f"file state {path}", errors)
    if output == "ABSENT":
        return {"status": "absent"}
    match = re.fullmatch(r"PRESENT ([0-9]+) (-[rwx-]{9}) ([0-9a-f]{64})", output or "")
    if match is None:
        errors.append(f"file state {path}: malformed")
        return {"status": "unknown"}
    return {
        "status": "present",
        "bytes": int(match[1]),
        "permissions": match[2],
        "sha256": match[3],
    }


def collect_snapshot(transport: Any, manifest: CompileManifest) -> dict[str, Any]:
    errors: list[str] = []
    snapshot: dict[str, Any] = {
        "serials": _device_serials(transport, errors),
        "kernel": None,
        "boot_id": None,
        "uptime": None,
        "galcore_device": None,
        "npu_parameters": {},
        "galcore_holders": [],
        "processes": [],
        "artifact_hashes": {},
        "absent_artifact_states": {},
        "output_states": {},
        "dmesg_tail": [],
        "errors": errors,
    }
    if snapshot["serials"] != [manifest.serial]:
        errors.append(
            f"connected device list is {snapshot['serials']!r}, "
            f"expected [{manifest.serial!r}]"
        )
        return snapshot

    snapshot["kernel"] = _shell_value(transport, "uname -a", "kernel", errors)
    snapshot["boot_id"] = _shell_value(
        transport,
        "cat /proc/sys/kernel/random/boot_id",
        "boot ID",
        errors,
    )
    snapshot["uptime"] = _shell_value(transport, "cat /proc/uptime", "uptime", errors)
    snapshot["galcore_device"] = _shell_value(
        transport, "ls -ln /dev/galcore", "/dev/galcore", errors
    )

    for name in ("nanoqFreq", "enableNN", "recovery"):
        path = f"/sys/module/galcore/parameters/{name}"
        snapshot["npu_parameters"][name] = _shell_value(
            transport, f"cat {shlex.quote(path)}", f"NPU parameter {name}", errors
        )

    snapshot["galcore_holders"] = _holder_snapshot(transport, errors)
    process_text = _shell_value(
        transport,
        (
            "ps 2>/dev/null | "
            "grep -E '([w]ave-npu-(compile-(add|erf|erf-affine|signal-frame|"
            "audio-denoiser|conv-audio-denoiser|fp16-conv|u8-fc)|"
            "onnx-(compile|execute)-v1|"
            "query-caps|clc-loader-probe|execute-erf-(zero|nonzero)|"
            "execute-(fp16-conv|u8-fc)|"
            "run-(audio-denoiser|conv-audio-denoiser|"
            "erf-mask-qualification)|soak-erf-mask)|"
            "[w]aved|[a]udioserver|[p]spapp)' | "
            "head -n 32 || true"
        ),
        "process scan",
        errors,
    )
    snapshot["processes"] = _bounded_lines(process_text or "", MAX_SNAPSHOT_LINES)

    for path in manifest.required_artifacts:
        quoted = shlex.quote(path)
        output = _shell_value(
            transport,
            f"sha256sum {quoted} 2>/dev/null || printf 'MISSING\\n'",
            f"artifact hash {path}",
            errors,
        )
        digest = (output or "").split()[0] if (output or "").split() else ""
        snapshot["artifact_hashes"][path] = (
            digest if SHA256_RE.fullmatch(digest) else None
        )

    absent_artifacts = getattr(
        getattr(manifest, "policy", None), "absent_artifacts", ()
    )
    for path in absent_artifacts:
        snapshot["absent_artifact_states"][path] = _remote_file_state(
            transport, path, errors
        )

    for candidate in manifest.candidates.values():
        snapshot["output_states"][candidate.output] = _remote_file_state(
            transport, candidate.output, errors
        )
        snapshot["output_states"][candidate.partial_output] = _remote_file_state(
            transport, candidate.partial_output, errors
        )

    dmesg = _shell_value(
        transport,
        f"dmesg 2>/dev/null | tail -n {MAX_DMESG_LINES}",
        "dmesg",
        errors,
    )
    snapshot["dmesg_tail"] = _bounded_lines(dmesg or "", MAX_DMESG_LINES)
    return snapshot


def verify_preflight(
    manifest: CompileManifest,
    candidate: Candidate,
    snapshot: Mapping[str, Any],
) -> None:
    if snapshot.get("errors"):
        raise PreflightError("; ".join(snapshot["errors"]))
    if snapshot.get("serials") != [manifest.serial]:
        raise PreflightError("authorized device is not the only connected device")
    if snapshot.get("kernel") != manifest.kernel:
        raise PreflightError("kernel identity mismatch")
    boot_id = snapshot.get("boot_id")
    if not isinstance(boot_id, str) or BOOT_ID_RE.fullmatch(boot_id) is None:
        raise PreflightError("boot ID is missing or malformed")
    if snapshot.get("npu_parameters") != dict(manifest.npu_parameters):
        raise PreflightError("NPU parameters differ from the allowlisted baseline")
    metadata = snapshot.get("galcore_device") or ""
    if re.search(r"\b199,\s*0\b", metadata) is None:
        raise PreflightError("/dev/galcore is not character device 199:0")
    if snapshot.get("galcore_holders"):
        raise PreflightError(
            f"/dev/galcore is already held: {snapshot['galcore_holders']}"
        )
    for path, expected in manifest.required_artifacts.items():
        observed = snapshot.get("artifact_hashes", {}).get(path)
        if observed != expected:
            raise PreflightError(
                f"artifact mismatch: {path}; expected {expected}, observed {observed}"
            )
    for path in manifest.policy.absent_artifacts:
        state = snapshot.get("absent_artifact_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise PreflightError(f"forbidden compiler artifact is visible: {path}")
    for path in (candidate.output, candidate.partial_output):
        state = snapshot.get("output_states", {}).get(path, {})
        if state.get("status") != "absent":
            raise PreflightError(f"selected output is not absent: {path}")
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")


def _new_dmesg_lines(
    before: Sequence[str], after: Sequence[str]
) -> tuple[list[str], bool]:
    if not before:
        return list(after), False
    marker = before[-1]
    matches = [index for index, line in enumerate(after) if line == marker]
    if not matches:
        return list(after), False
    return list(after[matches[-1] + 1 :]), True


def align_native_diagnostics(transport, manifest, before: dict, after: dict) -> None:
    """Preserve native ONNX markers when busy logs exceed the short snapshot."""
    if not getattr(manifest.policy, "poc_id", "").startswith(("17", "18")):
        return
    _, aligned = _new_dmesg_lines(before.get("dmesg_tail", []), after.get("dmesg_tail", []))
    if aligned or before.get("dmesg_tail") == after.get("dmesg_tail"):
        return
    text = _shell_value(transport, "dmesg 2>/dev/null | tail -n 16000",
                        "expanded native diagnostics", after["errors"])
    if text is not None:
        after["dmesg_tail"] = _bounded_lines(text, 16000)


def compare_health(
    manifest: CompileManifest,
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> tuple[list[str], list[str]]:
    differences: list[str] = []
    for key in (
        "serials",
        "kernel",
        "boot_id",
        "galcore_device",
        "npu_parameters",
        "artifact_hashes",
        "absent_artifact_states",
    ):
        if before.get(key) != after.get(key):
            differences.append(f"{key} changed")
    if after.get("errors"):
        differences.append("postflight snapshot errors: " + "; ".join(after["errors"]))
    if after.get("galcore_holders"):
        differences.append(
            f"postflight galcore holders remain: {after['galcore_holders']}"
        )
    if after.get("serials") != [manifest.serial]:
        differences.append("authorized device is no longer exclusively connected")

    new_lines, aligned = _new_dmesg_lines(
        before.get("dmesg_tail", []), after.get("dmesg_tail", [])
    )
    if not aligned and before.get("dmesg_tail") != after.get("dmesg_tail"):
        differences.append("could not align pre/post kernel diagnostics")
    alerts = [line for line in new_lines if KERNEL_ALERT_RE.search(line)]
    if alerts:
        differences.append("new kernel warning/error diagnostics were observed")
    if getattr(manifest.policy, "poc_id", "").startswith(("17", "18")):
        def services(snapshot):
            return sorted(
                re.sub(r"\s+", " ", line.strip())
                for line in snapshot.get("processes", [])
                if re.search(r"\bwaved\b|S60waved|audioserver|pspapp", line)
            )
        if services(before) != services(after):
            differences.append("service process identities changed")
    return differences, alerts


def build_remote_command(
    manifest: CompileManifest, candidate: Candidate, sentinel: str
) -> str:
    if re.fullmatch(r"__WAVE_COMPILE_EXIT_[0-9a-f]{32}__", sentinel) is None:
        raise ValueError("invalid internal sentinel")
    invocation = [
        "env",
        "-i",
        "PATH=/bin:/usr/bin",
        "LC_ALL=C",
        *(
            f"{key}={manifest.environment[key]}"
            for key in manifest.policy.expected_environment
        ),
        manifest.remote_executable,
        candidate.argument,
    ]
    command = " ".join(shlex.quote(token) for token in invocation)
    return (
        f"{command}\n"
        "wave_compile_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_compile_rc\"\n"
        'exit "$wave_compile_rc"\n'
    )


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    pattern = re.compile(rf"^{re.escape(sentinel)}:(?P<exit>[0-9]+)\r?$", re.MULTILINE)
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultError(f"expected one remote-exit sentinel, observed {len(matches)}")
    remote_exit = int(matches[0]["exit"])
    if not 0 <= remote_exit <= 255:
        raise ResultError("remote exit is outside 0..255")
    start, end = matches[0].span()
    cleaned = stdout[:start] + stdout[end:]
    return cleaned.strip(), remote_exit


def verify_transport_status(
    adb_returncode: int, remote_exit: int, process_name: str
) -> None:
    """Apply the observed legacy-shell status contract of the Wave.

    This device returns host ADB status zero even when the remote shell exits
    nonzero. The unpredictable, exactly-once sentinel is authoritative for
    the native process, while a nonzero host status remains a transport
    failure.
    """

    if adb_returncode != 0:
        raise ResultError(f"ADB transport exited {adb_returncode}")
    if remote_exit != 0:
        raise ResultError(f"{process_name} exited {remote_exit}")


def parse_native_result(
    stdout: str,
    manifest: CompileManifest,
    candidate: Candidate,
) -> dict[str, Any]:
    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    result_lines = [
        line.rstrip("\r") for line in stdout.splitlines() if line.startswith("RESULT ")
    ]
    if len(identity_lines) != 1 or len(result_lines) != 1:
        raise ResultError(
            f"expected one identity and result line; observed "
            f"{len(identity_lines)} and {len(result_lines)}"
        )
    identity_match = IDENTITY_RE.fullmatch(identity_lines[0])
    result_match = RESULT_RE.fullmatch(result_lines[0])
    if identity_match is None or result_match is None:
        raise ResultError("identity or result line is malformed")
    identity = identity_match.groupdict()
    result = result_match.groupdict()
    expected = manifest.expected_nbg
    if identity["chip_model"] != expected["chip_model"]:
        raise ResultError("live chip model mismatch")
    if identity["customer_id"] != expected["product"]:
        raise ResultError("live customer/target ID mismatch")
    if result["candidate"] != candidate.candidate_id:
        raise ResultError("candidate ID mismatch")
    if result["output"] != candidate.output:
        raise ResultError("fixed output path mismatch")
    if result["version"] != expected["version"]:
        raise ResultError("native NBG version mismatch")
    if result["product"] != expected["product"]:
        raise ResultError("native NBG product mismatch")
    byte_count = int(result["bytes"])
    if not expected["minimum_bytes"] <= byte_count <= expected["maximum_bytes"]:
        raise ResultError("native NBG size is outside policy")
    selection: dict[str, str] | None = None
    policy = manifest.policy
    if policy.selector_backend is not None:
        if policy.selector_kernel is None:
            raise ResultError("fixed selector policy is incomplete")
        selector_matches = [
            match.groupdict()
            for match in SELECTOR_RE.finditer(stdout)
            if match["kernel"] == policy.selector_kernel
        ]
        if len(selector_matches) != 1:
            raise ResultError(
                f"expected one {policy.selector_kernel} selector result, "
                f"observed {len(selector_matches)}"
            )
        selection = selector_matches[0]
        if selection["backend"] != policy.selector_backend:
            raise ResultError(
                f"{policy.selector_kernel} selected {selection['backend']}, "
                f"expected {policy.selector_backend}"
            )
    return {
        "identity": identity,
        "result": {**result, "bytes": byte_count},
        "selector": selection,
    }


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _write_json_exclusive(path: Path, value: Any) -> None:
    with path.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(value, handle, indent=2, sort_keys=True, ensure_ascii=False)
        handle.write("\n")


def _write_text_exclusive(path: Path, value: str) -> None:
    with path.open("x", encoding="utf-8", newline="") as handle:
        handle.write(npu_lab.bounded_text(value))


def _timestamp() -> str:
    return (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _git_commit() -> str | None:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPOSITORY_ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip()


def validate_nbg_report(report: Mapping[str, Any], manifest: CompileManifest) -> None:
    expected = manifest.expected_nbg
    if report.get("magic") != expected["magic"]:
        raise ResultError("strict parser magic mismatch")
    if report.get("version_hex") != expected["version"]:
        raise ResultError("strict parser version mismatch")
    if report.get("product_hex") != expected["product"]:
        raise ResultError("strict parser product mismatch")
    if (
        not expected["minimum_bytes"]
        <= report.get("size", 0)
        <= expected["maximum_bytes"]
    ):
        raise ResultError("strict parser size is outside policy")
    inputs = report.get("inputs")
    outputs = report.get("outputs")
    policy = manifest.policy
    if not isinstance(inputs, list) or len(inputs) != policy.input_count:
        raise ResultError(
            f"strict parser input count differs from fixed {policy.operation} policy"
        )
    if not isinstance(outputs, list) or len(outputs) != policy.output_count:
        raise ResultError(
            f"strict parser output count differs from fixed {policy.operation} policy"
        )
    for direction, tensors, tensor_policy in (
        ("input", inputs, policy.inputs),
        ("output", outputs, policy.outputs),
    ):
        for index, (tensor, expected_tensor) in enumerate(
            zip(tensors, tensor_policy, strict=True)
        ):
            label = f"{direction} {index}"
            if tensor.get("dimensions") != list(expected_tensor.dimensions):
                raise ResultError(f"strict parser {label} shape mismatch")
            if tensor.get("data_format_name") != expected_tensor.data_format_name:
                raise ResultError(f"strict parser {label} data type mismatch")
            if tensor.get("byte_size") != expected_tensor.byte_size:
                raise ResultError(f"strict parser {label} byte size mismatch")
            quantization = tensor.get("quantization")
            if not isinstance(quantization, dict):
                raise ResultError(f"strict parser {label} quantization is missing")
            if (
                quantization.get("format_name")
                not in expected_tensor.quantization_formats
            ):
                raise ResultError(f"strict parser {label} quantization mismatch")
            if (
                expected_tensor.quantization_scale is not None
                and quantization.get("scale") != expected_tensor.quantization_scale
            ):
                raise ResultError(f"strict parser {label} quantization scale mismatch")
            if (
                expected_tensor.quantization_zero_point is not None
                and quantization.get("zero_point")
                != expected_tensor.quantization_zero_point
            ):
                raise ResultError(
                    f"strict parser {label} quantization zero point mismatch"
                )


def pull_and_parse(
    transport: Any,
    manifest: CompileManifest,
    candidate: Candidate,
    run_directory: Path,
    native_result: Mapping[str, Any],
    postflight: Mapping[str, Any],
) -> tuple[dict[str, Any], dict[str, Any]]:
    output_state = postflight["output_states"][candidate.output]
    if output_state.get("status") != "present":
        raise ResultError("native pass did not leave the fixed output")
    if output_state.get("permissions") != "-r--r--r--":
        raise ResultError("candidate output is not read-only mode 0444")
    if output_state.get("bytes") != native_result["result"]["bytes"]:
        raise ResultError("remote file size differs from native result")

    local_path = run_directory / f"{candidate.candidate_id}.nbg"
    pull = transport.run(("pull", candidate.output, str(local_path)), PULL_TIMEOUT_MS)
    _write_text_exclusive(run_directory / "pull.stdout.txt", pull.stdout)
    _write_text_exclusive(run_directory / "pull.stderr.txt", pull.stderr)
    if pull.timed_out or pull.returncode != 0 or not local_path.is_file():
        raise ResultError("ADB pull failed")
    local_digest = _sha256_file(local_path)
    if local_digest != output_state.get("sha256"):
        raise ResultError("pulled NBG hash differs from the remote hash")
    if local_path.stat().st_size != output_state.get("bytes"):
        raise ResultError("pulled NBG size differs from the remote size")

    report = nbg_format.parse_path(
        local_path,
        display_path=local_path.relative_to(REPOSITORY_ROOT).as_posix(),
    )
    validate_nbg_report(report, manifest)
    _write_json_exclusive(run_directory / "nbg-report.json", report)
    return (
        {
            "path": local_path.relative_to(REPOSITORY_ROOT).as_posix(),
            "bytes": local_path.stat().st_size,
            "sha256": local_digest,
        },
        report,
    )


def run_candidate(
    transport: Any,
    manifest: CompileManifest,
    candidate_id: str,
    run_id: str,
) -> tuple[dict[str, Any], Path]:
    if RUN_ID_RE.fullmatch(run_id) is None:
        raise CompileLabError(f"run ID must match {RUN_ID_RE.pattern}")
    try:
        candidate = manifest.candidates[candidate_id]
    except KeyError as exc:
        raise CompileLabError(f"unknown candidate: {candidate_id}") from exc

    run_directory = npu_lab.create_run_directory(DEFAULT_RUN_ROOT, run_id)
    envelope: dict[str, Any] = {
        "schema_version": 1,
        "experiment": manifest.policy.experiment,
        "operation": manifest.policy.operation,
        "candidate": candidate_id,
        "run_id": run_id,
        "started_at": _timestamp(),
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
        "git_commit": _git_commit(),
        "status": "INCOMPLETE",
        "errors": [],
        "health_differences": [],
        "kernel_alerts": [],
        "command": {
            "remote_executable": manifest.remote_executable,
            "argument": candidate.argument,
            "timeout_ms": manifest.timeout_ms,
        },
        "native": None,
        "candidate_artifact": None,
    }

    preflight = collect_snapshot(transport, manifest)
    _write_json_exclusive(run_directory / "preflight.json", preflight)
    try:
        verify_preflight(manifest, candidate, preflight)
    except PreflightError as exc:
        envelope["status"] = "PREFLIGHT_FAILED"
        envelope["errors"].append(str(exc))
        envelope["finished_at"] = _timestamp()
        _write_json_exclusive(run_directory / "run.json", envelope)
        return envelope, run_directory

    sentinel = f"__WAVE_COMPILE_EXIT_{uuid.uuid4().hex}__"
    remote_command = build_remote_command(manifest, candidate, sentinel)
    started = time.monotonic()
    command_result = transport.run(("shell", remote_command), manifest.timeout_ms)
    duration_ms = int((time.monotonic() - started) * 1000)
    _write_text_exclusive(run_directory / "stdout.txt", command_result.stdout)
    _write_text_exclusive(run_directory / "stderr.txt", command_result.stderr)

    postflight = collect_snapshot(transport, manifest)
    align_native_diagnostics(transport, manifest, preflight, postflight)
    _write_json_exclusive(run_directory / "postflight.json", postflight)
    health_differences, kernel_alerts = compare_health(manifest, preflight, postflight)
    envelope["health_differences"] = health_differences
    envelope["kernel_alerts"] = kernel_alerts
    envelope["transport"] = {
        "adb_returncode": command_result.returncode,
        "duration_ms": duration_ms,
        "timed_out": command_result.timed_out,
    }

    native_result: dict[str, Any] | None = None
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
            verify_transport_status(
                command_result.returncode,
                remote_exit,
                "native compiler",
            )
            native_result = parse_native_result(cleaned_stdout, manifest, candidate)
            envelope["native"] = native_result
        except ResultError as exc:
            envelope["errors"].append(str(exc))

    if health_differences:
        envelope["errors"].append("device health gate failed")

    if not envelope["errors"] and native_result is not None:
        try:
            artifact, _ = pull_and_parse(
                transport,
                manifest,
                candidate,
                run_directory,
                native_result,
                postflight,
            )
            envelope["candidate_artifact"] = artifact
        except (ResultError, nbg_format.NbgError) as exc:
            envelope["errors"].append(f"candidate validation failed: {exc}")

    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = _timestamp()
    _write_json_exclusive(run_directory / "run.json", envelope)
    return envelope, run_directory


def compare_runs(left: Path, right: Path) -> dict[str, Any]:
    def resolve_run(path: Path) -> Path:
        resolved = path.resolve()
        try:
            resolved.relative_to(DEFAULT_RUN_ROOT.resolve())
        except ValueError as exc:
            raise CompileLabError("run path must be under artifacts/npu/runs") from exc
        return resolved

    left_dir = resolve_run(left)
    right_dir = resolve_run(right)
    left_run = json.loads((left_dir / "run.json").read_text(encoding="utf-8"))
    right_run = json.loads((right_dir / "run.json").read_text(encoding="utf-8"))
    if left_run.get("status") != "PASS" or right_run.get("status") != "PASS":
        raise CompileLabError("both compile runs must have PASS status")
    if left_run.get("experiment") != right_run.get("experiment"):
        raise CompileLabError("compile runs belong to different experiments")
    if left_run.get("manifest_sha256") != right_run.get("manifest_sha256"):
        raise CompileLabError("compile runs used different manifests")
    left_report = json.loads((left_dir / "nbg-report.json").read_text(encoding="utf-8"))
    right_report = json.loads(
        (right_dir / "nbg-report.json").read_text(encoding="utf-8")
    )
    left_normalized = {k: v for k, v in left_report.items() if k != "path"}
    right_normalized = {k: v for k, v in right_report.items() if k != "path"}
    left_artifact = left_run["candidate_artifact"]
    right_artifact = right_run["candidate_artifact"]
    return {
        "schema_version": 1,
        "left_run": left_run["run_id"],
        "right_run": right_run["run_id"],
        "left_sha256": left_artifact["sha256"],
        "right_sha256": right_artifact["sha256"],
        "byte_identical": left_artifact["sha256"] == right_artifact["sha256"],
        "normalized_parser_report_identical": (left_normalized == right_normalized),
    }


def _parser(policy: CompilePolicy = ADD_POLICY) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            f"Fixed compile-only Wave NPU PoC {policy.poc_id} "
            f"{policy.operation} harness; never executes an NBG"
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    run = subparsers.add_parser("run")
    run.add_argument("candidate", choices=sorted(policy.candidates))
    run.add_argument("--run-id", required=True)
    compare = subparsers.add_parser("compare")
    compare.add_argument("left", type=Path)
    compare.add_argument("right", type=Path)
    subparsers.add_parser("manifest")
    return parser


def main(
    argv: Sequence[str] | None = None,
    policy: CompilePolicy = ADD_POLICY,
) -> int:
    args = _parser(policy).parse_args(argv)
    try:
        manifest = load_manifest(policy=policy)
        if args.command == "manifest":
            print(
                json.dumps(
                    {
                        "manifest_sha256": manifest.sha256,
                        "candidates": sorted(manifest.candidates),
                        "mode": "compile-only",
                        "operation": manifest.policy.operation,
                        "poc_id": manifest.policy.poc_id,
                        "executes_graph": False,
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        if args.command == "run":
            envelope, run_directory = run_candidate(
                npu_lab.AdbTransport(),
                manifest,
                args.candidate,
                args.run_id,
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
        if args.command == "compare":
            print(
                json.dumps(
                    compare_runs(args.left, args.right), indent=2, sort_keys=True
                )
            )
            return 0
        raise AssertionError(args.command)
    except CompileLabError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
