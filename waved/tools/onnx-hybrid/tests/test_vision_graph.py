"""Bounded SqueezeNet graph recipes use spatial shapes and exact FP16 bytes."""

import copy
import hashlib

import numpy as np
import onnx
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.vision_graph import (
    CHECKPOINT_SHA256,
    import_graph,
    independent_graph_oracle,
    validate_graph_output,
    validate_graph_record,
)


def model(control="20a"):
    constants = []
    nodes = []

    def conv(inp, out, ci, co, kernel, stride=1, pad=0):
        weights = np.arange(co * ci * kernel * kernel, dtype=np.float32)
        weights = ((weights % 17 - 8) / 256).astype("<f2")
        weights = weights.reshape(co, ci, kernel, kernel)
        bias = (np.arange(co) / 512).astype("<f2")
        constants.extend(
            [
                numpy_helper.from_array(weights, out + "W"),
                numpy_helper.from_array(bias, out + "B"),
            ]
        )
        nodes.append(
            helper.make_node(
                "Conv",
                [inp, out + "W", out + "B"],
                [out],
                kernel_shape=[kernel, kernel],
                strides=[stride, stride],
                dilations=[1, 1],
                pads=[pad] * 4,
                group=1,
            )
        )
        nodes.append(helper.make_node("Relu", [out], [out + "R"]))
        return out + "R"

    if control == "20a":
        incoming, outgoing = [1, 3, 17, 13], [1, 64, 8, 6]
        out = conv("input", "first", 3, 64, 3, stride=2)
    elif control == "20b":
        incoming, outgoing = [1, 64, 15, 19], [1, 64, 7, 9]
        nodes.append(
            helper.make_node(
                "MaxPool",
                ["input"],
                ["pool"],
                kernel_shape=[3, 3],
                strides=[2, 2],
                ceil_mode=1,
                pads=[0] * 4,
            )
        )
        out = "pool"
    elif control == "20c":
        incoming, outgoing = [1, 64, 7, 9], [1, 128, 7, 9]
        squeeze = conv("input", "squeeze", 64, 16, 1)
        one = conv(squeeze, "one", 16, 64, 1)
        three = conv(squeeze, "three", 16, 64, 3, pad=1)
        nodes.append(helper.make_node("Concat", [one, three], ["cat"], axis=1))
        out = "cat"
    elif control == "20d":
        incoming, outgoing = [1, 512, 13, 13], [1, 1000]
        activated = conv("input", "classifier", 512, 1000, 1)
        nodes.append(helper.make_node("GlobalAveragePool", [activated], ["avg"]))
        nodes.append(helper.make_node("Flatten", ["avg"], ["logits"], axis=1))
        out = "logits"
    else:
        incoming, outgoing = [1, 3, 224, 224], [1, 1000]

        def pool(inp, out):
            nodes.append(
                helper.make_node(
                    "MaxPool",
                    [inp],
                    [out],
                    kernel_shape=[3, 3],
                    strides=[2, 2],
                    ceil_mode=1,
                    pads=[0] * 4,
                )
            )
            return out

        out = pool(conv("input", "stem", 3, 64, 3, stride=2), "pool0")
        ci = 64
        for i, (squeeze, expand) in enumerate(
            (
                (16, 64),
                (16, 64),
                (32, 128),
                (32, 128),
                (48, 192),
                (48, 192),
                (64, 256),
                (64, 256),
            )
        ):
            squeezed = conv(out, f"s{i}", ci, squeeze, 1)
            one = conv(squeezed, f"e1{i}", squeeze, expand, 1)
            three = conv(squeezed, f"e3{i}", squeeze, expand, 3, pad=1)
            out = f"cat{i}"
            nodes.append(helper.make_node("Concat", [one, three], [out], axis=1))
            ci = 2 * expand
            if i in (1, 3):
                out = pool(out, f"pool{i + 1}")
        activated = conv(out, "head", 512, 1000, 1)
        nodes.append(helper.make_node("GlobalAveragePool", [activated], ["avg"]))
        nodes.append(helper.make_node("Flatten", ["avg"], ["logits"], axis=1))
        out = "logits"
    graph = helper.make_graph(
        nodes,
        "fixture",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT16, incoming)],
        [helper.make_tensor_value_info(out, TensorProto.FLOAT16, outgoing)],
        constants,
    )
    result = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    result.ir_version = 8
    for key, value in {
        "wave.native_profile": "fp16-squeezenet-v1",
        "wave.checkpoint_sha256": CHECKPOINT_SHA256,
        "wave.control": control,
    }.items():
        prop = result.metadata_props.add()
        prop.key, prop.value = key, value
    return result


