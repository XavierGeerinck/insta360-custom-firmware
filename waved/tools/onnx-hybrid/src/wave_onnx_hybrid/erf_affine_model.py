"""Exact integer quantization model for an affine-plus-Erf Wave partition."""

from __future__ import annotations

import hashlib
import io
import json
import struct
from dataclasses import dataclass, field
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
    cpu_erf_u8,
    quantize_logits,
)
from .errors import ErrorCode, HybridError
from .fixtures import deterministic_npz_bytes

AFFINE_FEATURE_SCALE = 0.0625
AFFINE_ONNX_OPSET = 18
AFFINE_ONNX_IR_VERSION = 10
AFFINE_SCHEMA_VERSION = 1
WAVE_PRODUCT_ID = 0x1000000E
NATIVE_AFFINE_MAGIC = b"AFC1"
NATIVE_AFFINE_HEADER = struct.Struct("<4sIIIIffii32s")
_NATIVE_INPUT_TYPES = {"UINT8": 1, "INT8": 2}
_NATIVE_WEIGHT_GRANULARITY = {"per-tensor": 1, "per-channel": 2}


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.MODEL_PACKAGE_INVALID, message)


def _invalid_input(message: str) -> HybridError:
    return HybridError(ErrorCode.ONNX_INPUT_INVALID, message)


@dataclass(frozen=True, slots=True)
class AffineQuantizationCandidate:
    """One member of the closed PoC 14A deployment registry."""

    name: str
    feature_dtype: str
    feature_scale: float
    feature_zero_point: int
    weight_granularity: str

    def __post_init__(self) -> None:
        expected = {
            "u8-per-tensor": ("UINT8", 128, "per-tensor"),
            "u8-per-channel": ("UINT8", 128, "per-channel"),
            "i8-per-tensor": ("INT8", 0, "per-tensor"),
            "i8-per-channel": ("INT8", 0, "per-channel"),
        }
        if (
            self.name not in expected
            or (
                self.feature_dtype,
                self.feature_zero_point,
                self.weight_granularity,
            )
            != expected[self.name]
            or self.feature_scale != AFFINE_FEATURE_SCALE
        ):
            raise _invalid("affine quantization candidate is not in the registry")

    @property
    def numpy_dtype(self) -> np.dtype[Any]:
        return np.dtype(np.uint8 if self.feature_dtype == "UINT8" else np.int8)

    @property
    def qmin(self) -> int:
        return 0 if self.feature_dtype == "UINT8" else -128

    @property
    def qmax(self) -> int:
        return 255 if self.feature_dtype == "UINT8" else 127

    def to_document(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "feature": {
                "dtype": self.feature_dtype,
                "shape": [ERF_BINS],
                "scale": self.feature_scale,
                "zero_point": self.feature_zero_point,
            },
            "weights": {
                "dtype": "INT8",
                "shape": [ERF_BINS],
                "granularity": self.weight_granularity,
                "zero_point": 0,
            },
            "bias": {
                "dtype": "INT32",
                "shape": [ERF_BINS],
                "zero_point": 0,
                "scale": "feature_scale*weight_scale",
            },
        }


_CANDIDATES = (
    AffineQuantizationCandidate(
        "u8-per-tensor",
        "UINT8",
        AFFINE_FEATURE_SCALE,
        128,
        "per-tensor",
    ),
    AffineQuantizationCandidate(
        "u8-per-channel",
        "UINT8",
        AFFINE_FEATURE_SCALE,
        128,
        "per-channel",
    ),
    AffineQuantizationCandidate(
        "i8-per-tensor",
        "INT8",
        AFFINE_FEATURE_SCALE,
        0,
        "per-tensor",
    ),
    AffineQuantizationCandidate(
        "i8-per-channel",
        "INT8",
        AFFINE_FEATURE_SCALE,
        0,
        "per-channel",
    ),
)


def quantization_candidates() -> tuple[AffineQuantizationCandidate, ...]:
    return _CANDIDATES


def _readonly(values: np.ndarray, dtype: np.dtype[Any]) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=dtype)
    result.flags.writeable = False
    return result


