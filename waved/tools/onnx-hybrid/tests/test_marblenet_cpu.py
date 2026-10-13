"""CPU fallback timing uses stock ORT without any NPU runtime path."""

import copy
import json

import pytest
from test_marblenet_benchmark import full_record

from wave_onnx_hybrid import marblenet_cpu

ROOT = "/tmp/wave-npu-onnx18-0123456789abcdef-execute"


def source():
    return marblenet_cpu.generate_cpu_source(full_record(), ROOT, 366942)


def test_cpu_program_has_no_npu_loading_or_calls():
    generated = source()
    marblenet_cpu.validate_cpu_source(full_record(), ROOT, 366942, generated)
    for forbidden in [
        "tim::vx",
        "tim/vx",
        "libGAL",
        "gcoHAL",
        "NBG",
        "native-reference",
        "QueryChipIdentity",
        "graph->",
        "libOpenVX",
        "libCLC",
    ]:
        assert forbidden not in generated
    assert '"/usr/lib/libonnxruntime.so.1.15.1"' in generated
    assert (
        "CPU_IDENTITY ort_version=1.15.1 provider=CPUExecutionProvider threads=1"
        in generated
    )
    assert "CPU_EXECUTION status=PASS" in generated


def test_cpu_call_budget_files_guards_and_teardown():
    generated = source()
    assert (
        "kWarmupIterations = 4" in generated and "kMeasuredIterations = 32" in generated
    )
    assert "iteration < kWarmupIterations + kMeasuredIterations" in generated
    assert "iterations=36 input_bytes=%zu output_bytes=%zu" in generated
    assert "kInputBytes = 128000" in generated and "kOutputBytes = 1600" in generated
    assert "kWatchdogSeconds = 8" in generated and "alarm(0)" not in generated
    assert "PR_SET_PDEATHSIG, SIGKILL" in generated
    assert "O_RDONLY | O_CLOEXEC | O_NOFOLLOW" in generated
    assert "O_EXCL | O_CLOEXEC | O_NOFOLLOW" in generated
    assert "(file_stat.st_mode & 07777) != 0444" in generated
    assert "kGuardBytes = 16" in generated and "GuardsIntact" in generated
    assert "std::numeric_limits<float>::quiet_NaN()" in generated
    assert "0.0001F + 0.00001F" in generated
    assert generated.index(
        "WriteAtomicExclusive(kOutputPath, kPartialPath, observed)"
    ) < generated.index("if (!cpu.Close())")
    assert generated.index("if (!cpu.Close())") < generated.index(
        "CPU_EXECUTION status=PASS"
    )


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("kMeasuredIterations = 32", "kMeasuredIterations = 33"),
        lambda s: s.replace("if (!cpu.Close())", "alarm(0); if (!cpu.Close())"),
        lambda s: s.replace("ORT_ENABLE_ALL", "ORT_DISABLE_ALL"),
        lambda s: s.replace("0.0001F + 0.00001F", "1.0F + 1.0F"),
        lambda s: s.replace("quiet_NaN()", "infinity()"),
    ],
)
def test_changed_cpu_source_refuses(mutation):
    generated = source()
    changed = mutation(generated)
    assert changed != generated
    with pytest.raises(ValueError, match="source"):
        marblenet_cpu.validate_cpu_source(full_record(), ROOT, 366942, changed)


@pytest.mark.parametrize(
    "root,size",
    [
        (ROOT + "/", 366942),
        (ROOT.replace("18-", "17-"), 366942),
        (ROOT.replace("execute", "compile"), 366942),
        (ROOT, True),
        (ROOT, 0),
        (ROOT, 8 * 1024 * 1024 + 1),
    ],
)
def test_cpu_root_and_model_size_refuses(root, size):
    with pytest.raises(ValueError):
        marblenet_cpu.generate_cpu_source(full_record(), root, size)


def test_cpu_template_pin_refuses_mutation(tmp_path, monkeypatch):
    p = tmp_path / "cpu_fp32_graph.cc.in"
    p.write_text(
        marblenet_cpu._TEMPLATE_PATH.read_text().replace(
            "iterations=36", "iterations=37"
        )
    )
    monkeypatch.setattr(marblenet_cpu, "_TEMPLATE_PATH", p)
    with pytest.raises(ValueError, match="template"):
        source()


def test_cpu_header_pin_and_changed_model_size_refuse(tmp_path, monkeypatch):
    generated = source()
    with pytest.raises(ValueError, match="source"):
        marblenet_cpu.validate_cpu_source(full_record(), ROOT, 366944, generated)
    header = tmp_path / "onnxruntime_c_api.h"
    header.write_text("#define ORT_API_VERSION 16\n")
    monkeypatch.setattr(marblenet_cpu, "_ORT_HEADER_PATH", header)
    with pytest.raises(ValueError, match="header"):
        source()


def test_cpu_run_and_softmax_are_reviewed_benchmark_functions():
    from wave_onnx_hybrid import marblenet_benchmark

    baseline = marblenet_benchmark._TEMPLATE_PATH.read_text()
    generated = source()
    start, end = "bool RunCpu(", "bool RunNpu("
    reviewed = baseline.split(start, 1)[1].split(end, 1)[0]
    actual = generated.split(start, 1)[1].split("void PrintDefinition", 1)[0]
    assert reviewed.strip() == actual.strip()
    for marker in ("bool CpuOutputMatches(", "bool SoftmaxSpeechSum("):
        assert marker in generated


def test_cpu_parser_refuses_duplicate_json_fields():
    text = receipt(samples()).replace(
        '"sample_index": 0', '"sample_index": 0, "sample_index": 0', 1
    )
    with pytest.raises(ValueError, match="duplicate"):
        marblenet_cpu.parse_cpu_samples(text)


def samples():
    return [
        {
            "sample_index": i,
            "iteration": i + 4,
            "boundary_ns": 1000,
            "thread_ns": 900,
            "input_copy_ns": 50,
            "run_ns": 700,
            "postprocess_ns": 100,
            "speech_probability_sum": 100.0,
        }
        for i in range(32)
    ]


def receipt(values):
    return "\n".join("CPU_SAMPLE " + json.dumps(v) for v in values)


def test_cpu_sample_parser_preserves_actual_timings():
    assert marblenet_cpu.parse_cpu_samples(receipt(samples())) == samples()


@pytest.mark.parametrize(
    "change",
    [
        lambda v: v.pop(),
        lambda v: v.append(copy.deepcopy(v[0])),
        lambda v: v[0].update(iteration=5),
        lambda v: v[0].update(run_ns=True),
        lambda v: v[0].update(boundary_ns=100),
        lambda v: v[0].update(speech_probability_sum=float("nan")),
    ],
)
def test_cpu_sample_tampering_refuses(change):
    values = samples()
    change(values)
    with pytest.raises(ValueError):
        marblenet_cpu.parse_cpu_samples(receipt(values))
