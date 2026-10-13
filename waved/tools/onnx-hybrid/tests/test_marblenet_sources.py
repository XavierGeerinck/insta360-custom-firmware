"""MarbleNet uses actual bit patterns and the bounded native lifecycle."""

import copy
import hashlib
import math
import struct

import pytest

from wave_onnx_hybrid import marblenet_sources
from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.native_sources import (
    generate_compile_source,
    generate_execute_source,
    validate_compile_source,
    validate_execute_source,
)

COMPILE_ROOT = "/tmp/wave-npu-onnx18-0123456789abcdef-compile"
EXECUTE_ROOT = "/tmp/wave-npu-onnx18-0123456789abcdef-execute"


def seal(record):
    record.pop("lowering_sha256", None)
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    return record


def tensor(name, shape, payload=None):
    result = {
        "name": name,
        "dtype": "FLOAT16",
        "shape": shape,
        "byte_size": 2 * math.prod(shape),
    }
    if payload is not None:
        result.update(
            payload_hex=payload.hex(), sha256=hashlib.sha256(payload).hexdigest()
        )
    return result


def graph_record(*, dilation=1, bias=True):
    incoming = 'hostile\\"; graph->Run(); //'
    input_time = 400 if dilation == 1 else 200
    stride = 2 if dilation == 1 else 1
    dw_bits = tuple(0x3000 + index for index in range(22))
    constants = {
        "depthwise": tensor("depthwise", [2, 1, 11], struct.pack("<22H", *dw_bits)),
        "dense": tensor(
            "dense", [2, 2, 1], struct.pack("<4H", 0x3400, 0x3800, 0xB400, 0x3C00)
        ),
    }
    if bias:
        constants["bias"] = tensor("bias", [2], struct.pack("<2H", 0x3000, 0xB000))
    nodes = [
        {
            "type": "Conv",
            "inputs": [incoming],
            "output": "depth",
            "input_shape": [1, 2, input_time],
            "output_shape": [1, 2, 200],
            "weights": "depthwise",
            "bias": None,
            "kernel": 11,
            "stride": stride,
            "dilation": dilation,
            "pads": [5 * dilation, 5 * dilation],
            "group": 2,
        },
        {
            "type": "Conv",
            "inputs": ["depth"],
            "output": "dense-result",
            "input_shape": [1, 2, 200],
            "output_shape": [1, 2, 200],
            "weights": "dense",
            "bias": "bias" if bias else None,
            "kernel": 1,
            "stride": 1,
            "dilation": 1,
            "pads": [0, 0],
            "group": 1,
        },
        {
            "type": "Relu",
            "inputs": ["dense-result"],
            "output": "activated",
            "input_shape": [1, 2, 200],
            "output_shape": [1, 2, 200],
        },
        {
            "type": "Add",
            "inputs": ["activated", "depth"],
            "output": "output",
            "input_shapes": [[1, 2, 200], [1, 2, 200]],
            "output_shape": [1, 2, 200],
        },
    ]
    return seal(
        {
            "schema_version": 1,
            "profile_id": "fp16-marblenet-v1",
            "source_model_sha256": "1" * 64,
            "graph_signature": "2" * 64,
            "onnx_input": tensor(incoming, [1, 2, input_time]),
            "onnx_output": tensor("output", [1, 2, 200]),
            "native_input": tensor("input", [input_time, 1, 2, 1]),
            "native_output": tensor("output", [200, 1, 2, 1]),
            "constants": constants,
            "nodes": nodes,
            "operation": {"type": "ConvGraph", "node_count": len(nodes)},
            "numerical_policy": {"atol": 0.02, "rtol": 0.02},
        }
    )


