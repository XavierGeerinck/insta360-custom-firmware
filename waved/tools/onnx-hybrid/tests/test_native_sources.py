"""Bounded native templates are immutable, input-driven, and fail closed."""

import copy
import hashlib
import struct

import pytest

from wave_onnx_hybrid.audit import canonical_json
from wave_onnx_hybrid.native_sources import (
    generate_compile_source,
    generate_execute_source,
    validate_compile_source,
    validate_execute_source,
)

COMPILE_ROOT = "/tmp/wave-npu-onnx17-0123456789abcdef-compile"
EXECUTE_ROOT = "/tmp/wave-npu-onnx17-0123456789abcdef-execute"
NBG = 513


def seal(record):
    record.pop("lowering_sha256", None)
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    return record


def tensor(dtype, shape, size, payload=None, quantized=False):
    value = {"dtype": dtype, "shape": shape, "byte_size": size}
    if payload is not None:
        value.update(
            payload_hex=payload.hex(), sha256=hashlib.sha256(payload).hexdigest()
        )
    if quantized:
        value["quantization"] = {"type": "ASYMMETRIC", "scale": 1.0, "zero_point": 0}
    return value


def lowering(profile="fp16-conv-v1"):
    conv = profile == "fp16-conv-v1"
    record = {
        "schema_version": 1,
        "profile_id": profile,
        "source_model_sha256": "1" * 64,
        "graph_signature": "2" * 64,
        "onnx_input": {"name": 'hostile\\"; graph->Run(); //'},
        "onnx_output": {"name": "do not inject me"},
        "native_input": tensor("FLOAT16", [16, 1, 1, 1], 32)
        if conv
        else tensor("UINT8", [16, 1], 16, quantized=True),
        "native_output": tensor("FLOAT16", [16, 1, 1, 1], 32)
        if conv
        else tensor("UINT8", [4, 1], 4, quantized=True),
        "weights": tensor(
            "FLOAT16", [3, 1, 1, 1], 6, struct.pack("<3H", 0x3800, 0x3400, 0xB400)
        )
        if conv
        else tensor("UINT8", [16, 4], 64, bytes(range(64)), True),
        "bias": tensor("FLOAT32", [1], 4, struct.pack("<f", 0.125))
        if conv
        else tensor("INT32", [4], 16, bytes(16), True),
        "operation": {
            "type": "Conv2d",
            "padding": "SAME",
            "strides": [1, 1],
            "dilations": [0, 0],
            "group": 1,
            "kernel": [3, 1],
        }
        if conv
        else {"type": "FullyConnected", "axis": 0, "units": 4},
    }
    return seal(record)


@pytest.mark.parametrize("profile", ["fp16-conv-v1", "u8-fc-v1"])
def test_fixed_native_lifecycle_and_actual_constants(profile):
    record = lowering(profile)
    compiler = generate_compile_source(record, COMPILE_ROOT)
    executor = generate_execute_source(record, EXECUTE_ROOT, NBG)
    validate_compile_source(record, COMPILE_ROOT, compiler)
    validate_execute_source(record, EXECUTE_ROOT, NBG, executor)
    assert compiler.count("graph->CompileToBinary(") == 2
    assert "graph->Run(" not in compiler
    assert executor.count("graph->Compile()") == executor.count("graph->Run()") == 1
    assert "alarm(0)" not in compiler + executor
    assert "PR_SET_PDEATHSIG, SIGKILL" in compiler + executor
    assert "hostile" not in compiler + executor
    assert "do not inject me" not in compiler + executor
    assert "kExpected" not in executor.replace("kExpectedNbg", "").replace(
        "kExpectedLibrary", ""
    ).replace("kExpectedSdk", "").replace("kExpectedModel", "")
    assert "memcmp(observed" not in executor
    assert f"{EXECUTE_ROOT}/input/input.bin" in executor
    assert "ReadFixedFile(kInputPath, kInputBytes, &input_bytes)" in executor
    assert (
        executor.index("graph->Compile()")
        < executor.index("output->CopyDataToTensor(device_canary.data(), kOutputBytes)")
        < executor.index("graph->Run()")
    )
    assert (
        executor.index("guarded_output.fill(kGuardByte)")
        < executor.index("std::memset(guarded_output.data() + kGuardBytes, kCanaryByte")
        < executor.index("output->CopyDataFromTensor(")
    )
    assert (
        executor.index("WriteAtomicExclusive(kOutputPath, kPartialPath, observed)")
        < executor.index("context.reset();")
        < executor.index("EXECUTION status=PASS")
    )
    if profile == "fp16-conv-v1":
        assert "0x3800U, 0x3400U, 0xb400U" in compiler
        assert "0x3e000000U" in compiler  # FLOAT32 bias bit pattern
    else:
        assert ", ".join(str(i) for i in range(64)) in compiler