def _source_weights(
    slopes: np.ndarray, biases: np.ndarray
) -> tuple[np.ndarray, np.ndarray]:
    slope_values = np.asarray(slopes)
    bias_values = np.asarray(biases)
    if (
        slope_values.shape != (ERF_BINS,)
        or bias_values.shape != (ERF_BINS,)
        or not np.issubdtype(slope_values.dtype, np.floating)
        or not np.issubdtype(bias_values.dtype, np.floating)
        or not np.isfinite(slope_values).all()
        or not np.isfinite(bias_values).all()
        or np.any(slope_values < 0.0)
    ):
        raise _invalid("affine source weights must be finite 481-element vectors")
    return (
        np.asarray(slope_values, dtype=np.float64),
        np.asarray(bias_values, dtype=np.float64),
    )


@dataclass(frozen=True, slots=True)
class QuantizedAffineModel:
    """Frozen integer tensors and quantization scales for Target A."""

    candidate: AffineQuantizationCandidate
    weights: np.ndarray = field(repr=False)
    weight_scales: np.ndarray = field(repr=False)
    biases: np.ndarray = field(repr=False)
    accumulator_scales: np.ndarray = field(repr=False)
    source_sha256: str
    contract_sha256: str = field(init=False)

    def __post_init__(self) -> None:
        if self.candidate not in _CANDIDATES:
            raise _invalid("quantized affine candidate is not registered")
        weights = _readonly(self.weights, np.dtype(np.int8))
        weight_scales = _readonly(self.weight_scales, np.dtype(np.float32))
        biases = _readonly(self.biases, np.dtype(np.int32))
        accumulator_scales = _readonly(
            self.accumulator_scales,
            np.dtype(np.float32),
        )
        if (
            weights.shape != (ERF_BINS,)
            or weight_scales.shape != (ERF_BINS,)
            or biases.shape != (ERF_BINS,)
            or accumulator_scales.shape != (ERF_BINS,)
            or np.any(weight_scales <= 0.0)
            or np.any(accumulator_scales <= 0.0)
            or not np.isfinite(weight_scales).all()
            or not np.isfinite(accumulator_scales).all()
            or len(self.source_sha256) != 64
        ):
            raise _invalid("invalid quantized affine tensor contract")
        object.__setattr__(self, "weights", weights)
        object.__setattr__(self, "weight_scales", weight_scales)
        object.__setattr__(self, "biases", biases)
        object.__setattr__(self, "accumulator_scales", accumulator_scales)
        document = self.to_document(include_contract_hash=False)
        object.__setattr__(
            self,
            "contract_sha256",
            hashlib.sha256(canonical_json(document)).hexdigest(),
        )

    def to_document(self, *, include_contract_hash: bool = True) -> dict[str, Any]:
        document: dict[str, Any] = {
            "schema_version": AFFINE_SCHEMA_VERSION,
            "candidate": self.candidate.to_document(),
            "operations": ["Multiply", "Add", "Erf"],
            "rounding": "round-to-nearest-ties-to-even",
            "saturation": {
                "feature": [self.candidate.qmin, self.candidate.qmax],
                "erf_input": [0, 255],
            },
            "erf_input": {
                "dtype": "UINT8",
                "shape": [ERF_BINS],
                "scale": float(ERF_INPUT_SCALE),
                "zero_point": ERF_INPUT_ZERO_POINT,
            },
            "erf_output": {
                "dtype": "UINT8",
                "shape": [ERF_BINS],
                "scale": float(ERF_OUTPUT_SCALE),
                "zero_point": ERF_OUTPUT_ZERO_POINT,
            },
            "source_sha256": self.source_sha256,
            "tensor_sha256": {
                "weights": hashlib.sha256(self.weights.tobytes()).hexdigest(),
                "weight_scales": hashlib.sha256(
                    self.weight_scales.astype("<f4", copy=False).tobytes()
                ).hexdigest(),
                "biases": hashlib.sha256(
                    self.biases.astype("<i4", copy=False).tobytes()
                ).hexdigest(),
                "accumulator_scales": hashlib.sha256(
                    self.accumulator_scales.astype("<f4", copy=False).tobytes()
                ).hexdigest(),
            },
            "target_product": WAVE_PRODUCT_ID,
        }
        if include_contract_hash:
            document["contract_sha256"] = self.contract_sha256
        return document


