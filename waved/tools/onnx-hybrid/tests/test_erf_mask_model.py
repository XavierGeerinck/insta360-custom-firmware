from __future__ import annotations

import io
from dataclasses import replace
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import pytest

from wave_onnx_hybrid.audit import audit_model_bytes, load_policy
from wave_onnx_hybrid.erf_mask_model import (
    ErfPartitionContract,
    build_erf_mask_onnx,
    erf_partition_signature,
    serialize_erf_mask_weights,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS
from wave_onnx_hybrid.erf_mask_train import predict_quantized_masks
from wave_onnx_hybrid.errors import ErrorCode, HybridError

REPOSITORY_ROOT = Path(__file__).resolve().parents[4]
POLICY_PATH = REPOSITORY_ROOT / "docs/npu/onnx/hybrid-policy.json"


def _weights() -> tuple[np.ndarray, np.ndarray]:
    slopes = np.linspace(0.25, 1.75, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.75, 0.5, ERF_BINS, dtype=np.float32)
    return slopes, biases


def test_weight_archive_is_deterministic_with_exact_tensor_contract() -> None:
    slopes, biases = _weights()

    left = serialize_erf_mask_weights(slopes, biases)
    right = serialize_erf_mask_weights(slopes.copy(), biases.copy())

    assert left == right
    with np.load(io.BytesIO(left), allow_pickle=False) as archive:
        assert archive.files == ["a", "b"]
        np.testing.assert_array_equal(archive["a"], slopes)
        np.testing.assert_array_equal(archive["b"], biases)
        assert archive["a"].dtype == np.float32
        assert archive["b"].dtype == np.float32
        assert archive["a"].shape == (ERF_BINS,)
        assert archive["b"].shape == (ERF_BINS,)


def test_weight_archive_rejects_invalid_tensor() -> None:
    slopes, biases = _weights()
    with pytest.raises(HybridError) as shape:
        serialize_erf_mask_weights(slopes[:-1], biases)
    assert shape.value.code is ErrorCode.MODEL_PACKAGE_INVALID

    bad = biases.copy()
    bad[0] = np.nan
    with pytest.raises(HybridError) as finite:
        serialize_erf_mask_weights(slopes, bad)
    assert finite.value.code is ErrorCode.MODEL_PACKAGE_INVALID


def test_onnx_export_is_deterministic_checked_and_has_exact_topology() -> None:
    slopes, biases = _weights()

    left = build_erf_mask_onnx(slopes, biases)
    right = build_erf_mask_onnx(slopes.copy(), biases.copy())
    model = onnx.load_model_from_string(left)

    assert left == right
    onnx.checker.check_model(model, full_check=True)
    inferred = onnx.shape_inference.infer_shapes(model, strict_mode=True)
    assert inferred.graph.input[0].name == "feature"
    assert inferred.graph.output[0].name == "mask"
    assert [node.op_type for node in model.graph.node] == [
        "Mul",
        "Add",
        "Clip",
        "QuantizeLinear",
        "DequantizeLinear",
        "Erf",
        "QuantizeLinear",
        "DequantizeLinear",
        "Add",
        "Mul",
        "Clip",
    ]
    input_shape = model.graph.input[0].type.tensor_type.shape
    assert [dimension.dim_value for dimension in input_shape.dim] == [
        1,
        ERF_BINS,
    ]


def test_onnx_runtime_matches_quantized_numpy_model_exactly() -> None:
    slopes, biases = _weights()
    payload = build_erf_mask_onnx(slopes, biases)
    session = ort.InferenceSession(
        payload,
        providers=["CPUExecutionProvider"],
    )
    rng = np.random.Generator(np.random.PCG64(0x1309))
    features = rng.uniform(-8.0, 8.0, size=(1, ERF_BINS)).astype(np.float32)

    observed = session.run(["mask"], {"feature": features})[0]
    expected = predict_quantized_masks(features, slopes, biases)

    np.testing.assert_array_equal(observed, expected)


def test_onnx_graph_signature_is_stable_and_weight_sensitive() -> None:
    slopes, biases = _weights()
    policy = load_policy(POLICY_PATH, repository_root=REPOSITORY_ROOT)
    first = audit_model_bytes(
        build_erf_mask_onnx(slopes, biases),
        source_name="erf-mask.onnx",
        policy=policy,
    )
    changed = slopes.copy()
    changed[0] += np.float32(0.01)
    second = audit_model_bytes(
        build_erf_mask_onnx(changed, biases),
        source_name="erf-mask-changed.onnx",
        policy=policy,
    )

    assert len(first.graph_signature) == 64
    assert first.graph_signature != second.graph_signature
    assert all(node["placement"] == "cpu" for node in first.document["nodes"])
    assert first.document["policy"]["compiler_qualified"] is False


def test_partition_signature_covers_every_npu_boundary_field() -> None:
    contract = ErfPartitionContract()
    baseline = erf_partition_signature(contract)

    assert baseline.document["operation"] == "Erf"
    assert baseline.document["input"]["shape"] == [ERF_BINS]
    assert baseline.document["input"]["dtype"] == "UINT8"
    assert baseline.document["output"]["dtype"] == "UINT8"
    assert len(baseline.sha256) == 64
    assert (
        erf_partition_signature(replace(contract, input_scale=0.0625)).sha256
        != baseline.sha256
    )
    assert (
        erf_partition_signature(replace(contract, bins=ERF_BINS - 1)).sha256
        != baseline.sha256
    )
