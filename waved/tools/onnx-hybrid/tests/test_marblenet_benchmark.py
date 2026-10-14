"""Fixed benchmark source, clocks, arithmetic and sample receipts fail closed."""

import copy
import hashlib
import json
import math
import struct

import pytest

from wave_onnx_hybrid import marblenet_benchmark
from wave_onnx_hybrid.audit import canonical_json

ROOT = "/tmp/wave-npu-onnx18-0123456789abcdef-execute"


def full_record():
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

    weights = struct.pack("<160H", *([0x3000] * 160))
    record = {
        "schema_version": 1,
        "profile_id": "fp16-marblenet-v1",
        "source_model_sha256": "1" * 64,
        "graph_signature": "2" * 64,
        "onnx_input": tensor("input", [1, 80, 400]),
        "onnx_output": tensor("output", [1, 2, 200]),
        "native_input": tensor("input", [400, 1, 80, 1]),
        "native_output": tensor("output", [200, 1, 2, 1]),
        "constants": {"w": tensor("w", [2, 80, 1], weights)},
        "nodes": [
            {
                "type": "Conv",
                "inputs": ["input"],
                "output": "output",
                "input_shape": [1, 80, 400],
                "output_shape": [1, 2, 200],
                "weights": "w",
                "bias": None,
                "kernel": 1,
                "stride": 2,
                "dilation": 1,
                "pads": [0, 0],
                "group": 1,
            }
        ],
        "operation": {"type": "ConvGraph", "node_count": 1},
        "numerical_policy": {"atol": 0.02, "rtol": 0.02},
    }
    return seal(record)


def seal(record):
    record.pop("lowering_sha256", None)
    record["lowering_sha256"] = hashlib.sha256(canonical_json(record)).hexdigest()
    return record


def source():
    return marblenet_benchmark.generate_benchmark_source(
        full_record(), ROOT, 1800, 366942
    )


def test_fixed_benchmark_budget_and_teardown():
    generated = source()
    marblenet_benchmark.validate_benchmark_source(
        full_record(), ROOT, 1800, 366942, generated
    )
    assert generated.count("graph->Compile()") == 1
    assert generated.count("graph->Run()") == 1  # fixed loop body,36 iterations
    assert "kWarmupIterations = 4" in generated
    assert "kMeasuredIterations = 32" in generated
    assert "iteration < kWarmupIterations + kMeasuredIterations" in generated
    assert "iterations=36" in generated
    assert "operation=fp16-marblenet-benchmark" in generated
    assert "kInputBytes = 64000" in generated and "kOutputBytes = 800" in generated
    assert "kWatchdogSeconds = 8" in generated and "alarm(0)" not in generated
    assert "PR_SET_PDEATHSIG, SIGKILL" in generated
    assert generated.index("graph->Compile()") < generated.index(
        "for (size_t iteration"
    )
    assert generated.index("if (!cpu.Close())") < generated.index("context.reset();")
    assert generated.index("context.reset();") < generated.index(
        "EXECUTION status=PASS"
    )
    assert generated.index(
        "WriteAtomicExclusive(kOutputPath, kPartialPath, observed)"
    ) < generated.index("context.reset();")


def test_cpu_contract_and_same_boundary_are_visible():
    generated = source()
    assert '#include "onnxruntime_c_api.h"' in generated
    assert '"/usr/lib/libonnxruntime.so.1.15.1"' in generated
    assert '"1.15.1"' in generated and "ORT_API_VERSION == 15" in generated
    assert "CPUExecutionProvider" in generated
    for call in [
        "SetIntraOpNumThreads",
        "SetInterOpNumThreads",
        "SetSessionExecutionMode",
    ]:
        assert call in generated
    assert "ORT_SEQUENTIAL" in generated and "ORT_ENABLE_ALL" in generated
    assert "CreateSessionFromArray" in generated
    assert "SessionGetInputName" in generated and "SessionGetOutputName" in generated
    assert "CLOCK_THREAD_CPUTIME_ID" in generated and "CLOCK_MONOTONIC_RAW" in generated
    assert "iteration % 2 == 0" in generated
    assert "BENCH_SAMPLE " in generated
    assert "npu_input_copy_ns" in generated and "npu_output_copy_ns" in generated
    assert "cpu_postprocess_ns" in generated and "npu_postprocess_ns" in generated
    assert "feature_extraction_excluded" in generated


def test_fixed_files_and_actual_outputs():
    generated = source()
    for name in [
        "cpu.onnx",
        "cpu-input.bin",
        "cpu-reference.bin",
        "native-reference.bin",
    ]:
        assert f"{ROOT}/" in generated and name in generated
    assert "O_RDONLY | O_CLOEXEC | O_NOFOLLOW" in generated
    assert "(file_stat.st_mode & 07777) != 0444" in generated
    assert "S_ISREG(file_stat.st_mode)" in generated
    assert "CpuOutputMatches" in generated and "0.0001F + 0.00001F" in generated
    assert "std::memcmp(native_output.data(), native_reference.data()" in generated
    assert "!AllCanary(native_output.data()" in generated
    assert "PrimeHalfNaN(device_canary.data(), device_canary.size())" in generated
    assert "GuardsIntact" in generated
    assert 'std::printf("%02x"' in generated