def test_compile_source_keeps_call_budget_alarm_and_numeric_identifiers():
    record = graph_record()
    source = generate_compile_source(record, COMPILE_ROOT)
    validate_compile_source(record, COMPILE_ROOT, source)
    assert source.count("graph->CompileToBinary(") == 2
    assert "graph->Compile()" not in source and "graph->Run(" not in source
    assert "kWatchdogSeconds = 15" in source
    assert "alarm(0)" not in source
    assert "PR_SET_PDEATHSIG, SIGKILL" in source
    assert "hostile" not in source and "dense-result" not in source
    assert "tim::vx::ops::Conv2d" in source
    assert "tim::vx::ops::Add" in source and "tim::vx::ops::Relu" in source
    assert source.index("operations.clear();") < source.index("tensors.clear();")
    assert source.index("tensors.clear();") < source.index("graph.reset();")
    assert source.index("graph.reset();") < source.index("context.reset();")
    assert source.index("context.reset();") < source.index("RESULT status=PASS")


@pytest.mark.parametrize("dilation", [1, 2])
def test_dense_depthwise_layout_group_and_rate_are_exact(dilation):
    source = generate_compile_source(graph_record(dilation=dilation), COMPILE_ROOT)
    assert "kWeights0{0x3000U, 0x3001U" in source
    assert "kWeights1{0x3400U, 0x3800U, 0xb400U, 0x3c00U}" in source
    assert "kBiasBits1{0x3e000000U, 0xbe000000U}" in source
    assert "weight_shape0{11, 1, 2, 1}" in source
    assert "weight_shape1{1, 1, 2, 2}" in source
    assert f"pad0{{{5 * dilation}, {5 * dilation}, 0, 0}}" in source
    assert f"stride0{{{2 if dilation == 1 else 1}, 1}}" in source
    assert f"dilation0{{{dilation}, 1}}" in source
    assert "pad0, stride0, dilation0, 1" in source
    assert "pad1, stride1, dilation1, 0" in source


def test_missing_bias_is_fp32_zero_and_never_invents_a_constant():
    source = generate_compile_source(graph_record(bias=False), COMPILE_ROOT)
    assert "kBiasBits0{0x00000000U, 0x00000000U}" in source
    assert "kBiasBits1{0x00000000U, 0x00000000U}" in source
    assert source.count("tim::vx::DataType::FLOAT32") == 2


def test_residual_edges_and_no_activation_between_depthwise_and_pointwise():
    source = generate_compile_source(graph_record(), COMPILE_ROOT)
    assert (
        "operations[0]->BindInput(tensors[0])\n"
        "        .BindInput(tensors[5])\n"
        "        .BindInput(tensors[6])\n"
        "        .BindOutput(tensors[1]);"
    ) in source
    assert (
        "operations[1]->BindInput(tensors[1])\n"
        "        .BindInput(tensors[7])\n"
        "        .BindInput(tensors[8])\n"
        "        .BindOutput(tensors[2]);"
    ) in source
    assert (
        "operations[3]->BindInput(tensors[3])\n"
        "        .BindInput(tensors[1])\n"
        "        .BindOutput(tensors[4]);"
    ) in source
    assert source.count("CreateOperation<tim::vx::ops::Relu>") == 1
    assert source.index("operations[1]->BindInput") < source.index(
        "CreateOperation<tim::vx::ops::Relu>"
    )


def test_bias_signed_zero_bits_survive_fp16_to_fp32_conversion():
    record = graph_record()
    payload = struct.pack("<2H", 0x8000, 0x0000)
    record["constants"]["bias"] = tensor("bias", [2], payload)
    source = generate_compile_source(seal(record), COMPILE_ROOT)
    assert "kBiasBits1{0x80000000U, 0x00000000U}" in source


@pytest.mark.parametrize(
    "change",
    [
        lambda r: r["nodes"][0].update(group=3),
        lambda r: r["nodes"][0].update(dilation=True),
        lambda r: r["nodes"][2].update(type="Relu>(); graph->Run(); //"),
        lambda r: r["nodes"][3].update(inputs=["activated", "missing"]),
    ],
)
def test_layout_and_wiring_mutations_are_refused_before_cpp(change):
    record = graph_record()
    change(record)
    with pytest.raises(ValueError):
        generate_compile_source(seal(record), COMPILE_ROOT)


