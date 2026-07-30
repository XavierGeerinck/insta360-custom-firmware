"""Deterministic ONNX controls for capability and fallback testing."""

from __future__ import annotations

import hashlib
import io
import json
import os
import tempfile
import zipfile
from pathlib import Path
from typing import Any

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

from .errors import ErrorCode, HybridError

FIXTURE_SCHEMA_VERSION = 1
FIXTURE_OPSET = 18
FIXTURE_IR_VERSION = 10


def _conv_initializers() -> list[onnx.TensorProto]:
    return [
        numpy_helper.from_array(
            np.array([[[0.25, 0.5, 0.25]]], dtype=np.float32),
            name="conv.weight",
        ),
        numpy_helper.from_array(
            np.array([0.0], dtype=np.float32),
            name="conv.bias",
        ),
    ]


def _tensor_info(name: str, shape: list[int | str | None]) -> onnx.ValueInfoProto:
    return helper.make_tensor_value_info(name, TensorProto.FLOAT, shape)


def _model(
    graph: onnx.GraphProto,
    *,
    purpose: str,
) -> onnx.ModelProto:
    model = helper.make_model(
        graph,
        ir_version=FIXTURE_IR_VERSION,
        opset_imports=[helper.make_opsetid("", FIXTURE_OPSET)],
        producer_name="wave-onnx-hybrid-fixtures",
        producer_version="1",
    )
    model.model_version = 1
    property_entry = model.metadata_props.add()
    property_entry.key = "wave.fixture.purpose"
    property_entry.value = purpose
    onnx.checker.check_model(model, full_check=True)
    return model


def _spectral_mask_model() -> onnx.ModelProto:
    graph = helper.make_graph(
        [
            helper.make_node(
                "Conv",
                ["features", "conv.weight", "conv.bias"],
                ["conv.out"],
                name="mask.conv",
                pads=[1, 1],
                strides=[1],
            ),
            helper.make_node(
                "Sigmoid",
                ["conv.out"],
                ["mask"],
                name="mask.sigmoid",
            ),
        ],
        "spectral-mask-conv-control",
        [_tensor_info("features", [1, 1, 8])],
        [_tensor_info("mask", [1, 1, 8])],
        initializer=_conv_initializers(),
    )
    return _model(
        graph,
        purpose=(
            "Fixed tensor-level Conv/Sigmoid spectral mask control; "
            "not a trained denoiser"
        ),
    )


def _mixed_sin_model() -> onnx.ModelProto:
    graph = helper.make_graph(
        [
            helper.make_node(
                "Conv",
                ["features", "conv.weight", "conv.bias"],
                ["conv.out"],
                name="mixed.conv",
                pads=[1, 1],
                strides=[1],
            ),
            helper.make_node(
                "Sin",
                ["conv.out"],
                ["output"],
                name="mixed.sin",
            ),
        ],
        "mixed-conv-sin-control",
        [_tensor_info("features", [1, 1, 8])],
        [_tensor_info("output", [1, 1, 8])],
        initializer=_conv_initializers(),
    )
    return _model(
        graph,
        purpose="Conv followed by an explicitly future-NPU-unsupported Sin",
    )


def _dynamic_identity_model() -> onnx.ModelProto:
    graph = helper.make_graph(
        [
            helper.make_node(
                "Identity",
                ["features"],
                ["output"],
                name="dynamic.identity",
            )
        ],
        "dynamic-identity-control",
        [_tensor_info("features", [1, 1, "frames"])],
        [_tensor_info("output", [1, 1, "frames"])],
    )
    return _model(
        graph,
        purpose="CPU-admissible dynamic shape and NPU-ineligible control",
    )


def build_fixture_models() -> dict[str, onnx.ModelProto]:
    """Return fixtures in canonical filename order."""

    return {
        "dynamic_identity.onnx": _dynamic_identity_model(),
        "mixed_sin.onnx": _mixed_sin_model(),
        "spectral_mask_conv.onnx": _spectral_mask_model(),
    }


def serialize_model(model: onnx.ModelProto) -> bytes:
    return model.SerializeToString(deterministic=True)


def spectral_mask_input() -> np.ndarray:
    return np.array(
        [[[-1.0, -0.5, 0.0, 0.5, 1.0, 0.25, -0.25, 0.75]]],
        dtype=np.float32,
    )


def spectral_mask_expected(values: np.ndarray) -> np.ndarray:
    padded = np.pad(values, ((0, 0), (0, 0), (1, 1)))
    convolved = (
        padded[:, :, 0:8] * np.float32(0.25)
        + padded[:, :, 1:9] * np.float32(0.5)
        + padded[:, :, 2:10] * np.float32(0.25)
    )
    return np.asarray(
        1.0 / (1.0 + np.exp(-convolved)),
        dtype=np.float32,
    )


