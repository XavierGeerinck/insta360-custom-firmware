"""Deterministic model artifacts for the learned Erf-mask denoiser."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from typing import Any

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

from .audit import canonical_json
from .erf_mask_signal import (
    ERF_BINS,
    ERF_INPUT_SCALE,
    ERF_INPUT_ZERO_POINT,
    ERF_OUTPUT_SCALE,
    ERF_OUTPUT_ZERO_POINT,
    MASK_FLOOR,
)
from .errors import ErrorCode, HybridError
from .fixtures import deterministic_npz_bytes

MODEL_OPSET = 18
MODEL_IR_VERSION = 10
MODEL_SCHEMA_VERSION = 1
WAVE_PRODUCT_ID = 0x1000000E
VPMN_VERSION = 2


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.MODEL_PACKAGE_INVALID, message)


def _weight_tensor(values: np.ndarray, *, name: str) -> np.ndarray:
    array = np.asarray(values)
    if array.shape != (ERF_BINS,):
        raise _invalid(f"{name} must have shape ({ERF_BINS},)")
    if array.dtype != np.dtype(np.float32):
        raise _invalid(f"{name} must have dtype float32")
    if not np.isfinite(array).all():
        raise _invalid(f"{name} contains non-finite values")
    if name == "a" and np.any(array < 0.0):
        raise _invalid("a must contain non-negative slopes")
    return np.ascontiguousarray(array)


def serialize_erf_mask_weights(slopes: np.ndarray, biases: np.ndarray) -> bytes:
    """Serialize the exact learned tensors with deterministic ZIP metadata."""

    return deterministic_npz_bytes(
        {
            "a": _weight_tensor(slopes, name="a"),
            "b": _weight_tensor(biases, name="b"),
        }
    )


def _initializer(name: str, value: np.ndarray) -> onnx.TensorProto:
    return numpy_helper.from_array(np.ascontiguousarray(value), name=name)


def build_erf_mask_onnx(slopes: np.ndarray, biases: np.ndarray) -> bytes:
    """Build the complete deterministic reference ONNX graph."""

    slope_values = _weight_tensor(slopes, name="a")
    bias_values = _weight_tensor(biases, name="b")
    initializers = [
        _initializer("a", slope_values),
        _initializer("b", bias_values),
        _initializer("logit.min", np.array(-4.0, dtype=np.float32)),
        _initializer("logit.max", np.array(3.96875, dtype=np.float32)),
        _initializer(
            "erf.input.scale",
            np.array(ERF_INPUT_SCALE, dtype=np.float32),
        ),
        _initializer(
            "erf.input.zero_point",
            np.array(ERF_INPUT_ZERO_POINT, dtype=np.uint8),
        ),
        _initializer(
            "erf.output.scale",
            np.array(ERF_OUTPUT_SCALE, dtype=np.float32),
        ),
        _initializer(
            "erf.output.zero_point",
            np.array(ERF_OUTPUT_ZERO_POINT, dtype=np.uint8),
        ),
        _initializer("mask.one", np.array(1.0, dtype=np.float32)),
        _initializer("mask.half", np.array(0.5, dtype=np.float32)),
        _initializer("mask.min", np.array(MASK_FLOOR, dtype=np.float32)),
        _initializer("mask.max", np.array(255.0 / 256.0, dtype=np.float32)),
    ]
    nodes = [
        helper.make_node(
            "Mul",
            ["feature", "a"],
            ["affine.mul"],
            name="affine.mul",
        ),
        helper.make_node(
            "Add",
            ["affine.mul", "b"],
            ["affine.add"],
            name="affine.add",
        ),
        helper.make_node(
            "Clip",
            ["affine.add", "logit.min", "logit.max"],
            ["logit.clipped"],
            name="logit.clip",
        ),
        helper.make_node(
            "QuantizeLinear",
            [
                "logit.clipped",
                "erf.input.scale",
                "erf.input.zero_point",
            ],
            ["erf.input.u8"],
            name="erf.input.quantize",
        ),
        helper.make_node(
            "DequantizeLinear",
            [
                "erf.input.u8",
                "erf.input.scale",
                "erf.input.zero_point",
            ],
            ["erf.input.f32"],
            name="erf.input.dequantize",
        ),
        helper.make_node(
            "Erf",
            ["erf.input.f32"],
            ["erf.output.f32"],
            name="erf.partition",
        ),
        helper.make_node(
            "QuantizeLinear",
            [
                "erf.output.f32",
                "erf.output.scale",
                "erf.output.zero_point",
            ],
            ["erf.output.u8"],
            name="erf.output.quantize",
        ),
        helper.make_node(
            "DequantizeLinear",
            [
                "erf.output.u8",
                "erf.output.scale",
                "erf.output.zero_point",
            ],
            ["erf.output.dequantized"],
            name="erf.output.dequantize",
        ),
        helper.make_node(
            "Add",
            ["erf.output.dequantized", "mask.one"],
            ["mask.shifted"],
            name="mask.shift",
        ),
        helper.make_node(
            "Mul",
            ["mask.shifted", "mask.half"],
            ["mask.scaled"],
            name="mask.scale",
        ),
        helper.make_node(
            "Clip",
            ["mask.scaled", "mask.min", "mask.max"],
            ["mask"],
            name="mask.floor",
        ),
    ]
    feature = helper.make_tensor_value_info(
        "feature",
        TensorProto.FLOAT,
        [1, ERF_BINS],
    )
    mask = helper.make_tensor_value_info(
        "mask",
        TensorProto.FLOAT,
        [1, ERF_BINS],
    )
    graph = helper.make_graph(
        nodes,
        "wave-erf-mask",
        [feature],
        [mask],
        initializer=initializers,
    )
    model = helper.make_model(
        graph,
        ir_version=MODEL_IR_VERSION,
        opset_imports=[helper.make_opsetid("", MODEL_OPSET)],
        producer_name="wave-onnx-hybrid",
        producer_version="1",
    )
    model.model_version = MODEL_SCHEMA_VERSION
    properties = {
        "wave.model.name": "wave-erf-mask",
        "wave.model.partition": "quantized-u8-erf",
        "wave.model.schema_version": str(MODEL_SCHEMA_VERSION),
    }
    for key, value in sorted(properties.items()):
        entry = model.metadata_props.add()
        entry.key = key
        entry.value = value
    onnx.checker.check_model(model, full_check=True)
    return model.SerializeToString(deterministic=True)


@dataclass(frozen=True, slots=True)
class ErfPartitionContract:
    """Exact Wave NPU boundary covered by the sealed partition signature."""

    schema_version: int = 1
    operation: str = "Erf"
    bins: int = ERF_BINS
    input_dtype: str = "UINT8"
    input_scale: float = float(ERF_INPUT_SCALE)
    input_zero_point: int = ERF_INPUT_ZERO_POINT
    output_dtype: str = "UINT8"
    output_scale: float = float(ERF_OUTPUT_SCALE)
    output_zero_point: int = ERF_OUTPUT_ZERO_POINT
    target_product: int = WAVE_PRODUCT_ID
    nbg_container: str = "VPMN"
    nbg_version: int = VPMN_VERSION

    def __post_init__(self) -> None:
        if (
            type(self.schema_version) is not int
            or self.schema_version != MODEL_SCHEMA_VERSION
            or self.operation != "Erf"
            or type(self.bins) is not int
            or self.bins <= 0
            or self.input_dtype != "UINT8"
            or not np.isfinite(self.input_scale)
            or self.input_scale <= 0.0
            or type(self.input_zero_point) is not int
            or not 0 <= self.input_zero_point <= 255
            or self.output_dtype != "UINT8"
            or not np.isfinite(self.output_scale)
            or self.output_scale <= 0.0
            or type(self.output_zero_point) is not int
            or not 0 <= self.output_zero_point <= 255
            or type(self.target_product) is not int
            or self.target_product <= 0
            or self.nbg_container != "VPMN"
            or type(self.nbg_version) is not int
            or self.nbg_version != VPMN_VERSION
        ):
            raise _invalid("invalid Erf partition contract")

    def to_document(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "operation": self.operation,
            "input": {
                "shape": [self.bins],
                "dtype": self.input_dtype,
                "scale": self.input_scale,
                "zero_point": self.input_zero_point,
            },
            "output": {
                "shape": [self.bins],
                "dtype": self.output_dtype,
                "scale": self.output_scale,
                "zero_point": self.output_zero_point,
            },
            "target": {
                "product": self.target_product,
                "container": self.nbg_container,
                "version": self.nbg_version,
            },
        }


@dataclass(frozen=True, slots=True)
class PartitionSignature:
    document: dict[str, Any]
    sha256: str


def erf_partition_signature(
    contract: ErfPartitionContract,
) -> PartitionSignature:
    """Hash every semantic field at the CPU/NPU tensor boundary."""

    document = contract.to_document()
    return PartitionSignature(
        document=document,
        sha256=hashlib.sha256(canonical_json(document)).hexdigest(),
    )
