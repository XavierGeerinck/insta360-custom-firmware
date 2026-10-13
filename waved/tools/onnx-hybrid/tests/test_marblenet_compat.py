"""Frozen diagnostic tap/dilation variants; no device calls."""

from pathlib import Path

import numpy as np
import onnx
import pytest
from onnx import numpy_helper

from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.marblenet_compat import (
    _derive_variant,
    _validate_variant,
    prepare_isolation,
)


@pytest.fixture(scope="module")
def prepared():
    path = Path(__file__).resolve().parents[4] / "artifacts/npu/poc18/prepared"
    if not path.is_dir():
        pytest.skip("ignored frozen PoC18 inputs unavailable")
    return path


@pytest.mark.parametrize(
    "control,kernel,dilation,pad,start,end",
    [
        ("19a", 13, 2, 12, 8, 21),
        ("19b", 29, 1, 14, 0, 29),
    ],
)
def test_variant_tap_bits_geometry_and_downstream(
    prepared, control, kernel, dilation, pad, start, end
):
    source = onnx.load(prepared / "18c.onnx")
    model = _derive_variant(source, control)
    _validate_variant(source, model, control)
    weights = numpy_helper.to_array(source.graph.initializer[0])
    actual = numpy_helper.to_array(model.graph.initializer[0])
    assert (
        actual.shape == (64, 1, kernel)
        and actual.tobytes() == weights[..., start:end].copy().tobytes()
    )
    attrs = {
        a.name: onnx.helper.get_attribute_value(a)
        for a in model.graph.node[0].attribute
    }
    assert (
        attrs["kernel_shape"] == [kernel]
        and attrs["dilations"] == [dilation]
        and attrs["pads"] == [pad, pad]
    )
    assert [n.SerializeToString() for n in source.graph.node[1:]] == [
        n.SerializeToString() for n in model.graph.node[1:]
    ]
    assert [t.SerializeToString() for t in source.graph.initializer[1:]] == [
        t.SerializeToString() for t in model.graph.initializer[1:]
    ]


@pytest.mark.parametrize(
    "change",
    ["taps", "pads", "dilation", "downstream-node", "downstream-weight", "metadata"],
)
def test_wrong_taps_padding_or_unchanged_fields_refuse(prepared, change):
    source = onnx.load(prepared / "18c.onnx")
    variant = _derive_variant(source, "19a")
    if change == "taps":
        wrong = numpy_helper.to_array(source.graph.initializer[0])[..., 7:20].copy()
        variant.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(wrong, source.graph.initializer[0].name)
        )
    elif change in {"pads", "dilation"}:
        name = "pads" if change == "pads" else "dilations"
        attr = next(a for a in variant.graph.node[0].attribute if a.name == name)
        del attr.ints[:]
        attr.ints.extend([11, 11] if change == "pads" else [1])
    elif change == "downstream-node":
        variant.graph.node[1].input[0] = "input"
    elif change == "downstream-weight":
        weight = variant.graph.initializer[1]
        array = numpy_helper.to_array(weight).copy()
        array.flat[0] += np.float16(0.01)
        weight.CopyFrom(numpy_helper.from_array(array, weight.name))
    else:
        variant.producer_name = "changed metadata"
    with pytest.raises(HybridError):
        _validate_variant(source, variant, "19a")


@pytest.fixture
def output(tmp_path, monkeypatch):
    from wave_onnx_hybrid import marblenet_compat

    root = tmp_path / "poc19"
    monkeypatch.setattr(marblenet_compat, "OUTPUT_ROOT", root)
    return root / "prepared"


@pytest.mark.parametrize("change", ["preparation", "source", "input"])
def test_changed_frozen_artifact_refuses_before_ort(
    prepared, output, tmp_path, monkeypatch, change
):
    import shutil

    from wave_onnx_hybrid import marblenet_compat

    copy = tmp_path / "source"
    shutil.copytree(prepared, copy)
    target = (
        copy
        / {
            "preparation": "preparation.json",
            "source": "18c.onnx",
            "input": "18c_input.npz",
        }[change]
    )
    target.write_bytes(b"changed artifact")
    monkeypatch.setattr(
        marblenet_compat.ort,
        "InferenceSession",
        lambda *a, **kw: pytest.fail("ORT constructed after frozen artifact change"),
    )
    with pytest.raises(HybridError):
        prepare_isolation(copy, output)
    assert not output.exists()


