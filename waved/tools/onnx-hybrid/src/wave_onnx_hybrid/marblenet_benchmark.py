"""Generate the fixed same-device MarbleNet benchmark and inspect sample receipts.

The caller owns native staging, exact artifact hashes and qualified package
admission. This module never instantiates ORT or contacts the device.
"""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any

from .marblenet_sources import _record, _root
from .native_sources import _definition

_REPOSITORY = Path(__file__).resolve().parents[5]
_TEMPLATE_PATH = (
    _REPOSITORY / "waved/tools/npu-compiler/marblenet/benchmark_fp16_graph.cc.in"
)
_TEMPLATE_SHA256 = "bf8abce2f1213fc98d3757ad831027f17a2ceb30fca5a111bd97fc46aaf77468"
_ORT_HEADER_PATH = (
    _REPOSITORY
    / "artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/onnxruntime_c_api.h"
)
_ORT_HEADER_SHA256 = "cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91"
_ORT_LIBRARY_SHA256 = "457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639"
_TIMING_KEYS = {
    "cpu_boundary_ns",
    "cpu_thread_ns",
    "cpu_input_copy_ns",
    "cpu_run_ns",
    "cpu_postprocess_ns",
    "npu_boundary_ns",
    "npu_thread_ns",
    "npu_input_copy_ns",
    "npu_canary_copy_ns",
    "npu_run_ns",
    "npu_output_copy_ns",
    "npu_postprocess_ns",
}
_PROBABILITY_KEYS = {"cpu_speech_probability_sum", "npu_speech_probability_sum"}


def generate_benchmark_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int, cpu_model_bytes: int
) -> str:
    """Emit one fixed36-pair program; qualification is required independently."""
    root = _root(remote_root, "execute")
    _record(record)
    for key, name, shape, size in (
        ("onnx_input", "input", [1, 80, 400], 64000),
        ("onnx_output", "output", [1, 2, 200], 800),
        ("native_input", "input", [400, 1, 80, 1], 64000),
        ("native_output", "output", [200, 1, 2, 1], 800),
    ):
        if record[key] != {
            "name": name,
            "shape": shape,
            "dtype": "FLOAT16",
            "byte_size": size,
        }:
            raise ValueError("benchmark requires the fixed full MarbleNet I/O contract")
    if type(nbg_bytes) is not int or not 513 <= nbg_bytes <= 64 * 1024 * 1024:
        raise ValueError("invalid benchmark NBG size")
    if type(cpu_model_bytes) is not int or not 1 <= cpu_model_bytes <= 8 * 1024 * 1024:
        raise ValueError("invalid benchmark CPU model size")
    data = _TEMPLATE_PATH.read_bytes()
    if hashlib.sha256(data).hexdigest() != _TEMPLATE_SHA256:
        raise ValueError("immutable benchmark template hash mismatch")
    if hashlib.sha256(_ORT_HEADER_PATH.read_bytes()).hexdigest() != _ORT_HEADER_SHA256:
        raise ValueError("pinned ORT C API15 header hash mismatch")
    definition = {
        "schema_version": 1,
        "mode": "execute-once",
        "operation": "NBG",
        "model_operation": "fp16-marblenet-benchmark",
        "lowering_sha256": record["lowering_sha256"],
        "source_model_sha256": record["source_model_sha256"],
        "graph_signature": record["graph_signature"],
        "model": f"{root}/model/model.nbg",
        "model_bytes": nbg_bytes,
        "cpu_model": f"{root}/model/cpu.onnx",
        "cpu_model_bytes": cpu_model_bytes,
        "cpu_ort_version": "1.15.1",
        "cpu_ort_api": 15,
        "cpu_ort_library": "/usr/lib/libonnxruntime.so.1.15.1",
        "cpu_ort_library_sha256": _ORT_LIBRARY_SHA256,
        "cpu_ort_header_sha256": _ORT_HEADER_SHA256,
        "cpu_provider": "CPUExecutionProvider",
        "cpu_threads": 1,
        "feature_extraction_excluded": True,
        "prepared_feature_inputs": True,
        "float32_to_float16_input_conversion_excluded": True,
        "cpu_input_bytes": 128000,
        "cpu_reference_bytes": 1600,
        "native_reference_bytes": 800,
        "input_file": f"{root}/input/input.bin",
        "output_file": f"{root}/output/output.bin",
        "inputs": [{"type": "FLOAT16", "shape": [400, 1, 80, 1]}],
        "outputs": [{"type": "FLOAT16", "shape": [200, 1, 2, 1]}],
        "warmup_iterations": 4,
        "measured_iterations": 32,
        "iterations": 36,
        "executes_graph": True,
        "output_prime_half_bits": "0x7e00",
        "boundary_includes_npu_canary_copy": True,
    }
    source = data.decode("utf-8")
    for key, value in {
        "ROOT": root,
        "NBG_BYTES": str(nbg_bytes),
        "CPU_MODEL_BYTES": str(cpu_model_bytes),
        "DEFINITION": _definition(definition),
    }.items():
        source = source.replace(f"@@{key}@@", value)
    if "@@" in source:
        raise ValueError("unresolved benchmark source marker")
    return source


