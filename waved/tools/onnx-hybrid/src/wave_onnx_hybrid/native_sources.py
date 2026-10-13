"""Generate hash-pinned, fixed native constructors and input-file executors.

These sources have no vendor execution on the host. Native admission also requires
package, ELF, input, NBG, environment, and device evidence in the lab bridge.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
import struct
from pathlib import Path
from typing import Any

from .audit import canonical_json

_TEMPLATE_ROOT = Path(__file__).resolve().parents[3] / "npu-compiler" / "onnx-native"
_TEMPLATE_SHA256 = {
    "compile_fp16_conv.cc.in": (
        "90c82a8e897f2d52f9a028e8ec553d35f2ec2c07f1f461b02ff0e7a70726e0ec"
    ),
    "compile_u8_fc.cc.in": (
        "184225bef386a753d10ec00b4b84b1730015b150b02714f1aa5ea82ab4401936"
    ),
    "execute.cc.in": (
        "76aa4063f1395adb364aca35c53ea10d544aa6675879addc0b0b58ba3dd8f5cb"
    ),
}
_QUANTIZATION = {"type": "ASYMMETRIC", "scale": 1.0, "zero_point": 0}
_OPERATIONS = {
    "fp16-conv-v1": {
        "type": "Conv2d",
        "padding": "SAME",
        "strides": [1, 1],
        "dilations": [0, 0],
        "group": 1,
        "kernel": [3, 1],
    },
    "u8-fc-v1": {"type": "FullyConnected", "axis": 0, "units": 4},
}
_CONTRACTS = {
    "fp16-conv-v1": {
        "native_input": ("FLOAT16", [16, 1, 1, 1], 32),
        "native_output": ("FLOAT16", [16, 1, 1, 1], 32),
        "weights": ("FLOAT16", [3, 1, 1, 1], 6),
        "bias": ("FLOAT32", [1], 4),
    },
    "u8-fc-v1": {
        "native_input": ("UINT8", [16, 1], 16),
        "native_output": ("UINT8", [4, 1], 4),
        "weights": ("UINT8", [16, 4], 64),
        "bias": ("INT32", [4], 16),
    },
}


def _root(value: str, mode: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(
        rf"/tmp/wave-npu-onnx17-[0-9a-f]{{16}}-{mode}", value
    ):
        raise ValueError(f"unsafe {mode} remote root")
    return value


def _record(record: dict[str, Any]) -> str:
    if not isinstance(record, dict):
        raise ValueError("lowering record must be a dict")
    unsigned = {key: value for key, value in record.items() if key != "lowering_sha256"}
    try:
        digest = hashlib.sha256(canonical_json(unsigned)).hexdigest()
    except (ValueError, TypeError) as error:
        raise ValueError("invalid lowering hash input") from error
    if record.get("lowering_sha256") != digest:
        raise ValueError("lowering hash mismatch")
    profile = record.get("profile_id")
    if (
        type(record.get("schema_version")) is not int
        or record["schema_version"] != 1
        or not isinstance(profile, str)
        or profile not in _CONTRACTS
    ):
        raise ValueError("unsupported lowering schema/profile")
    if canonical_json(record.get("operation")) != canonical_json(_OPERATIONS[profile]):
        raise ValueError("unsupported native operation")
    for key, (dtype, shape, size) in _CONTRACTS[profile].items():
        tensor = record.get(key)
        if not isinstance(tensor, dict):
            raise ValueError(f"invalid {key} tensor")
        actual = {name: tensor.get(name) for name in ("dtype", "shape", "byte_size")}
        expected = {"dtype": dtype, "shape": shape, "byte_size": size}
        if canonical_json(actual) != canonical_json(expected):
            raise ValueError(f"invalid {key} native tensor contract")
        quantizer = tensor.get("quantization")
        if profile == "u8-fc-v1":
            if canonical_json(quantizer) != canonical_json(_QUANTIZATION):
                raise ValueError(f"invalid {key} quantization")
        elif quantizer is not None:
            raise ValueError(f"unexpected {key} quantization")
        if key in ("weights", "bias"):
            _payload(tensor, size)
    if profile == "fp16-conv-v1":
        coefficients = struct.unpack(
            "<3e", bytes.fromhex(record["weights"]["payload_hex"])
        )
        bias = struct.unpack("<f", bytes.fromhex(record["bias"]["payload_hex"]))
        if not all(math.isfinite(value) for value in (*coefficients, *bias)):
            raise ValueError("nonfinite native constants")
        # The native bias must be an exact conversion from finite ONNX FLOAT16.
        try:
            if struct.unpack("<e", struct.pack("<e", bias[0]))[0] != bias[0]:
                raise ValueError("native bias is not exact FLOAT16 conversion")
        except OverflowError as error:
            raise ValueError("native bias is outside FLOAT16") from error
    elif bytes.fromhex(record["bias"]["payload_hex"]) != bytes(16):
        raise ValueError("FC profile requires zero bias")
    return profile


def _payload(tensor: dict[str, Any], size: int) -> bytes:
    value = tensor.get("payload_hex")
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]+", value):
        raise ValueError("constant payload must be lowercase numeric hex")
    payload = bytes.fromhex(value)
    if len(payload) != size:
        raise ValueError("constant payload length mismatch")
    if tensor.get("sha256") != hashlib.sha256(payload).hexdigest():
        raise ValueError("constant digest mismatch")
    return payload


def _template(name: str, substitutions: dict[str, str]) -> str:
    data = (_TEMPLATE_ROOT / name).read_bytes()
    if hashlib.sha256(data).hexdigest() != _TEMPLATE_SHA256[name]:
        raise ValueError("immutable native source template hash mismatch")
    source = data.decode("utf-8")
    for key, value in substitutions.items():
        source = source.replace(f"@@{key}@@", value)
    if "@@" in source:
        raise ValueError("unresolved native source template marker")
    return source


def _definition(value: dict[str, Any]) -> str:
    # All values come from fixed contracts or decoded numeric patterns. No ONNX names.
    return json.dumps(json.dumps(value, sort_keys=True, separators=(",", ":")))


def generate_compile_source(record: dict[str, Any], remote_root: str) -> str:
    """Emit one immutable, bounded constructor with actual imported constants."""
    root = _root(remote_root, "compile")
    profile = _record(record)
    conv = profile == "fp16-conv-v1"
    weights = _payload(record["weights"], 6 if conv else 64)
    bias = _payload(record["bias"], 4 if conv else 16)
    if conv:
        half_bits = struct.unpack("<3H", weights)
        bias_bits = struct.unpack("<I", bias)[0]
        constants = (
            "// Preserve the imported IEEE binary16 and binary32 bit patterns.\n"
            "constexpr std::array<uint16_t, 3> kWeights{"
            + ", ".join(f"0x{value:04x}U" for value in half_bits)
            + "};\nconstexpr std::array<uint32_t, 1> kBiasBits{"
            + f"0x{bias_bits:08x}U"
            + "};"
        )
    else:
        constants = (
            "// Imported FC output columns are already contiguous: j*16+k.\n"
            "constexpr std::array<uint8_t, 64> kWeights{"
            + ", ".join(str(value) for value in weights)
            + "};\nconstexpr std::array<int32_t, 4> kBias{0, 0, 0, 0};"
        )
    contracts = _CONTRACTS[profile]
    definition = {
        "schema_version": 1,
        "mode": "compile-only",
        "target": {
            "chip_model": "0x00009000",
            "customer_id": "0x1000000e",
            "nbg_version": "0x00020000",
        },
        "candidates": [1],
        "operation": _OPERATIONS[profile]["type"],
        "backend_required": "OPENVX",
        "model": profile,
        "data_type": contracts["native_input"][0],
        "executes_graph": False,
        "input": {
            "dtype": contracts["native_input"][0],
            "shape": contracts["native_input"][1],
        },
        "output": {
            "dtype": contracts["native_output"][0],
            "shape": contracts["native_output"][1],
        },
        "constant_weights": {
            "dtype": contracts["weights"][0],
            "shape": contracts["weights"][1],
        },
        "constant_bias": {"dtype": contracts["bias"][0], "shape": contracts["bias"][1]},
    }
    if conv:
        definition["constant_weights"]["half_bits"] = list(half_bits)
        definition["constant_bias"]["values"] = list(struct.unpack("<f", bias))
        definition.update(padding="SAME", stride=[1, 1], dilation=[0, 0])
    else:
        definition["constant_weights"]["values"] = list(weights)
        definition["constant_bias"].update(values=[0, 0, 0, 0], scale=1, zero_point=0)
        definition.update(quantization=_QUANTIZATION, axis=0, units=4)
    return _template(
        "compile_fp16_conv.cc.in" if conv else "compile_u8_fc.cc.in",
        {"ROOT": root, "CONSTANTS": constants, "DEFINITION": _definition(definition)},
    )


def generate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int
) -> str:
    """Emit one reusable-input runner; host pins the exact NBG and current oracle."""
    root = _root(remote_root, "execute")
    profile = _record(record)
    if type(nbg_bytes) is not int or not 513 <= nbg_bytes <= 64 * 1024 * 1024:
        raise ValueError("invalid fixed NBG byte size")
    conv = profile == "fp16-conv-v1"
    contracts = _CONTRACTS[profile]
    dtype, input_shape, input_bytes = contracts["native_input"]
    _, output_shape, output_bytes = contracts["native_output"]
    tensors = (
        "    const tim::vx::ShapeType input_shape{"
        + ", ".join(map(str, input_shape))
        + "};\n"
    )
    tensors += (
        "    const tim::vx::ShapeType output_shape{"
        + ", ".join(map(str, output_shape))
        + "};\n"
    )
    if not conv:
        tensors += (
            "    const tim::vx::Quantization quantization(\n"
            "        tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);\n"
        )
    for name, attribute in (("input", "INPUT"), ("output", "OUTPUT")):
        tensors += (
            f"    const tim::vx::TensorSpec {name}_spec(\n"
            f"        tim::vx::DataType::{dtype},\n"
            f"        {name}_shape,\n"
            f"        tim::vx::TensorAttribute::{attribute}"
            + (",\n        quantization" if not conv else "")
            + ");\n"
        )
    operation = "fp16-conv" if conv else "u8-fc"
    definition = {
        "schema_version": 1,
        "mode": "execute-once",
        "operation": "NBG",
        "model": f"{root}/model/model.nbg",
        "model_bytes": nbg_bytes,
        "model_operation": operation,
        "input_file": f"{root}/input/input.bin",
        "output_file": f"{root}/output/output.bin",
        "inputs": [{"type": dtype, "shape": input_shape}],
        "outputs": [{"type": dtype, "shape": output_shape}],
        "iterations": 1,
        "executes_graph": True,
    }
    if not conv:
        definition["quantization"] = _QUANTIZATION
    return _template(
        "execute.cc.in",
        {
            "ROOT": root,
            "NBG_BYTES": str(nbg_bytes),
            "INPUT_BYTES": str(input_bytes),
            "OUTPUT_BYTES": str(output_bytes),
            "TENSORS": tensors.rstrip(),
            "OPERATION": operation,
            "DEFINITION": _definition(definition),
        },
    )


def validate_compile_source(
    record: dict[str, Any], remote_root: str, source: str
) -> None:
    """Refuse any source differing from the exact approved regeneration."""
    if source != generate_compile_source(record, remote_root):
        raise ValueError("native compile source policy mismatch")


def validate_execute_source(
    record: dict[str, Any], remote_root: str, nbg_bytes: int, source: str
) -> None:
    """Refuse modifications to constants, calls, files, guards, or lifecycle."""
    if source != generate_execute_source(record, remote_root, nbg_bytes):
        raise ValueError("native execute source policy mismatch")
