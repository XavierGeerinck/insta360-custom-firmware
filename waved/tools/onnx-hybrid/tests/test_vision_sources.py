"""Exact spatial source contracts retain call budgets and numeric IDs."""

import copy
import hashlib

import pytest
from test_vision_graph import lower, model, seal

from wave_onnx_hybrid import vision_sources
from wave_onnx_hybrid.errors import HybridError
from wave_onnx_hybrid.native_import import import_native_model
from wave_onnx_hybrid.native_sources import (
    generate_compile_source,
    generate_execute_source,
)

COMPILE = "/tmp/wave-npu-onnx20-0123456789abcdef-compile"
EXECUTE = "/tmp/wave-npu-onnx20-0123456789abcdef-execute"


def test_non_square_conv_source_bits_spatial_layout_and_lifecycle(tmp_path):
    r = lower(tmp_path)
    s = generate_compile_source(r, COMPILE)
    vision_sources.validate_compile_source(r, COMPILE, s)
    assert "tensor_shape0{13, 17, 3, 1}" in s
    assert "weight_shape0{3, 3, 3, 64}" in s
    assert "pad0{0, 0, 0, 0}" in s
    assert "stride0{2, 2}" in s and "dilation0{1, 1}" in s
    assert "0xa800U, 0xa700U" in s
    assert "kBiasBits0{0x00000000U, 0x3b000000U" in s
    assert "pad0, stride0, dilation0, 0" in s
    assert s.count("graph->CompileToBinary(") == 2
    assert "graph->Run(" not in s and "graph->Compile()" not in s
    assert "kWatchdogSeconds = 15" in s and "alarm(0)" not in s
    assert "PR_SET_PDEATHSIG, SIGKILL" in s
    assert (
        s.index("operations.clear();")
        < s.index("tensors.clear();")
        < s.index("graph.reset();")
        < s.index("context.reset();")
        < s.index("RESULT status=PASS")
    )
    assert "firstW" not in s and "firstR" not in s


@pytest.mark.parametrize("control", ["20b", "20c", "20d"])
def test_pool_concat_gap_and_explicit_flatten_source(tmp_path, control):
    r = lower(tmp_path, model(control))
    s = generate_compile_source(r, COMPILE)
    if control == "20b":
        assert "tim::vx::PoolType::MAX" in s
        assert "tim::vx::RoundType::CEILING" in s
        assert "pool_kernel0{3, 3}" in s and "pool_stride0{2, 2}" in s
        assert "tensor_shape0{19, 15, 64, 1}" in s
    elif control == "20c":
        assert "tim::vx::ops::Concat>(2, 2)" in s
        assert (
            "operations[6]->BindInput(tensors[4])\n        .BindInput(tensors[6])" in s
        )
        assert "pad4{1, 1, 1, 1}" in s
    else:
        assert "tim::vx::PoolType::AVG" in s
        assert "pool_kernel2{13, 13}" in s
        assert "pool_stride2{13, 13}" in s
        assert "tim::vx::ops::Reshape>" in s and "reshape_target3{1000, 1}" in s


def test_executor_nan_canary_after_compile_actual_output_and_limits(tmp_path):
    r = lower(tmp_path)
    s = generate_execute_source(r, EXECUTE, 1800)
    vision_sources.validate_execute_source(r, EXECUTE, 1800, s)
    assert "kInputBytes = 1326" in s and "kOutputBytes = 6144" in s
    assert "kCanaryHalfBits = 0x7e00U" in s
    assert "kCanaryByte" not in s and "kGuardByte = 0x5aU" in s
    assert (
        s.index("graph->Compile()")
        < s.index("device_canary[index]")
        < s.index("graph->Run()")
    )
    assert s.count("graph->Compile()") == s.count("graph->Run()") == 1
    assert "kWatchdogSeconds = 8" in s and "alarm(0)" not in s
    assert "fp16-squeezenet" in s and "expected_output" not in s
    assert "kOutputPath" in s and "O_NOFOLLOW" in s and "fchmod" in s
    for source in (
        s + "\n",
        s.replace("kWatchdogSeconds = 8", "kWatchdogSeconds = 9"),
        s.replace("0x7e00U", "0x0000U"),
    ):
        with pytest.raises(ValueError, match="policy"):
            vision_sources.validate_execute_source(r, EXECUTE, 1800, source)
    for size in (True, 512, 64 * 1024 * 1024 + 1):
        with pytest.raises(ValueError):
            generate_execute_source(r, EXECUTE, size)