def test_resealed_nonfinite_constants_are_refused():
    record = graph_record()
    record["constants"]["bias"] = tensor(
        "bias", [2], struct.pack("<2H", 0x7C00, 0x0000)
    )
    with pytest.raises(ValueError):
        generate_compile_source(seal(record), COMPILE_ROOT)


def test_executor_reuses_pinned_input_file_lifecycle_and_actual_readback():
    record = graph_record()
    source = generate_execute_source(record, EXECUTE_ROOT, 1800)
    validate_execute_source(record, EXECUTE_ROOT, 1800, source)
    assert source.count("graph->Compile()") == source.count("graph->Run()") == 1
    assert "operation=fp16-marblenet" in source
    assert "kInputBytes = 1600" in source and "kOutputBytes = 800" in source
    assert "input_shape{400, 1, 2, 1}" in source
    assert "output_shape{200, 1, 2, 1}" in source
    assert "kWatchdogSeconds = 8" in source and "alarm(0)" not in source
    assert "kGuardBytes = 16" in source
    assert source.index("graph->Compile()") < source.index(
        "device_canary[index] = static_cast<uint8_t>(kCanaryHalfBits & 0xffU)"
    )
    assert source.index("device_canary[index]") < source.index("graph->Run()")
    assert source.index(
        "WriteAtomicExclusive(kOutputPath, kPartialPath, observed)"
    ) < source.index("context.reset();")
    assert source.index("context.reset();") < source.index("EXECUTION status=PASS")
    assert "memcmp(observed" not in source and "kWeights" not in source
    assert "hostile" not in source and "kExpectedOutput" not in source


@pytest.mark.parametrize(
    "root",
    [
        "/tmp/wave-npu-onnx17-0123456789abcdef-compile",
        EXECUTE_ROOT,
        COMPILE_ROOT + "/",
        COMPILE_ROOT + "/../compile",
        COMPILE_ROOT.upper(),
        '/tmp/wave-npu-onnx18-0123456789abcdef-compile";system("id");',
    ],
)
def test_compile_refuses_wrong_or_unsafe_root(root):
    with pytest.raises(ValueError, match="root"):
        generate_compile_source(graph_record(), root)


@pytest.mark.parametrize("root", [COMPILE_ROOT, EXECUTE_ROOT + "/", "/tmp/other"])
def test_execute_refuses_wrong_or_unsafe_root(root):
    with pytest.raises(ValueError, match="root"):
        generate_execute_source(graph_record(), root, 1800)


@pytest.mark.parametrize("size", [0, 512, 64 * 1024 * 1024 + 1, True, "1800"])
def test_invalid_nbg_bound_is_refused(size):
    with pytest.raises(ValueError, match="NBG"):
        generate_execute_source(graph_record(), EXECUTE_ROOT, size)


@pytest.mark.parametrize(
    "field,value",
    [
        ("profile_id", "unknown"),
        ("schema_version", True),
        ("numerical_policy", {"atol": 1.0, "rtol": 0.02}),
        ("native_input", tensor("input", [200, 1, 2, 1])),
    ],
)
def test_resealed_invalid_graph_contract_is_refused(field, value):
    record = graph_record()
    record[field] = value
    with pytest.raises(ValueError):
        marblenet_sources.generate_compile_source(seal(record), COMPILE_ROOT)