def test_output_alias_symlink_and_existing_directory_refuse(
    prepared, output, tmp_path, monkeypatch
):
    from wave_onnx_hybrid import marblenet_compat

    monkeypatch.setattr(
        marblenet_compat,
        "_frozen",
        lambda *a: pytest.fail("input read for unsafe output"),
    )
    for path in [prepared, prepared / "new-output", prepared.parent]:
        with pytest.raises(HybridError):
            prepare_isolation(prepared, path)
    output.parent.mkdir()
    output.symlink_to(tmp_path / "future-directory", target_is_directory=True)
    with pytest.raises(HybridError):
        prepare_isolation(prepared, output)
    output.unlink()
    output.mkdir()
    with pytest.raises(HybridError):
        prepare_isolation(prepared, output)


@pytest.mark.parametrize("bad", ["nonfinite", "shape", "dtype", "values", "provider"])
def test_invalid_cpu_reference_refuses(prepared, bad, monkeypatch):
    from wave_onnx_hybrid import marblenet_compat
    from wave_onnx_hybrid.marblenet_graph import import_graph

    source = onnx.load(prepared / "18c.onnx")
    record = import_graph(prepared / "18c.onnx")
    with np.load(prepared / "18c_input.npz", allow_pickle=False) as z:
        values = z["input"]

    class Session:
        def __init__(self, *a, **kw):
            assert kw["providers"] == ["CPUExecutionProvider"]

        def get_providers(self):
            return (
                ["CUDAExecutionProvider"]
                if bad == "provider"
                else ["CPUExecutionProvider"]
            )

        def run(self, *a):
            if bad == "nonfinite":
                return [np.full((1, 128, 200), np.nan, np.float16)]
            if bad == "shape":
                return [np.zeros((1, 200, 128), np.float16)]
            if bad == "dtype":
                return [np.zeros((1, 128, 200), np.float32)]
            return [np.full((1, 128, 200), 10, np.float16)]

    monkeypatch.setattr(marblenet_compat.ort, "InferenceSession", Session)
    with pytest.raises(HybridError):
        marblenet_compat._references(source.SerializeToString(), record, values)


def test_actual_preparation_preserves_input_and_fresh_identities(prepared, output):
    import hashlib
    import json

    receipt = prepare_isolation(prepared, output)
    assert receipt["diagnostic_only"] and not receipt["full_model_equivalent"]
    assert set(receipt["controls"]) == {"19a", "19b"}
    assert len({v["model_sha256"] for v in receipt["controls"].values()}) == 2
    assert len({v["lowering_sha256"] for v in receipt["controls"].values()}) == 2
    for name, control in receipt["controls"].items():
        assert control["input_sha256"] == receipt["source_input_sha256"]
        assert (output / (name + "_input.npz")).read_bytes() == (
            prepared / "18c_input.npz"
        ).read_bytes()
        assert control["cpu_comparison"]["status"] == "PASS"
        assert (
            control["depthwise"]["stride"] == 1 and control["depthwise"]["group"] == 64
        )
    for name, identity in receipt["files"].items():
        assert (
            hashlib.sha256((output / name).read_bytes()).hexdigest()
            == identity["sha256"]
        )
    assert json.loads((output / "preparation.json").read_text()) == receipt


def test_concurrent_child_symlink_cannot_overwrite_frozen_source(
    prepared, output, tmp_path, monkeypatch
):
    import shutil

    from wave_onnx_hybrid import marblenet_compat

    source_copy = tmp_path / "source"
    shutil.copytree(prepared, source_copy)
    original_bytes = (source_copy / "18c.onnx").read_bytes()
    derive = marblenet_compat._derive_variant

    def planted(source, control):
        result = derive(source, control)
        if control == "19a":
            (output / "19a.onnx").symlink_to(source_copy / "18c.onnx")
        return result

    monkeypatch.setattr(marblenet_compat, "_derive_variant", planted)
    with pytest.raises(HybridError):
        prepare_isolation(source_copy, output)
    assert (source_copy / "18c.onnx").read_bytes() == original_bytes
