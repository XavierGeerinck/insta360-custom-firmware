"""Explicit Pad profile, native stride policy and deterministic full rewrite."""

import copy
import hashlib
from pathlib import Path

import numpy as np
import onnx
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.marblenet_graph import (
    import_graph,
    independent_graph_oracle,
    validate_graph_record,
)
from wave_onnx_hybrid.marblenet_pad import (
    PROFILE_ID,
    add_profile_marker,
    rewrite_explicit_pad,
    validate_full_pad_model,
)


def source_model(*, stride2=False):
    shape = [1, 64, 400 if stride2 else 200]
    kernel, dilation, pads = (11, 1, [5, 5]) if stride2 else (29, 2, [28, 28])
    w = np.arange(64 * kernel, dtype=np.float32).reshape(64, 1, kernel) / 2048
    node = helper.make_node(
        "Conv",
        ["input", "w"],
        ["output"],
        kernel_shape=[kernel],
        dilations=[dilation],
        pads=pads,
        strides=[2 if stride2 else 1],
        group=64,
    )
    return helper.make_model(
        helper.make_graph(
            [node],
            "test",
            [helper.make_tensor_value_info("input", TensorProto.FLOAT16, shape)],
            [
                helper.make_tensor_value_info(
                    "output", TensorProto.FLOAT16, [1, 64, 200]
                )
            ],
            [numpy_helper.from_array(w.astype(np.float16), "w")],
        ),
        opset_imports=[helper.make_opsetid("", 13)],
        ir_version=8,
    )


def save(tmp_path, model):
    path = tmp_path / "pad.onnx"
    path.write_bytes(model.SerializeToString())
    return path


def sealed(record):
    record["lowering_sha256"] = hashlib.sha256(
        canonical_json({k: v for k, v in record.items() if k != "lowering_sha256"})
    ).hexdigest()
    return record


def test_bound_pad_contract_and_original_profile_refusal(tmp_path):
    model = rewrite_explicit_pad(source_model(), 0)
    path = save(tmp_path, model)
    with pytest.raises(HybridError):
        import_graph(path)
    record = import_graph(path, profile_id=PROFILE_ID)
    validate_graph_record(record)
    pad = record["nodes"][0]
    assert pad["pads"] == [28, 28] and pad["value"] == 0.0
    assert pad["input_shape"] == [1, 64, 200] and pad["output_shape"] == [1, 64, 256]
    assert record["constants"][pad["pads_constant"]]["dtype"] == "INT64"
    assert record["constants"][pad["value_constant"]]["payload_hex"] == "0000"
    assert record["nodes"][1]["native_strides"] == [1, 1]


@pytest.mark.parametrize(
    "mutation",
    [
        "pads",
        "negative-zero",
        "mode",
        "scalar-shape",
        "pads-dtype",
        "external",
        "extra-input",
        "width",
        "consumer",
        "marker",
    ],
)
def test_malformed_source_pad_refuses(tmp_path, mutation):
    model = rewrite_explicit_pad(source_model(), 0)
    tensors = {t.name: t for t in model.graph.initializer}
    pad = model.graph.node[0]
    if mutation == "pads":
        tensors[pad.input[1]].CopyFrom(
            numpy_helper.from_array(
                np.array([0, 0, 27, 0, 0, 29], np.int64), pad.input[1]
            )
        )
    elif mutation == "negative-zero":
        tensors[pad.input[2]].CopyFrom(
            numpy_helper.from_array(np.array(-0.0, np.float16), pad.input[2])
        )
    elif mutation == "mode":
        pad.attribute[0].s = b"edge"
    elif mutation == "scalar-shape":
        tensors[pad.input[2]].CopyFrom(
            numpy_helper.from_array(np.array([0], np.float16), pad.input[2])
        )
    elif mutation == "pads-dtype":
        tensors[pad.input[1]].CopyFrom(
            numpy_helper.from_array(
                np.array([0, 0, 28, 0, 0, 28], np.int32), pad.input[1]
            )
        )
    elif mutation == "external":
        tensors[pad.input[1]].data_location = TensorProto.EXTERNAL
        tensors[pad.input[1]].external_data.add(key="location", value="outside")
    elif mutation == "extra-input":
        pad.input.append("input")
    elif mutation == "width":
        model.graph.input[0].type.tensor_type.shape.dim[2].dim_value = 256
    elif mutation == "consumer":
        next(a for a in model.graph.node[1].attribute if a.name == "pads").ints[:] = [
            1,
            1,
        ]
    else:
        model.metadata_props[-1].value = "another-profile"
    with pytest.raises(HybridError):
        import_graph(save(tmp_path, model), profile_id=PROFILE_ID)