def test_real_coefficients_change_source_and_stale_hash_refuses():
    record = graph_record()
    original = generate_compile_source(record, COMPILE_ROOT)
    changed = copy.deepcopy(record)
    changed["constants"]["dense"]["payload_hex"] = "003800380034003c"
    with pytest.raises(ValueError):
        generate_compile_source(changed, COMPILE_ROOT)
    payload = bytes.fromhex(changed["constants"]["dense"]["payload_hex"])
    changed["constants"]["dense"]["sha256"] = hashlib.sha256(payload).hexdigest()
    seal(changed)
    assert generate_compile_source(changed, COMPILE_ROOT) != original
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(changed, COMPILE_ROOT, original)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("0x3400U", "0x3000U"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
        lambda s: s.replace(
            "size_t nbg_size = 0;", "graph->Run(); size_t nbg_size = 0;"
        ),
        lambda s: s.replace("0x1000000eU", "0x10000000U"),
        lambda s: s.replace("dilation0{1, 1}", "dilation0{0, 0}"),
    ],
)
def test_compiler_policy_mutations_are_refused(mutation):
    record = graph_record()
    source = generate_compile_source(record, COMPILE_ROOT)
    changed = mutation(source)
    assert changed != source
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(record, COMPILE_ROOT, changed)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("if (!graph->Run())", "graph->Run(); if (!graph->Run())"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
        lambda s: s.replace("kCanaryHalfBits = 0x7e00U", "kCanaryHalfBits = 0xa5a5U"),
        lambda s: s.replace(
            "guarded_output.fill(kGuardByte)", "guarded_output.fill(0)"
        ),
        lambda s: s.replace("O_EXCL | O_CLOEXEC | O_NOFOLLOW", "O_CLOEXEC"),
    ],
)
def test_executor_policy_mutations_are_refused(mutation):
    record = graph_record()
    source = generate_execute_source(record, EXECUTE_ROOT, 1800)
    changed = mutation(source)
    assert changed != source
    with pytest.raises(ValueError, match="source"):
        validate_execute_source(record, EXECUTE_ROOT, 1800, changed)


def test_changed_graph_template_is_refused(tmp_path, monkeypatch):
    template = marblenet_sources._TEMPLATE_ROOT / "compile_fp16_graph.cc.in"
    (tmp_path / template.name).write_text(
        template.read_text().replace("context.reset();", "alarm(0); context.reset();")
    )
    monkeypatch.setattr(marblenet_sources, "_TEMPLATE_ROOT", tmp_path)
    with pytest.raises(ValueError, match="template hash"):
        generate_compile_source(graph_record(), COMPILE_ROOT)


def test_changed_reused_executor_template_is_refused(tmp_path, monkeypatch):
    from wave_onnx_hybrid import native_sources

    template = native_sources._TEMPLATE_ROOT / "execute.cc.in"
    (tmp_path / template.name).write_text(
        template.read_text().replace("if (!graph->Run())", "if (true)")
    )
    monkeypatch.setattr(native_sources, "_TEMPLATE_ROOT", tmp_path)
    with pytest.raises(ValueError, match="template hash"):
        generate_execute_source(graph_record(), EXECUTE_ROOT, 1800)


def test_nan_prime_on_device_and_host_refuses_partial_unwritten_half():
    import numpy as np

    from wave_onnx_hybrid.errors import HybridError
    from wave_onnx_hybrid.marblenet_graph import validate_graph_output

    record = graph_record()
    source = generate_execute_source(record, EXECUTE_ROOT, 1800)
    assert "constexpr uint16_t kCanaryHalfBits = 0x7e00U" in source
    assert (
        "device_canary[index + 1] = static_cast<uint8_t>(kCanaryHalfBits >> 8U)"
        in source
    )
    assert "guarded_output[kGuardBytes + index + 1] =" in source
    assert source.count("static_cast<uint8_t>(kCanaryHalfBits >> 8U)") == 2
    assert "device_canary.fill(kCanaryByte)" not in source
    assert "std::memset(guarded_output.data() + kGuardBytes, kCanaryByte" not in source
    expected = np.full((1, 2, 200), -0.022, dtype="<f2").tobytes()
    partial_old_prime = b"\xa5\xa5" + expected[2:]
    assert validate_graph_output(record, expected, partial_old_prime)["passed"]
    with pytest.raises(HybridError):
        validate_graph_output(record, expected, b"\x00\x7e" + expected[2:])


