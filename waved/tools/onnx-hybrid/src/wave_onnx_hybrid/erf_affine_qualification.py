"""Deterministic six-vector pack for the affine-plus-Erf partition."""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

import numpy as np

from .erf_affine_model import (
    QuantizedAffineModel,
    affine_erf_output_u8,
    quantize_features,
)
from .erf_mask_signal import ERF_BINS
from .errors import ErrorCode, HybridError

AFFINE_VECTOR_MAGIC = b"AEF1"
AFFINE_VECTOR_SCHEMA_VERSION = 1
AFFINE_VECTOR_HEADER = struct.Struct("<4sIIII32s")
AFFINE_VECTOR_IDS = (
    "zero-point",
    "minimum",
    "maximum",
    "repeated-ramp",
    "seeded-random",
    "learned-validation",
)
AFFINE_VECTOR_RANDOM_SEED = 0x14A57335
_INPUT_TYPE_CODES = {"UINT8": 1, "INT8": 2}


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _readonly(values: np.ndarray, dtype: np.dtype[Any]) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=dtype)
    result.flags.writeable = False
    return result


@dataclass(frozen=True, slots=True)
class AffineQualificationVectors:
    inputs: tuple[np.ndarray, ...]
    outputs: tuple[np.ndarray, ...]
    learned_frame_index: int

    def __post_init__(self) -> None:
        if (
            len(self.inputs) != len(AFFINE_VECTOR_IDS)
            or len(self.outputs) != len(AFFINE_VECTOR_IDS)
            or type(self.learned_frame_index) is not int
            or self.learned_frame_index < 0
        ):
            raise _invalid("invalid affine qualification vector collection")
        input_dtype = self.inputs[0].dtype
        if input_dtype not in (np.dtype(np.uint8), np.dtype(np.int8)):
            raise _invalid("invalid affine qualification input dtype")
        for inputs, outputs in zip(self.inputs, self.outputs, strict=True):
            if (
                inputs.shape != (ERF_BINS,)
                or inputs.dtype != input_dtype
                or inputs.flags.writeable
                or not inputs.flags.c_contiguous
                or outputs.shape != (ERF_BINS,)
                or outputs.dtype != np.dtype(np.uint8)
                or outputs.flags.writeable
                or not outputs.flags.c_contiguous
            ):
                raise _invalid("invalid immutable affine qualification vector")


def _seeded_random(model: QuantizedAffineModel) -> np.ndarray:
    values = np.empty(ERF_BINS, dtype=model.candidate.numpy_dtype)
    state = AFFINE_VECTOR_RANDOM_SEED
    for index in range(ERF_BINS):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        state &= 0xFFFFFFFF
        values[index] = model.candidate.qmin + (state & 0xFF)
    return _readonly(values, model.candidate.numpy_dtype)