@pytest.mark.parametrize(
    "root",
    [
        "/tmp/wave-npu-onnx17-0123456789abcdef-compile/../compile",
        "/tmp/wave-npu-onnx17-0123456789ABCDEF-compile",
        "/tmp/wave-npu-onnx17-0123456789abcde-compile",
        "/tmp/wave-npu-onnx17-0123456789abcdef-compile/",
        '/tmp/wave-npu-onnx17-0123456789abcdef-compile";system("id");',
        EXECUTE_ROOT,
    ],
)
def test_refuses_unsafe_compile_roots(root):
    with pytest.raises(ValueError, match="root"):
        generate_compile_source(lowering(), root)


@pytest.mark.parametrize("root", [COMPILE_ROOT, EXECUTE_ROOT + "/", "/tmp/other"])
def test_refuses_unsafe_execute_roots(root):
    with pytest.raises(ValueError, match="root"):
        generate_execute_source(lowering(), root, NBG)


def test_constant_changes_invalidate_old_source_and_stale_digest():
    record = lowering()
    old_source = generate_compile_source(record, COMPILE_ROOT)
    changed = copy.deepcopy(record)
    changed["weights"]["payload_hex"] = "003800380038"
    with pytest.raises(ValueError, match=r"digest|hash"):
        generate_compile_source(changed, COMPILE_ROOT)
    payload = bytes.fromhex(changed["weights"]["payload_hex"])
    changed["weights"]["sha256"] = hashlib.sha256(payload).hexdigest()
    seal(changed)
    assert generate_compile_source(changed, COMPILE_ROOT) != old_source
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(changed, COMPILE_ROOT, old_source)


@pytest.mark.parametrize(
    "field,value",
    [
        ("operation", {"type": "Conv2d", "strides": [2, 2]}),
        ("native_input", tensor("FLOAT16", [32, 1, 1, 1], 64)),
        ("profile_id", "unknown"),
        ("schema_version", True),
    ],
)
def test_refuses_resealed_invalid_contract(field, value):
    record = lowering()
    record[field] = value
    seal(record)
    with pytest.raises(ValueError):
        generate_compile_source(record, COMPILE_ROOT)


@pytest.mark.parametrize("payload", [struct.pack("<3H", 0x7C00, 0, 0), bytes(5)])
def test_refuses_nonfinite_or_wrong_length_half_constants(payload):
    record = lowering()
    record["weights"]["payload_hex"] = payload.hex()
    record["weights"]["sha256"] = hashlib.sha256(payload).hexdigest()
    seal(record)
    with pytest.raises(ValueError):
        generate_compile_source(record, COMPILE_ROOT)


@pytest.mark.parametrize("bad_nbg", [0, 512, 64 * 1024 * 1024 + 1, True, "513", b"x"])
def test_refuses_invalid_nbg_before_source_generation(bad_nbg):
    with pytest.raises(ValueError, match="NBG"):
        generate_execute_source(lowering(), EXECUTE_ROOT, bad_nbg)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("if (!graph->Run())", "graph->Run(); if (!graph->Run())"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
        lambda s: s.replace(
            "device_canary.fill(kCanaryByte);", "device_canary.fill(0);"
        ),
        lambda s: s.replace(
            "guarded_output.fill(kGuardByte);", "guarded_output.fill(0);"
        ),
        lambda s: s.replace("S_ISREG(file_stat.st_mode)", "true"),
        lambda s: s.replace("(file_stat.st_mode & 07777) != 0444", "false"),
        lambda s: s.replace(
            "file_stat.st_size != static_cast<off_t>(expected_bytes)", "false"
        ),
        lambda s: s.replace("O_EXCL | O_CLOEXEC | O_NOFOLLOW", "O_CLOEXEC"),
        lambda s: s.replace("link(partial_path, path)", "rename(partial_path, path)"),
        lambda s: s.replace("ReadLe32(bytes + 8) != kWaveProductId", "false"),
        lambda s: s.replace("kCanaryByte = 0xa5U", "kCanaryByte = 0U"),
    ],
)
def test_refuses_executor_policy_mutations(mutation):
    record = lowering()
    source = generate_execute_source(record, EXECUTE_ROOT, NBG)
    modified = mutation(source)
    assert modified != source
    with pytest.raises(ValueError, match="source"):
        validate_execute_source(record, EXECUTE_ROOT, NBG, modified)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("0x3800U", "0x3000U"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
        lambda s: s.replace(
            "size_t nbg_size = 0;", "graph->Run(); size_t nbg_size = 0;"
        ),
        lambda s: s.replace("0x1000000eU", "0x10000000U"),
    ],
)
def test_refuses_compiler_policy_mutations(mutation):
    record = lowering()
    source = generate_compile_source(record, COMPILE_ROOT)
    modified = mutation(source)
    assert modified != source
    with pytest.raises(ValueError, match="source"):
        validate_compile_source(record, COMPILE_ROOT, modified)


