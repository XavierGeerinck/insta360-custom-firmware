"""Bounded graph admission and independent numeric qualification."""

from pathlib import Path

import numpy as np
import onnx
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.marblenet_graph import (
    import_graph,
    independent_graph_oracle,
    validate_graph_output,
    validate_graph_record,
)


def save_graph(tmp_path: Path, *, group=1, dilation=1, kernel=1):
    channels = 64
    shape = [1, channels, 200]
    weights = np.zeros((channels, channels // group, kernel), np.float16)
    for c in range(channels):
        weights[c, 0 if group == channels else c, 0] = np.float16((c + 1) / 64)
    bias = np.linspace(-0.5, 0.5, channels, dtype=np.float16)
    pad = dilation * (kernel - 1) // 2
    nodes = [
        helper.make_node(
            "Conv",
            ["x", "w", "b"],
            ["conv"],
            group=group,
            pads=[pad, pad],
            strides=[1],
            dilations=[dilation],
            kernel_shape=[kernel],
        ),
        helper.make_node("Relu", ["conv"], ["y"]),
    ]
    graph = helper.make_graph(
        nodes,
        "trained",
        [helper.make_tensor_value_info("x", TensorProto.FLOAT16, shape)],
        [helper.make_tensor_value_info("y", TensorProto.FLOAT16, shape)],
        [numpy_helper.from_array(weights, "w"), numpy_helper.from_array(bias, "b")],
    )
    model = helper.make_model(
        graph, opset_imports=[helper.make_opsetid("", 13)], ir_version=8
    )
    path = tmp_path / "model.onnx"
    onnx.save(model, path)
    return path, model, weights, bias


def test_actual_coefficient_layout_and_oracle(tmp_path):
    path, _, weights, bias = save_graph(tmp_path)
    record = import_graph(path)
    validate_graph_record(record)
    assert record["constants"]["w"]["payload_hex"] == weights.tobytes().hex()
    assert record["native_input"]["shape"] == [200, 1, 64, 1]
    x = np.ones((1, 64, 200), np.float16)
    actual = independent_graph_oracle(record, {"x": x})["y"]
    expected = np.maximum(
        weights[:, :, 0].astype(np.float32) @ x[0].astype(np.float32)
        + bias[:, None].astype(np.float32),
        0,
    ).astype(np.float16)[None]
    np.testing.assert_array_equal(actual, expected)


def test_tampered_record_refuses(tmp_path):
    record = import_graph(save_graph(tmp_path)[0])
    record["nodes"][0]["dilation"] = 2
    with pytest.raises(HybridError):
        validate_graph_record(record)


def test_numeric_bound_and_real_readbacks(tmp_path):
    record = import_graph(save_graph(tmp_path)[0])
    expected = np.zeros((1, 64, 200), np.float16).tobytes()
    okay = np.full((1, 64, 200), 0.01, np.float16).tobytes()
    result = validate_graph_output(record, expected, okay)
    assert result["passed"] and result["max_abs_error"] > 0
    for bad in [
        np.full((1, 64, 200), 0.03, np.float16).tobytes(),
        np.full((1, 64, 200), np.inf, np.float16).tobytes(),
        okay[:-2],
    ]:
        with pytest.raises(HybridError):
            validate_graph_output(record, expected, bad)


def reseal(record):
    import hashlib

    from wave_onnx_hybrid.audit import canonical_json

    record["lowering_sha256"] = hashlib.sha256(
        canonical_json({k: v for k, v in record.items() if k != "lowering_sha256"})
    ).hexdigest()
    return record


def test_boolean_schema_cannot_be_resealed(tmp_path):
    record = import_graph(save_graph(tmp_path)[0])
    record["schema_version"] = True
    with pytest.raises(HybridError):
        validate_graph_record(reseal(record))


@pytest.mark.parametrize(
    "field,value",
    [
        ("group", 2),
        ("stride", 0),
        ("dilation", 3),
        ("pads", [0, 1]),
        ("kernel", 3),
        ("bias", "missing"),
        ("weights", []),
    ],
)
def test_resealed_conv_parameters_refuse(tmp_path, field, value):
    record = import_graph(save_graph(tmp_path)[0])
    record["nodes"][0][field] = value
    with pytest.raises(HybridError):
        validate_graph_record(reseal(record))


@pytest.mark.parametrize(
    "mutation",
    [
        "domain",
        "opset",
        "fp32",
        "dynamic",
        "external",
        "unused",
        "broadcast",
        "bad-edge",
        "extra-output",
        "unknown-attribute",
    ],
)
def test_unsupported_onnx_refuses(tmp_path, mutation):
    path, model, _, _ = save_graph(tmp_path)
    graph = model.graph
    if mutation == "domain":
        graph.node[0].domain = "vendor"
    elif mutation == "opset":
        model.opset_import[0].version = 14
    elif mutation == "fp32":
        graph.input[0].type.tensor_type.elem_type = TensorProto.FLOAT
    elif mutation == "dynamic":
        graph.input[0].type.tensor_type.shape.dim[2].dim_param = "T"
    elif mutation == "external":
        graph.initializer[0].data_location = TensorProto.EXTERNAL
        graph.initializer[0].external_data.add(key="location", value="weights.bin")
    elif mutation == "unused":
        graph.initializer.append(
            numpy_helper.from_array(np.zeros((64,), np.float16), "unused")
        )
    elif mutation == "broadcast":
        graph.node[-1].op_type = "Add"
        graph.node[-1].input.append("b")
    elif mutation == "bad-edge":
        graph.node[1].input[0] = "later"
    elif mutation == "extra-output":
        graph.output.append(
            helper.make_tensor_value_info("conv", TensorProto.FLOAT16, [1, 64, 200])
        )
    elif mutation == "unknown-attribute":
        graph.node[0].attribute.append(helper.make_attribute("unknown", 1))
    path.write_bytes(model.SerializeToString())
    with pytest.raises(HybridError):
        import_graph(path)


def test_depthwise_dilation_and_changed_trained_parameters(tmp_path):
    path, model, w, _ = save_graph(tmp_path, group=64, dilation=2, kernel=11)
    w.fill(0)
    w[:, 0, 4] = 0.5
    w[:, 0, 6] = -0.25
    model.graph.initializer[0].CopyFrom(numpy_helper.from_array(w, "w"))
    path.write_bytes(model.SerializeToString())
    record = import_graph(path)
    x = np.linspace(-2, 2, 200, dtype=np.float16)[None, None, :].repeat(64, axis=1)
    output = independent_graph_oracle(record, {"x": x})["y"]
    padded = np.pad(x.astype(np.float32), ((0, 0), (0, 0), (10, 10)))
    manual = padded[:, :, 8:208] * 0.5 - padded[:, :, 12:212] * 0.25
    bias = np.frombuffer(
        bytes.fromhex(record["constants"]["b"]["payload_hex"]), dtype="<f2"
    ).astype(np.float32)
    manual = np.maximum(manual + bias[None, :, None], 0).astype(np.float16)
    np.testing.assert_array_equal(output, manual)
    record["nodes"][0]["dilation"] = 1
    record["nodes"][0]["pads"] = [5, 5]
    reseal(record)
    wrong = independent_graph_oracle(record, {"x": x})["y"]
    assert np.max(np.abs(wrong.astype(np.float32) - output.astype(np.float32))) > 0.1


def test_equal_shape_add_oracle_and_unused_nodes_refusal(tmp_path):
    path, model, _, _ = save_graph(tmp_path)
    model.graph.node[-1].CopyFrom(helper.make_node("Add", ["conv", "x"], ["y"]))
    path.write_bytes(model.SerializeToString())
    record = import_graph(path)
    x = np.ones((1, 64, 200), np.float16)
    actual = independent_graph_oracle(record, {"x": x})["y"]
    w = np.frombuffer(
        bytes.fromhex(record["constants"]["w"]["payload_hex"]), dtype="<f2"
    ).reshape(64, 64)
    b = np.frombuffer(
        bytes.fromhex(record["constants"]["b"]["payload_hex"]), dtype="<f2"
    )
    expected = (
        (
            w.astype(np.float32) @ x[0].astype(np.float32)
            + b[:, None].astype(np.float32)
        ).astype(np.float16)
        + x[0]
    ).astype(np.float16)[None]
    np.testing.assert_array_equal(actual, expected)
    model.graph.node.insert(0, helper.make_node("Relu", ["x"], ["dead"]))
    path.write_bytes(model.SerializeToString())
    with pytest.raises(HybridError):
        import_graph(path)


def test_constant_identity_and_input_contract_refusal(tmp_path):
    import copy

    record = import_graph(save_graph(tmp_path)[0])
    bad = copy.deepcopy(record)
    bad["constants"]["w"]["payload_hex"] = (
        "0000" + bad["constants"]["w"]["payload_hex"][4:]
    )
    with pytest.raises(HybridError):
        validate_graph_record(reseal(bad))
    for x in [
        np.zeros((1, 64, 200), np.float32),
        np.zeros((1, 2, 200), np.float16),
        np.full((1, 64, 200), np.nan, np.float16),
    ]:
        with pytest.raises(HybridError):
            independent_graph_oracle(record, {"x": x})


def test_unchanged_canary_refuses_even_within_numeric_tolerance(tmp_path):
    record = import_graph(save_graph(tmp_path)[0])
    expected = np.full((1, 64, 200), -0.01, np.float16).tobytes()
    canary = b"\xa5" * len(expected)
    with pytest.raises(HybridError):
        validate_graph_output(record, expected, canary)
