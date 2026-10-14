"""Metadata-only derivation and fixed mean lowering keep original references."""

import copy
import hashlib

import numpy as np
import onnx
import pytest
from test_vision_graph import model, seal

from wave_onnx_hybrid import vision_graph, vision_mean, vision_sources
from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.native_import import import_native_model, independent_oracle
from wave_onnx_hybrid.native_sources import (
    generate_compile_source,
    generate_execute_source,
)

COMPILE = "/tmp/wave-npu-onnx21-0123456789abcdef-compile"
EXECUTE = "/tmp/wave-npu-onnx21-0123456789abcdef-execute"


@pytest.fixture
def parents(tmp_path, monkeypatch):
    originals, lowerings, derived = {}, {}, {}
    paths = {}
    for control in ("20d", "20e"):
        value = model(control)
        value.graph.node[-1].output[0] = "output"
        value.graph.output[0].name = "output"
        path = tmp_path / f"{control}.onnx"
        path.write_bytes(value.SerializeToString())
        paths[control] = path
        originals[control] = hashlib.sha256(path.read_bytes()).hexdigest()
        lowerings[control] = vision_graph.import_graph(path)["lowering_sha256"]
        for prop in value.metadata_props:
            if prop.key == "wave.native_profile":
                prop.value = vision_mean.PROFILE_ID
        derived[control] = hashlib.sha256(value.SerializeToString()).hexdigest()
    monkeypatch.setattr(vision_mean, "ORIGINAL_MODEL_SHA256", originals)
    monkeypatch.setattr(vision_mean, "ORIGINAL_LOWERING_SHA256", lowerings)
    monkeypatch.setattr(vision_mean, "DERIVED_MODEL_SHA256", derived)
    return paths


def lower(tmp_path, payload):
    path = tmp_path / "derived.onnx"
    path.write_bytes(payload)
    return vision_mean.import_graph(path)


@pytest.mark.parametrize("control", ["20d", "20e"])
def test_derivation_changes_only_marker_and_recovers_exact_parent(
    tmp_path, parents, control
):
    original = parents[control].read_bytes()
    payload = vision_mean.derive_mean_model(original)
    value = onnx.load_model_from_string(payload)
    for prop in value.metadata_props:
        if prop.key == "wave.native_profile":
            assert prop.value == vision_mean.PROFILE_ID
            prop.value = vision_graph.PROFILE_ID
    assert value.SerializeToString() == original
    record = lower(tmp_path, payload)
    assert record["control_id"] == control
    assert (
        record["source_original_model_sha256"] == hashlib.sha256(original).hexdigest()
    )
    assert (
        record["constants"] == vision_graph.import_graph(parents[control])["constants"]
    )
    assert record["native_policy"]["weight_shape"] == [13, 13, 1000, 1]
    assert record["native_policy"]["group"] == 1000


def test_scalar_diagnostic_is_single_original_gap_with_original_mean_oracle(tmp_path):
    payload = vision_mean.make_mean_control()
    record = lower(tmp_path, payload)
    assert import_native_model(tmp_path / "derived.onnx") == record
    assert record["control_id"] == "21a" and record["constants"] == {}
    assert record["source_original_model_sha256"] is None
    assert [n["type"] for n in record["nodes"]] == ["GlobalAveragePool"]
    x = np.arange(1, 170, dtype="<f2").reshape(1, 1, 13, 13)
    actual = independent_oracle(record, {"input": x})["output"]
    assert actual.item() == 85
    vision_mean.compare_output(record, actual.tobytes(), actual.tobytes())
    with pytest.raises(HybridError):
        vision_mean.compare_output(
            record, actual.tobytes(), np.zeros_like(actual).tobytes()
        )
    with pytest.raises(HybridError):
        vision_mean.compare_output(
            record, actual.tobytes(), np.full_like(actual, np.nan).tobytes()
        )
    assert record["native_policy"]["half_bits"] == 0x1E0F