def lower(tmp_path, value=None):
    path = tmp_path / "graph.onnx"
    onnx.save(model() if value is None else value, path)
    return import_graph(path)


def seal(record):
    record.pop("lowering_sha256", None)
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    return record


@pytest.mark.parametrize("control", ["20a", "20b", "20c", "20d", "20e"])
def test_valid_recipe_and_native_spatial_layout(tmp_path, control):
    record = lower(tmp_path, model(control))
    validate_graph_record(record)
    s = record["onnx_input"]["shape"]
    assert record["native_input"]["shape"] == [s[3], s[2], s[1], 1]
    if control == "20d":
        assert record["native_output"]["shape"] == [1000, 1]
    assert record["numerical_policy"] == {"atol": 0.03, "rtol": 0.015}


def test_non_square_conv_independent_scalar_reference(tmp_path):
    record = lower(tmp_path)
    x = (np.arange(3 * 17 * 13).reshape(1, 3, 17, 13) % 19 / 16).astype("<f2")
    actual = next(iter(independent_graph_oracle(record, {"input": x}).values()))
    n = record["nodes"][0]
    w = np.frombuffer(
        bytes.fromhex(record["constants"][n["weights"]]["payload_hex"]), dtype="<f2"
    ).reshape(64, 3, 3, 3)
    b = np.frombuffer(
        bytes.fromhex(record["constants"][n["bias"]]["payload_hex"]), dtype="<f2"
    )
    expected = np.empty((1, 64, 8, 6), dtype="<f2")
    for o in range(64):
        for y in range(8):
            for z in range(6):
                total = np.sum(
                    x[0, :, 2 * y : 2 * y + 3, 2 * z : 2 * z + 3].astype(np.float32)
                    * w[o].astype(np.float32),
                    dtype=np.float32,
                ) + np.float32(b[o])
                expected[0, o, y, z] = max(np.float16(total), np.float16(0))
    np.testing.assert_array_equal(actual, expected)


def test_pool_non_square_values_and_nan_output_rejected(tmp_path):
    record = lower(tmp_path, model("20b"))
    x = np.arange(64 * 15 * 19, dtype=np.float32).reshape(1, 64, 15, 19).astype("<f2")
    actual = next(iter(independent_graph_oracle(record, {"input": x}).values()))
    expected = np.maximum.reduce(
        [x[:, :, y : y + 14 : 2, z : z + 18 : 2] for y in range(3) for z in range(3)]
    )
    np.testing.assert_array_equal(actual, expected)
    validate_graph_output(record, actual.tobytes(), actual.tobytes())
    bad = actual.copy()
    bad.flat[1] = np.nan
    with pytest.raises(HybridError, match="nonfinite"):
        validate_graph_output(record, actual.tobytes(), bad.tobytes())