def quantize_affine_model(
    slopes: np.ndarray,
    biases: np.ndarray,
    candidate: AffineQuantizationCandidate,
) -> QuantizedAffineModel:
    if candidate not in _CANDIDATES:
        raise _invalid("affine quantization candidate is not registered")
    slope_values, bias_values = _source_weights(slopes, biases)
    if candidate.weight_granularity == "per-tensor":
        maximum = float(np.max(np.abs(slope_values)))
        scale = maximum / 127.0 if maximum > 0.0 else 1.0 / 127.0
        weight_scales64 = np.full(ERF_BINS, scale, dtype=np.float64)
    else:
        weight_scales64 = np.where(
            slope_values > 0.0,
            slope_values / 127.0,
            1.0 / 127.0,
        )
    weight_quantized = np.clip(
        np.rint(slope_values / weight_scales64),
        -127,
        127,
    ).astype(np.int8)
    accumulator_scales64 = candidate.feature_scale * weight_scales64
    bias_rounded = np.rint(bias_values / accumulator_scales64)
    if np.any(bias_rounded < np.iinfo(np.int32).min) or np.any(
        bias_rounded > np.iinfo(np.int32).max
    ):
        raise _invalid("affine bias does not fit INT32 accumulator units")
    bias_quantized = bias_rounded.astype(np.int32)
    source_payload = (
        np.ascontiguousarray(slope_values, dtype="<f8").tobytes()
        + np.ascontiguousarray(bias_values, dtype="<f8").tobytes()
    )
    return QuantizedAffineModel(
        candidate=candidate,
        weights=weight_quantized,
        weight_scales=weight_scales64.astype(np.float32),
        biases=bias_quantized,
        accumulator_scales=accumulator_scales64.astype(np.float32),
        source_sha256=hashlib.sha256(source_payload).hexdigest(),
    )


