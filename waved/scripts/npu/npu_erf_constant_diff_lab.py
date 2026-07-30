#!/usr/bin/env python3
"""Fixed PoC 12B compile-only Erf constant-differential harness.

The public contracts in this module expose no arbitrary executable, graph,
candidate, environment, remote path, or retry interface.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import sys
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from types import MappingProxyType
from typing import Any, Callable, Mapping, Sequence


try:
    from . import nbg_format, nbg_image, npu_lab
except ImportError:
    import nbg_format  # type: ignore
    import nbg_image  # type: ignore
    import npu_lab  # type: ignore


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = Path(__file__).with_name(
    "erf_constant_diff_manifest.json"
)
DEFAULT_RUN_ROOT = (
    REPOSITORY_ROOT
    / "artifacts/npu/runs/poc12b-erf-constant-diff-20260729"
)
MINIMUM_TMP_BYTES = (3 * 64 * 1024 * 1024) + (8 * 1024 * 1024)
SNAPSHOT_TIMEOUT_MS = 5_000
MAX_SNAPSHOT_LINES = 128
KERNEL_ALERT_RE = re.compile(
    r"(warning|error|fault|timeout|timed out|hang|recover|"
    r"exception|mmu)",
    re.IGNORECASE,
)
REMOTE_ROOT = "/tmp/wave-npu-poc12b"
REMOTE_EXECUTABLE = (
    f"{REMOTE_ROOT}/bin/wave-npu-compile-erf-constant-diff-v1"
)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
SENTINEL_RE = re.compile(r"^__WAVE_POC12B_EXIT_[0-9a-f]{32}__$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")

ROOT_KEYS = frozenset(
    {
        "schema_version",
        "poc_id",
        "device",
        "environment",
        "remote_root",
        "remote_executable",
        "binary_hash_state",
        "timeout_ms",
        "pull_timeout_ms",
        "required_artifacts",
        "candidates",
        "graph",
        "expected_nbg",
        "policy",
        "outputs",
    }
)
DEVICE_KEYS = frozenset(
    {
        "serial",
        "kernel",
        "galcore_major_minor",
        "chip_model",
        "customer_id",
        "chip_revision",
        "chip_eco_id",
        "npu_parameters",
    }
)
ENVIRONMENT_KEYS = frozenset(
    {"LD_LIBRARY_PATH", "VIVANTE_SDK_DIR", "VSI_NN_LOG_LEVEL"}
)
CANDIDATE_KEYS = frozenset(
    {
        "id",
        "argument",
        "label",
        "constant_u8",
        "expected_oracle_u8",
        "output",
        "partial_output",
    }
)
GRAPH_KEYS = frozenset(
    {
        "operation",
        "attribute",
        "data_type",
        "shape",
        "quantization",
        "output_count",
        "executes_graph",
    }
)
QUANTIZATION_KEYS = frozenset({"type", "scale", "zero_point"})
EXPECTED_NBG_KEYS = frozenset(
    {
        "magic",
        "minimum_bytes",
        "maximum_bytes",
        "version",
        "product",
        "allowed_input_counts",
        "output_count",
    }
)
POLICY_KEYS = frozenset(
    {
        "allowed_operations",
        "approved_build_provenance",
        "prohibited_actions",
    }
)
BUILD_KEYS = frozenset({"container_runs", "image", "network"})
OUTPUT_KEYS = frozenset({"raw_run_root", "tracked_evidence"})
EXPECTED_BUILD_IMAGE = (
    "ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:"
    "7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
)
EXPECTED_ALLOWED_OPERATIONS = (
    "compile-fixed-erf-to-nbg",
    "hash-files",
    "parse-nbg",
    "pull-fixed-output",
    "read-device-health",
    "remove-exact-stage-root",
    "stage-hash-pinned-files",
    "write-evidence",
)
EXPECTED_PROHIBITED_ACTIONS = frozenset(
    {
        "alternate_operation_calls",
        "compiler_retries",
        "device_container_runs",
        "device_reboots",
        "direct_galcore_opens",
        "driver_resets",
        "graph_execution_calls",
        "nbg_load_calls",
        "persistent_mutated_nbgs",
        "quarantined_clc_calls",
        "raw_ioctl_calls",
        "service_stop_calls",
        "simulator_calls",
        "tensor_readback_calls",
        "tensor_upload_calls",
        "unapproved_container_runs",
    }
)
EXPECTED_OUTPUTS = {
    "raw_run_root": (
        "artifacts/npu/runs/poc12b-erf-constant-diff-20260729"
    ),
    "tracked_evidence": (
        "docs/npu/format/erf-constant-differential.json"
    ),
}
EXPECTED_BASE_ARTIFACTS = {
    "/lib/libc.so.6": (
        "cfc6aa042a880a87e4f0adbfb74b55c9ba9607561905b20f58da96fcb2d3bb86"
    ),
    "/lib/libgcc_s.so.1": (
        "859408230bf4baf943d0c7985870488683b6e61c0cb5725ed8934d8c45ea0ab1"
    ),
    "/lib/libpthread.so.0": (
        "7bbc9a4a3924f9da05f6969be2422f1ee65ce9432eb376f9be071b59e39a13f3"
    ),
    "/lib64/ld-2.31.so": (
        "912002c930f3096d98fc32e0897466049d6e5182cbb606b456b2ebcc99551366"
    ),
    f"{REMOTE_ROOT}/lib/libtim-vx.so": (
        "d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
    ),
    f"{REMOTE_ROOT}/sdk/include/CL/cl_viv_vx_ext.h": (
        "908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57"
    ),
    "/usr/lib/libArchModelSw.so": (
        "2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f"
    ),
    "/usr/lib/libGAL.so": (
        "13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6"
    ),
    "/usr/lib/libNNArchPerf.so": (
        "253167677e9839d09815de93a86423824f26f56f097c100d59c03170adecffa9"
    ),
    "/usr/lib/libOpenCL.so": (
        "40ea26cfc80a43b79af09c0798ced760bb5339f9eef9450e6d08931c3b320ded"
    ),
    "/usr/lib/libOpenVX.so": (
        "f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb"
    ),
    "/usr/lib/libOpenVXU.so": (
        "370083254b7de8478f95fe90914245ddb4a52b2b3520749fe01f3ab45fba791c"
    ),
    "/usr/lib/libVSC.so": (
        "e9a39fe3b522b3926ff34f3fb380c73983ad8d48c5f40ffd6e6d8024adc2ed18"
    ),
    "/usr/lib/libVSC_Lite.so": (
        "e7c2c38fa813c669b7370795fad5d7b1207881103c481a6ac90e4785d058591f"
    ),
    "/usr/lib/libovxlib.so": (
        "09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9"
    ),
    "/usr/lib/libstdc++.so.6": (
        "d2189eb10122876a7fd35909eaecbea22da38462ce345d9bf46a53f500435009"
    ),
}

EXPECTED_CANDIDATES: tuple[
    tuple[str, str, str, tuple[int, ...], str, str], ...
] = (
    (
        "a1",
        "--a1",
        "A1",
        (0, 1, 2, 3),
        f"{REMOTE_ROOT}/output/erf-constant-a1.nbg",
        f"{REMOTE_ROOT}/output/.erf-constant-a1.nbg.partial",
    ),
    (
        "a2",
        "--a2",
        "A2",
        (0, 1, 2, 3),
        f"{REMOTE_ROOT}/output/erf-constant-a2.nbg",
        f"{REMOTE_ROOT}/output/.erf-constant-a2.nbg.partial",
    ),
    (
        "b",
        "--b",
        "B",
        (0, 1, 2, 4),
        f"{REMOTE_ROOT}/output/erf-constant-b.nbg",
        f"{REMOTE_ROOT}/output/.erf-constant-b.nbg.partial",
    ),
)
EXPECTED_ORACLE = (0, 1, 1, 1)

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
    r"^RESULT status=PASS mode=compile-only "
    r"candidate=(?P<candidate>A1|A2|B) "
    r"bytes=(?P<bytes>[0-9]+) "
    r"version=(?P<version>0x[0-9a-f]{8}) "
    r"product=(?P<product>0x[0-9a-f]{8}) "
    r"output=(?P<output>/tmp/wave-npu-poc12b/output/"
    r"[a-z0-9][a-z0-9.-]{0,95}\.nbg)$"
)
SELECTOR_RE = re.compile(
    r'Instance (?P<backend>CPU|EVIS|CL|OPENVX|SP) node with kernel '
    r'"(?P<kernel>[a-z0-9_]+)"'
)


class ConstantDiffLabError(Exception):
    """Expected PoC 12B host-lab failure."""


class ManifestError(ConstantDiffLabError):
    """The strict experiment manifest is invalid."""


class PreflightError(ConstantDiffLabError):
    """The connected device does not match the approved baseline."""


class ResultError(ConstantDiffLabError):
    """A native or pulled-artifact result violates the fixed contract."""


@dataclass(frozen=True)
class CandidateSpec:
    """One fixed source constant and output slot."""

    candidate_id: str
    argument: str
    label: str
    constant_u8: tuple[int, ...]
    expected_oracle_u8: tuple[int, ...]
    output: str
    partial_output: str


@dataclass(frozen=True)
class GraphSpec:
    """The one admitted constant-input Erf graph."""

    operation: str
    attribute: str
    data_type: str
    shape: tuple[int, ...]
    quantization_type: str
    quantization_scale: float
    quantization_zero_point: int
    output_count: int
    executes_graph: bool


@dataclass(frozen=True)
class DeviceSpec:
    """Exact connected-device admission values."""

    serial: str
    kernel: str
    galcore_major_minor: str
    chip_model: str
    customer_id: str
    chip_revision: str
    chip_eco_id: str
    npu_parameters: Mapping[str, str]


@dataclass(frozen=True)
class ExperimentManifest:
    """Immutable PoC 12B policy and input identity."""

    source_path: Path
    sha256: str
    poc_id: str
    device: DeviceSpec
    environment: Mapping[str, str]
    remote_root: str
    remote_executable: str
    binary_hash_state: str
    timeout_ms: int
    pull_timeout_ms: int
    required_artifacts: Mapping[str, str]
    candidates: Mapping[str, CandidateSpec]
    graph: GraphSpec
    expected_nbg: Mapping[str, Any]
    allowed_operations: tuple[str, ...]
    approved_build_provenance: Mapping[str, Any]
    prohibited_actions: Mapping[str, int]
    outputs: Mapping[str, str]

    @property
    def hash_frozen(self) -> bool:
        return self.binary_hash_state == "frozen"


@dataclass(frozen=True)
class NativeResult:
    """Validated native success result."""

    candidate_id: str
    label: str
    bytes: int
    version: str
    product: str
    output: str
    chip_model: str
    chip_revision: str
    chip_product_id: str
    chip_eco_id: str
    customer_id: str
    selector_backend: str
    selector_kernel: str


@dataclass(frozen=True)
class CandidateCapture:
    """One pulled candidate used by the sequencing gate."""

    candidate_id: str
    payload: bytes
    sha256: str
    bytes: int
    metadata: Mapping[str, Any]

    @classmethod
    def fixture(
        cls, candidate: CandidateSpec, payload: bytes
    ) -> CandidateCapture:
        return cls(
            candidate_id=candidate.candidate_id,
            payload=payload,
            sha256=hashlib.sha256(payload).hexdigest(),
            bytes=len(payload),
            metadata=MappingProxyType({"fixture": True}),
        )


@dataclass(frozen=True)
class SequenceResult:
    """Pure sequencing result used by fake and live capture paths."""

    classification: str
    captures: tuple[CandidateCapture, ...]
    a_controls_exact: bool
    first_difference: int | None


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ManifestError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _mapping(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ManifestError(f"{context} must be an object")
    return value


def _array(value: Any, context: str) -> list[Any]:
    if not isinstance(value, list):
        raise ManifestError(f"{context} must be an array")
    return value


def _keys(
    value: Mapping[str, Any], expected: frozenset[str], context: str
) -> None:
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing:
        raise ManifestError(f"{context} missing keys: {', '.join(missing)}")
    if unknown:
        raise ManifestError(f"{context} has unknown keys: {', '.join(unknown)}")


def _string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise ManifestError(f"{context} must be a nonempty string")
    if CONTROL_RE.search(value):
        raise ManifestError(f"{context} contains a control character")
    return value


def _integer(
    value: Any, context: str, *, minimum: int | None = None
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ManifestError(f"{context} must be an integer")
    if minimum is not None and value < minimum:
        raise ManifestError(f"{context} must be at least {minimum}")
    return value


def _sha256(value: Any, context: str) -> str:
    text = _string(value, context)
    if SHA256_RE.fullmatch(text) is None:
        raise ManifestError(f"{context} must be a lowercase SHA-256")
    return text


def _string_map(value: Any, context: str) -> Mapping[str, str]:
    mapping = _mapping(value, context)
    result = {
        _string(key, f"{context}.key"): _string(
            item, f"{context}.{key}"
        )
        for key, item in mapping.items()
    }
    return MappingProxyType(dict(sorted(result.items())))


def _string_array(value: Any, context: str) -> tuple[str, ...]:
    values = tuple(
        _string(item, f"{context}[{index}]")
        for index, item in enumerate(_array(value, context))
    )
    if not values or len(values) != len(set(values)):
        raise ManifestError(f"{context} must contain unique values")
    return values


def _u8_array(value: Any, context: str) -> tuple[int, ...]:
    values = tuple(
        _integer(item, f"{context}[{index}]")
        for index, item in enumerate(_array(value, context))
    )
    if len(values) != 4 or any(item > 255 for item in values):
        raise ManifestError(f"{context} must contain exactly four U8 values")
    return values


def load_manifest(path: Path = DEFAULT_MANIFEST) -> ExperimentManifest:
    """Load and validate the closed PoC 12B policy."""

    if path.is_symlink() or not path.is_file():
        raise ManifestError(
            f"manifest must be a regular non-symlink file: {path}"
        )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise ManifestError(f"cannot read manifest {path}: {exc}") from exc
    try:
        raw = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except ManifestError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ManifestError(f"invalid manifest JSON: {exc}") from exc

    root = _mapping(raw, "root")
    _keys(root, ROOT_KEYS, "root")
    if _integer(root["schema_version"], "schema_version") != SCHEMA_VERSION:
        raise ManifestError("unsupported schema_version")
    poc_id = _string(root["poc_id"], "poc_id")
    if poc_id != "poc12b-erf-constant-diff":
        raise ManifestError("unexpected poc_id")

    raw_device = _mapping(root["device"], "device")
    _keys(raw_device, DEVICE_KEYS, "device")
    npu_parameters = _string_map(
        raw_device["npu_parameters"], "device.npu_parameters"
    )
    if dict(npu_parameters) != {
        "enableNN": "255",
        "nanoqFreq": "838860800",
        "recovery": "1",
    }:
        raise ManifestError("device.npu_parameters differ from fixed policy")
    device = DeviceSpec(
        serial=_string(raw_device["serial"], "device.serial"),
        kernel=_string(raw_device["kernel"], "device.kernel"),
        galcore_major_minor=_string(
            raw_device["galcore_major_minor"],
            "device.galcore_major_minor",
        ),
        chip_model=_string(raw_device["chip_model"], "device.chip_model"),
        customer_id=_string(
            raw_device["customer_id"], "device.customer_id"
        ),
        chip_revision=_string(
            raw_device["chip_revision"], "device.chip_revision"
        ),
        chip_eco_id=_string(
            raw_device["chip_eco_id"], "device.chip_eco_id"
        ),
        npu_parameters=npu_parameters,
    )
    if device != DeviceSpec(
        serial="PSP_0000000000000000",
        kernel=(
            "Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT "
            "Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
        ),
        galcore_major_minor="199:0",
        chip_model="0x00009000",
        customer_id="0x1000000e",
        chip_revision="0x00009002",
        chip_eco_id="0x04000000",
        npu_parameters=npu_parameters,
    ):
        raise ManifestError("device identity differs from fixed Wave policy")

    raw_environment = _mapping(root["environment"], "environment")
    _keys(raw_environment, ENVIRONMENT_KEYS, "environment")
    environment = _string_map(raw_environment, "environment")
    expected_environment = {
        "LD_LIBRARY_PATH": f"{REMOTE_ROOT}/lib:/usr/lib",
        "VIVANTE_SDK_DIR": f"{REMOTE_ROOT}/sdk",
        "VSI_NN_LOG_LEVEL": "5",
    }
    if dict(environment) != expected_environment:
        raise ManifestError("environment differs from fixed policy")

    remote_root = _string(root["remote_root"], "remote_root")
    remote_executable = _string(
        root["remote_executable"], "remote_executable"
    )
    if remote_root != REMOTE_ROOT:
        raise ManifestError("remote root differs from fixed policy")
    if remote_executable != REMOTE_EXECUTABLE:
        raise ManifestError("remote executable differs from fixed policy")

    binary_hash_state = _string(
        root["binary_hash_state"], "binary_hash_state"
    )
    if binary_hash_state not in {"unfrozen", "frozen"}:
        raise ManifestError("binary_hash_state must be unfrozen or frozen")

    raw_artifacts = _mapping(root["required_artifacts"], "required_artifacts")
    artifacts: dict[str, str] = {}
    for remote_path, digest in raw_artifacts.items():
        remote_path = _string(remote_path, "required_artifacts.path")
        if not remote_path.startswith("/") or ".." in Path(remote_path).parts:
            raise ManifestError("required artifact path must be absolute and fixed")
        artifacts[remote_path] = _sha256(
            digest, f"required_artifacts.{remote_path}"
        )
    if not artifacts:
        raise ManifestError("required_artifacts must not be empty")
    observed_base = {
        key: value
        for key, value in artifacts.items()
        if key != remote_executable
    }
    if observed_base != EXPECTED_BASE_ARTIFACTS:
        raise ManifestError(
            "required_artifacts differ from the fixed runtime policy"
        )
    if binary_hash_state == "frozen":
        if remote_executable not in artifacts:
            raise ManifestError("frozen manifest is missing executable hash")
    elif remote_executable in artifacts:
        raise ManifestError("unfrozen manifest may not claim executable hash")

    raw_candidates = _array(root["candidates"], "candidates")
    if len(raw_candidates) != len(EXPECTED_CANDIDATES):
        raise ManifestError("candidates must contain exactly three entries")
    candidates: dict[str, CandidateSpec] = {}
    observed_order: list[str] = []
    for index, (raw_candidate, expected) in enumerate(
        zip(raw_candidates, EXPECTED_CANDIDATES, strict=True)
    ):
        item = _mapping(raw_candidate, f"candidates[{index}]")
        _keys(item, CANDIDATE_KEYS, f"candidates[{index}]")
        (
            expected_id,
            expected_argument,
            expected_label,
            expected_constant,
            expected_output,
            expected_partial,
        ) = expected
        candidate_id = _string(item["id"], f"candidates[{index}].id")
        observed_order.append(candidate_id)
        if candidate_id != expected_id:
            raise ManifestError("candidate order differs from fixed policy")
        constant = _u8_array(
            item["constant_u8"], f"candidates[{index}].constant_u8"
        )
        if constant != expected_constant:
            raise ManifestError(
                f"{candidate_id} fixed constant differs from policy"
            )
        oracle = _u8_array(
            item["expected_oracle_u8"],
            f"candidates[{index}].expected_oracle_u8",
        )
        if oracle != EXPECTED_ORACLE:
            raise ManifestError("fixed expected oracle differs from policy")
        argument = _string(
            item["argument"], f"candidates[{index}].argument"
        )
        label = _string(item["label"], f"candidates[{index}].label")
        output = _string(item["output"], f"candidates[{index}].output")
        partial = _string(
            item["partial_output"],
            f"candidates[{index}].partial_output",
        )
        if argument != expected_argument or label != expected_label:
            raise ManifestError("candidate argument or label differs")
        if output != expected_output or partial != expected_partial:
            raise ManifestError("candidate fixed output differs from policy")
        candidates[candidate_id] = CandidateSpec(
            candidate_id=candidate_id,
            argument=argument,
            label=label,
            constant_u8=constant,
            expected_oracle_u8=oracle,
            output=output,
            partial_output=partial,
        )
    if observed_order != ["a1", "a2", "b"]:
        raise ManifestError("candidate order differs from fixed policy")

    raw_graph = _mapping(root["graph"], "graph")
    _keys(raw_graph, GRAPH_KEYS, "graph")
    quantization = _mapping(raw_graph["quantization"], "graph.quantization")
    _keys(quantization, QUANTIZATION_KEYS, "graph.quantization")
    scale = quantization["scale"]
    if isinstance(scale, bool) or not isinstance(scale, (int, float)):
        raise ManifestError("graph.quantization.scale must be numeric")
    executes_graph = raw_graph["executes_graph"]
    if not isinstance(executes_graph, bool):
        raise ManifestError("graph.executes_graph must be boolean")
    graph = GraphSpec(
        operation=_string(raw_graph["operation"], "graph.operation"),
        attribute=_string(raw_graph["attribute"], "graph.attribute"),
        data_type=_string(raw_graph["data_type"], "graph.data_type"),
        shape=tuple(
            _integer(item, f"graph.shape[{index}]", minimum=1)
            for index, item in enumerate(_array(raw_graph["shape"], "graph.shape"))
        ),
        quantization_type=_string(
            quantization["type"], "graph.quantization.type"
        ),
        quantization_scale=float(scale),
        quantization_zero_point=_integer(
            quantization["zero_point"],
            "graph.quantization.zero_point",
        ),
        output_count=_integer(
            raw_graph["output_count"], "graph.output_count", minimum=1
        ),
        executes_graph=executes_graph,
    )
    if graph != GraphSpec(
        "Erf", "CONSTANT", "UINT8", (4,), "ASYMMETRIC", 1.0, 0, 1, False
    ):
        raise ManifestError("graph differs from fixed constant Erf policy")

    raw_expected = _mapping(root["expected_nbg"], "expected_nbg")
    _keys(raw_expected, EXPECTED_NBG_KEYS, "expected_nbg")
    expected_nbg = {
        "allowed_input_counts": [
            _integer(item, "expected_nbg.allowed_input_counts")
            for item in _array(
                raw_expected["allowed_input_counts"],
                "expected_nbg.allowed_input_counts",
            )
        ],
        "magic": _string(raw_expected["magic"], "expected_nbg.magic"),
        "maximum_bytes": _integer(
            raw_expected["maximum_bytes"],
            "expected_nbg.maximum_bytes",
            minimum=1,
        ),
        "minimum_bytes": _integer(
            raw_expected["minimum_bytes"],
            "expected_nbg.minimum_bytes",
            minimum=1,
        ),
        "output_count": _integer(
            raw_expected["output_count"],
            "expected_nbg.output_count",
            minimum=1,
        ),
        "product": _string(raw_expected["product"], "expected_nbg.product"),
        "version": _string(raw_expected["version"], "expected_nbg.version"),
    }
    if expected_nbg != {
        "allowed_input_counts": [0, 1],
        "magic": "VPMN",
        "maximum_bytes": 67_108_864,
        "minimum_bytes": 513,
        "output_count": 1,
        "product": "0x1000000e",
        "version": "0x00020000",
    }:
        raise ManifestError("expected_nbg differs from fixed policy")

    raw_policy = _mapping(root["policy"], "policy")
    _keys(raw_policy, POLICY_KEYS, "policy")
    allowed_operations = _string_array(
        raw_policy["allowed_operations"], "policy.allowed_operations"
    )
    build = _mapping(
        raw_policy["approved_build_provenance"],
        "policy.approved_build_provenance",
    )
    _keys(build, BUILD_KEYS, "policy.approved_build_provenance")
    approved_build = {
        "container_runs": _integer(
            build["container_runs"],
            "policy.approved_build_provenance.container_runs",
        ),
        "image": _string(
            build["image"],
            "policy.approved_build_provenance.image",
        ),
        "network": _string(
            build["network"],
            "policy.approved_build_provenance.network",
        ),
    }
    if (
        approved_build["container_runs"] != 1
        or approved_build["network"] != "none"
        or approved_build["image"] != EXPECTED_BUILD_IMAGE
    ):
        raise ManifestError("approved build provenance differs from policy")
    raw_prohibited = _mapping(
        raw_policy["prohibited_actions"], "policy.prohibited_actions"
    )
    prohibited: dict[str, int] = {}
    for name, value in raw_prohibited.items():
        action = _string(name, "policy.prohibited_actions.key")
        count = _integer(
            value, f"policy.prohibited_actions.{action}", minimum=0
        )
        if count != 0:
            raise ManifestError(
                f"policy.prohibited_actions.{action} must be zero"
            )
        prohibited[action] = count
    if not prohibited:
        raise ManifestError("prohibited_actions must not be empty")
    if frozenset(prohibited) != EXPECTED_PROHIBITED_ACTIONS:
        raise ManifestError(
            "prohibited_actions differ from the fixed policy"
        )

    raw_outputs = _mapping(root["outputs"], "outputs")
    _keys(raw_outputs, OUTPUT_KEYS, "outputs")
    outputs = _string_map(raw_outputs, "outputs")
    if dict(outputs) != EXPECTED_OUTPUTS:
        raise ManifestError("outputs differ from the fixed policy")
    if allowed_operations != EXPECTED_ALLOWED_OPERATIONS:
        raise ManifestError(
            "allowed_operations differ from the fixed policy"
        )

    timeout_ms = _integer(
        root["timeout_ms"], "timeout_ms", minimum=1
    )
    pull_timeout_ms = _integer(
        root["pull_timeout_ms"], "pull_timeout_ms", minimum=1
    )
    if timeout_ms != 20_000 or pull_timeout_ms != 10_000:
        raise ManifestError("timeouts differ from the fixed policy")

    return ExperimentManifest(
        source_path=path.resolve(),
        sha256=hashlib.sha256(payload).hexdigest(),
        poc_id=poc_id,
        device=device,
        environment=environment,
        remote_root=remote_root,
        remote_executable=remote_executable,
        binary_hash_state=binary_hash_state,
        timeout_ms=timeout_ms,
        pull_timeout_ms=pull_timeout_ms,
        required_artifacts=MappingProxyType(dict(sorted(artifacts.items()))),
        candidates=MappingProxyType(candidates),
        graph=graph,
        expected_nbg=MappingProxyType(expected_nbg),
        allowed_operations=allowed_operations,
        approved_build_provenance=MappingProxyType(approved_build),
        prohibited_actions=MappingProxyType(dict(sorted(prohibited.items()))),
        outputs=outputs,
    )


def parse_native_result(
    stdout: str,
    manifest: ExperimentManifest,
    candidate: CandidateSpec,
) -> NativeResult:
    """Parse one exact identity/result pair and Erf selector."""

    identity_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("IDENTITY ")
    ]
    result_lines = [
        line.rstrip("\r")
        for line in stdout.splitlines()
        if line.startswith("RESULT ")
    ]
    if len(identity_lines) != 1 or len(result_lines) != 1:
        raise ResultError(
            "expected one identity and result line; observed "
            f"{len(identity_lines)} and {len(result_lines)}"
        )
    identity = IDENTITY_RE.fullmatch(identity_lines[0])
    result = RESULT_RE.fullmatch(result_lines[0])
    if identity is None or result is None:
        raise ResultError("identity or result line is malformed")
    identity_values = identity.groupdict()
    result_values = result.groupdict()
    if result_values["candidate"] != candidate.label:
        raise ResultError("candidate label mismatch")
    if result_values["output"] != candidate.output:
        raise ResultError("fixed output path mismatch")
    if result_values["version"] != manifest.expected_nbg["version"]:
        raise ResultError("native NBG version mismatch")
    if result_values["product"] != manifest.expected_nbg["product"]:
        raise ResultError("native NBG product mismatch")
    if identity_values["chip_model"] != manifest.device.chip_model:
        raise ResultError("live chip model mismatch")
    if identity_values["customer_id"] != manifest.device.customer_id:
        raise ResultError("live customer ID mismatch")
    if identity_values["chip_revision"] != manifest.device.chip_revision:
        raise ResultError("live chip revision mismatch")
    if identity_values["chip_eco_id"] != manifest.device.chip_eco_id:
        raise ResultError("live chip ECO ID mismatch")
    byte_count = int(result_values["bytes"])
    if not (
        manifest.expected_nbg["minimum_bytes"]
        <= byte_count
        <= manifest.expected_nbg["maximum_bytes"]
    ):
        raise ResultError("native NBG size is outside policy")
    selectors = [
        match.groupdict()
        for match in SELECTOR_RE.finditer(stdout)
        if match["kernel"] == "erf"
    ]
    if len(selectors) != 1:
        raise ResultError(
            f"expected one erf selector result, observed {len(selectors)}"
        )
    selector = selectors[0]
    if selector["backend"] != "OPENVX":
        raise ResultError(
            f"Erf selected {selector['backend']}, expected OPENVX"
        )
    return NativeResult(
        candidate_id=candidate.candidate_id,
        label=candidate.label,
        bytes=byte_count,
        version=result_values["version"],
        product=result_values["product"],
        output=result_values["output"],
        chip_model=identity_values["chip_model"],
        chip_revision=identity_values["chip_revision"],
        chip_product_id=identity_values["chip_product_id"],
        chip_eco_id=identity_values["chip_eco_id"],
        customer_id=identity_values["customer_id"],
        selector_backend=selector["backend"],
        selector_kernel=selector["kernel"],
    )


def build_remote_command(
    manifest: ExperimentManifest,
    candidate: CandidateSpec,
    sentinel: str,
) -> str:
    """Construct the one allowlisted clean-environment invocation."""

    if SENTINEL_RE.fullmatch(sentinel) is None:
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
        candidate.argument,
    ]
    command = " ".join(shlex.quote(token) for token in invocation)
    return (
        f"{command}\n"
        "wave_poc12b_rc=$?\n"
        f"printf '\\n{sentinel}:%s\\n' \"$wave_poc12b_rc\"\n"
        "exit \"$wave_poc12b_rc\"\n"
    )


def first_difference(left: bytes, right: bytes) -> int | None:
    """Return the first differing byte, including a length boundary."""

    for index, (left_byte, right_byte) in enumerate(zip(left, right)):
        if left_byte != right_byte:
            return index
    if len(left) != len(right):
        return min(len(left), len(right))
    return None


def run_sequence_with_capture(
    manifest: ExperimentManifest,
    capture: Callable[[CandidateSpec], CandidateCapture],
) -> SequenceResult:
    """Enforce A1, A2, exact determinism, then B."""

    a1 = capture(manifest.candidates["a1"])
    a2 = capture(manifest.candidates["a2"])
    difference = first_difference(a1.payload, a2.payload)
    if difference is not None:
        return SequenceResult(
            classification="serializer-nondeterministic",
            captures=(a1, a2),
            a_controls_exact=False,
            first_difference=difference,
        )
    b = capture(manifest.candidates["b"])
    return SequenceResult(
        classification="captured",
        captures=(a1, a2, b),
        a_controls_exact=True,
        first_difference=None,
    )


def canonical_json(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def _bounded_lines(text: str) -> list[str]:
    return [
        line.rstrip()
        for line in text.splitlines()
        if line.strip()
    ][-MAX_SNAPSHOT_LINES:]


def _snapshot_value(
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


def _connected_serials(
    transport: Any, errors: list[str]
) -> list[str]:
    result = transport.run(("devices", "-l"), SNAPSHOT_TIMEOUT_MS)
    if result.timed_out or result.returncode != 0:
        errors.append("adb devices failed")
        return []
    serials = []
    for line in result.stdout.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 2 and fields[1] == "device":
            serials.append(fields[0])
    return serials


def _remote_file_state(
    transport: Any,
    path: str,
    errors: list[str],
    *,
    allow_symlink: bool = False,
) -> dict[str, Any]:
    quoted = shlex.quote(path)
    command = (
        f"if [ -L {quoted} ]; then "
        f"digest=$(sha256sum {quoted} | cut -d ' ' -f 1) || exit 20; "
        "printf 'SYMLINK %s\\n' \"$digest\"; "
        f"elif [ -f {quoted} ]; then "
        f"bytes=$(wc -c < {quoted}) || exit 21; "
        f"mode=$(ls -ldn {quoted} | awk '{{print $1}}') || exit 22; "
        f"digest=$(sha256sum {quoted} | cut -d ' ' -f 1) || exit 23; "
        "printf 'PRESENT %s %s %s\\n' "
        "\"$bytes\" \"$mode\" \"$digest\"; "
        f"elif [ -e {quoted} ]; then printf 'OTHER\\n'; "
        "else printf 'ABSENT\\n'; fi"
    )
    output = _snapshot_value(
        transport, command, f"file state {path}", errors
    )
    symlink = re.fullmatch(r"SYMLINK ([0-9a-f]{64})", output or "")
    if symlink is not None:
        if allow_symlink:
            return {
                "sha256": symlink[1],
                "status": "present",
                "symlink": True,
            }
        return {"sha256": symlink[1], "status": "symlink"}
    if output in {"ABSENT", "OTHER"}:
        return {"status": output.lower()}
    match = re.fullmatch(
        r"PRESENT ([0-9]+) (-[rwx-]{9}) ([0-9a-f]{64})",
        output or "",
    )
    if match is None:
        errors.append(f"file state {path}: malformed")
        return {"status": "unknown"}
    return {
        "bytes": int(match[1]),
        "permissions": match[2],
        "sha256": match[3],
        "status": "present",
    }


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
    output = _snapshot_value(
        transport, command, "galcore holder scan", errors
    )
    holders: list[dict[str, int]] = []
    for line in _bounded_lines(output or ""):
        match = re.fullmatch(r"([0-9]+):([0-9]+)", line)
        if match is not None:
            holders.append(
                {"fd": int(match[2]), "pid": int(match[1])}
            )
    return holders


def collect_snapshot(
    transport: Any,
    manifest: ExperimentManifest,
) -> dict[str, Any]:
    """Collect a bounded read-only device snapshot."""

    errors: list[str] = []
    snapshot: dict[str, Any] = {
        "artifact_states": {},
        "boot_id": None,
        "dmesg_tail": [],
        "errors": errors,
        "galcore_device": None,
        "galcore_holders": [],
        "kernel": None,
        "npu_parameters": {},
        "output_states": {},
        "probe_processes": [],
        "remote_root_state": None,
        "serials": _connected_serials(transport, errors),
        "tmp_available_bytes": None,
        "uptime": None,
    }
    if snapshot["serials"] != [manifest.device.serial]:
        errors.append(
            f"connected device list is {snapshot['serials']!r}, "
            f"expected [{manifest.device.serial!r}]"
        )
        return snapshot
    snapshot["kernel"] = _snapshot_value(
        transport, "uname -a", "kernel", errors
    )
    snapshot["boot_id"] = _snapshot_value(
        transport,
        "cat /proc/sys/kernel/random/boot_id",
        "boot ID",
        errors,
    )
    snapshot["uptime"] = _snapshot_value(
        transport, "cat /proc/uptime", "uptime", errors
    )
    snapshot["galcore_device"] = _snapshot_value(
        transport, "ls -ln /dev/galcore", "/dev/galcore", errors
    )
    for name in ("enableNN", "nanoqFreq", "recovery"):
        path = f"/sys/module/galcore/parameters/{name}"
        snapshot["npu_parameters"][name] = _snapshot_value(
            transport,
            f"cat {shlex.quote(path)}",
            f"NPU parameter {name}",
            errors,
        )
    snapshot["galcore_holders"] = _holder_snapshot(transport, errors)
    process_text = _snapshot_value(
        transport,
        (
            "ps 2>/dev/null | "
            "grep '[w]ave-npu-compile-erf-constant-diff' | "
            "head -n 16 || true"
        ),
        "probe process scan",
        errors,
    )
    snapshot["probe_processes"] = _bounded_lines(process_text or "")
    root = shlex.quote(manifest.remote_root)
    snapshot["remote_root_state"] = _snapshot_value(
        transport,
        (
            f"if [ -L {root} ]; then printf 'symlink\\n'; "
            f"elif [ -d {root} ]; then printf 'directory\\n'; "
            f"elif [ -e {root} ]; then printf 'other\\n'; "
            "else printf 'absent\\n'; fi"
        ),
        "remote root",
        errors,
    )
    tmp_kib = _snapshot_value(
        transport,
        "df -Pk /tmp | awk 'END {print $4}'",
        "/tmp available space",
        errors,
    )
    if tmp_kib is not None and re.fullmatch(r"[0-9]+", tmp_kib):
        snapshot["tmp_available_bytes"] = int(tmp_kib) * 1024
    else:
        errors.append("/tmp available space: malformed")

    for path in manifest.required_artifacts:
        snapshot["artifact_states"][path] = _remote_file_state(
            transport,
            path,
            errors,
            allow_symlink=not path.startswith(
                f"{manifest.remote_root}/"
            ),
        )
    for candidate in manifest.candidates.values():
        snapshot["output_states"][candidate.output] = _remote_file_state(
            transport, candidate.output, errors
        )
        snapshot["output_states"][
            candidate.partial_output
        ] = _remote_file_state(
            transport, candidate.partial_output, errors
        )
    dmesg = _snapshot_value(
        transport,
        f"dmesg 2>/dev/null | tail -n {MAX_SNAPSHOT_LINES}",
        "dmesg",
        errors,
    )
    snapshot["dmesg_tail"] = _bounded_lines(dmesg or "")
    return snapshot


def verify_snapshot(
    manifest: ExperimentManifest,
    snapshot: Mapping[str, Any],
    *,
    staged: bool,
) -> None:
    """Verify either the unstaged read-only or staged admission state."""

    errors = snapshot.get("errors")
    if errors:
        raise PreflightError("; ".join(str(item) for item in errors))
    if snapshot.get("serials") != [manifest.device.serial]:
        raise PreflightError(
            "authorized device is not the only connected device"
        )
    if snapshot.get("kernel") != manifest.device.kernel:
        raise PreflightError("kernel identity mismatch")
    if snapshot.get("npu_parameters") != dict(
        manifest.device.npu_parameters
    ):
        raise PreflightError("NPU parameters differ from baseline")
    if re.search(
        r"\b199,\s*0\b", str(snapshot.get("galcore_device") or "")
    ) is None:
        raise PreflightError(
            "/dev/galcore is not character device 199:0"
        )
    boot_id = snapshot.get("boot_id")
    if (
        not isinstance(boot_id, str)
        or re.fullmatch(
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-"
            r"[0-9a-f]{4}-[0-9a-f]{12}",
            boot_id,
        )
        is None
    ):
        raise PreflightError("boot ID is missing or malformed")
    if snapshot.get("galcore_holders"):
        raise PreflightError("/dev/galcore already has a holder")
    if snapshot.get("probe_processes"):
        raise PreflightError("a PoC 12B probe process is already running")
    available = snapshot.get("tmp_available_bytes")
    if (
        isinstance(available, bool)
        or not isinstance(available, int)
        or available < MINIMUM_TMP_BYTES
    ):
        raise PreflightError(
            "/tmp lacks space for three maximum-size bounded outputs"
        )
    if not snapshot.get("dmesg_tail"):
        raise PreflightError("kernel diagnostic baseline is empty")
    expected_root = "directory" if staged else "absent"
    if snapshot.get("remote_root_state") != expected_root:
        raise PreflightError(
            f"remote root must be {expected_root} for this phase"
        )
    for path, expected_hash in manifest.required_artifacts.items():
        state = snapshot.get("artifact_states", {}).get(path, {})
        under_stage = path.startswith(f"{manifest.remote_root}/")
        if under_stage and not staged:
            if state.get("status") != "absent":
                raise PreflightError(
                    f"unstaged artifact unexpectedly exists: {path}"
                )
            continue
        if (
            state.get("status") != "present"
            or state.get("sha256") != expected_hash
        ):
            raise PreflightError(
                f"required artifact hash mismatch: {path}"
            )
    if staged:
        for path, state in snapshot.get("output_states", {}).items():
            if state.get("status") != "absent":
                raise PreflightError(
                    f"candidate output is not initially absent: {path}"
                )


def _new_dmesg_lines(
    before: Sequence[str], after: Sequence[str]
) -> tuple[list[str], bool]:
    if not before:
        return list(after), False
    marker = before[-1]
    matches = [
        index for index, line in enumerate(after) if line == marker
    ]
    if not matches:
        return list(after), False
    return list(after[matches[-1] + 1 :]), True


def compare_health(
    manifest: ExperimentManifest,
    before: Mapping[str, Any],
    after: Mapping[str, Any],
    *,
    require_staged: bool,
) -> tuple[list[str], list[str]]:
    """Compare immutable health fields and flag new kernel alerts."""

    differences: list[str] = []
    for key in (
        "serials",
        "kernel",
        "boot_id",
        "galcore_device",
        "npu_parameters",
    ):
        if before.get(key) != after.get(key):
            differences.append(f"{key} changed")
    if after.get("errors"):
        differences.append(
            "snapshot errors: "
            + "; ".join(str(item) for item in after["errors"])
        )
    if after.get("galcore_holders"):
        differences.append("a /dev/galcore holder remains")
    if after.get("probe_processes"):
        differences.append("a PoC 12B process remains")
    expected_root = "directory" if require_staged else "absent"
    if after.get("remote_root_state") != expected_root:
        differences.append(
            f"remote root is not {expected_root}"
        )
    for path, expected_hash in manifest.required_artifacts.items():
        under_stage = path.startswith(f"{manifest.remote_root}/")
        state = after.get("artifact_states", {}).get(path, {})
        if under_stage and not require_staged:
            if state.get("status") != "absent":
                differences.append(
                    f"cleaned stage artifact remains: {path}"
                )
        elif (
            state.get("status") != "present"
            or state.get("sha256") != expected_hash
        ):
            differences.append(f"artifact changed: {path}")
    new_lines, aligned = _new_dmesg_lines(
        before.get("dmesg_tail", []),
        after.get("dmesg_tail", []),
    )
    if not aligned and before.get("dmesg_tail") != after.get(
        "dmesg_tail"
    ):
        differences.append("could not align kernel diagnostics")
    alerts = [line for line in new_lines if KERNEL_ALERT_RE.search(line)]
    if alerts:
        differences.append("new kernel alert diagnostics were observed")
    return differences, alerts


def parse_remote_exit(stdout: str, sentinel: str) -> tuple[str, int]:
    """Remove and validate the one exact remote-exit sentinel."""

    if SENTINEL_RE.fullmatch(sentinel) is None:
        raise ValueError("invalid internal sentinel")
    pattern = re.compile(
        rf"(?:^|\n){re.escape(sentinel)}:(?P<status>[0-9]+)\r?\n?$"
    )
    matches = list(pattern.finditer(stdout))
    if len(matches) != 1:
        raise ResultError("missing or duplicate remote-exit sentinel")
    status = int(matches[0]["status"])
    cleaned = stdout[: matches[0].start()].rstrip("\r\n")
    return cleaned, status


def _validate_nbg(
    payload: bytes,
    manifest: ExperimentManifest,
    candidate: CandidateSpec,
) -> tuple[dict[str, Any], nbg_image.NbgImage]:
    expected = manifest.expected_nbg
    if not (
        expected["minimum_bytes"]
        <= len(payload)
        <= expected["maximum_bytes"]
    ):
        raise ResultError("pulled NBG size is outside policy")
    try:
        report = nbg_format.parse_bytes(
            payload, display_path=f"<poc12b-{candidate.candidate_id}>"
        )
        image = nbg_image.NbgImage.parse(
            payload,
            artifact_id=candidate.candidate_id,
            display_path=f"<poc12b-{candidate.candidate_id}>",
        )
        image.assert_total_ownership()
        rebuilt = image.to_bytes()
    except Exception as exc:
        raise ResultError(f"strict NBG admission failed: {exc}") from exc
    if rebuilt != payload:
        raise ResultError("unchanged NBG round trip is not exact")
    if (
        report.get("magic") != expected["magic"]
        or report.get("version_hex") != expected["version"]
        or report.get("product_hex") != expected["product"]
    ):
        raise ResultError("pulled NBG identity differs from policy")
    inputs = report.get("inputs")
    outputs = report.get("outputs")
    if (
        not isinstance(inputs, list)
        or len(inputs) not in expected["allowed_input_counts"]
    ):
        raise ResultError("pulled NBG input count differs from policy")
    if (
        not isinstance(outputs, list)
        or len(outputs) != expected["output_count"]
    ):
        raise ResultError("pulled NBG output count differs from policy")
    for tensor in [*inputs, *outputs]:
        if (
            tensor.get("dimensions") != [4]
            or tensor.get("data_format_name") != "uint8"
            or tensor.get("byte_size") != 4
        ):
            raise ResultError("pulled NBG tensor contract mismatch")
        quantization = tensor.get("quantization")
        if (
            not isinstance(quantization, dict)
            or quantization.get("format_name")
            not in {"affine-asymmetric", "none"}
        ):
            raise ResultError("pulled NBG quantization mismatch")
    return report, image


def _write_bytes_exclusive(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
        0o600,
    )
    try:
        with os.fdopen(descriptor, "wb") as handle:
            descriptor = -1
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _write_json_exclusive(path: Path, value: Any) -> None:
    _write_bytes_exclusive(path, canonical_json(value))


def _write_text_exclusive(path: Path, value: str) -> None:
    _write_bytes_exclusive(
        path, npu_lab.bounded_text(value).encode("utf-8")
    )


def _timestamp() -> str:
    return (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _pull_candidate(
    transport: Any,
    manifest: ExperimentManifest,
    candidate: CandidateSpec,
    native: NativeResult,
    postflight: Mapping[str, Any],
    run_root: Path,
) -> CandidateCapture:
    state = postflight.get("output_states", {}).get(
        candidate.output, {}
    )
    partial = postflight.get("output_states", {}).get(
        candidate.partial_output, {}
    )
    if state.get("status") != "present":
        raise ResultError("native pass did not leave the fixed output")
    if state.get("permissions") != "-r--r--r--":
        raise ResultError("fixed output is not mode 0444")
    if state.get("bytes") != native.bytes:
        raise ResultError("remote size differs from native result")
    if partial.get("status") != "absent":
        raise ResultError("partial output remains after native pass")
    local_path = run_root / f"{candidate.candidate_id}.nbg"
    if local_path.exists() or local_path.is_symlink():
        raise ResultError("exclusive local candidate path already exists")
    pull = transport.run(
        ("pull", candidate.output, str(local_path)),
        manifest.pull_timeout_ms,
    )
    _write_text_exclusive(
        run_root / f"{candidate.candidate_id}.pull.stdout.txt",
        pull.stdout,
    )
    _write_text_exclusive(
        run_root / f"{candidate.candidate_id}.pull.stderr.txt",
        pull.stderr,
    )
    if (
        pull.timed_out
        or pull.returncode != 0
        or local_path.is_symlink()
        or not local_path.is_file()
    ):
        raise ResultError("ADB pull failed")
    payload = local_path.read_bytes()
    digest = hashlib.sha256(payload).hexdigest()
    if (
        digest != state.get("sha256")
        or len(payload) != state.get("bytes")
    ):
        raise ResultError("pulled NBG identity differs from remote output")
    report, image = _validate_nbg(payload, manifest, candidate)
    _write_json_exclusive(
        run_root / f"{candidate.candidate_id}.nbg-report.json",
        report,
    )
    return CandidateCapture(
        candidate_id=candidate.candidate_id,
        payload=payload,
        sha256=digest,
        bytes=len(payload),
        metadata=MappingProxyType(
            {
                "connection_tail_mode": image.connection_tail_mode,
                "output": candidate.output,
                "selector_backend": native.selector_backend,
                "selector_kernel": native.selector_kernel,
            }
        ),
    )


def _capture_candidate(
    transport: Any,
    manifest: ExperimentManifest,
    candidate: CandidateSpec,
    baseline: Mapping[str, Any],
    run_root: Path,
) -> tuple[CandidateCapture, dict[str, Any]]:
    sentinel = f"__WAVE_POC12B_EXIT_{uuid.uuid4().hex}__"
    command = build_remote_command(manifest, candidate, sentinel)
    started = time.monotonic()
    result = transport.run(
        ("shell", command), manifest.timeout_ms
    )
    duration_ms = int((time.monotonic() - started) * 1000)
    _write_text_exclusive(
        run_root / f"{candidate.candidate_id}.stdout.txt",
        result.stdout,
    )
    _write_text_exclusive(
        run_root / f"{candidate.candidate_id}.stderr.txt",
        result.stderr,
    )
    postflight = collect_snapshot(transport, manifest)
    _write_json_exclusive(
        run_root / f"{candidate.candidate_id}.postflight.json",
        postflight,
    )
    differences, alerts = compare_health(
        manifest, baseline, postflight, require_staged=True
    )
    if result.timed_out:
        raise ResultError(
            "host ADB deadline expired; no retry was attempted"
        )
    cleaned, remote_exit = parse_remote_exit(result.stdout, sentinel)
    if result.returncode != remote_exit or remote_exit != 0:
        raise ResultError(
            "native compiler returned a nonzero transport/remote status"
        )
    native = parse_native_result(
        f"{cleaned}\n{result.stderr}",
        manifest,
        candidate,
    )
    if differences:
        raise ResultError(
            "post-candidate health gate failed: "
            + "; ".join(differences)
        )
    capture = _pull_candidate(
        transport,
        manifest,
        candidate,
        native,
        postflight,
        run_root,
    )
    metadata = {
        "alerts": alerts,
        "artifact": {
            "bytes": capture.bytes,
            "path": f"{candidate.candidate_id}.nbg",
            "sha256": capture.sha256,
        },
        "candidate": candidate.candidate_id,
        "duration_ms": duration_ms,
        "native": {
            "bytes": native.bytes,
            "chip_eco_id": native.chip_eco_id,
            "chip_model": native.chip_model,
            "chip_product_id": native.chip_product_id,
            "chip_revision": native.chip_revision,
            "customer_id": native.customer_id,
            "output": native.output,
            "selector_backend": native.selector_backend,
            "selector_kernel": native.selector_kernel,
        },
        "remote_exit": remote_exit,
    }
    _write_json_exclusive(
        run_root / f"{candidate.candidate_id}.capture.json",
        metadata,
    )
    return capture, postflight


def cleanup_exact_stage(
    transport: Any,
    manifest: ExperimentManifest,
) -> dict[str, Any]:
    """Remove only the exact PoC 12B stage root and verify absence."""

    if manifest.remote_root != REMOTE_ROOT:
        raise PreflightError("cleanup root differs from the compiled policy")
    before = collect_snapshot(transport, manifest)
    if before.get("serials") != [manifest.device.serial]:
        raise PreflightError("cleanup device identity is not exclusive")
    if before.get("probe_processes"):
        raise PreflightError(
            "cleanup refused while the PoC 12B process is visible"
        )
    if before.get("galcore_holders"):
        raise PreflightError(
            "cleanup refused while /dev/galcore has a holder"
        )
    result = transport.run(
        (
            "shell",
            "rm -rf '/tmp/wave-npu-poc12b' && "
            "if [ -e '/tmp/wave-npu-poc12b' ] || "
            "[ -L '/tmp/wave-npu-poc12b' ]; then exit 31; fi",
        ),
        SNAPSHOT_TIMEOUT_MS,
    )
    if result.timed_out or result.returncode != 0:
        raise PreflightError("exact stage-root cleanup failed")
    after = collect_snapshot(transport, manifest)
    differences, alerts = compare_health(
        manifest, before, after, require_staged=False
    )
    if differences:
        raise PreflightError(
            "cleanup verification failed: " + "; ".join(differences)
        )
    return {
        "alerts": alerts,
        "remote_root": manifest.remote_root,
        "removed": True,
        "snapshot": after,
    }


def run_live_sequence(
    transport: Any,
    manifest: ExperimentManifest,
    run_root: Path = DEFAULT_RUN_ROOT,
) -> tuple[dict[str, Any], Path]:
    """Run the one approved staged A1/A2/B sequence with exact cleanup."""

    if not manifest.hash_frozen:
        raise PreflightError("probe binary hash is not frozen")
    if run_root.resolve() != DEFAULT_RUN_ROOT.resolve():
        raise PreflightError("run root differs from the fixed policy")
    run_root.parent.mkdir(parents=True, exist_ok=True)
    try:
        run_root.mkdir(mode=0o700)
    except FileExistsError as exc:
        raise PreflightError("raw run directory already exists") from exc
    envelope: dict[str, Any] = {
        "candidate_captures": [],
        "classification": "compile-blocked",
        "cleanup": None,
        "errors": [],
        "finished_at": None,
        "manifest_sha256": manifest.sha256,
        "native_invocations": 0,
        "poc_id": manifest.poc_id,
        "prohibited_actions": dict(manifest.prohibited_actions),
        "schema_version": SCHEMA_VERSION,
        "started_at": _timestamp(),
        "status": "INCOMPLETE",
    }
    _write_json_exclusive(
        run_root / "definition.json",
        {
            "candidates": {
                item.label: list(item.constant_u8)
                for item in manifest.candidates.values()
            },
            "executes_graph": manifest.graph.executes_graph,
            "graph": {
                "attribute": manifest.graph.attribute,
                "data_type": manifest.graph.data_type,
                "operation": manifest.graph.operation,
                "shape": list(manifest.graph.shape),
            },
            "manifest_sha256": manifest.sha256,
            "schema_version": SCHEMA_VERSION,
        },
    )
    baseline: dict[str, Any] | None = None
    latest: dict[str, Any] | None = None
    try:
        baseline = collect_snapshot(transport, manifest)
        _write_json_exclusive(run_root / "preflight.json", baseline)
        verify_snapshot(manifest, baseline, staged=True)
        latest = baseline

        def capture(candidate: CandidateSpec) -> CandidateCapture:
            nonlocal latest
            if envelope["native_invocations"] >= 3:
                raise ResultError("native invocation bound exceeded")
            envelope["native_invocations"] += 1
            captured, postflight = _capture_candidate(
                transport,
                manifest,
                candidate,
                latest or baseline,
                run_root,
            )
            latest = postflight
            envelope["candidate_captures"].append(
                {
                    "bytes": captured.bytes,
                    "candidate": captured.candidate_id,
                    "sha256": captured.sha256,
                }
            )
            return captured

        sequence = run_sequence_with_capture(manifest, capture)
        envelope["classification"] = sequence.classification
        envelope["a_controls_exact"] = sequence.a_controls_exact
        envelope["a_first_difference"] = sequence.first_difference
        final_snapshot = collect_snapshot(transport, manifest)
        _write_json_exclusive(
            run_root / "postflight.json", final_snapshot
        )
        health, alerts = compare_health(
            manifest,
            baseline,
            final_snapshot,
            require_staged=True,
        )
        envelope["health_differences"] = health
        envelope["kernel_alerts"] = alerts
        if health:
            envelope["classification"] = "device-health-failure"
            envelope["errors"].extend(health)
    except (ConstantDiffLabError, OSError) as exc:
        envelope["errors"].append(str(exc))
        envelope["classification"] = "compile-blocked"
    finally:
        try:
            cleanup = cleanup_exact_stage(transport, manifest)
            envelope["cleanup"] = {
                "alerts": cleanup["alerts"],
                "remote_root": cleanup["remote_root"],
                "removed": cleanup["removed"],
            }
            _write_json_exclusive(
                run_root / "cleanup.json", cleanup
            )
        except (ConstantDiffLabError, OSError) as exc:
            envelope["errors"].append(str(exc))
            envelope["classification"] = "device-health-failure"
    envelope["status"] = "PASS" if not envelope["errors"] else "FAIL"
    envelope["finished_at"] = _timestamp()
    _write_json_exclusive(run_root / "run-envelope.json", envelope)
    return envelope, run_root


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Fixed PoC 12B constant-input Erf compile-only harness; "
            "never executes an NBG"
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("manifest")
    subparsers.add_parser("preflight")
    subparsers.add_parser("cleanup")
    run = subparsers.add_parser("run")
    run.add_argument(
        "--run-id",
        default="poc12b-erf-constant-diff-20260729",
        choices=("poc12b-erf-constant-diff-20260729",),
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        manifest = load_manifest()
        if args.command == "manifest":
            print(
                canonical_json(
                    {
                        "binary_hash_state": manifest.binary_hash_state,
                        "candidates": list(manifest.candidates),
                        "executes_graph": manifest.graph.executes_graph,
                        "manifest_sha256": manifest.sha256,
                        "poc_id": manifest.poc_id,
                    }
                ).decode(),
                end="",
            )
            return 0
        if not manifest.hash_frozen:
            raise PreflightError(
                "probe hash is not frozen; build and update the manifest "
                "before device preflight"
            )
        transport = npu_lab.AdbTransport()
        if args.command == "preflight":
            snapshot = collect_snapshot(transport, manifest)
            verify_snapshot(manifest, snapshot, staged=False)
            print(canonical_json(snapshot).decode(), end="")
            return 0
        if args.command == "cleanup":
            result = cleanup_exact_stage(transport, manifest)
            print(canonical_json(result).decode(), end="")
            return 0
        envelope, run_root = run_live_sequence(
            transport, manifest, DEFAULT_RUN_ROOT
        )
        print(
            canonical_json(
                {
                    "classification": envelope["classification"],
                    "native_invocations": envelope[
                        "native_invocations"
                    ],
                    "run_root": run_root.relative_to(
                        REPOSITORY_ROOT
                    ).as_posix(),
                    "status": envelope["status"],
                }
            ).decode(),
            end="",
        )
        return 0 if envelope["status"] == "PASS" else 1
    except ConstantDiffLabError as exc:
        print(f"poc12b: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