@pytest.mark.parametrize(
    "mutation",
    ["unknown_attr", "unknown_op", "unused", "ceil", "metadata", "float32", "external"],
)
def test_import_refusals(tmp_path, mutation):
    m = model("20b" if mutation == "ceil" else "20a")
    if mutation == "unknown_attr":
        m.graph.node[0].attribute.append(helper.make_attribute("auto_pad", "VALID"))
    elif mutation == "unknown_op":
        m.graph.node[-1].op_type = "Sigmoid"
    elif mutation == "unused":
        m.graph.initializer.append(
            numpy_helper.from_array(np.ones(1, dtype="<f2"), "unused")
        )
    elif mutation == "ceil":
        m.graph.input[0].type.tensor_type.shape.dim[2].dim_value = 16
    elif mutation == "metadata":
        m.metadata_props[1].value = "0" * 64
    elif mutation == "float32":
        m.graph.input[0].type.tensor_type.elem_type = TensorProto.FLOAT
    else:
        m.graph.initializer[0].data_location = TensorProto.EXTERNAL
        prop = m.graph.initializer[0].external_data.add()
        prop.key, prop.value = "location", "outside.bin"
    with pytest.raises(HybridError):
        lower(tmp_path, m)


@pytest.mark.parametrize(
    "mutation",
    [
        "bool",
        "geometry",
        "weight",
        "bias",
        "payload",
        "layout",
        "order",
        "policy",
        "dead",
    ],
)
def test_resealed_record_refusals(tmp_path, mutation):
    r = copy.deepcopy(lower(tmp_path, model("20c")))
    if mutation == "bool":
        r["schema_version"] = True
    elif mutation == "geometry":
        r["nodes"][0]["strides"] = [2, 1]
    elif mutation == "weight":
        r["constants"][r["nodes"][0]["weights"]]["shape"] = [16, 64, 1]
    elif mutation == "bias":
        r["nodes"][0]["bias"] = None
    elif mutation == "payload":
        c = r["constants"][r["nodes"][0]["weights"]]
        c["payload_hex"] = "7e00" + c["payload_hex"][4:]
    elif mutation == "layout":
        r["native_input"]["shape"] = [7, 9, 64, 1]
    elif mutation == "order":
        r["nodes"][-1]["inputs"].reverse()
    elif mutation == "policy":
        r["native_policy"]["flatten"] = "elided"
    else:
        r["nodes"][3]["inputs"] = [r["nodes"][1]["output"]]
    with pytest.raises(HybridError):
        validate_graph_record(seal(r))


@pytest.mark.parametrize("node_index", [0, 6])
def test_resealed_boolean_input_shape_is_refused(tmp_path, node_index):
    record = lower(tmp_path, model("20c"))
    node = record["nodes"][node_index]
    if node_index == 6:
        node["input_shapes"] = copy.deepcopy(node["input_shapes"])
        node["input_shapes"][0][0] = True
    else:
        node["input_shape"] = [True, 64, 7, 9]
    with pytest.raises(HybridError):
        validate_graph_record(seal(record))


def replace_constant(record, name, values):
    c = record["constants"][name]
    payload = np.ascontiguousarray(values, dtype="<f2").tobytes()
    assert len(payload) == c["byte_size"]
    c["payload_hex"], c["sha256"] = payload.hex(), hashlib.sha256(payload).hexdigest()


def test_concat_oracle_keeps_distinct_expand_branch_channel_order(tmp_path):
    r = lower(tmp_path, model("20c"))
    for index in (0, 2, 4):
        n = r["nodes"][index]
        replace_constant(
            r, n["weights"], np.zeros(r["constants"][n["weights"]]["shape"])
        )
        replace_constant(
            r, n["bias"], np.full(n["output_shape"][1], {0: 1, 2: 2, 4: 3}[index])
        )
    output = next(
        iter(
            independent_graph_oracle(
                seal(r), {"input": np.ones((1, 64, 7, 9), dtype="<f2")}
            ).values()
        )
    )
    assert np.all(output[:, :64] == 2)
    assert np.all(output[:, 64:] == 3)


