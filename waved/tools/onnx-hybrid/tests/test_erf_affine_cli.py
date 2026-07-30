from __future__ import annotations

import numpy as np

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.erf_affine_cli import (
    _affine_feature_partition,
    _build_affine_package_payloads,
    main,
)
from wave_onnx_hybrid.erf_affine_model import (
    affine_erf_output_u8,
    quantization_candidates,
    quantize_affine_model,
    quantize_features,
)
from wave_onnx_hybrid.erf_affine_package import verify_affine_package
from wave_onnx_hybrid.erf_affine_qualification import (
    build_affine_qualification_vectors,
)
from wave_onnx_hybrid.erf_mask_signal import ERF_BINS


def _inputs():
    slopes = np.linspace(0.01, 1.7, ERF_BINS, dtype=np.float32)
    biases = np.linspace(-0.7, 0.5, ERF_BINS, dtype=np.float32)
    model = quantize_affine_model(
        slopes,
        biases,
        quantization_candidates()[1],
    )
    features = np.resize(
        np.linspace(-8.0, 8.0, ERF_BINS, dtype=np.float32),
        (12, ERF_BINS),
    )
    vectors = build_affine_qualification_vectors(model, features)
    evaluation = {
        "schema_version": 1,
        "passed": True,
        "private_inputs_used": False,
        "affine": {
            "model_contract_sha256": model.contract_sha256,
            "selected_candidate": model.candidate.name,
        },
    }
    source = {
        "schema_version": 1,
        "private_inputs_used": False,
        "source": "unit-test",
    }
    return model, vectors, evaluation, source


def test_complete_feature_partition_matches_independent_integer_oracle() -> None:
    model, _, _, _ = _inputs()
    feature = np.linspace(-8.0, 8.0, ERF_BINS, dtype=np.float64)

    observed = _affine_feature_partition(model)(feature)
    expected = affine_erf_output_u8(
        quantize_features(feature[np.newaxis, :], model.candidate),
        model,
    )[0]

    np.testing.assert_array_equal(observed, expected)
    assert observed.shape == (ERF_BINS,)
    assert observed.dtype == np.uint8


def test_affine_package_payload_builder_is_deterministic_and_verified() -> None:
    model, vectors, evaluation, source = _inputs()

    first_artifacts, first_manifest = _build_affine_package_payloads(
        model,
        vectors,
        evaluation,
        source,
    )
    second_artifacts, second_manifest = _build_affine_package_payloads(
        model,
        vectors,
        evaluation,
        source,
    )

    assert first_artifacts == second_artifacts
    assert canonical_json(first_manifest) == canonical_json(second_manifest)
    assert (
        verify_affine_package(first_manifest, model, first_artifacts) == first_manifest
    )
    assert first_manifest["execution_approved"] is False
    assert first_manifest["compiler_qualified"] is False


def test_native_constant_export_is_exclusive_and_verifiable(
    tmp_path,
) -> None:
    model, vectors, evaluation, source = _inputs()
    artifacts, manifest = _build_affine_package_payloads(
        model,
        vectors,
        evaluation,
        source,
    )
    package = tmp_path / "package"
    package.mkdir()
    for name, payload in artifacts.items():
        (package / name).write_bytes(payload)
    (package / "model-manifest.json").write_bytes(canonical_json(manifest))
    output = tmp_path / "affine-constants.bin"

    assert (
        main(
            [
                "export-native-constants",
                "--package",
                str(package),
                "--output",
                str(output),
            ]
        )
        == 0
    )
    assert output.read_bytes()[:4] == b"AFC1"
    assert (
        main(
            [
                "export-native-constants",
                "--package",
                str(package),
                "--output",
                str(output),
            ]
        )
        == 3
    )