def pad_profile_record(*, include_pad=True):
    """Exact Pad control, or the separately admitted flat stride2 control."""
    if not include_pad:
        record = graph_record()
        record["profile_id"] = "fp16-marblenet-pad-v1"
        record["native_policy"] = {"id": "flat-height-depthwise-symmetric-v1"}
        for node in record["nodes"]:
            if node["type"] == "Conv":
                node["native_strides"] = [
                    node["stride"],
                    2 if node["group"] != 1 and node["stride"] == 2 else 1,
                ]
        return seal(record)
    incoming = 'input\\"; graph->Run(); //'
    pads_payload = struct.pack("<6q", 0, 0, 28, 0, 0, 28)
    constants = {
        "pad-widths": {
            "name": "pad-widths",
            "dtype": "INT64",
            "shape": [6],
            "byte_size": 48,
            "payload_hex": pads_payload.hex(),
            "sha256": hashlib.sha256(pads_payload).hexdigest(),
        },
        "pad-value": tensor("pad-value", [], struct.pack("<H", 0)),
        "depthwise": tensor(
            "depthwise", [64, 1, 29], struct.pack("<1856H", *([0x3400] * 1856))
        ),
        "pointwise": tensor(
            "pointwise", [128, 64, 1], struct.pack("<8192H", *([0x3000] * 8192))
        ),
        "bias": tensor("bias", [128], struct.pack("<128H", *([0x3000] * 128))),
    }
    nodes = [
        {
            "type": "Pad",
            "inputs": [incoming],
            "output": "padded",
            "input_shape": [1, 64, 200],
            "output_shape": [1, 64, 256],
            "pads": [28, 28],
            "value": 0.0,
            "pads_constant": "pad-widths",
            "value_constant": "pad-value",
        },
        {
            "type": "Conv",
            "inputs": ["padded"],
            "output": "depthwise-out",
            "input_shape": [1, 64, 256],
            "output_shape": [1, 64, 200],
            "weights": "depthwise",
            "bias": None,
            "kernel": 29,
            "stride": 1,
            "native_strides": [1, 1],
            "dilation": 2,
            "pads": [0, 0],
            "group": 64,
        },
        {
            "type": "Conv",
            "inputs": ["depthwise-out"],
            "output": "pointwise-out",
            "input_shape": [1, 64, 200],
            "output_shape": [1, 128, 200],
            "weights": "pointwise",
            "bias": "bias",
            "kernel": 1,
            "stride": 1,
            "native_strides": [1, 1],
            "dilation": 1,
            "pads": [0, 0],
            "group": 1,
        },
        {
            "type": "Relu",
            "inputs": ["pointwise-out"],
            "output": "output",
            "input_shape": [1, 128, 200],
            "output_shape": [1, 128, 200],
        },
    ]
    return seal(
        {
            "schema_version": 1,
            "profile_id": "fp16-marblenet-pad-v1",
            "native_policy": {"id": "flat-height-depthwise-symmetric-v1"},
            "source_model_sha256": "1" * 64,
            "graph_signature": "2" * 64,
            "onnx_input": tensor(incoming, [1, 64, 200]),
            "onnx_output": tensor("output", [1, 128, 200]),
            "native_input": tensor("input", [200, 1, 64, 1]),
            "native_output": tensor("output", [200, 1, 128, 1]),
            "constants": constants,
            "nodes": nodes,
            "operation": {"type": "ConvGraph", "node_count": len(nodes)},
            "numerical_policy": {"atol": 0.02, "rtol": 0.02},
        }
    )