def test_optional_zero_and_stride2_geometry_are_explicit(tmp_path):
    model = rewrite_explicit_pad(source_model(), 0)
    zero = model.graph.node[0].input.pop()
    del model.graph.initializer[-1]
    assert zero.endswith("zero")
    record = import_graph(save(tmp_path, model), profile_id=PROFILE_ID)
    assert record["nodes"][0]["value_constant"] is None
    plain = add_profile_marker(source_model(stride2=True))
    record = import_graph(save(tmp_path, plain), profile_id=PROFILE_ID)
    assert record["nodes"][0]["native_strides"] == [2, 2]
    record["nodes"][0]["native_strides"] = [2, 1]
    with pytest.raises(HybridError):
        validate_graph_record(sealed(record))


@pytest.mark.parametrize(
    "field",
    [
        "negative-zero",
        "boolean-pad",
        "width",
        "structural-bytes",
        "policy",
        "native-strides",
        "unused-pad-constant",
    ],
)
def test_resealed_pad_record_refuses(tmp_path, field):
    record = import_graph(
        save(tmp_path, rewrite_explicit_pad(source_model(), 0)), profile_id=PROFILE_ID
    )
    pad = record["nodes"][0]
    if field == "negative-zero":
        pad["value"] = -0.0
    elif field == "boolean-pad":
        pad["pads"] = [True, True]
    elif field == "width":
        pad["output_shape"] = [1, 64, 400]
    elif field == "structural-bytes":
        c = record["constants"][pad["value_constant"]]
        c["payload_hex"] = "0080"
        c["sha256"] = hashlib.sha256(bytes.fromhex(c["payload_hex"])).hexdigest()
    elif field == "policy":
        record["native_policy"]["id"] = "asymmetric-stride"
    elif field == "native-strides":
        record["nodes"][1]["native_strides"] = [1, 2]
    else:
        c = copy.deepcopy(record["constants"][pad["value_constant"]])
        c["name"] = "unused"
        record["constants"]["unused"] = c
    with pytest.raises(HybridError):
        validate_graph_record(sealed(record))


@pytest.mark.parametrize("position", [0, 1, 27, 28, 171, 172, 198, 199])
def test_padding_preserves_independent_boundary_terms(tmp_path, position):
    original = source_model()
    old = import_graph(save(tmp_path, original))
    new = import_graph(
        save(tmp_path, rewrite_explicit_pad(original, 0)), profile_id=PROFILE_ID
    )
    x = np.zeros((1, 64, 200), np.float16)
    x[..., position] = 1
    expected = independent_graph_oracle(old, {"input": x})["output"]
    observed = independent_graph_oracle(new, {"input": x})["output"]
    assert expected.tobytes() == observed.tobytes()


def test_original_record_remains_exact_after_explicit_profile_import(tmp_path):
    source = source_model(stride2=True)
    old = import_graph(save(tmp_path, source))
    original_bytes = canonical_json(old)
    import_graph(save(tmp_path, add_profile_marker(source)), profile_id=PROFILE_ID)
    assert canonical_json(import_graph(save(tmp_path, source))) == original_bytes


@pytest.fixture
def frozen_full():
    path = Path(__file__).resolve().parents[4] / "artifacts/npu/poc18/prepared/18d.onnx"
    if not path.exists():
        pytest.skip("ignored frozen checkpoint preparation unavailable")
    return onnx.load(path)


def test_full_model_inverse_binds_original_checkpoint_and_all_taps(frozen_full):
    model = rewrite_explicit_pad(frozen_full, 27)
    result = validate_full_pad_model(model.SerializeToString())
    assert (
        result["profile_id"] == PROFILE_ID
        and result["recipe_id"] == "explicit-pad28-k29-d2-valid-v1"
    )
    assert (
        result["source_original_model_sha256"]
        == hashlib.sha256(frozen_full.SerializeToString()).hexdigest()
    )


@pytest.mark.parametrize("change", ["tap", "pad", "marker", "metadata", "renamed-pad"])
def test_full_model_inverse_refuses_resealed_arbitrary_graph(frozen_full, change):
    model = rewrite_explicit_pad(frozen_full, 27)
    if change == "tap":
        t = model.graph.initializer[0]
        w = numpy_helper.to_array(t).copy()
        w.flat[0] += np.float16(0.01)
        t.CopyFrom(numpy_helper.from_array(w, t.name))
    elif change == "pad":
        model.graph.initializer[-2].raw_data = np.array(
            [0, 0, 27, 0, 0, 29], np.int64
        ).tobytes()
    elif change == "marker":
        model.metadata_props.add(key="wave.native_profile", value=PROFILE_ID)
    elif change == "metadata":
        model.producer_name = "changed"
    else:
        model.graph.node[27].output[0] = "arbitrary"
        model.graph.node[28].input[0] = "arbitrary"
    with pytest.raises(HybridError):
        validate_full_pad_model(model.SerializeToString())


