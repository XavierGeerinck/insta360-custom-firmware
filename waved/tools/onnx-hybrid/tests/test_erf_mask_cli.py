from __future__ import annotations

import json
from pathlib import Path

import numpy as np

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.erf_mask_cli import main
from wave_onnx_hybrid.erf_mask_corpus import (
    ProceduralCorpusConfig,
    generate_clean_source,
)
from wave_onnx_hybrid.erf_mask_metrics import (
    FixtureSpeechItem,
    build_device_fixture,
)
from wave_onnx_hybrid.erf_mask_model import (
    build_erf_mask_onnx,
    serialize_erf_mask_weights,
)
from wave_onnx_hybrid.erf_mask_package import build_unsealed_model_manifest
from wave_onnx_hybrid.erf_mask_signal import (
    ERF_BINS,
    SAMPLE_RATE,
    build_erf_u8_table,
)
from wave_onnx_hybrid.fixtures import deterministic_npz_bytes


def _artifacts() -> dict[str, bytes]:
    slopes = np.ones(ERF_BINS, dtype=np.float32)
    biases = np.zeros(ERF_BINS, dtype=np.float32)
    return {
        "weights.npz": serialize_erf_mask_weights(slopes, biases),
        "model.onnx": build_erf_mask_onnx(slopes, biases),
        "erf-u8-lut.bin": build_erf_u8_table().tobytes(),
        "training-manifest.json": b'{"schema_version":1,"run":"test"}\n',
        "corpus-manifest.json": b'{"schema_version":1,"corpus":"test"}\n',
        "config.json": b'{"schema_version":1,"config":"test"}\n',
    }


def _model_directory(root: Path) -> Path:
    model = root / "model"
    model.mkdir()
    artifacts = _artifacts()
    for name, payload in artifacts.items():
        (model / name).write_bytes(payload)
    (model / "model-manifest.json").write_bytes(
        canonical_json(build_unsealed_model_manifest(artifacts))
    )
    return model


def _fixture_directory(root: Path) -> Path:
    sources = [
        FixtureSpeechItem(
            identity=f"speaker/{index}",
            source_sha256=f"{index:064x}",
            samples=generate_clean_source(
                "voiced",
                SAMPLE_RATE * 2,
                seed=100 + index,
            ),
        )
        for index in range(1, 5)
    ]
    fixture = build_device_fixture(sources)
    output = root / "fixture"
    output.mkdir()
    (output / "fixture-manifest.json").write_bytes(canonical_json(fixture.manifest))
    (output / "fixture.npz").write_bytes(
        deterministic_npz_bytes(
            {
                "clean": fixture.clean,
                "noise": fixture.noise,
                "mixture": fixture.mixture,
            }
        )
    )
    return output


def test_all_host_commands_exist_and_require_explicit_arguments(capsys) -> None:
    for command in (
        "generate-procedural",
        "acquire-slr31",
        "build-examples",
        "train",
        "evaluate",
        "build-fixture",
        "verify-package",
        "qualify-host",
    ):
        assert main([command]) == 2
        error = json.loads(capsys.readouterr().err)
        assert error["error"]["code"] == "ONNX_INPUT_INVALID"


def test_generate_procedural_is_atomic_exclusive_and_deterministic(
    tmp_path: Path,
    capsys,
) -> None:
    config = tmp_path / "config.json"
    config.write_bytes(canonical_json(ProceduralCorpusConfig().to_document()))
    output = tmp_path / "procedural"
    args = [
        "generate-procedural",
        "--config",
        str(config),
        "--output",
        str(output),
    ]

    assert main(args) == 0
    capsys.readouterr()
    assert (output / "config.json").read_bytes() == config.read_bytes()
    manifest = json.loads((output / "corpus-manifest.json").read_text(encoding="utf-8"))
    assert manifest["private_inputs_used"] is False
    assert manifest["sample_rate"] == SAMPLE_RATE
    assert not output.with_name(".procedural.partial").exists()

    assert main(args) == 3
    error = json.loads(capsys.readouterr().err)
    assert error["error"]["code"] == "OUTPUT_EXISTS"


def test_failed_command_removes_atomic_partial_directory(
    tmp_path: Path,
    capsys,
) -> None:
    bad_config = tmp_path / "bad.json"
    bad_config.write_text("not-json", encoding="utf-8")
    output = tmp_path / "failed"

    assert (
        main(
            [
                "generate-procedural",
                "--config",
                str(bad_config),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    capsys.readouterr()
    assert not output.exists()
    assert not output.with_name(".failed.partial").exists()


def test_verify_package_writes_canonical_evidence_once(
    tmp_path: Path,
    capsys,
) -> None:
    model = _model_directory(tmp_path)
    output = tmp_path / "verification"
    args = [
        "verify-package",
        "--model",
        str(model),
        "--output",
        str(output),
    ]

    assert main(args) == 0
    capsys.readouterr()
    evidence = json.loads((output / "verification.json").read_text(encoding="utf-8"))
    assert evidence["verified"] is True
    assert evidence["execution_approved"] is False
    assert main(args) == 3
    assert json.loads(capsys.readouterr().err)["error"]["code"] == "OUTPUT_EXISTS"


def test_qualify_host_has_stable_quality_failure_exit_and_no_output(
    tmp_path: Path,
    capsys,
) -> None:
    model = _model_directory(tmp_path)
    fixture = _fixture_directory(tmp_path)
    evaluation = tmp_path / "evaluation.json"
    evaluation.write_bytes(
        canonical_json(
            {
                "schema_version": 1,
                "passed": False,
                "items_sha256": "a" * 64,
                "summary": {},
            }
        )
    )
    output = tmp_path / "qualification"

    assert (
        main(
            [
                "qualify-host",
                "--model",
                str(model),
                "--evaluation",
                str(evaluation),
                "--fixture",
                str(fixture),
                "--output",
                str(output),
            ]
        )
        == 4
    )
    error = json.loads(capsys.readouterr().err)
    assert error["error"]["code"] == "AUDIO_QUALITY_GATE_FAILURE"
    assert not output.exists()


def test_cli_module_has_no_device_transport_import() -> None:
    source = (
        Path(__file__).parents[1] / "src" / "wave_onnx_hybrid" / "erf_mask_cli.py"
    ).read_text(encoding="utf-8")
    lowered = source.lower()

    assert "import adb" not in lowered
    assert "import waved_npu" not in lowered
    assert "from waved_npu" not in lowered
    assert "subprocess" not in lowered
