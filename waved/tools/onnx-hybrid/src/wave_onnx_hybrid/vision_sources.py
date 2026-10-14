"""Exact source emission for the fixed SqueezeNet spatial graph recipes."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any

from .errors import HybridError
from .marblenet_sources import _constants, _patterns, _shape, _tensor_creation
from .native_sources import _definition
from .native_sources import _template as _execute_template
from .vision_graph import PROFILE_ID, native_shape, validate_graph_record

_TEMPLATE_ROOT = Path(__file__).resolve().parents[3] / "npu-compiler" / "vision"
_COMPILE_TEMPLATE_SHA256 = (
    "7af1228477e0fb8738655f18c6521725e6ccaf7c5b6bbf5bef6034d0ac9cd55b"
)
# Constants are emitted as fixed uint16/uint32 patterns in read-only storage.
# These independent limits also cover generated text and runtime stack arrays.
_MAX_SOURCE_BYTES = 48 * 1024 * 1024
_MAX_EMITTED_CONSTANT_BYTES = 8 * 1024 * 1024
_MAX_STACK_PAYLOAD_BYTES = 128 * 1024
_MEAN_PROFILE = "fp16-squeezenet-mean-v1"


def _root(value: str, mode: str, profile_id: str = PROFILE_ID) -> str:
    version = 21 if profile_id == _MEAN_PROFILE else 20
    if not isinstance(value, str) or not re.fullmatch(
        rf"/tmp/wave-npu-onnx{version}-[0-9a-f]{{16}}-{mode}", value
    ):
        raise ValueError(f"unsafe {mode} remote root")
    return value


def _record(record: dict[str, Any]) -> None:
    mean = isinstance(record, dict) and record.get("profile_id") == _MEAN_PROFILE
    try:
        if mean:
            from .vision_mean import validate_graph_record as validate_mean

            validate_mean(record)
        else:
            validate_graph_record(record)
    except HybridError as error:
        raise ValueError(f"invalid vision lowering record: {error}") from error
    if record["profile_id"] not in (PROFILE_ID, _MEAN_PROFILE):
        raise ValueError("unsupported vision source profile")
    emitted = sum(
        record["constants"][n["weights"]]["byte_size"] + 4 * n["output_shape"][1]
        for n in record["nodes"]
        if n["type"] == "Conv"
    )
    if emitted > _MAX_EMITTED_CONSTANT_BYTES:
        raise ValueError("emitted vision constant byte bound exceeded")
    # Input bytes are file-backed heap vectors. The largest stack allocation is
    # output/readback storage, plus retained per-Conv promoted bias arrays.
    bias_bytes = sum(
        4 * n["output_shape"][1] for n in record["nodes"] if n["type"] == "Conv"
    )
    if mean:
        for node in record["nodes"]:
            if node["type"] == "GlobalAveragePool":
                channels = node["input_shape"][1]
                emitted += 169 * channels * 2 + channels * 4
                bias_bytes += channels * 4
        if emitted > _MAX_EMITTED_CONSTANT_BYTES:
            raise ValueError("emitted derived mean constant byte bound exceeded")
    if (
        3 * record["native_output"]["byte_size"] + 64 + bias_bytes
        > _MAX_STACK_PAYLOAD_BYTES
    ):
        raise ValueError("vision stack payload bound exceeded")


def _bounded(source: str) -> str:
    if len(source.encode("utf-8")) > _MAX_SOURCE_BYTES or "@@" in source:
        raise ValueError("vision generated source bound/marker violation")
    return source


def _graph(record: dict[str, Any]) -> tuple[str, list[dict[str, Any]]]:
    nodes = record["nodes"]
    mean_profile = record["profile_id"] == _MEAN_PROFILE
    ids = {record["onnx_input"]["name"]: 0}
    ids.update({n["output"]: i + 1 for i, n in enumerate(nodes)})
    count = sum(
        n["type"] == "Conv" or (mean_profile and n["type"] == "GlobalAveragePool")
        for n in nodes
    )
    lines = [
        "    std::vector<std::shared_ptr<tim::vx::Tensor>> tensors;",
        "    std::vector<std::shared_ptr<tim::vx::Operation>> operations;",
        f"    tensors.reserve({1 + len(nodes) + 2 * count});",
        f"    operations.reserve({len(nodes)});",
    ]
    lines.extend(_tensor_creation(0, record["native_input"]["shape"], "INPUT"))
    for i, n in enumerate(nodes):
        lines.extend(
            _tensor_creation(
                i + 1,
                native_shape(n["output_shape"]),
                "OUTPUT"
                if n["output"] == record["onnx_output"]["name"]
                else "TRANSIENT",
            )
        )
    ci = 1 + len(nodes)
    definitions = []
    for i, n in enumerate(nodes):
        inputs, output = [ids[v] for v in n["inputs"]], ids[n["output"]]
        definition = {
            "id": i,
            "type": n["type"],
            "inputs": inputs.copy(),
            "output": output,
        }
        typ = n["type"]
        mean = mean_profile and typ == "GlobalAveragePool"
        if mean:
            n = {
                **n,
                "kernel": [13, 13],
                "pads": [0, 0, 0, 0],
                "strides": [1, 1],
                "dilations": [1, 1],
            }
        if typ == "Conv" or mean:
            co = n["output_shape"][1]
            weight_shape = (
                [13, 13, co, 1]
                if mean
                else [n["kernel"][1], n["kernel"][0], n["input_shape"][1], co]
            )
            weight_name = f"kMeanWeights{i}" if mean else f"kWeights{i}"
            bias_name = f"kMeanBiasBits{i}" if mean else f"kBiasBits{i}"
            multiplier = 1 if mean else 0
            pads = [n["pads"][1], n["pads"][3], n["pads"][0], n["pads"][2]]
            strides, dilations = n["strides"][::-1], n["dilations"][::-1]
            lines.extend(
                [
                    f"    const tim::vx::ShapeType "
                    f"weight_shape{i}{{{_shape(weight_shape)}}};",
                    f"    const tim::vx::TensorSpec weight_spec{i}(",
                    "        tim::vx::DataType::FLOAT16,",
                    f"        weight_shape{i}, tim::vx::TensorAttribute::CONSTANT);",
                    "    tensors.push_back(graph->CreateTensor("
                    f"weight_spec{i}, {weight_name}.data()));",
                    "    if (!tensors.back()) {",
                    '        std::fprintf(stderr, "wave_npu_onnx_compile: '
                    'weight create failed\\n");',
                    "        return 3;",
                    "    }",
                    f"    const tim::vx::ShapeType bias_shape{i}{{{co}}};",
                    f"    const tim::vx::TensorSpec bias_spec{i}(",
                    "        tim::vx::DataType::FLOAT32,",
                    f"        bias_shape{i}, tim::vx::TensorAttribute::CONSTANT);",
                    f"    std::array<float, {co}> bias_values{i}{{}};",
                    f"    std::memcpy(bias_values{i}.data(), "
                    f"{bias_name}.data(), sizeof({bias_name}));",
                    "    tensors.push_back(graph->CreateTensor("
                    f"bias_spec{i}, bias_values{i}.data()));",
                    "    if (!tensors.back()) {",
                    '        std::fprintf(stderr, "wave_npu_onnx_compile: '
                    'bias create failed\\n");',
                    "        return 3;",
                    "    }",
                    f"    const std::array<uint32_t, 4> pad{i}{{{_shape(pads)}}};",
                    f"    const std::array<uint32_t, 2> "
                    f"stride{i}{{{_shape(strides)}}};",
                    f"    const std::array<uint32_t, 2> "
                    f"dilation{i}{{{_shape(dilations)}}};",
                    "    operations.push_back(graph->CreateOperation<"
                    "tim::vx::ops::Conv2d>(",
                    f"        pad{i}, stride{i}, dilation{i}, {multiplier}));",
                ]
            )
            inputs.extend([ci, ci + 1])
            ci += 2
            definition.update(
                weight_shape=weight_shape,
                bias_shape=[co],
                pads=pads,
                stride=strides,
                dilation=dilations,
                multiplier=multiplier,
                bias_promotion="fixed-f32-positive-zero"
                if mean
                else "exact-f16-to-f32",
            )
            if mean:
                definition.update(
                    native_operation="Conv2d",
                    effective_group=co,
                    derived_weight_half_bits=record["native_policy"]["half_bits"],
                    numerical_mean_approximation=True,
                )
        elif typ in {"MaxPool", "GlobalAveragePool"}:
            maximum = typ == "MaxPool"
            kernel = (
                n["kernel"][::-1]
                if maximum
                else [n["input_shape"][3], n["input_shape"][2]]
            )
            stride = n["strides"][::-1] if maximum else kernel
            round_type = "CEILING" if maximum else "FLOOR"
            pool_type = "MAX" if maximum else "AVG"
            lines.extend(
                [
                    f"    const std::array<uint32_t, 4> pool_pad{i}{{0, 0, 0, 0}};",
                    f"    const std::array<uint32_t, 2> "
                    f"pool_kernel{i}{{{_shape(kernel)}}};",
                    f"    const std::array<uint32_t, 2> "
                    f"pool_stride{i}{{{_shape(stride)}}};",
                    "    operations.push_back(graph->CreateOperation<"
                    "tim::vx::ops::Pool2d>(",
                    f"        tim::vx::PoolType::{pool_type}, pool_pad{i}, "
                    f"pool_kernel{i},",
                    f"        pool_stride{i}, tim::vx::RoundType::{round_type}));",
                ]
            )
            definition.update(
                pool_type=pool_type,
                pads=[0] * 4,
                kernel=kernel,
                stride=stride,
                round_type=round_type,
            )
        elif typ == "Concat":
            lines.append(
                "    operations.push_back(graph->CreateOperation<"
                "tim::vx::ops::Concat>(2, 2));"
            )
            definition.update(axis=2, input_count=2)
        elif typ == "Flatten":
            shape = native_shape(n["output_shape"])
            lines.extend(
                [
                    f"    const std::vector<uint32_t> "
                    f"reshape_target{i}{{{_shape(shape)}}};",
                    "    operations.push_back(graph->CreateOperation<"
                    "tim::vx::ops::Reshape>(",
                    f"        reshape_target{i}));",
                ]
            )
            definition.update(native_operation="Reshape", target_shape=shape)
        else:
            lines.append(
                "    operations.push_back(graph->CreateOperation<"
                "tim::vx::ops::Relu>());"
            )
        lines.extend(
            [
                "    if (!operations.back()) {",
                '        std::fprintf(stderr, "wave_npu_onnx_compile: '
                'operation create failed\\n");',
                "        return 4;",
                "    }",
                f"    operations[{i}]->BindInput(tensors[{inputs[0]}])",
                *(f"        .BindInput(tensors[{v}])" for v in inputs[1:]),
                f"        .BindOutput(tensors[{output}]);",
            ]
        )
        definitions.append(definition)
    return "\n".join(lines), definitions


def generate_compile_source(record: dict[str, Any], remote_root: str) -> str:
    """Emit numeric IDs/actual bits and one size query plus one binary export."""
    root = _root(remote_root, "compile", record.get("profile_id"))
    _record(record)
    data = (_TEMPLATE_ROOT / "compile_fp16_graph.cc.in").read_bytes()
    if hashlib.sha256(data).hexdigest() != _COMPILE_TEMPLATE_SHA256:
        raise ValueError("immutable vision source template hash mismatch")
    graph, nodes = _graph(record)
    definition = {
        "schema_version": 1,
        "mode": "compile-only",
        "candidates": [1],
        "target": {
            "chip_model": "0x00009000",
            "customer_id": "0x1000000e",
            "nbg_version": "0x00020000",
        },
        "operation": "VisionGraph",
        "backend_required": "OPENVX",
        "model": record["profile_id"],
        "data_type": "FLOAT16",
        "executes_graph": False,
        "input": {"dtype": "FLOAT16", "shape": record["native_input"]["shape"]},
        "output": {"dtype": "FLOAT16", "shape": record["native_output"]["shape"]},
        "node_count": len(nodes),
        "nodes": nodes,
        "native_policy": record["native_policy"],
        "control_id": record["control_id"],
        "lowering_sha256": record["lowering_sha256"],
    }
    # The reused numeric constants helper reads actual F16 payloads and exactly
    # promotes their bias values; replace its 1-D comment for this separate route.
    constants = _constants(record).replace(
        "ONNX OIK bytes already match native K1IO",
        "ONNX OIHW bytes already match native WHIO",
    )
    if record["profile_id"] == _MEAN_PROFILE:
        definition["source_original_model_sha256"] = record[
            "source_original_model_sha256"
        ]
        derived = []
        for index, node in enumerate(record["nodes"]):
            if node["type"] == "GlobalAveragePool":
                channels = node["input_shape"][1]
                derived.extend(
                    [
                        _patterns(
                            f"kMeanWeights{index}",
                            (record["native_policy"]["half_bits"],) * (169 * channels),
                            4,
                        ),
                        _patterns(f"kMeanBiasBits{index}", (0,) * channels, 8),
                    ]
                )
        constants += "\n// Fixed native mean approximation; learned bits unchanged.\n"
        constants += "\n".join(derived)
    source = data.decode("utf-8")
    for key, value in {
        "ROOT": root,
        "CONSTANTS": constants,
        "GRAPH": graph,
        "DEFINITION": _definition(definition),
    }.items():
        source = source.replace(f"@@{key}@@", value)
    return _bounded(source)


def generate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int
) -> str:
    """Run one fixed NBG with a half-NaN output prime and actual byte readback."""
    root = _root(remote_root, "execute", record.get("profile_id"))
    _record(record)
    if type(nbg_bytes) is not int or not 513 <= nbg_bytes <= 64 * 1024 * 1024:
        raise ValueError("invalid fixed NBG byte size")
    operation = (
        "fp16-squeezenet-mean"
        if record["profile_id"] == _MEAN_PROFILE
        else "fp16-squeezenet"
    )
    tensors = []
    for direction, attribute in (("input", "INPUT"), ("output", "OUTPUT")):
        tensors.extend(
            [
                f"    const tim::vx::ShapeType {direction}_shape"
                f"{{{_shape(record[f'native_{direction}']['shape'])}}};",
                f"    const tim::vx::TensorSpec {direction}_spec(",
                "        tim::vx::DataType::FLOAT16,",
                f"        {direction}_shape, tim::vx::TensorAttribute::{attribute});",
            ]
        )
    definition = {
        "schema_version": 1,
        "mode": "execute-once",
        "operation": "NBG",
        "model": f"{root}/model/model.nbg",
        "model_bytes": nbg_bytes,
        "model_operation": operation,
        "input_file": f"{root}/input/input.bin",
        "output_file": f"{root}/output/output.bin",
        "inputs": [{"type": "FLOAT16", "shape": record["native_input"]["shape"]}],
        "outputs": [{"type": "FLOAT16", "shape": record["native_output"]["shape"]}],
        "iterations": 1,
        "executes_graph": True,
        "native_policy": record["native_policy"],
        "control_id": record["control_id"],
        "lowering_sha256": record["lowering_sha256"],
    }
    if record["profile_id"] == _MEAN_PROFILE:
        definition["source_original_model_sha256"] = record[
            "source_original_model_sha256"
        ]
    source = _execute_template(
        "execute.cc.in",
        {
            "ROOT": root,
            "NBG_BYTES": str(nbg_bytes),
            "INPUT_BYTES": str(record["native_input"]["byte_size"]),
            "OUTPUT_BYTES": str(record["native_output"]["byte_size"]),
            "TENSORS": "\n".join(tensors),
            "OPERATION": operation,
            "DEFINITION": _definition(definition),
        },
    )
    replacements = {
        "constexpr uint8_t kCanaryByte = 0xa5U;": (
            "constexpr uint16_t kCanaryHalfBits = 0x7e00U;"
        ),
        "    device_canary.fill(kCanaryByte);": (
            "    for (size_t index = 0; index < kOutputBytes; index += 2) {\n"
            "        device_canary[index] = "
            "static_cast<uint8_t>(kCanaryHalfBits & 0xffU);\n"
            "        device_canary[index + 1] = "
            "static_cast<uint8_t>(kCanaryHalfBits >> 8U);\n"
            "    }"
        ),
        (
            "    std::memset(guarded_output.data() + kGuardBytes, "
            "kCanaryByte, kOutputBytes);"
        ): (
            "    for (size_t index = 0; index < kOutputBytes; index += 2) {\n"
            "        guarded_output[kGuardBytes + index] =\n"
            "            static_cast<uint8_t>(kCanaryHalfBits & 0xffU);\n"
            "        guarded_output[kGuardBytes + index + 1] =\n"
            "            static_cast<uint8_t>(kCanaryHalfBits >> 8U);\n"
            "    }"
        ),
    }
    for previous, replacement in replacements.items():
        if source.count(previous) != 1:
            raise ValueError("pinned vision executor NaN prime replacement mismatch")
        source = source.replace(previous, replacement)
    return _bounded(source)


def validate_compile_source(
    record: dict[str, Any], remote_root: str, source: str
) -> None:
    if source != generate_compile_source(record, remote_root):
        raise ValueError("vision compile source policy mismatch")


def validate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int, source: str
) -> None:
    if source != generate_execute_source(record, remote_root, nbg_bytes):
        raise ValueError("vision execute source policy mismatch")
