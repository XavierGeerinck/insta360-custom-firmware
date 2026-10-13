"""Fixed stock-ORT CPU fallback source and independently bounded sample parser."""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any

from .marblenet_benchmark import (
    _ORT_HEADER_PATH,
    _ORT_HEADER_SHA256,
    _ORT_LIBRARY_SHA256,
    _unique_object,
)
from .marblenet_sources import _record, _root
from .native_sources import _definition

_TEMPLATE_PATH = (
    Path(__file__).resolve().parents[3] / "npu-compiler/marblenet/cpu_fp32_graph.cc.in"
)
_TEMPLATE_SHA256 = "59b32de19b5052eaed63c8fceef13df1a584bc8ae0f788f55d280466e5c965c9"
_TIMINGS = {"boundary_ns", "thread_ns", "input_copy_ns", "run_ns", "postprocess_ns"}


def generate_cpu_source(
    record: dict[str, Any], remote_root: str, cpu_model_bytes: int
) -> str:
    """Emit36 checked CPU runs; no hardware initialization or vendor NPU link."""
    root = _root(remote_root, "execute")
    _record(record)
    if record["onnx_input"] != {
        "name": "input",
        "dtype": "FLOAT16",
        "shape": [1, 80, 400],
        "byte_size": 64000,
    } or record["onnx_output"] != {
        "name": "output",
        "dtype": "FLOAT16",
        "shape": [1, 2, 200],
        "byte_size": 800,
    }:
        raise ValueError("CPU runner requires the fixed full MarbleNet neural contract")
    if type(cpu_model_bytes) is not int or not 1 <= cpu_model_bytes <= 8 * 1024 * 1024:
        raise ValueError("invalid CPU model size")
    data = _TEMPLATE_PATH.read_bytes()
    if hashlib.sha256(data).hexdigest() != _TEMPLATE_SHA256:
        raise ValueError("immutable CPU template hash mismatch")
    if hashlib.sha256(_ORT_HEADER_PATH.read_bytes()).hexdigest() != _ORT_HEADER_SHA256:
        raise ValueError("pinned CPU ORT C API15 header hash mismatch")
    definition = {
        "schema_version": 1,
        "mode": "execute-once",
        "operation": "CPU",
        "model_operation": "fp32-marblenet-cpu",
        "executes_graph": True,
        "lowering_sha256": record["lowering_sha256"],
        "source_model_sha256": record["source_model_sha256"],
        "graph_signature": record["graph_signature"],
        "model": f"{root}/model/cpu.onnx",
        "model_bytes": cpu_model_bytes,
        "input_file": f"{root}/input/cpu-input.bin",
        "reference_file": f"{root}/input/cpu-reference.bin",
        "output_file": f"{root}/output/output.bin",
        "inputs": [{"type": "FLOAT32", "shape": [1, 80, 400], "byte_size": 128000}],
        "outputs": [{"type": "FLOAT32", "shape": [1, 2, 200], "byte_size": 1600}],
        "ort_version": "1.15.1",
        "ort_api": 15,
        "threads": 1,
        "provider": "CPUExecutionProvider",
        "ort_library_sha256": _ORT_LIBRARY_SHA256,
        "ort_header_sha256": _ORT_HEADER_SHA256,
        "numerical_policy": {"atol": 0.0001, "rtol": 0.00001, "finite_required": True},
        "warmup_iterations": 4,
        "measured_iterations": 32,
        "iterations": 36,
        "feature_extraction_excluded": True,
        "prepared_feature_inputs": True,
        "output_prime": "FLOAT32_QUIET_NAN",
    }
    source = data.decode("utf-8")
    for key, value in {
        "ROOT": root,
        "CPU_MODEL_BYTES": str(cpu_model_bytes),
        "DEFINITION": _definition(definition),
    }.items():
        source = source.replace(f"@@{key}@@", value)
    if "@@" in source:
        raise ValueError("unresolved CPU source marker")
    return source


def validate_cpu_source(
    record: dict[str, Any], remote_root: str, cpu_model_bytes: int, source: str
) -> None:
    if source != generate_cpu_source(record, remote_root, cpu_model_bytes):
        raise ValueError("CPU source policy mismatch")


def parse_cpu_samples(stdout: str) -> list[dict[str, Any]]:
    """Validate32 actual CPU timings; identity/PASS/health are harness-owned."""
    if not isinstance(stdout, str) or len(stdout) > 1024 * 1024:
        raise ValueError("invalid bounded CPU stdout")
    result = []
    keys = _TIMINGS | {"sample_index", "iteration", "speech_probability_sum"}
    for line in stdout.splitlines():
        if not line.startswith("CPU_SAMPLE"):
            continue
        if not line.startswith("CPU_SAMPLE ") or len(line) > 2048:
            raise ValueError("invalid CPU sample line")
        try:
            value = json.loads(
                line.removeprefix("CPU_SAMPLE "), object_pairs_hook=_unique_object
            )
        except (json.JSONDecodeError, TypeError) as error:
            raise ValueError("invalid CPU sample JSON") from error
        i = len(result)
        if not isinstance(value, dict) or set(value) != keys or i >= 32:
            raise ValueError("invalid CPU sample schema/count")
        if (
            type(value["sample_index"]) is not int
            or value["sample_index"] != i
            or type(value["iteration"]) is not int
            or value["iteration"] != i + 4
        ):
            raise ValueError("CPU iteration order mismatch")
        for key in _TIMINGS:
            if type(value[key]) is not int or not 0 <= value[key] <= 8_000_000_000:
                raise ValueError("invalid CPU timing")
        if (
            value["boundary_ns"] == 0
            or sum(value[k] for k in ("input_copy_ns", "run_ns", "postprocess_ns"))
            > value["boundary_ns"]
        ):
            raise ValueError("CPU component/boundary timing mismatch")
        probability = value["speech_probability_sum"]
        if (
            type(probability) not in (int, float)
            or not math.isfinite(probability)
            or not 0 <= probability <= 200
        ):
            raise ValueError("invalid CPU probability checksum")
        result.append(value)
    if len(result) != 32:
        raise ValueError("CPU runner requires32 measured samples")
    return result