def test_old_profile_generated_bytes_stay_frozen():
    record = graph_record()
    compiler = generate_compile_source(record, COMPILE_ROOT)
    executor = generate_execute_source(record, EXECUTE_ROOT, 1800)
    assert hashlib.sha256(compiler.encode()).hexdigest() == (
        "6cc11a1e3f2ba9422c0141ca1d32063c67ea21ef7c237efd969a520e4cc0e00f"
    )
    assert hashlib.sha256(executor.encode()).hexdigest() == (
        "c239564fbc65e8eca57c75a1485fdef82bf24829c823abe6cb4aba466f07c772"
    )


def test_pad_profile_emits_exact_native_pad_and_valid_trained_conv():
    record = pad_profile_record()
    source = generate_compile_source(record, COMPILE_ROOT)
    validate_compile_source(record, COMPILE_ROOT, source)
    assert '#include "tim/vx/ops/pad.h"' in source
    assert "const std::vector<uint32_t> pad_front0{28, 0, 0, 0}" in source
    assert "const std::vector<uint32_t> pad_back0{28, 0, 0, 0}" in source
    assert "pad_front0, pad_back0, 0, tim::vx::ops::Pad::PAD_MODE_CONSTANT" in source
    assert "tensor_shape1{256, 1, 64, 1}" in source
    assert "weight_shape1{29, 1, 64, 1}" in source
    assert "pad1{0, 0, 0, 0}" in source
    assert "stride1{1, 1}" in source and "dilation1{2, 1}" in source
    assert (
        "operations[0]->BindInput(tensors[0])\n"
        "        .BindOutput(tensors[1]);"
    ) in source
    assert "kBiasBits1{0x00000000U" in source
    assert "flat-height-depthwise-symmetric-v1" in source
    assert record["lowering_sha256"] in source
    assert record["onnx_input"]["name"] not in source and "pad-widths" not in source
    assert source.count("graph->CompileToBinary(") == 2
    assert "graph->Run(" not in source and "alarm(0)" not in source


def test_new_flat_depthwise_stride2_is_native_symmetric_and_old_is_unchanged():
    record = pad_profile_record(include_pad=False)
    source = generate_compile_source(record, COMPILE_ROOT)
    assert "stride0{2, 2}" in source
    assert "stride1{1, 1}" in source
    assert "tensor_shape1{200, 1, 2, 1}" in source
    assert "stride0{2, 1}" in generate_compile_source(graph_record(), COMPILE_ROOT)
    assert "fp16-marblenet-pad-v1" in source


def test_pad_executor_binds_new_policy_operation_and_same_guarded_lifecycle():
    record = pad_profile_record()
    source = generate_execute_source(record, EXECUTE_ROOT, 1800)
    validate_execute_source(record, EXECUTE_ROOT, 1800, source)
    assert "operation=fp16-marblenet-pad" in source
    assert "flat-height-depthwise-symmetric-v1" in source
    assert record["lowering_sha256"] in source
    assert "kInputBytes = 25600" in source and "kOutputBytes = 51200" in source
    assert source.count("graph->Compile()") == source.count("graph->Run()") == 1
    assert "kWatchdogSeconds = 8" in source and "alarm(0)" not in source
    assert "kCanaryHalfBits = 0x7e00U" in source
    assert source.count("static_cast<uint8_t>(kCanaryHalfBits >> 8U)") == 2
    assert source.index("graph->Compile()") < source.index("device_canary[index]")
    assert source.index("context.reset();") < source.index("EXECUTION status=PASS")


def test_pad_omitted_zero_scalar_still_emits_exact_zero():
    record = pad_profile_record()
    record["nodes"][0]["value_constant"] = None
    del record["constants"]["pad-value"]
    source = generate_compile_source(seal(record), COMPILE_ROOT)
    assert "pad_front0, pad_back0, 0, tim::vx::ops::Pad::PAD_MODE_CONSTANT" in source