@pytest.mark.parametrize(
    "root",
    [
        ROOT + "/",
        ROOT + "/../execute",
        ROOT.replace("18-", "17-"),
        ROOT.replace("execute", "compile"),
        "/tmp/other",
    ],
)
def test_wrong_roots_refused(root):
    with pytest.raises(ValueError, match="root"):
        marblenet_benchmark.generate_benchmark_source(full_record(), root, 1800, 366942)


@pytest.mark.parametrize(
    "nbg,cpu",
    [
        (512, 366942),
        (True, 366942),
        (1800, 0),
        (1800, True),
        (1800, 8 * 1024 * 1024 + 1),
    ],
)
def test_wrong_model_sizes_refused(nbg, cpu):
    with pytest.raises(ValueError, match="size"):
        marblenet_benchmark.generate_benchmark_source(full_record(), ROOT, nbg, cpu)


def test_only_full_shape_and_fixed_io_names_are_eligible():
    record = full_record()
    record["onnx_input"]["name"] = "features"
    record["nodes"][0]["inputs"] = ["features"]
    with pytest.raises(ValueError):
        marblenet_benchmark.generate_benchmark_source(seal(record), ROOT, 1800, 366942)


def test_changed_record_invalidates_existing_source():
    record = full_record()
    old = source()
    payload = struct.pack("<160H", *([0x3400] * 160))
    record["constants"]["w"].update(
        payload_hex=payload.hex(), sha256=hashlib.sha256(payload).hexdigest()
    )
    seal(record)
    with pytest.raises(ValueError, match="source"):
        marblenet_benchmark.validate_benchmark_source(record, ROOT, 1800, 366942, old)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda s: s.replace("kMeasuredIterations = 32", "kMeasuredIterations = 33"),
        lambda s: s.replace("if (!graph->Run())", "graph->Run(); if (!graph->Run())"),
        lambda s: s.replace("context.reset();", "alarm(0); context.reset();"),
        lambda s: s.replace("ORT_ENABLE_ALL", "ORT_DISABLE_ALL"),
        lambda s: s.replace(
            "PrimeHalfNaN(device_canary.data(), device_canary.size())",
            "device_canary.fill(0)",
        ),
        lambda s: s.replace("0.0001F + 0.00001F", "1.0F + 1.0F"),
    ],
)
def test_benchmark_policy_mutations_refused(mutation):
    generated = source()
    changed = mutation(generated)
    assert changed != generated
    with pytest.raises(ValueError, match="source"):
        marblenet_benchmark.validate_benchmark_source(
            full_record(), ROOT, 1800, 366942, changed
        )


def test_changed_template_or_header_refused(tmp_path, monkeypatch):
    template = marblenet_benchmark._TEMPLATE_PATH
    changed = tmp_path / template.name
    changed.write_text(
        template.read_text().replace(
            "kMeasuredIterations = 32", "kMeasuredIterations = 33"
        )
    )
    monkeypatch.setattr(marblenet_benchmark, "_TEMPLATE_PATH", changed)
    with pytest.raises(ValueError, match="template"):
        source()
    monkeypatch.setattr(marblenet_benchmark, "_TEMPLATE_PATH", template)
    header = tmp_path / "onnxruntime_c_api.h"
    header.write_text("#define ORT_API_VERSION 16\n")
    monkeypatch.setattr(marblenet_benchmark, "_ORT_HEADER_PATH", header)
    with pytest.raises(ValueError, match="header"):
        source()


def samples():
    return [
        {
            "sample_index": i,
            "iteration": i + 4,
            "order": "cpu-npu" if i % 2 == 0 else "npu-cpu",
            "cpu_boundary_ns": 1000,
            "cpu_thread_ns": 900,
            "cpu_input_copy_ns": 50,
            "cpu_run_ns": 700,
            "cpu_postprocess_ns": 100,
            "npu_boundary_ns": 500,
            "npu_thread_ns": 200,
            "npu_input_copy_ns": 50,
            "npu_canary_copy_ns": 25,
            "npu_run_ns": 200,
            "npu_output_copy_ns": 50,
            "npu_postprocess_ns": 100,
            "cpu_speech_probability_sum": 100.0,
            "npu_speech_probability_sum": 100.1,
        }
        for i in range(32)
    ]


def receipt(values):
    return "IDENTITY example\n" + "\n".join(
        "BENCH_SAMPLE " + json.dumps(value) for value in values
    )


def test_sample_parser_preserves_actual_timings_and_order():
    assert marblenet_benchmark.parse_benchmark_samples(receipt(samples())) == samples()


