"""Hash-pinned source generation for the bounded FP16 MarbleNet graph family.

Graph admission is delegated to the importer. This module emits numeric tensor
identifiers and bit patterns; native qualification remains a separate device gate.
"""

from __future__ import annotations

import hashlib
import re
import struct
from pathlib import Path
from typing import Any

from .errors import HybridError
from .native_sources import _definition
from .native_sources import _template as _execute_template

_PROFILE = "fp16-marblenet-v1"
_PAD_PROFILE = "fp16-marblenet-pad-v1"
_PAD_NATIVE_POLICY = {"id": "flat-height-depthwise-symmetric-v1"}
_TEMPLATE_ROOT = Path(__file__).resolve().parents[3] / "npu-compiler" / "marblenet"
_COMPILE_TEMPLATE_SHA256 = (
    "2a196a53756e8e3b399d776d889ba7c417b8ac7713f6bed3dd0ff4cbf10860a3"
)
_PAD_COMPILE_TEMPLATE_SHA256 = (
    "8a340f1b573e707a0b704d41cb5f83438fdf6c2c5223488c73ec5ed789513845"
)


def _root(value: str, mode: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(
        rf"/tmp/wave-npu-onnx18-[0-9a-f]{{16}}-{mode}", value
    ):
        raise ValueError(f"unsafe {mode} remote root")
    return value


def _record(record: dict[str, Any]) -> None:
    from .marblenet_graph import validate_graph_record

    try:
        validate_graph_record(record)
    except HybridError as error:
        raise ValueError(f"invalid MarbleNet lowering record: {error}") from error
    if record["profile_id"] not in (_PROFILE, _PAD_PROFILE):
        raise ValueError("unsupported MarbleNet source profile")
    if record["profile_id"] == _PAD_PROFILE:
        if record.get("native_policy") != _PAD_NATIVE_POLICY:
            raise ValueError("unsupported MarbleNet native policy")
        for node in record["nodes"]:
            if node["type"] == "Conv":
                strides = node.get("native_strides")
                expected = [
                    node["stride"],
                    2 if node["group"] != 1 and node["stride"] == 2 else 1,
                ]
                if (
                    not isinstance(strides, list)
                    or len(strides) != 2
                    or any(type(value) is not int for value in strides)
                    or strides != expected
                ):
                    raise ValueError("unsupported flat depthwise native strides")


def _shape(shape: list[int]) -> str:
    return ", ".join(map(str, shape))


def _native_shape(shape: list[int]) -> list[int]:
    return [shape[2], 1, shape[1], 1]


def _patterns(name: str, values: tuple[int, ...], width: int) -> str:
    dtype = "uint16_t" if width == 4 else "uint32_t"
    return (
        f"constexpr std::array<{dtype}, {len(values)}> {name}{{"
        + ", ".join(f"0x{value:0{width}x}U" for value in values)
        + "};"
    )


def _constants(record: dict[str, Any]) -> str:
    lines = ["// ONNX OIK bytes already match native K1IO, dimension zero fastest."]
    for index, node in enumerate(record["nodes"]):
        if node["type"] != "Conv":
            continue
        weights = bytes.fromhex(record["constants"][node["weights"]]["payload_hex"])
        half_bits = struct.unpack(f"<{len(weights) // 2}H", weights)
        lines.append(_patterns(f"kWeights{index}", half_bits, 4))
        channels = node["output_shape"][1]
        bias_name = node["bias"]
        if bias_name is None:
            bias_bits = (0,) * channels
        else:
            bias = bytes.fromhex(record["constants"][bias_name]["payload_hex"])
            values = struct.unpack(f"<{channels}e", bias)
            bias_bits = struct.unpack(
                f"<{channels}I", struct.pack(f"<{channels}f", *values)
            )
        lines.append(_patterns(f"kBiasBits{index}", bias_bits, 8))
    return "\n".join(lines)


def _tensor_creation(index: int, shape: list[int], attribute: str) -> list[str]:
    return [
        f"    const tim::vx::ShapeType tensor_shape{index}{{{_shape(shape)}}};",
        f"    const tim::vx::TensorSpec tensor_spec{index}(",
        "        tim::vx::DataType::FLOAT16,",
        f"        tensor_shape{index}, tim::vx::TensorAttribute::{attribute});",
        f"    tensors.push_back(graph->CreateTensor(tensor_spec{index}));",
        "    if (!tensors.back()) {",
        "        std::fprintf(stderr, "
        '"wave_npu_onnx_compile: tensor create failed\\n");',
        "        return 3;",
        "    }",
    ]


def _graph(record: dict[str, Any]) -> tuple[str, list[dict[str, Any]]]:
    nodes = record["nodes"]
    padded_profile = record["profile_id"] == _PAD_PROFILE
    ids = {record["onnx_input"]["name"]: 0}
    ids.update({node["output"]: index + 1 for index, node in enumerate(nodes)})
    conv_count = sum(node["type"] == "Conv" for node in nodes)
    lines = [
        "    std::vector<std::shared_ptr<tim::vx::Tensor>> tensors;",
        "    std::vector<std::shared_ptr<tim::vx::Operation>> operations;",
        f"    tensors.reserve({1 + len(nodes) + 2 * conv_count});",
        f"    operations.reserve({len(nodes)});",
    ]
    lines.extend(_tensor_creation(0, record["native_input"]["shape"], "INPUT"))
    output_name = record["onnx_output"]["name"]
    for index, node in enumerate(nodes):
        attribute = "OUTPUT" if node["output"] == output_name else "TRANSIENT"
        lines.extend(
            _tensor_creation(index + 1, _native_shape(node["output_shape"]), attribute)
        )
    constant_index = 1 + len(nodes)
    definitions = []
    for index, node in enumerate(nodes):
        inputs = [ids[name] for name in node["inputs"]]
        output = ids[node["output"]]
        definition = {
            "id": index,
            "type": node["type"],
            "inputs": inputs,
            "output": output,
        }
        if node["type"] == "Conv":
            input_channels = node["input_shape"][1]
            output_channels = node["output_shape"][1]
            multiplier = 0 if node["group"] == 1 else 1
            native_strides = (
                node["native_strides"] if padded_profile else [node["stride"], 1]
            )
            weight_shape = [
                node["kernel"],
                1,
                input_channels,
                output_channels if multiplier == 0 else 1,
            ]
            lines.extend(
                [
                    f"    const tim::vx::ShapeType weight_shape{index}"
                    f"{{{_shape(weight_shape)}}};",
                    f"    const tim::vx::TensorSpec weight_spec{index}(",
                    "        tim::vx::DataType::FLOAT16,",
                    f"        weight_shape{index}, "
                    "tim::vx::TensorAttribute::CONSTANT);",
                    "    tensors.push_back(graph->CreateTensor("
                    f"weight_spec{index}, kWeights{index}.data()));",
                    "    if (!tensors.back()) {",
                    "        std::fprintf(stderr, "
                    '"wave_npu_onnx_compile: weight create failed\\n");',
                    "        return 3;",
                    "    }",
                    f"    const tim::vx::ShapeType bias_shape{index}"
                    f"{{{output_channels}}};",
                    f"    const tim::vx::TensorSpec bias_spec{index}(",
                    "        tim::vx::DataType::FLOAT32,",
                    f"        bias_shape{index}, tim::vx::TensorAttribute::CONSTANT);",
                    f"    std::array<float, {output_channels}> bias_values{index}{{}};",
                    f"    std::memcpy(bias_values{index}.data(), "
                    f"kBiasBits{index}.data(), sizeof(kBiasBits{index}));",
                    "    tensors.push_back(graph->CreateTensor("
                    f"bias_spec{index}, bias_values{index}.data()));",
                    "    if (!tensors.back()) {",
                    "        std::fprintf(stderr, "
                    '"wave_npu_onnx_compile: bias create failed\\n");',
                    "        return 3;",
                    "    }",
                    f"    const std::array<uint32_t, 4> pad{index}"
                    f"{{{_shape(node['pads'])}, 0, 0}};",
                    f"    const std::array<uint32_t, 2> stride{index}"
                    f"{{{_shape(native_strides)}}};",
                    f"    const std::array<uint32_t, 2> dilation{index}"
                    f"{{{node['dilation']}, 1}};",
                    "    operations.push_back(graph->CreateOperation<"
                    "tim::vx::ops::Conv2d>(",
                    f"        pad{index}, stride{index}, dilation{index}, "
                    f"{multiplier}));",
                ]
            )
            inputs.extend([constant_index, constant_index + 1])
            constant_index += 2
            definition.update(
                weight_shape=weight_shape,
                bias_shape=[output_channels],
                pads=[*node["pads"], 0, 0],
                stride=native_strides,
                dilation=[node["dilation"], 1],
                multiplier=multiplier,
            )
        elif node["type"] == "Pad":
            front = [node["pads"][0], 0, 0, 0]
            back = [node["pads"][1], 0, 0, 0]
            lines.extend(
                [
                    f"    const std::vector<uint32_t> pad_front{index}"
                    f"{{{_shape(front)}}};",
                    f"    const std::vector<uint32_t> pad_back{index}"
                    f"{{{_shape(back)}}};",
                    "    operations.push_back(graph->CreateOperation<"
                    "tim::vx::ops::Pad>(",
                    f"        pad_front{index}, pad_back{index}, 0, "
                    "tim::vx::ops::Pad::PAD_MODE_CONSTANT));",
                ]
            )
            definition.update(
                front_size=front,
                back_size=back,
                value=0,
                pad_mode="CONSTANT",
            )
        else:
            lines.append(
                "    operations.push_back(graph->CreateOperation<"
                f"tim::vx::ops::{node['type']}>());"
            )
        lines.extend(
            [
                "    if (!operations.back()) {",
                "        std::fprintf(stderr, "
                '"wave_npu_onnx_compile: operation create failed\\n");',
                "        return 4;",
                "    }",
                f"    operations[{index}]->BindInput(tensors[{inputs[0]}])",
                *(f"        .BindInput(tensors[{value}])" for value in inputs[1:]),
                f"        .BindOutput(tensors[{output}]);",
            ]
        )
        definitions.append(definition)
    return "\n".join(lines), definitions


def generate_compile_source(record: dict[str, Any], remote_root: str) -> str:
    """Emit actual graph parameters under the immutable compile-only lifecycle."""
    root = _root(remote_root, "compile")
    _record(record)
    padded_profile = record["profile_id"] == _PAD_PROFILE
    template_name = (
        "compile_fp16_pad_graph.cc.in" if padded_profile else "compile_fp16_graph.cc.in"
    )
    template_digest = (
        _PAD_COMPILE_TEMPLATE_SHA256 if padded_profile else _COMPILE_TEMPLATE_SHA256
    )
    data = (_TEMPLATE_ROOT / template_name).read_bytes()
    if hashlib.sha256(data).hexdigest() != template_digest:
        raise ValueError("immutable MarbleNet source template hash mismatch")
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
        "operation": "ConvGraph",
        "backend_required": "OPENVX",
        "model": record["profile_id"],
        "data_type": "FLOAT16",
        "executes_graph": False,
        "input": {"dtype": "FLOAT16", "shape": record["native_input"]["shape"]},
        "output": {"dtype": "FLOAT16", "shape": record["native_output"]["shape"]},
        "node_count": len(nodes),
        "nodes": nodes,
    }
    if padded_profile:
        definition.update(
            native_policy=record["native_policy"],
            lowering_sha256=record["lowering_sha256"],
        )
    source = data.decode("utf-8")
    for key, value in {
        "ROOT": root,
        "CONSTANTS": _constants(record),
        "GRAPH": graph,
        "DEFINITION": _definition(definition),
    }.items():
        source = source.replace(f"@@{key}@@", value)
    if "@@" in source:
        raise ValueError("unresolved MarbleNet source template marker")
    return source