def validate_benchmark_source(
    record: dict[str, Any],
    remote_root: str,
    nbg_bytes: int,
    cpu_model_bytes: int,
    source: str,
) -> None:
    if source != generate_benchmark_source(
        record, remote_root, nbg_bytes, cpu_model_bytes
    ):
        raise ValueError("benchmark source policy mismatch")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate benchmark JSON field")
        result[key] = value
    return result


def parse_benchmark_samples(stdout: str) -> list[dict[str, Any]]:
    """Validate32 measured pairs; native PASS/health are separate bridge checks."""
    if not isinstance(stdout, str) or len(stdout) > 1024 * 1024:
        raise ValueError("invalid bounded benchmark stdout")
    result = []
    keys = _TIMING_KEYS | _PROBABILITY_KEYS | {"sample_index", "iteration", "order"}
    for line in stdout.splitlines():
        if not line.startswith("BENCH_SAMPLE"):
            continue
        if not line.startswith("BENCH_SAMPLE ") or len(line) > 4096:
            raise ValueError("invalid benchmark sample line")
        try:
            value = json.loads(
                line.removeprefix("BENCH_SAMPLE "), object_pairs_hook=_unique_object
            )
        except (json.JSONDecodeError, TypeError) as error:
            raise ValueError("invalid benchmark sample JSON") from error
        i = len(result)
        if not isinstance(value, dict) or set(value) != keys or i >= 32:
            raise ValueError("invalid benchmark sample schema/count")
        if (
            type(value["sample_index"]) is not int
            or value["sample_index"] != i
            or type(value["iteration"]) is not int
            or value["iteration"] != i + 4
            or value["order"] != ("cpu-npu" if i % 2 == 0 else "npu-cpu")
        ):
            raise ValueError("benchmark iteration/order mismatch")
        for key in _TIMING_KEYS:
            if type(value[key]) is not int or not 0 <= value[key] <= 8_000_000_000:
                raise ValueError("invalid benchmark timing")
        for role, components in (
            ("cpu", ("input_copy", "run", "postprocess")),
            ("npu", ("input_copy", "canary_copy", "run", "output_copy", "postprocess")),
        ):
            boundary = value[f"{role}_boundary_ns"]
            if (
                boundary == 0
                or sum(value[f"{role}_{component}_ns"] for component in components)
                > boundary
            ):
                raise ValueError("benchmark component/boundary timing mismatch")
        for key in _PROBABILITY_KEYS:
            if (
                type(value[key]) not in (int, float)
                or not math.isfinite(value[key])
                or not 0 <= value[key] <= 200
            ):
                raise ValueError("invalid benchmark probability checksum")
        result.append(value)
    if len(result) != 32:
        raise ValueError("benchmark requires32 measured samples")
    return result