def deterministic_npz_bytes(arrays: dict[str, np.ndarray]) -> bytes:
    """Serialize numeric arrays with fixed ZIP metadata and canonical order."""

    output = io.BytesIO()
    with zipfile.ZipFile(output, mode="w", compression=zipfile.ZIP_STORED) as archive:
        for name in sorted(arrays):
            array = arrays[name]
            if not isinstance(array, np.ndarray) or array.dtype.hasobject:
                raise HybridError(
                    ErrorCode.EVIDENCE_INVALID,
                    f"tensor {name!r} must be a non-object NumPy array",
                )
            tensor = io.BytesIO()
            np.lib.format.write_array(
                tensor,
                np.ascontiguousarray(array),
                allow_pickle=False,
                version=(2, 0),
            )
            entry = zipfile.ZipInfo(f"{name}.npy", date_time=(1980, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_STORED
            entry.create_system = 3
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, tensor.getvalue())
    return output.getvalue()


def _canonical_json(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def _write_exclusive(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        descriptor = os.open(
            path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL,
            0o644,
        )
    except FileExistsError as exc:
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {path}",
        ) from exc
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _atomic_write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def generate_fixtures(output_directory: Path | str) -> dict[str, Any]:
    """Generate all fixtures exactly once and return their strict manifest."""

    output = Path(output_directory)
    manifest_path = output / "manifest.json"
    if manifest_path.exists():
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {manifest_path}",
        )

    records: list[dict[str, Any]] = []
    tensor_records: list[dict[str, Any]] = []
    written: list[Path] = []
    try:
        for filename, model in build_fixture_models().items():
            payload = serialize_model(model)
            path = output / filename
            _write_exclusive(path, payload)
            written.append(path)
            records.append(
                {
                    "path": filename,
                    "bytes": len(payload),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                    "ir_version": model.ir_version,
                    "opsets": {
                        (item.domain or "ai.onnx"): item.version
                        for item in model.opset_import
                    },
                    "node_count": len(model.graph.node),
                }
            )
        tensor_sets = {
            "spectral_mask_expected.npz": {
                "mask": spectral_mask_expected(spectral_mask_input())
            },
            "spectral_mask_input.npz": {"features": spectral_mask_input()},
        }
        for filename, arrays in sorted(tensor_sets.items()):
            payload = deterministic_npz_bytes(arrays)
            path = output / filename
            _write_exclusive(path, payload)
            written.append(path)
            tensor_records.append(
                {
                    "path": filename,
                    "bytes": len(payload),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                    "tensors": {
                        name: {
                            "dtype": str(array.dtype),
                            "shape": list(array.shape),
                            "bytes": array.nbytes,
                            "sha256": hashlib.sha256(
                                np.ascontiguousarray(array).tobytes(order="C")
                            ).hexdigest(),
                        }
                        for name, array in sorted(arrays.items())
                    },
                }
            )
        manifest = {
            "schema_version": FIXTURE_SCHEMA_VERSION,
            "generator": ("wave_onnx_hybrid.fixtures.generate_fixtures"),
            "models": records,
            "tensor_sets": tensor_records,
        }
        _atomic_write(manifest_path, _canonical_json(manifest))
        return manifest
    except BaseException:
        for path in written:
            path.unlink(missing_ok=True)
        manifest_path.unlink(missing_ok=True)
        raise


def verify_fixtures(output_directory: Path | str) -> dict[str, Any]:
    """Regenerate in a temporary directory and compare every tracked byte."""

    tracked = Path(output_directory)
    if not tracked.is_dir():
        raise HybridError(
            ErrorCode.EVIDENCE_INVALID,
            f"fixture directory does not exist: {tracked}",
        )
    with tempfile.TemporaryDirectory(prefix="wave-onnx-fixture-verify-") as temp:
        generated = Path(temp)
        manifest = generate_fixtures(generated)
        expected_paths = sorted(
            path.relative_to(generated)
            for path in generated.iterdir()
            if path.is_file()
        )
        tracked_paths = sorted(
            path.relative_to(tracked) for path in tracked.iterdir() if path.is_file()
        )
        if tracked_paths != expected_paths:
            raise HybridError(
                ErrorCode.EVIDENCE_INVALID,
                "fixture file set differs from deterministic generation",
            )
        for relative in expected_paths:
            if (tracked / relative).read_bytes() != (generated / relative).read_bytes():
                raise HybridError(
                    ErrorCode.EVIDENCE_INVALID,
                    f"fixture differs from deterministic generation: {relative}",
                )
    return manifest