def generate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int
) -> str:
    """Reuse the pinned file-driven executor, returning actual FP16 output bytes."""
    root = _root(remote_root, "execute")
    _record(record)
    if type(nbg_bytes) is not int or not 513 <= nbg_bytes <= 64 * 1024 * 1024:
        raise ValueError("invalid fixed NBG byte size")
    tensors = []
    padded_profile = record["profile_id"] == _PAD_PROFILE
    operation = "fp16-marblenet-pad" if padded_profile else "fp16-marblenet"
    for name, attribute in (("input", "INPUT"), ("output", "OUTPUT")):
        tensors.extend(
            [
                f"    const tim::vx::ShapeType {name}_shape"
                f"{{{_shape(record[f'native_{name}']['shape'])}}};",
                f"    const tim::vx::TensorSpec {name}_spec(",
                "        tim::vx::DataType::FLOAT16,",
                f"        {name}_shape, tim::vx::TensorAttribute::{attribute});",
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
    }
    if padded_profile:
        definition.update(
            native_policy=record["native_policy"],
            lowering_sha256=record["lowering_sha256"],
        )
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
    # Preserve the immutable legacy template. A quiet NaN in every half element
    # makes partially unwritten FP16 output fail the new finite-output policy.
    nan_half_declaration = "constexpr uint16_t kCanaryHalfBits = 0x7e00U;"
    replacements = {
        "constexpr uint8_t kCanaryByte = 0xa5U;": nan_half_declaration,
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
            raise ValueError("pinned executor NaN prime replacement mismatch")
        source = source.replace(previous, replacement)
    return source


def validate_compile_source(
    record: dict[str, Any], remote_root: str, source: str
) -> None:
    if source != generate_compile_source(record, remote_root):
        raise ValueError("MarbleNet compile source policy mismatch")


def validate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int, source: str
) -> None:
    if source != generate_execute_source(record, remote_root, nbg_bytes):
        raise ValueError("MarbleNet execute source policy mismatch")
