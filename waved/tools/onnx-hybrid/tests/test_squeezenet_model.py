import hashlib

import numpy as np
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid import squeezenet_model as model


def tiny_model():
    weights = np.array([[[[1.125, -0.25], [0.5, 0.0625]]]], dtype=np.float32)
    graph = helper.make_graph(
        [helper.make_node("Conv", ["input", "weights"], ["output"])],
        "precision",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT, [1, 1, 3, 4])],
        [helper.make_tensor_value_info("output", TensorProto.FLOAT, [1, 1, 2, 3])],
        [numpy_helper.from_array(weights, "weights")],
    )
    return helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])


def test_conversion_preserves_every_half_parameter_bit_and_ir8():
    original = tiny_model()
    payload = original.SerializeToString()
    converted = model.fp16_model(payload, "20a")
    assert original.SerializeToString() == payload
    assert converted.ir_version == 8
    assert converted.graph.input[0].type.tensor_type.elem_type == TensorProto.FLOAT16
    assert converted.graph.output[0].type.tensor_type.elem_type == TensorProto.FLOAT16
    for old, new in zip(
        original.graph.initializer, converted.graph.initializer, strict=True
    ):
        expected = numpy_helper.to_array(old).astype("<f2")
        assert new.data_type == TensorProto.FLOAT16
        assert numpy_helper.to_array(new).tobytes() == expected.tobytes()
    assert (
        dict((p.key, p.value) for p in converted.metadata_props)["wave.native_profile"]
        == "fp16-squeezenet-v1"
    )


@pytest.mark.parametrize("fault", ["nan", "integer", "dropout", "opset"])
def test_conversion_refuses_nonfinite_or_changed_semantics(fault):
    original = tiny_model()
    if fault == "nan":
        original.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(
                np.full((1, 1, 2, 2), np.nan, dtype=np.float32), "weights"
            )
        )
    elif fault == "integer":
        original.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(np.ones((1, 1, 2, 2), dtype=np.int64), "weights")
        )
    elif fault == "dropout":
        original.graph.node[0].op_type = "Dropout"
    else:
        original.opset_import[0].version = 17
    with pytest.raises(model.SqueezeNetError):
        model.fp16_model(original.SerializeToString(), "20a")


def test_wrong_checkpoint_refuses_before_torch_import(tmp_path, monkeypatch):
    assets = tmp_path / "assets"
    assets.mkdir()
    (assets / model.CHECKPOINT_NAME).write_bytes(b"untrusted checkpoint")
    monkeypatch.setattr(
        model, "_torch", lambda: pytest.fail("checkpoint was deserialized")
    )
    with pytest.raises(model.SqueezeNetError, match="checkpoint"):
        model.prepare_squeezenet(assets, tmp_path / "prepared")
    assert not (tmp_path / "prepared").exists()


def test_existing_output_refuses_before_deserialization(tmp_path, monkeypatch):
    destination = tmp_path / "prepared"
    destination.mkdir()
    monkeypatch.setattr(model, "_torch", lambda: pytest.fail("model was loaded"))
    with pytest.raises(model.SqueezeNetError, match="output"):
        model.prepare_squeezenet(tmp_path / "absent", destination)


def test_classification_gate_is_relative_to_original_fp32():
    reference = np.zeros((1, 1000), dtype="<f4")
    reference[0, :5] = [10, 9, 8, 7, 6]
    observed = reference.astype("<f2")
    result = model.compare_classification(reference.tobytes(), observed.tobytes())
    assert result["status"] == "PASS"
    assert result["reference_top1"] == 0
    assert result["top5_overlap"] == 5
    assert result["reference_margin"] == 1
    assert result["observed_sha256"] == hashlib.sha256(observed.tobytes()).hexdigest()
    observed[0, :2] = [9, 10]
    with pytest.raises(model.SqueezeNetError):
        model.compare_classification(reference.tobytes(), observed.tobytes())


def test_classification_gate_refuses_nonfinite_or_wrong_length():
    reference = np.zeros(1000, dtype="<f4")
    observed = np.zeros(1000, dtype="<f2")
    observed[4] = np.nan
    with pytest.raises(model.SqueezeNetError):
        model.compare_classification(reference.tobytes(), observed.tobytes())
    with pytest.raises(model.SqueezeNetError):
        model.compare_classification(reference.tobytes(), observed[:-1].tobytes())