def test_template_hash_rejects_changed_immutable_template(tmp_path, monkeypatch):
    from wave_onnx_hybrid import native_sources

    template = native_sources._TEMPLATE_ROOT / "execute.cc.in"
    changed = template.read_text().replace("if (!graph->Run())", "if (true)")
    (tmp_path / "execute.cc.in").write_text(changed)
    monkeypatch.setattr(native_sources, "_TEMPLATE_ROOT", tmp_path)
    with pytest.raises(ValueError, match="template hash"):
        generate_execute_source(lowering(), EXECUTE_ROOT, NBG)


def test_nbg_size_change_invalidates_old_runner():
    record = lowering()
    source = generate_execute_source(record, EXECUTE_ROOT, NBG)
    with pytest.raises(ValueError, match="source"):
        validate_execute_source(record, EXECUTE_ROOT, NBG + 8, source)


@pytest.mark.parametrize("control", ["17a1", "17a2", "17b1", "17b2"])
def test_real_importer_control_bytes_reach_source(control):
    import ast
    import re
    from pathlib import Path

    from wave_onnx_hybrid.native_import import import_native_model

    fixtures = Path(__file__).resolve().parents[1] / "fixtures" / "native-poc17"
    record = import_native_model(fixtures / f"{control}.onnx")
    source = generate_compile_source(record, COMPILE_ROOT)
    literal = re.search(r"void PrintDefinition\(\) \{\s*std::puts\((.*)\);", source)[1]
    import json

    definition = json.loads(ast.literal_eval(literal))
    if control.startswith("17a"):
        encoded = struct.pack("<3H", *definition["constant_weights"]["half_bits"])
        assert encoded.hex() == record["weights"]["payload_hex"]
        bias = struct.pack("<f", *definition["constant_bias"]["values"])
        assert bias.hex() == record["bias"]["payload_hex"]
    else:
        encoded = bytes(definition["constant_weights"]["values"])
        assert encoded.hex() == record["weights"]["payload_hex"]
    runner = generate_execute_source(record, EXECUTE_ROOT, NBG)
    assert "kInput{" not in runner
    assert "expected_half_bits" not in runner
    assert 'expected\\"' not in runner


def test_file_contract_and_fixed_environment_are_visible_in_approved_source():
    compiler = generate_compile_source(lowering(), COMPILE_ROOT)
    runner = generate_execute_source(lowering(), EXECUTE_ROOT, NBG)
    for source in (compiler, runner):
        assert "O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW" in source
        assert "S_ISREG(output_stat.st_mode)" in source
        assert "output_stat.st_size == static_cast<off_t>(data.size())" in source
        assert "fchmod(fd, 0444)" in source
        assert "fsync(fd)" in source
        assert "link(partial_path, path)" in source
        assert 'std::getenv("VSI_NN_LOG_LEVEL")' in source
        assert 'std::strcmp(log_level, "5")' in source
    assert "O_RDONLY | O_CLOEXEC | O_NOFOLLOW" in runner
    assert "!S_ISREG(file_stat.st_mode)" in runner
    assert "(file_stat.st_mode & 07777) != 0444" in runner
    assert "file_stat.st_size != static_cast<off_t>(expected_bytes)" in runner
    assert "extra_bytes != 0 || close_status != 0" in runner
    assert "kGuardBytes = 16" in runner
    assert "kWatchdogSeconds = 8" in runner
    assert "kWatchdogSeconds = 15" in compiler
    assert f"{COMPILE_ROOT}/output/model.nbg" in compiler
    assert f"{COMPILE_ROOT}/output/.model.nbg.partial" in compiler