def build_affine_qualification_vectors(
    model: QuantizedAffineModel,
    learned_features: np.ndarray,
) -> AffineQualificationVectors:
    """Build fixed boundaries plus one nondegenerate validation feature."""

    values = np.asarray(learned_features)
    if (
        values.ndim != 2
        or values.shape[0] == 0
        or values.shape[1] != ERF_BINS
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid("learned affine vectors need finite [N,481] features")
    quantized = quantize_features(values, model.candidate)
    learned_outputs = affine_erf_output_u8(quantized, model)
    ranked = [
        (
            int(np.unique(output).size),
            int(np.unique(inputs).size),
            float(np.var(output.astype(np.float64))),
            -index,
            index,
        )
        for index, (inputs, output) in enumerate(
            zip(quantized, learned_outputs, strict=True)
        )
    ]
    learned_frame_index = max(ranked)[-1]
    learned = _readonly(
        quantized[learned_frame_index],
        model.candidate.numpy_dtype,
    )
    learned_output = _readonly(
        learned_outputs[learned_frame_index],
        np.dtype(np.uint8),
    )
    if np.unique(learned).size < 2 or np.unique(learned_output).size < 2:
        raise _invalid("learned affine qualification vector is degenerate")

    ramp_values = np.arange(
        model.candidate.qmin,
        model.candidate.qmax + 1,
        dtype=np.int16,
    )
    inputs = (
        _readonly(
            np.full(ERF_BINS, model.candidate.feature_zero_point),
            model.candidate.numpy_dtype,
        ),
        _readonly(
            np.full(ERF_BINS, model.candidate.qmin),
            model.candidate.numpy_dtype,
        ),
        _readonly(
            np.full(ERF_BINS, model.candidate.qmax),
            model.candidate.numpy_dtype,
        ),
        _readonly(
            np.resize(ramp_values, ERF_BINS),
            model.candidate.numpy_dtype,
        ),
        _seeded_random(model),
        learned,
    )
    outputs = tuple(
        _readonly(
            affine_erf_output_u8(vector[np.newaxis, :], model)[0],
            np.dtype(np.uint8),
        )
        for vector in inputs
    )
    return AffineQualificationVectors(
        inputs=inputs,
        outputs=outputs,
        learned_frame_index=learned_frame_index,
    )


def serialize_affine_qualification_vectors(
    collection: AffineQualificationVectors,
    model: QuantizedAffineModel,
) -> bytes:
    input_code = _INPUT_TYPE_CODES[model.candidate.feature_dtype]
    expected_dtype = model.candidate.numpy_dtype
    for inputs, outputs in zip(
        collection.inputs,
        collection.outputs,
        strict=True,
    ):
        if inputs.dtype != expected_dtype or not np.array_equal(
            outputs,
            affine_erf_output_u8(inputs[np.newaxis, :], model)[0],
        ):
            raise _invalid("affine qualification vectors violate the model")
    payload = b"".join(vector.tobytes() for vector in collection.inputs)
    payload += b"".join(vector.tobytes() for vector in collection.outputs)
    return (
        AFFINE_VECTOR_HEADER.pack(
            AFFINE_VECTOR_MAGIC,
            AFFINE_VECTOR_SCHEMA_VERSION,
            len(collection.inputs),
            ERF_BINS,
            input_code,
            bytes.fromhex(model.contract_sha256),
        )
        + payload
    )


def parse_affine_qualification_vectors(
    payload: bytes,
    model: QuantizedAffineModel,
) -> tuple[tuple[np.ndarray, ...], tuple[np.ndarray, ...]]:
    if not isinstance(payload, bytes) or len(payload) < AFFINE_VECTOR_HEADER.size:
        raise _invalid("affine qualification vector pack is truncated")
    magic, schema, count, width, input_code, contract_hash = (
        AFFINE_VECTOR_HEADER.unpack_from(payload)
    )
    expected_bytes = AFFINE_VECTOR_HEADER.size + count * width * 2
    if (
        magic != AFFINE_VECTOR_MAGIC
        or schema != AFFINE_VECTOR_SCHEMA_VERSION
        or count != len(AFFINE_VECTOR_IDS)
        or width != ERF_BINS
        or input_code != _INPUT_TYPE_CODES[model.candidate.feature_dtype]
        or contract_hash != bytes.fromhex(model.contract_sha256)
        or len(payload) != expected_bytes
    ):
        raise _invalid("affine qualification vector header or length differs")
    matrix = np.frombuffer(
        payload,
        dtype=model.candidate.numpy_dtype,
        count=count * width,
        offset=AFFINE_VECTOR_HEADER.size,
    ).reshape(count, width)
    output_offset = AFFINE_VECTOR_HEADER.size + count * width
    output_matrix = np.frombuffer(
        payload,
        dtype=np.uint8,
        count=count * width,
        offset=output_offset,
    ).reshape(count, width)
    inputs = tuple(_readonly(row, model.candidate.numpy_dtype) for row in matrix)
    outputs = tuple(_readonly(row, np.dtype(np.uint8)) for row in output_matrix)
    if any(
        not np.array_equal(
            output,
            affine_erf_output_u8(inputs_value[np.newaxis, :], model)[0],
        )
        for inputs_value, output in zip(inputs, outputs, strict=True)
    ):
        raise _invalid("affine qualification oracle output differs")
    return inputs, outputs


def qualification_vector_manifest(
    collection: AffineQualificationVectors,
    model: QuantizedAffineModel,
) -> dict[str, Any]:
    records = []
    for vector_id, inputs, outputs in zip(
        AFFINE_VECTOR_IDS,
        collection.inputs,
        collection.outputs,
        strict=True,
    ):
        records.append(
            {
                "id": vector_id,
                "input_sha256": hashlib.sha256(inputs).hexdigest(),
                "output_sha256": hashlib.sha256(outputs).hexdigest(),
                "input_min": int(inputs.min()),
                "input_max": int(inputs.max()),
                "input_unique": int(np.unique(inputs).size),
                "output_min": int(outputs.min()),
                "output_max": int(outputs.max()),
                "output_unique": int(np.unique(outputs).size),
            }
        )
    return {
        "schema_version": AFFINE_VECTOR_SCHEMA_VERSION,
        "vector_count": len(records),
        "width": ERF_BINS,
        "input_dtype": model.candidate.feature_dtype,
        "random_seed": AFFINE_VECTOR_RANDOM_SEED,
        "learned_frame_index": collection.learned_frame_index,
        "model_contract_sha256": model.contract_sha256,
        "vectors": records,
    }


def verify_qualification_vector_manifest(
    manifest: Mapping[str, Any],
    inputs: tuple[np.ndarray, ...],
    outputs: tuple[np.ndarray, ...],
    model: QuantizedAffineModel,
) -> dict[str, Any]:
    if not isinstance(manifest, Mapping):
        raise _invalid("affine qualification manifest must be an object")
    learned_frame_index = manifest.get("learned_frame_index")
    if type(learned_frame_index) is not int or learned_frame_index < 0:
        raise _invalid("affine qualification learned frame is invalid")
    collection = AffineQualificationVectors(
        inputs=inputs,
        outputs=outputs,
        learned_frame_index=learned_frame_index,
    )
    expected = qualification_vector_manifest(collection, model)
    if dict(manifest) != expected:
        raise _invalid("affine qualification vector manifest differs")
    return expected
