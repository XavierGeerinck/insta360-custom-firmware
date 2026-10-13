"""Exact profile admission, coefficient import and independent oracle controls."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.native_fixtures import generate_native_fixtures
from wave_onnx_hybrid.native_import import import_native_model, independent_oracle


@pytest.fixture
def controls(tmp_path: Path) -> Path:
    generate_native_fixtures(tmp_path)
    return tmp_path


def rewrite(path: Path, model: onnx.ModelProto) -> None:
    path.write_bytes(model.SerializeToString(deterministic=True))


def test_fc_output_columns_are_transposed(controls: Path) -> None:
    record = import_native_model(controls / "17b2.onnx")
    weights = np.frombuffer(
        bytes.fromhex(record["weights"]["payload_hex"]), dtype=np.uint8
    ).reshape(4, 16)
    expected = np.array(
        [[int((k + 2 * j) % 5 == 0) for k in range(16)] for j in range(4)],
        dtype=np.uint8,
    )
    np.testing.assert_array_equal(weights, expected)
    assert record["native_input"]["shape"] == [16, 1]
    assert record["native_output"]["shape"] == [4, 1]
    assert record["weights"]["shape"] == [16, 4]
    assert record["bias"]["payload_hex"] == "00" * 16
    assert record["bias"]["dtype"] == "INT32"


def test_changed_weights_change_all_graph_identities(controls: Path) -> None:
    path = controls / "17b2.onnx"
    before = import_native_model(path)
    model = onnx.load(path)
    tensor = next(t for t in model.graph.initializer if t.name == "weight")
    values = numpy_helper.to_array(tensor).copy()
    values[1, 2] = 7
    tensor.CopyFrom(numpy_helper.from_array(values, "weight"))
    rewrite(path, model)
    after = import_native_model(path)
    for key in ("source_model_sha256", "graph_signature", "lowering_sha256"):
        assert before[key] != after[key]
    assert before["weights"]["sha256"] != after["weights"]["sha256"]
    actual = np.frombuffer(
        bytes.fromhex(after["weights"]["payload_hex"]), dtype=np.uint8
    ).reshape(4, 16)
    assert actual[2, 1] == 7


@pytest.mark.parametrize("control", ["17a1", "17a2", "17b1", "17b2"])
def test_frozen_oracle_matches_ort_every_byte(controls: Path, control: str) -> None:
    model_path = controls / f"{control}.onnx"
    model = onnx.load(model_path)
    assert model.ir_version == 10
    assert [(o.domain, o.version) for o in model.opset_import] == [("", 13)]
    record = import_native_model(model_path)
    with np.load(controls / f"{control}_input.npz", allow_pickle=False) as archive:
        inputs = {name: archive[name] for name in archive.files}
    independent = independent_oracle(record, inputs)
    session = ort.InferenceSession(str(model_path), providers=["CPUExecutionProvider"])
    actual = session.run(None, inputs)[0]
    expected = independent[record["onnx_output"]["name"]]
    assert expected.dtype == actual.dtype
    assert expected.shape == actual.shape
    assert expected.tobytes() == actual.tobytes()
    with np.load(controls / f"{control}_expected.npz", allow_pickle=False) as archive:
        assert expected.tobytes() == archive[record["onnx_output"]["name"]].tobytes()
    identity = copy.deepcopy(record)
    identity.pop("lowering_sha256")
    assert (
        record["lowering_sha256"]
        == hashlib.sha256(canonical_json(identity)).hexdigest()
    )
    for name in ("weights", "bias"):
        tensor = record[name]
        payload = bytes.fromhex(tensor["payload_hex"])
        assert len(payload) == tensor["byte_size"]
        assert tensor["sha256"] == hashlib.sha256(payload).hexdigest()


def test_conv_ranks_bias_and_coefficient_bits(controls: Path) -> None:
    first = import_native_model(controls / "17a1.onnx")
    second = import_native_model(controls / "17a2.onnx")
    assert first["onnx_input"]["shape"] == [1, 1, 16]
    assert second["onnx_input"]["shape"] == [1, 1, 1, 16]
    for record in (first, second):
        assert record["native_input"]["shape"] == [16, 1, 1, 1]
        assert record["native_output"]["shape"] == [16, 1, 1, 1]
        assert record["weights"]["shape"] == [3, 1, 1, 1]
        assert record["operation"]["dilations"] == [0, 0]
    assert first["bias"]["payload_hex"] == np.array([0], dtype="<f4").tobytes().hex()
    assert (
        second["bias"]["payload_hex"] == np.array([0.125], dtype="<f4").tobytes().hex()
    )
    path = controls / "17a2.onnx"
    model = onnx.load(path)
    bits = np.array([0x8000, 0x0001, 0x3555], dtype="<u2")
    model.graph.initializer[0].CopyFrom(
        numpy_helper.from_array(bits.view("<f2").reshape(1, 1, 1, 3), "weight")
    )
    model.graph.initializer[1].CopyFrom(
        numpy_helper.from_array(np.array([-0.0], dtype="<f2"), "bias")
    )
    rewrite(path, model)
    record = import_native_model(path)
    assert record["weights"]["payload_hex"] == bits.tobytes().hex()
    assert (
        record["bias"]["payload_hex"] == np.array([-0.0], dtype="<f4").tobytes().hex()
    )


@pytest.mark.parametrize("auto_pad", [False, True])
def test_equivalent_conv_padding_is_admitted(controls: Path, auto_pad: bool) -> None:
    path = controls / "17a2.onnx"
    model = onnx.load(path)
    del model.graph.node[0].attribute[:]
    attrs = {"auto_pad": "SAME_UPPER"} if auto_pad else {"pads": [0, 1, 0, 1]}
    attrs.update(kernel_shape=[1, 3], strides=[1, 1], dilations=[1, 1], group=1)
    model.graph.node[0].attribute.extend(
        helper.make_attribute(k, v) for k, v in attrs.items()
    )
    rewrite(path, model)
    assert import_native_model(path)["profile_id"] == "fp16-conv-v1"


@pytest.mark.parametrize(
    "mutation",
    [
        "dynamic_input",
        "dynamic_output",
        "wrong_dtype",
        "wrong_weight_dtype",
        "unknown_attribute",
        "wrong_opset",
        "extra_opset",
        "wrong_domain",
        "overridable",
        "external",
        "nested",
        "hidden_tensor",
        "function",
        "training",
        "extra_initializer",
        "duplicate_initializer",
        "nonfinite_weight",
        "conflicting_padding",
        "wrong_padding",
        "wrong_stride",
        "wrong_dilation",
        "wrong_group",
        "wrong_kernel",
        "duplicate_attribute",
        "extra_node",
        "extra_input",
        "wrong_output_shape",
    ],
)
def test_conv_refuses_unproven_boundaries(controls: Path, mutation: str) -> None:
    path = controls / "17a1.onnx"
    model = onnx.load(path)
    node = model.graph.node[0]
    if mutation.startswith("dynamic_"):
        value = (
            model.graph.input[0]
            if mutation == "dynamic_input"
            else model.graph.output[0]
        )
        value.type.tensor_type.shape.dim[-1].dim_param = "frames"
    elif mutation == "wrong_dtype":
        model.graph.input[0].type.tensor_type.elem_type = TensorProto.FLOAT
    elif mutation == "wrong_weight_dtype":
        model.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(np.ones((1, 1, 3), dtype=np.float32), "weight")
        )
    elif mutation == "unknown_attribute":
        node.attribute.append(helper.make_attribute("unproven", 1))
    elif mutation == "wrong_opset":
        model.opset_import[0].version = 18
    elif mutation == "extra_opset":
        model.opset_import.append(helper.make_opsetid("unused", 1))
    elif mutation == "wrong_domain":
        node.domain = "vendor"
        model.opset_import.append(helper.make_opsetid("vendor", 1))
    elif mutation == "overridable":
        model.graph.input.append(
            helper.make_tensor_value_info("weight", TensorProto.FLOAT16, [1, 1, 3])
        )
    elif mutation == "external":
        tensor = model.graph.initializer[0]
        tensor.data_location = TensorProto.EXTERNAL
        tensor.external_data.add(key="location", value="missing.bin")
    elif mutation == "nested":
        child = helper.make_graph(
            [],
            "hidden",
            [],
            [],
            initializer=[
                numpy_helper.from_array(np.array([1], dtype=np.uint8), "secret")
            ],
        )
        node.attribute.append(helper.make_attribute("hidden", child))
    elif mutation == "hidden_tensor":
        node.attribute.append(
            helper.make_attribute(
                "hidden",
                numpy_helper.from_array(np.array([1], dtype=np.uint8), "secret"),
            )
        )
    elif mutation == "function":
        model.functions.append(
            helper.make_function(
                "custom",
                "Hidden",
                ["x"],
                ["y"],
                [helper.make_node("Identity", ["x"], ["y"])],
                [helper.make_opsetid("", 13)],
            )
        )
    elif mutation == "training":
        model.training_info.add()
    elif mutation == "extra_initializer":
        model.graph.initializer.append(
            numpy_helper.from_array(np.array([1], dtype=np.uint8), "secret")
        )
    elif mutation == "duplicate_initializer":
        model.graph.initializer.append(model.graph.initializer[0])
    elif mutation == "nonfinite_weight":
        model.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(
                np.array([[[np.inf, 1, 1]]], dtype=np.float16), "weight"
            )
        )
    elif mutation == "conflicting_padding":
        node.attribute.append(helper.make_attribute("auto_pad", "SAME_UPPER"))
    elif mutation == "wrong_padding":
        del node.attribute[:]
        node.attribute.append(helper.make_attribute("pads", [0, 2]))
    elif mutation in {"wrong_stride", "wrong_dilation", "wrong_group", "wrong_kernel"}:
        names = {
            "wrong_stride": "strides",
            "wrong_dilation": "dilations",
            "wrong_group": "group",
            "wrong_kernel": "kernel_shape",
        }
        node.attribute.append(
            helper.make_attribute(
                names[mutation], 2 if mutation == "wrong_group" else [2]
            )
        )
    elif mutation == "duplicate_attribute":
        node.attribute.append(node.attribute[0])
    elif mutation == "extra_node":
        model.graph.node.append(helper.make_node("Identity", ["output"], ["unused"]))
    elif mutation == "extra_input":
        model.graph.input.append(
            helper.make_tensor_value_info("unused", TensorProto.FLOAT16, [1])
        )
    elif mutation == "wrong_output_shape":
        model.graph.output[0].type.tensor_type.shape.dim[-1].dim_value = 15
    rewrite(path, model)
    with pytest.raises(HybridError):
        import_native_model(path)


@pytest.mark.parametrize(
    "mutation",
    [
        "scale",
        "zero_point",
        "vector_scale",
        "vector_zero_point",
        "scale_dtype",
        "signed_input",
        "signed_weight",
        "dynamic_quantizer",
        "attribute",
        "shape",
    ],
)
def test_fc_refuses_unproven_quantizers(controls: Path, mutation: str) -> None:
    path = controls / "17b2.onnx"
    model = onnx.load(path)
    if mutation in {
        "scale",
        "vector_scale",
        "scale_dtype",
        "zero_point",
        "vector_zero_point",
    }:
        target = "a_scale" if "scale" in mutation else "a_zero_point"
        tensor = next(t for t in model.graph.initializer if t.name == target)
        dtype = (
            np.float16
            if mutation == "scale_dtype"
            else np.float32
            if "scale" in mutation
            else np.uint8
        )
        value = (
            2
            if mutation == "scale"
            else 1
            if mutation == "zero_point"
            else 1
            if "scale" in mutation
            else 0
        )
        values = np.array(
            [value] if mutation.startswith("vector") else value, dtype=dtype
        )
        tensor.CopyFrom(numpy_helper.from_array(values, target))
    elif mutation == "signed_input":
        model.graph.input[0].type.tensor_type.elem_type = TensorProto.INT8
    elif mutation == "signed_weight":
        tensor = next(t for t in model.graph.initializer if t.name == "weight")
        tensor.CopyFrom(
            numpy_helper.from_array(
                numpy_helper.to_array(tensor).astype(np.int8), "weight"
            )
        )
    elif mutation == "dynamic_quantizer":
        tensor = next(t for t in model.graph.initializer if t.name == "a_scale")
        model.graph.initializer.remove(tensor)
        model.graph.input.append(
            helper.make_tensor_value_info("a_scale", TensorProto.FLOAT, [])
        )
    elif mutation == "attribute":
        model.graph.node[0].attribute.append(helper.make_attribute("axis", 0))
    elif mutation == "shape":
        model.graph.input[0].type.tensor_type.shape.dim[-1].dim_value = 15
    rewrite(path, model)
    with pytest.raises(HybridError):
        import_native_model(path)


def test_fc_oracle_saturates_without_uint8_overflow(controls: Path) -> None:
    path = controls / "17b2.onnx"
    model = onnx.load(path)
    tensor = next(t for t in model.graph.initializer if t.name == "weight")
    tensor.CopyFrom(
        numpy_helper.from_array(np.full((16, 4), 255, dtype=np.uint8), "weight")
    )
    rewrite(path, model)
    record = import_native_model(path)
    inputs = {"input": np.full((1, 16), 255, dtype=np.uint8)}
    actual = independent_oracle(record, inputs)["output"]
    assert actual.tobytes() == bytes([255] * 4)
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    assert actual.tobytes() == session.run(None, inputs)[0].tobytes()


@pytest.mark.parametrize("mutation", ["names", "dtype", "shape", "nonfinite", "record"])
def test_oracle_rejects_invalid_input_or_mutated_identity(
    controls: Path, mutation: str
) -> None:
    record = import_native_model(controls / "17a1.onnx")
    inputs = {"input": np.ones((1, 1, 16), dtype=np.float16)}
    if mutation == "names":
        inputs["extra"] = inputs["input"]
    elif mutation == "dtype":
        inputs["input"] = inputs["input"].astype(np.float32)
    elif mutation == "shape":
        inputs["input"] = inputs["input"].reshape(16)
    elif mutation == "nonfinite":
        inputs["input"][0, 0, 0] = np.nan
    elif mutation == "record":
        record["weights"]["payload_hex"] = "00" * 6
    with pytest.raises(HybridError):
        independent_oracle(record, inputs)


def test_fixture_generation_is_deterministic_and_exclusive(
    controls: Path, tmp_path: Path
) -> None:
    other = tmp_path / "other"
    manifest = generate_native_fixtures(other)
    for filename in ["manifest.json", *manifest["files"]]:
        assert (controls / filename).read_bytes() == (other / filename).read_bytes()
    with pytest.raises(HybridError):
        generate_native_fixtures(controls)
    tracked = Path(__file__).parents[1] / "fixtures" / "native-poc17"
    if tracked.exists():
        assert json.loads((tracked / "manifest.json").read_text()) == manifest
        for filename in ["manifest.json", *manifest["files"]]:
            assert (tracked / filename).read_bytes() == (other / filename).read_bytes()