@pytest.mark.parametrize(
    "mutation", ["parent_weight", "marker", "metadata", "op", "shape", "checkpoint"]
)
def test_changed_parent_or_diagnostic_refused(tmp_path, parents, mutation):
    payload = parents["20d"].read_bytes()
    value = onnx.load_model_from_string(payload)
    if mutation == "parent_weight":
        value.graph.initializer[0].raw_data = bytes(
            len(value.graph.initializer[0].raw_data)
        )
    elif mutation == "marker":
        value.metadata_props[0].value = vision_mean.PROFILE_ID
    elif mutation == "metadata":
        extra = value.metadata_props.add()
        extra.key, extra.value = "extra", "unsupported"
    elif mutation == "checkpoint":
        value.metadata_props[1].value = "0" * 64
    else:
        value = onnx.load_model_from_string(vision_mean.make_mean_control())
        if mutation == "op":
            value.graph.node[0].op_type = "ReduceMean"
        else:
            value.graph.input[0].type.tensor_type.shape.dim[2].dim_value = 12
        with pytest.raises(HybridError):
            lower(tmp_path, value.SerializeToString())
        return
    with pytest.raises(HybridError):
        vision_mean.derive_mean_model(value.SerializeToString())


@pytest.mark.parametrize(
    "mutation",
    [
        "bits",
        "group",
        "multiplier",
        "shape",
        "bias",
        "stride",
        "profile",
        "bool",
        "parent",
        "weight",
    ],
)
def test_resealed_record_refusals(tmp_path, parents, mutation):
    record = lower(tmp_path, vision_mean.derive_mean_model(parents["20d"].read_bytes()))
    policy = record["native_policy"]
    if mutation == "bits":
        policy["half_bits"] = 0x1E10
    elif mutation == "group":
        policy["group"] = 1
    elif mutation == "multiplier":
        policy["multiplier"] = 0
    elif mutation == "shape":
        policy["weight_shape"] = [13, 13, 1, 1000]
    elif mutation == "bias":
        policy["bias_bits"] = 1
    elif mutation == "stride":
        policy["strides"] = [13, 13]
    elif mutation == "profile":
        record["profile_id"] = vision_graph.PROFILE_ID
    elif mutation == "bool":
        policy["multiplier"] = True
    elif mutation == "parent":
        record["source_original_model_sha256"] = "0" * 64
    else:
        c = record["constants"][record["nodes"][0]["weights"]]
        payload = bytes(c["byte_size"])
        c.update(payload_hex=payload.hex(), sha256=hashlib.sha256(payload).hexdigest())
    with pytest.raises(HybridError):
        vision_mean.validate_graph_record(seal(record))


@pytest.mark.parametrize("control", ["21a", "20d", "20e"])
def test_native_mean_source_shape_derived_bits_bounds_and_call_lifecycle(
    tmp_path, parents, control
):
    payload = (
        vision_mean.make_mean_control()
        if control == "21a"
        else vision_mean.derive_mean_model(parents[control].read_bytes())
    )
    record = lower(tmp_path, payload)
    source = generate_compile_source(record, COMPILE)
    vision_sources.validate_compile_source(record, COMPILE, source)
    channels = 1 if control == "21a" else 1000
    node = next(
        i for i, n in enumerate(record["nodes"]) if n["type"] == "GlobalAveragePool"
    )
    assert f"weight_shape{node}{{13, 13, {channels}, 1}}" in source
    assert f"pad{node}{{0, 0, 0, 0}}" in source
    assert f"stride{node}{{1, 1}}" in source and f"dilation{node}{{1, 1}}" in source
    assert f"pad{node}, stride{node}, dilation{node}, 1" in source
    assert "tim::vx::PoolType::AVG" not in source
    assert source.count("0x1e0fU") >= 169 * channels
    assert f"kMeanBiasBits{node}{{" in source
    assert source.count("graph->CompileToBinary(") == 2 and "graph->Run(" not in source
    assert "kWatchdogSeconds = 15" in source and "alarm(0)" not in source
    assert "fixed-depthwise-mean13-v1" in source
    executor = generate_execute_source(record, EXECUTE, 1800)
    assert (
        "fp16-squeezenet-mean" in executor and "kCanaryHalfBits = 0x7e00U" in executor
    )
    assert executor.count("graph->Compile()") == executor.count("graph->Run()") == 1
    assert (
        executor.index("graph->Compile()")
        < executor.index("device_canary[index]")
        < executor.index("graph->Run()")
    )
    assert "kWatchdogSeconds = 8" in executor and "alarm(0)" not in executor
    vision_sources.validate_execute_source(record, EXECUTE, 1800, executor)