def quantize_features(
    features: np.ndarray,
    candidate: AffineQuantizationCandidate,
) -> np.ndarray:
    if candidate not in _CANDIDATES:
        raise _invalid("affine quantization candidate is not registered")
    values = np.asarray(features)
    if (
        values.size == 0
        or values.shape[-1] != ERF_BINS
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid_input("affine features must end in 481 finite floats")
    quantized = np.rint(
        np.asarray(values, dtype=np.float64) / candidate.feature_scale
        + candidate.feature_zero_point
    )
    quantized = np.clip(quantized, candidate.qmin, candidate.qmax)
    return np.ascontiguousarray(quantized, dtype=candidate.numpy_dtype)


def _quantized_input(values: np.ndarray, model: QuantizedAffineModel) -> np.ndarray:
    array = np.asarray(values)
    if (
        array.size == 0
        or array.shape[-1] != ERF_BINS
        or array.dtype != model.candidate.numpy_dtype
    ):
        raise _invalid_input("quantized affine input has the wrong shape or dtype")
    return array


def _accumulator(features: np.ndarray, model: QuantizedAffineModel) -> np.ndarray:
    values = _quantized_input(features, model)
    centered = values.astype(np.int64) - model.candidate.feature_zero_point
    accumulator = centered * model.weights.astype(np.int64)
    accumulator += model.biases.astype(np.int64)
    return accumulator


def dequantize_affine(features: np.ndarray, model: QuantizedAffineModel) -> np.ndarray:
    accumulator = _accumulator(features, model)
    return np.ascontiguousarray(
        accumulator.astype(np.float64) * model.accumulator_scales,
        dtype=np.float64,
    )


def affine_erf_input_u8(
    features: np.ndarray, model: QuantizedAffineModel
) -> np.ndarray:
    accumulator = _accumulator(features, model)
    multiplier = np.asarray(
        model.accumulator_scales / np.float32(ERF_INPUT_SCALE),
        dtype=np.float32,
    )
    scaled = accumulator.astype(np.float32) * multiplier
    requantized = np.rint(scaled) + np.float32(ERF_INPUT_ZERO_POINT)
    return np.ascontiguousarray(
        np.clip(requantized, 0.0, 255.0),
        dtype=np.uint8,
    )


def affine_erf_output_u8(
    features: np.ndarray, model: QuantizedAffineModel
) -> np.ndarray:
    return cpu_erf_u8(affine_erf_input_u8(features, model))


def serialize_quantized_affine(model: QuantizedAffineModel) -> bytes:
    return deterministic_npz_bytes(
        {
            "accumulator_scales": model.accumulator_scales,
            "biases": model.biases,
            "weight_scales": model.weight_scales,
            "weights": model.weights,
        }
    )


def serialize_affine_config(model: QuantizedAffineModel) -> bytes:
    return canonical_json(model.to_document())


def deserialize_quantized_affine(
    config_payload: bytes,
    weights_payload: bytes,
) -> QuantizedAffineModel:
    """Load only the canonical, fully hash-bound affine representation."""

    if not isinstance(config_payload, bytes) or not isinstance(
        weights_payload,
        bytes,
    ):
        raise _invalid("quantized affine payloads must be immutable bytes")

    def strict_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate affine config key: {key}")
            result[key] = value
        return result

    try:
        document = json.loads(
            config_payload,
            object_pairs_hook=strict_pairs,
            parse_constant=lambda token: (_ for _ in ()).throw(
                ValueError(f"non-finite affine config token: {token}")
            ),
        )
        if not isinstance(document, dict):
            raise ValueError("affine config root is not an object")
        candidate_name = document["candidate"]["name"]
        candidate = next(value for value in _CANDIDATES if value.name == candidate_name)
        with np.load(io.BytesIO(weights_payload), allow_pickle=False) as archive:
            if archive.files != [
                "accumulator_scales",
                "biases",
                "weight_scales",
                "weights",
            ]:
                raise ValueError("affine tensor names differ")
            accumulator_scales = np.array(
                archive["accumulator_scales"],
                copy=True,
                order="C",
            )
            biases = np.array(archive["biases"], copy=True, order="C")
            weight_scales = np.array(
                archive["weight_scales"],
                copy=True,
                order="C",
            )
            weights = np.array(archive["weights"], copy=True, order="C")
        model = QuantizedAffineModel(
            candidate=candidate,
            weights=weights,
            weight_scales=weight_scales,
            biases=biases,
            accumulator_scales=accumulator_scales,
            source_sha256=document["source_sha256"],
        )
    except (
        HybridError,
        KeyError,
        OSError,
        StopIteration,
        TypeError,
        ValueError,
    ) as exc:
        raise _invalid(f"cannot load quantized affine model: {exc}") from exc
    if (
        serialize_affine_config(model) != config_payload
        or serialize_quantized_affine(model) != weights_payload
    ):
        raise _invalid("quantized affine payload is noncanonical or changed")
    return model


def serialize_native_affine_constants(model: QuantizedAffineModel) -> bytes:
    """Serialize the exact fixed-layout constants consumed by native PoCs."""

    header = NATIVE_AFFINE_HEADER.pack(
        NATIVE_AFFINE_MAGIC,
        AFFINE_SCHEMA_VERSION,
        ERF_BINS,
        _NATIVE_INPUT_TYPES[model.candidate.feature_dtype],
        _NATIVE_WEIGHT_GRANULARITY[model.candidate.weight_granularity],
        model.candidate.feature_scale,
        float(ERF_INPUT_SCALE),
        model.candidate.feature_zero_point,
        ERF_INPUT_ZERO_POINT,
        bytes.fromhex(model.contract_sha256),
    )
    return b"".join(
        (
            header,
            model.weights.astype(np.int8, copy=False).tobytes(),
            model.weight_scales.astype("<f4", copy=False).tobytes(),
            model.biases.astype("<i4", copy=False).tobytes(),
            model.accumulator_scales.astype("<f4", copy=False).tobytes(),
        )
    )


def parse_native_affine_constants(
    payload: bytes,
    expected_model: QuantizedAffineModel,
) -> QuantizedAffineModel:
    """Parse and independently bind a native constants blob to its model."""

    expected_bytes = NATIVE_AFFINE_HEADER.size + ERF_BINS * 13
    if not isinstance(payload, bytes) or len(payload) != expected_bytes:
        raise _invalid("native affine constants length differs")
    (
        magic,
        schema,
        width,
        input_type,
        weight_granularity,
        feature_scale,
        erf_input_scale,
        feature_zero_point,
        erf_input_zero_point,
        contract_sha256,
    ) = NATIVE_AFFINE_HEADER.unpack_from(payload)
    candidate = expected_model.candidate
    if (
        magic != NATIVE_AFFINE_MAGIC
        or schema != AFFINE_SCHEMA_VERSION
        or width != ERF_BINS
        or input_type != _NATIVE_INPUT_TYPES[candidate.feature_dtype]
        or weight_granularity
        != _NATIVE_WEIGHT_GRANULARITY[candidate.weight_granularity]
        or struct.pack("<f", feature_scale)
        != struct.pack("<f", candidate.feature_scale)
        or struct.pack("<f", erf_input_scale) != struct.pack("<f", ERF_INPUT_SCALE)
        or feature_zero_point != candidate.feature_zero_point
        or erf_input_zero_point != ERF_INPUT_ZERO_POINT
        or contract_sha256 != bytes.fromhex(expected_model.contract_sha256)
    ):
        raise _invalid("native affine constants header differs")
    offset = NATIVE_AFFINE_HEADER.size
    weights = np.frombuffer(
        payload,
        dtype=np.int8,
        count=ERF_BINS,
        offset=offset,
    ).copy()
    offset += ERF_BINS
    weight_scales = np.frombuffer(
        payload,
        dtype="<f4",
        count=ERF_BINS,
        offset=offset,
    ).copy()
    offset += ERF_BINS * 4
    biases = np.frombuffer(
        payload,
        dtype="<i4",
        count=ERF_BINS,
        offset=offset,
    ).copy()
    offset += ERF_BINS * 4
    accumulator_scales = np.frombuffer(
        payload,
        dtype="<f4",
        count=ERF_BINS,
        offset=offset,
    ).copy()
    restored = QuantizedAffineModel(
        candidate=candidate,
        weights=weights,
        weight_scales=weight_scales,
        biases=biases,
        accumulator_scales=accumulator_scales,
        source_sha256=expected_model.source_sha256,
    )
    if (
        restored.contract_sha256 != expected_model.contract_sha256
        or serialize_native_affine_constants(restored) != payload
    ):
        raise _invalid("native affine constants violate the frozen model")
    return restored


def _initializer(name: str, value: np.ndarray) -> onnx.TensorProto:
    return numpy_helper.from_array(np.ascontiguousarray(value), name=name)


def build_affine_erf_onnx(model: QuantizedAffineModel) -> bytes:
    """Build a deterministic integer-oracle ONNX representation."""

    feature_type = (
        TensorProto.UINT8
        if model.candidate.feature_dtype == "UINT8"
        else TensorProto.INT8
    )
    initializers = [
        _initializer(
            "feature.zero.i32",
            np.array(model.candidate.feature_zero_point, dtype=np.int32),
        ),
        _initializer("weight.q", model.weights),
        _initializer("bias.q", model.biases),
        _initializer(
            "requant.multiplier",
            np.asarray(
                model.accumulator_scales / np.float32(ERF_INPUT_SCALE),
                dtype=np.float32,
            ),
        ),
        _initializer(
            "erf.input.zero.f32",
            np.array(ERF_INPUT_ZERO_POINT, dtype=np.float32),
        ),
        _initializer("clip.min", np.array(0.0, dtype=np.float32)),
        _initializer("clip.max", np.array(255.0, dtype=np.float32)),
        _initializer(
            "erf.input.scale",
            np.array(ERF_INPUT_SCALE, dtype=np.float32),
        ),
        _initializer(
            "erf.input.zero.u8",
            np.array(ERF_INPUT_ZERO_POINT, dtype=np.uint8),
        ),
        _initializer(
            "erf.output.scale",
            np.array(ERF_OUTPUT_SCALE, dtype=np.float32),
        ),
        _initializer(
            "erf.output.zero.u8",
            np.array(ERF_OUTPUT_ZERO_POINT, dtype=np.uint8),
        ),
    ]
    nodes = [
        helper.make_node(
            "Cast",
            ["feature.q"],
            ["feature.i32"],
            name="feature.cast",
            to=TensorProto.INT32,
        ),
        helper.make_node(
            "Sub",
            ["feature.i32", "feature.zero.i32"],
            ["feature.centered"],
            name="feature.center",
        ),
        helper.make_node(
            "Cast",
            ["weight.q"],
            ["weight.i32"],
            name="weight.cast",
            to=TensorProto.INT32,
        ),
        helper.make_node(
            "Mul",
            ["feature.centered", "weight.i32"],
            ["affine.mul"],
            name="affine.mul",
        ),
        helper.make_node(
            "Add",
            ["affine.mul", "bias.q"],
            ["affine.accumulator"],
            name="affine.add",
        ),
        helper.make_node(
            "Cast",
            ["affine.accumulator"],
            ["affine.f32"],
            name="affine.cast",
            to=TensorProto.FLOAT,
        ),
        helper.make_node(
            "Mul",
            ["affine.f32", "requant.multiplier"],
            ["requant.scaled"],
            name="requant.scale",
        ),
        helper.make_node(
            "Round",
            ["requant.scaled"],
            ["requant.rounded"],
            name="requant.round",
        ),
        helper.make_node(
            "Add",
            ["requant.rounded", "erf.input.zero.f32"],
            ["requant.shifted"],
            name="requant.shift",
        ),
        helper.make_node(
            "Clip",
            ["requant.shifted", "clip.min", "clip.max"],
            ["requant.clipped"],
            name="requant.clip",
        ),
        helper.make_node(
            "Cast",
            ["requant.clipped"],
            ["erf.input.u8"],
            name="requant.cast",
            to=TensorProto.UINT8,
        ),
        helper.make_node(
            "DequantizeLinear",
            ["erf.input.u8", "erf.input.scale", "erf.input.zero.u8"],
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
            ["erf.output.f32", "erf.output.scale", "erf.output.zero.u8"],
            ["erf.output.u8"],
            name="erf.output.quantize",
        ),
    ]
    graph = helper.make_graph(
        nodes,
        "wave-erf-affine-mask",
        [
            helper.make_tensor_value_info(
                "feature.q",
                feature_type,
                [1, ERF_BINS],
            )
        ],
        [
            helper.make_tensor_value_info(
                "erf.output.u8",
                TensorProto.UINT8,
                [1, ERF_BINS],
            )
        ],
        initializer=initializers,
    )
    onnx_model = helper.make_model(
        graph,
        ir_version=AFFINE_ONNX_IR_VERSION,
        opset_imports=[helper.make_opsetid("", AFFINE_ONNX_OPSET)],
        producer_name="wave-onnx-hybrid",
        producer_version="1",
    )
    onnx_model.model_version = AFFINE_SCHEMA_VERSION
    properties = {
        "wave.model.name": "wave-erf-affine-mask",
        "wave.model.partition": "quantized-affine-plus-erf",
        "wave.model.quantization": model.candidate.name,
        "wave.model.schema_version": str(AFFINE_SCHEMA_VERSION),
        "wave.model.contract_sha256": model.contract_sha256,
    }
    for key, value in sorted(properties.items()):
        entry = onnx_model.metadata_props.add()
        entry.key = key
        entry.value = value
    onnx.checker.check_model(onnx_model, full_check=True)
    return onnx_model.SerializeToString(deterministic=True)


def select_affine_candidate(
    features: np.ndarray,
    slopes: np.ndarray,
    biases: np.ndarray,
) -> tuple[QuantizedAffineModel, dict[str, Any]]:
    feature_values = np.asarray(features)
    slope_values, bias_values = _source_weights(slopes, biases)
    if (
        feature_values.ndim != 2
        or feature_values.shape[0] == 0
        or feature_values.shape[1] != ERF_BINS
        or not np.issubdtype(feature_values.dtype, np.floating)
        or not np.isfinite(feature_values).all()
    ):
        raise _invalid_input("candidate selection needs finite [N,481] features")
    baseline = cpu_erf_u8(
        quantize_logits(
            np.asarray(feature_values, dtype=np.float64) * slope_values[np.newaxis, :]
            + bias_values[np.newaxis, :]
        )
    )
    records: list[dict[str, Any]] = []
    deployments: list[QuantizedAffineModel] = []
    for index, candidate in enumerate(_CANDIDATES):
        deployment = quantize_affine_model(
            slope_values,
            bias_values,
            candidate,
        )
        observed = affine_erf_output_u8(
            quantize_features(feature_values, candidate),
            deployment,
        )
        errors = np.abs(observed.astype(np.int16) - baseline.astype(np.int16))
        record = {
            "name": candidate.name,
            "mean_squared_u8_error": float(
                np.mean(np.square(errors.astype(np.float64)))
            ),
            "mean_absolute_u8_error": float(np.mean(errors)),
            "maximum_u8_error": int(np.max(errors)),
            "exact_fraction": float(np.mean(errors == 0)),
            "registry_index": index,
        }
        deployments.append(deployment)
        records.append(record)
    selected_index = min(
        range(len(records)),
        key=lambda index: (
            records[index]["mean_squared_u8_error"],
            records[index]["maximum_u8_error"],
            records[index]["registry_index"],
        ),
    )
    return deployments[selected_index], {
        "schema_version": 1,
        "selection": "minimum-mse-then-max-error-then-registry-order",
        "selected": records[selected_index]["name"],
        "baseline": "float-affine-then-qualified-u8-erf",
        "candidates": records,
    }