def test_exact_source_template_record_and_root_refusals(tmp_path, monkeypatch):
    r = lower(tmp_path)
    s = generate_compile_source(r, COMPILE)
    for source in (
        s + "\n",
        s.replace("stride0{2, 2}", "stride0{2, 1}"),
        s.replace("CompileToBinary", "Run"),
    ):
        with pytest.raises(ValueError, match="policy"):
            vision_sources.validate_compile_source(r, COMPILE, source)
    bad = copy.deepcopy(r)
    bad["nodes"][0]["group"] = 3
    with pytest.raises(ValueError):
        generate_compile_source(seal(bad), COMPILE)
    for root in (
        COMPILE + "/..",
        COMPILE.replace("20", "18"),
        COMPILE.replace("compile", "execute"),
    ):
        with pytest.raises(ValueError):
            generate_compile_source(r, root)
    fake = tmp_path / "template"
    fake.mkdir()
    (fake / "compile_fp16_graph.cc.in").write_text("changed")
    monkeypatch.setattr(vision_sources, "_TEMPLATE_ROOT", fake)
    with pytest.raises(ValueError, match="template hash"):
        generate_compile_source(r, COMPILE)


def test_explicit_import_dispatch_does_not_accept_unmarked_vision(tmp_path):
    m = model()
    path = tmp_path / "marked.onnx"
    path.write_bytes(m.SerializeToString())
    assert import_native_model(path)["profile_id"] == "fp16-squeezenet-v1"
    m.ClearField("metadata_props")
    path.write_bytes(m.SerializeToString())
    with pytest.raises(HybridError):
        import_native_model(path)
    assert hashlib.sha256(path.read_bytes()).hexdigest() != "0" * 64


@pytest.mark.parametrize(
    "bound",
    ["_MAX_SOURCE_BYTES", "_MAX_EMITTED_CONSTANT_BYTES", "_MAX_STACK_PAYLOAD_BYTES"],
)
def test_independent_source_emission_and_stack_bounds(tmp_path, monkeypatch, bound):
    record = lower(tmp_path)
    monkeypatch.setattr(vision_sources, bound, 1)
    with pytest.raises(ValueError, match="bound"):
        generate_compile_source(record, COMPILE)


def test_all_actual_half_bits_and_exact_promoted_bias_bits_are_emitted(tmp_path):
    import re
    import struct

    from onnx import numpy_helper

    m = model()
    weights = numpy_helper.to_array(m.graph.initializer[0]).copy()
    weights.view("<u2").flat[:3] = (0x8000, 0x0001, 0x7BFF)
    m.graph.initializer[0].CopyFrom(numpy_helper.from_array(weights, "firstW"))
    bias = numpy_helper.to_array(m.graph.initializer[1]).copy()
    bias.view("<u2").flat[:3] = (0x8000, 0x0001, 0x3C00)
    m.graph.initializer[1].CopyFrom(numpy_helper.from_array(bias, "firstB"))
    r = lower(tmp_path, m)
    source = generate_compile_source(r, COMPILE)
    bits = re.search(r"kWeights0\{([^}]+)\}", source)[1]
    actual = [int(token.strip()[:-1], 16) for token in bits.split(",")]
    assert struct.pack(f"<{len(actual)}H", *actual) == weights.tobytes()
    promoted = re.search(r"kBiasBits0\{([^}]+)\}", source)[1]
    actual_bias = [int(token.strip()[:-1], 16) for token in promoted.split(",")]
    assert (
        struct.pack(f"<{len(actual_bias)}I", *actual_bias)
        == bias.astype("<f4").tobytes()
    )