def test_old_vision_source_bytes_unchanged(tmp_path, parents):
    for path in parents.values():
        record = vision_graph.import_graph(path)
        compiler = generate_compile_source(record, COMPILE.replace("21-", "20-"))
        assert "kMeanWeights" not in compiler
        assert "tim::vx::PoolType::AVG" in compiler
        with pytest.raises(ValueError):
            generate_compile_source(record, COMPILE)


def test_new_source_root_changed_source_and_synthesized_emission_bounds(
    tmp_path, monkeypatch
):
    r = lower(tmp_path, vision_mean.make_mean_control())
    for root in (COMPILE.replace("21-", "20-"), COMPILE + "/../compile"):
        with pytest.raises(ValueError):
            generate_compile_source(r, root)
    source = generate_compile_source(r, COMPILE)
    with pytest.raises(ValueError, match="policy"):
        vision_sources.validate_compile_source(
            r, COMPILE, source.replace("0x1e0fU", "0x1e10U")
        )
    monkeypatch.setattr(vision_sources, "_MAX_EMITTED_CONSTANT_BYTES", 169 * 2 + 3)
    with pytest.raises(ValueError, match="bound"):
        generate_compile_source(r, COMPILE)


def test_classifier_oracle_wrapper_uses_exact_original_record(
    tmp_path, parents, monkeypatch
):
    r = lower(tmp_path, vision_mean.derive_mean_model(parents["20d"].read_bytes()))
    original_record = vision_graph.import_graph(parents["20d"])
    result = {"output": np.zeros((1, 1000), dtype="<f2")}
    seen = []

    def original_oracle(record, inputs):
        seen.append((record, inputs))
        return result

    monkeypatch.setattr(vision_graph, "independent_graph_oracle", original_oracle)
    inputs = {"input": np.ones((1, 512, 13, 13), dtype="<f2")}
    assert vision_mean.independent_graph_oracle(r, inputs) is result
    assert seen == [(original_record, inputs)]


@pytest.mark.parametrize("control", ["21a", "20d", "20e"])
def test_derived_source_payload_is_exact_fixed_half_and_f32_zero(
    tmp_path, parents, control
):
    import re
    import struct

    payload = (
        vision_mean.make_mean_control()
        if control == "21a"
        else vision_mean.derive_mean_model(parents[control].read_bytes())
    )
    r = lower(tmp_path, payload)
    source = generate_compile_source(r, COMPILE)
    index = next(
        i for i, n in enumerate(r["nodes"]) if n["type"] == "GlobalAveragePool"
    )
    channels = 1 if control == "21a" else 1000
    for label, width, expected in (
        ("MeanWeights", "H", b"\x0f\x1e" * (169 * channels)),
        ("MeanBiasBits", "I", bytes(4 * channels)),
    ):
        matched = re.search(rf"k{label}{index}\{{([^}}]+)\}}", source)
        bits = [int(v.strip()[:-1], 16) for v in matched[1].split(",")]
        assert struct.pack(f"<{len(bits)}{width}", *bits) == expected


def test_diagnostic_resealed_shapes_source_identity_and_extra_fields_refused(tmp_path):
    r = lower(tmp_path, vision_mean.make_mean_control())
    for key, value in (
        ("source_model_sha256", "0" * 64),
        ("source_original_model_sha256", "1" * 64),
        ("numerical_policy", {"atol": 1.0, "rtol": 1.0}),
        ("constants", {"untrained": {}}),
    ):
        changed = copy.deepcopy(r)
        changed[key] = value
        with pytest.raises(HybridError):
            vision_mean.validate_graph_record(seal(changed))
    changed = copy.deepcopy(r)
    changed["nodes"][0]["input_shape"] = [True, 1, 13, 13]
    with pytest.raises(HybridError):
        vision_mean.validate_graph_record(seal(changed))
    changed = copy.deepcopy(r)
    changed["extra"] = 0
    with pytest.raises(HybridError):
        vision_mean.validate_graph_record(seal(changed))