@pytest.mark.parametrize(
    "change",
    [
        lambda v: v.pop(),
        lambda v: v.append(copy.deepcopy(v[0])),
        lambda v: v[0].update(order="npu-cpu"),
        lambda v: v[0].update(cpu_run_ns=True),
        lambda v: v[0].update(npu_run_ns=-1),
        lambda v: v[0].update(npu_boundary_ns=100),
        lambda v: v[0].update(cpu_speech_probability_sum=float("nan")),
    ],
)
def test_changed_sample_receipts_refused(change):
    values = samples()
    change(values)
    with pytest.raises(ValueError):
        marblenet_benchmark.parse_benchmark_samples(receipt(values))


@pytest.fixture(scope="module")
def portable_numerics(tmp_path_factory):
    """Execute only our extracted arithmetic; no vendor headers or libraries."""
    import ctypes
    import subprocess

    folder = tmp_path_factory.mktemp("benchmark-numerics")
    template = marblenet_benchmark._TEMPLATE_PATH.read_text()
    body = template.split("// BEGIN_PORTABLE_NUMERICS\n", 1)[1].split(
        "// END_PORTABLE_NUMERICS", 1
    )[0]
    standalone = (
        "#include <algorithm>\n#include <cmath>\n#include <cstring>\n"
        "#include <cstdint>\n#include <cstddef>\n"
        "constexpr size_t kFrames=200, kOutputElements=400;\n"
        + body
        + '\nextern "C" float decode(uint16_t v) { return HalfBitsToFloat(v); }\n'
        + 'extern "C" uint16_t encode(float v) { return FloatToHalfBits(v); }\n'
        + 'extern "C" bool softmax(const float* v,float* s) '
        "{ return SoftmaxSpeechSum(v,s); }\n"
        + 'extern "C" bool matches(const float* v,const float* r) '
        "{ return CpuOutputMatches(v,r); }\n"
        + 'extern "C" void prime(uint8_t* v,size_t n) { PrimeHalfNaN(v,n); }\n'
    )
    source_path = folder / "arithmetic.cc"
    source_path.write_text(standalone)
    library_path = folder / "arithmetic.so"
    subprocess.run(
        [
            "c++",
            "-std=c++17",
            "-shared",
            "-fPIC",
            "-O2",
            str(source_path),
            "-o",
            str(library_path),
        ],
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    library = ctypes.CDLL(str(library_path))
    library.decode.argtypes = [ctypes.c_uint16]
    library.decode.restype = ctypes.c_float
    library.encode.argtypes = [ctypes.c_float]
    library.encode.restype = ctypes.c_uint16
    float_pointer = ctypes.POINTER(ctypes.c_float)
    library.softmax.argtypes = [float_pointer, float_pointer]
    library.softmax.restype = ctypes.c_bool
    library.matches.argtypes = [float_pointer, float_pointer]
    library.matches.restype = ctypes.c_bool
    library.prime.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    return library


def test_native_portable_half_decoder_all_finite_bit_patterns(portable_numerics):
    import numpy as np

    bits = np.arange(65536, dtype="<u2")
    finite = bits[(bits & 0x7C00) != 0x7C00]
    expected = finite.view("<f2").astype("<f4")
    actual = np.array([portable_numerics.decode(int(v)) for v in finite], dtype="<f4")
    np.testing.assert_array_equal(actual.view("<u4"), expected.view("<u4"))


def test_native_portable_half_encoder_matches_independent_rounding(portable_numerics):
    import numpy as np

    values = np.concatenate(
        [
            np.random.default_rng(18).normal(size=4096).astype("<f4") * 100,
            np.array(
                [
                    0,
                    -0.0,
                    2**-25,
                    -(2**-25),
                    2**-24,
                    1 + 2**-11,
                    1 + 3 * 2**-11,
                    65504,
                    -65504,
                ],
                dtype="<f4",
            ),
        ]
    )
    expected = values.astype("<f2").view("<u2")
    actual = np.array([portable_numerics.encode(float(v)) for v in values], dtype="<u2")
    np.testing.assert_array_equal(actual, expected)


def test_native_softmax_is_stable_and_cpu_comparison_rejects_bad_output(
    portable_numerics,
):
    import ctypes

    import numpy as np

    def pointer(value):
        return value.ctypes.data_as(ctypes.POINTER(ctypes.c_float))

    logits = np.full(400, 1000, dtype="<f4")
    total = ctypes.c_float()
    assert portable_numerics.softmax(pointer(logits), ctypes.byref(total))
    assert total.value == 100
    logits[:200] = -1000
    assert portable_numerics.softmax(pointer(logits), ctypes.byref(total))
    assert total.value == 200
    logits[0] = np.nan
    assert not portable_numerics.softmax(pointer(logits), ctypes.byref(total))
    reference = np.ones(400, dtype="<f4")
    observed = reference + np.float32(0.0001)
    assert portable_numerics.matches(pointer(observed), pointer(reference))
    observed[0] = np.float32(1.00012)
    assert not portable_numerics.matches(pointer(observed), pointer(reference))
    observed[0] = np.nan
    assert not portable_numerics.matches(pointer(observed), pointer(reference))
    output = (ctypes.c_uint8 * 800)()
    portable_numerics.prime(output, 800)
    assert bytes(output) == b"\x00\x7e" * 400