def test_pad_sources_bind_canonical_lowering_identity():
    record = pad_profile_record()
    compiler = generate_compile_source(record, COMPILE_ROOT)
    executor = generate_execute_source(record, EXECUTE_ROOT, 1800)
    changed = copy.deepcopy(record)
    changed["graph_signature"] = "3" * 64
    seal(changed)
    assert generate_compile_source(changed, COMPILE_ROOT) != compiler
    assert generate_execute_source(changed, EXECUTE_ROOT, 1800) != executor
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(changed, COMPILE_ROOT, compiler)
    with pytest.raises(ValueError, match="source"):
        validate_execute_source(changed, EXECUTE_ROOT, 1800, executor)


def test_legacy_profile_cannot_admit_pad_even_after_record_reseal():
    record = pad_profile_record()
    record["profile_id"] = "fp16-marblenet-v1"
    del record["native_policy"]
    for node in record["nodes"]:
        node.pop("native_strides", None)
    with pytest.raises(ValueError):
        generate_compile_source(seal(record), COMPILE_ROOT)


@pytest.mark.parametrize(
    "change",
    [
        lambda r: r.update(native_policy={"id": "anything-else"}),
        lambda r: r.update(
            native_policy={"id": "flat-height-depthwise-symmetric-v1", "extra": 1}
        ),
        lambda r: r["nodes"][0].update(pads=[27, 29]),
        lambda r: r["nodes"][0].update(value=1.0),
        lambda r: r["nodes"][0].update(pads_constant="missing"),
        lambda r: r["nodes"][1].update(pads=[28, 28]),
        lambda r: r["nodes"][1].update(native_strides=[1, 2]),
        lambda r: r["nodes"][1].update(native_strides=[True, 1]),
    ],
)
def test_resealed_pad_or_native_policy_mutations_are_refused(change):
    record = pad_profile_record()
    change(record)
    with pytest.raises(ValueError):
        generate_compile_source(seal(record), COMPILE_ROOT)


def test_new_profile_missing_native_policy_or_conv_strides_is_refused():
    removals = (
        lambda r: r.pop("native_policy"),
        lambda r: r["nodes"][0].pop("native_strides"),
    )
    for remove in removals:
        record = pad_profile_record(include_pad=False)
        remove(record)
        with pytest.raises(ValueError):
            generate_compile_source(seal(record), COMPILE_ROOT)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("pad_front0{28, 0, 0, 0}", "pad_front0{0, 0, 28, 0}"),
        lambda s: s.replace("pad_back0{28, 0, 0, 0}", "pad_back0{27, 0, 0, 0}"),
        lambda s: s.replace("Pad::PAD_MODE_CONSTANT", "Pad::PAD_MODE_EDGE"),
        lambda s: s.replace("pad_front0, pad_back0, 0,", "pad_front0, pad_back0, 1,"),
        lambda s: s.replace("dilation1{2, 1}", "dilation1{1, 1}"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
    ],
)
def test_pad_generated_source_edits_are_refused(mutation):
    record = pad_profile_record()
    source = generate_compile_source(record, COMPILE_ROOT)
    changed = mutation(source)
    assert changed != source
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(record, COMPILE_ROOT, changed)


def test_new_flat_stride_source_edit_is_refused():
    record = pad_profile_record(include_pad=False)
    source = generate_compile_source(record, COMPILE_ROOT)
    changed = source.replace("stride0{2, 2}", "stride0{2, 1}")
    assert changed != source
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(record, COMPILE_ROOT, changed)


def test_changed_pad_template_is_refused(tmp_path, monkeypatch):
    template = marblenet_sources._TEMPLATE_ROOT / "compile_fp16_pad_graph.cc.in"
    (tmp_path / template.name).write_text(
        template.read_text().replace("context.reset();", "alarm(0); context.reset();")
    )
    monkeypatch.setattr(marblenet_sources, "_TEMPLATE_ROOT", tmp_path)
    with pytest.raises(ValueError, match="template hash"):
        generate_compile_source(pad_profile_record(), COMPILE_ROOT)