@pytest.mark.parametrize(
    "target,field,values",
    [
        (0, "input_shape", [True, 64, 200]),
        (0, "output_shape", [1.0, 64, 256]),
        (1, "input_shape", [True, 64, 256]),
        (1, "input_shape", [1, 64, 256.0]),
        (1, "pads", [False, False]),
        (1, "pads", [0.0, 0.0]),
    ],
)
def test_erased_projection_geometry_requires_exact_integers(
    tmp_path, target, field, values
):
    record = import_graph(
        save(tmp_path, rewrite_explicit_pad(source_model(), 0)), profile_id=PROFILE_ID
    )
    record["nodes"][target][field] = values
    with pytest.raises(HybridError):
        validate_graph_record(sealed(record))


def test_full_inverse_validation_creates_no_cpu_session(frozen_full, monkeypatch):
    import onnxruntime

    monkeypatch.setattr(
        onnxruntime,
        "InferenceSession",
        lambda *a, **kw: pytest.fail(
            "CPU neural session created by inverse validation"
        ),
    )
    validate_full_pad_model(rewrite_explicit_pad(frozen_full, 27).SerializeToString())


def test_actual_pad_preparation_binds_all_trained_bits_and_four_clips(
    tmp_path, monkeypatch, frozen_full
):
    from wave_onnx_hybrid import marblenet_compat

    prepared = Path(__file__).resolve().parents[4] / "artifacts/npu/poc18/prepared"
    root = tmp_path / "poc19"
    monkeypatch.setattr(marblenet_compat, "OUTPUT_ROOT", root)
    output = root / "pad-prepared"
    receipt = marblenet_compat.prepare_pad_candidate(prepared, output)
    assert set(receipt["controls"]) == {"19c", "19d", "19e", "19f"}
    assert [
        receipt["controls"][name]["nodes"] for name in ("19c", "19d", "19e", "19f")
    ] == [4, 3, 16, 34]
    assert receipt["qualification"] == "UNQUALIFIED"
    assert receipt["full_identity"] == validate_full_pad_model(
        (output / "19f.onnx").read_bytes()
    )
    assert receipt["controls"]["19f"]["learned_initializers"][
        "initializer_count"
    ] == len(frozen_full.graph.initializer)
    for identity in receipt["controls"].values():
        assert identity["learned_initializers"]["bits_identical"]
        assert identity["cpu_vs_independent"]["status"] == "PASS"
    for name, clip in receipt["clips"].items():
        assert clip["cpu_fp16_vs_original_fp16"]["max_abs_error"] == 0
        assert clip["cpu_fp16_probabilities"]["decision_agreement"] == 1
        assert clip["independent_fp16_probabilities"]["status"] == "PASS"
        assert (output / ("19f_" + name + "_input.npz")).read_bytes() == (
            prepared / ("18d_" + name + "_input.npz")
        ).read_bytes()
    for name, identity in receipt["files"].items():
        raw = (output / name).read_bytes()
        assert (
            len(raw) == identity["bytes"]
            and hashlib.sha256(raw).hexdigest() == identity["sha256"]
        )


def test_bad_pad_output_alias_refuses_before_reference_construction(
    tmp_path, monkeypatch
):
    from wave_onnx_hybrid import marblenet_compat

    source = tmp_path / "source"
    source.mkdir()
    monkeypatch.setattr(
        marblenet_compat,
        "_frozen",
        lambda *a: pytest.fail("source read before output admission"),
    )
    monkeypatch.setattr(marblenet_compat, "OUTPUT_ROOT", tmp_path)
    for path in (source, source / "child", source.parent):
        with pytest.raises(HybridError):
            marblenet_compat.prepare_pad_candidate(source, path)


@pytest.mark.parametrize("field,value", [("byte_size", 48.0), ("shape", [6.0])])
def test_structural_numeric_types_cannot_be_resealed(tmp_path, field, value):
    record = import_graph(
        save(tmp_path, rewrite_explicit_pad(source_model(), 0)), profile_id=PROFILE_ID
    )
    constant = record["constants"][record["nodes"][0]["pads_constant"]]
    constant[field] = value
    with pytest.raises(HybridError):
        validate_graph_record(sealed(record))