def test_gap_and_explicit_flatten_oracle_reduce_each_classifier_channel(tmp_path):
    r = lower(tmp_path, model("20d"))
    n = r["nodes"][0]
    weights = np.zeros((1000, 512, 1, 1), dtype="<f2")
    weights[:, 0, 0, 0] = 1
    replace_constant(r, n["weights"], weights)
    x = np.zeros((1, 512, 13, 13), dtype="<f2")
    x[0, 0] = np.arange(169).reshape(13, 13) / 128
    bias = np.frombuffer(
        bytes.fromhex(r["constants"][n["bias"]]["payload_hex"]), dtype="<f2"
    )
    stage = (
        x[:, :1].astype(np.float32) + bias[None, :, None, None].astype(np.float32)
    ).astype("<f2")
    expected = np.mean(stage.astype(np.float32), axis=(2, 3), dtype=np.float32).astype(
        "<f2"
    )
    actual = next(iter(independent_graph_oracle(seal(r), {"input": x}).values()))
    np.testing.assert_array_equal(actual, expected)
    assert actual.shape == (1, 1000)


def test_numerical_policy_is_per_element_and_never_accepts_nonfinite_reference(
    tmp_path,
):
    r = lower(tmp_path, model("20b"))
    reference = np.zeros((1, 64, 7, 9), dtype="<f2")
    observed = reference.copy()
    observed.flat[-1] = 0.04
    with pytest.raises(HybridError, match="frozen tolerance") as failure:
        validate_graph_output(r, reference.tobytes(), observed.tobytes())
    assert failure.value.details["mismatched_elements"] == 1
    reference.flat[0] = np.inf
    with pytest.raises(HybridError, match="nonfinite"):
        validate_graph_output(r, reference.tobytes(), observed.tobytes())


@pytest.mark.parametrize(
    "alteration",
    [
        "ir",
        "opset",
        "duplicate_metadata",
        "duplicate_attr",
        "declared_shape",
        "nan_constant",
        "dropout",
        "indices_output",
    ],
)
def test_additional_source_model_refusals(tmp_path, alteration):
    m = model("20b" if alteration == "indices_output" else "20a")
    if alteration == "ir":
        m.ir_version = 10
    elif alteration == "opset":
        m.opset_import[0].version = 14
    elif alteration == "duplicate_metadata":
        m.metadata_props.append(m.metadata_props[0])
    elif alteration == "duplicate_attr":
        m.graph.node[0].attribute.append(m.graph.node[0].attribute[0])
    elif alteration == "declared_shape":
        m.graph.output[0].type.tensor_type.shape.dim[2].dim_value = 9
    elif alteration == "nan_constant":
        values = numpy_helper.to_array(m.graph.initializer[0]).copy()
        values.flat[0] = np.nan
        m.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(values, m.graph.initializer[0].name)
        )
    elif alteration == "dropout":
        m.graph.node[-1].op_type = "Dropout"
    else:
        m.graph.node[0].output.append("indices")
    with pytest.raises(HybridError):
        lower(tmp_path, m)


@pytest.mark.parametrize("invalid_bit", [-1, 65536])
def test_typed_half_payload_cannot_alias_out_of_range_bits(tmp_path, invalid_bit):
    m = model()
    t = m.graph.initializer[0]
    bits = np.frombuffer(t.raw_data, dtype="<u2").astype(np.int32)
    t.ClearField("raw_data")
    t.int32_data.extend(bits.tolist())
    t.int32_data[0] = invalid_bit
    with pytest.raises(HybridError):
        lower(tmp_path, m)


def test_full_recipe_accounting_and_resealed_pruned_classifier_refusal(tmp_path):
    r = lower(tmp_path, model("20e"))
    assert len(r["nodes"]) == 65
    assert len(r["constants"]) == 52
    assert sum(c["byte_size"] for c in r["constants"].values()) == 2470992
    r["nodes"].pop(-3)
    r["operation"]["node_count"] -= 1
    with pytest.raises(HybridError, match="recipe node count"):
        validate_graph_record(seal(r))
