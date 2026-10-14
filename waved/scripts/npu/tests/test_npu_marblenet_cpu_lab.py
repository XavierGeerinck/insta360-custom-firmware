"""CPU-only preparation/parser/admission tests; never connect to ADB."""

import json
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_marblenet_cpu_lab as cpu


def elf(root):
    return f"""Class: ELF64
Data: 2's complement, little endian
Machine: AArch64
Type: DYN
GNU_RELRO
GNU_STACK 0x0 0x0 0x0 0x0 0x0 RW 0x10
(FLAGS) BIND_NOW
(NEEDED) Shared library: [libc.so.6]
(NEEDED) Shared library: [libdl.so.2]
(RUNPATH) Library runpath: [{root}/lib:/usr/lib]
UND dlopen@GLIBC_2.17
UND dlsym@GLIBC_2.17
"""


def test_cpu_elf_allows_cpu_only_and_rejects_vendor(root=None):
    root = "/tmp/wave-npu-onnx18-" + "a" * 16 + "-execute"
    cpu.validate_cpu_elf(elf(root), root)
    for addition in [
        "(NEEDED) Shared library: [libGAL.so]",
        "UND gcoHAL_QueryChipIdentityEx",
        "UND vxCreateContext",
        "GLIBC_2.28",
    ]:
        with pytest.raises(cpu.CpuLabError):
            cpu.validate_cpu_elf(elf(root) + "\n" + addition, root)


def test_actual_fp32_readbacks_finite_and_numeric():
    reference = np.ones(400, dtype="<f4").tobytes()
    metric = cpu.compare_cpu_output(
        reference, np.full(400, 1.00001, dtype="<f4").tobytes()
    )
    assert metric["status"] == "PASS" and metric["max_abs_error"] > 0
    for observed in [
        bytes(1599),
        np.full(400, np.nan, dtype="<f4").tobytes(),
        np.zeros(400, dtype="<f4").tobytes(),
    ]:
        with pytest.raises(cpu.CpuLabError):
            cpu.compare_cpu_output(reference, observed)


def output_stdout(actual):
    samples = []
    for i in range(32):
        samples.append(
            "CPU_SAMPLE "
            + json.dumps(
                {
                    "sample_index": i,
                    "iteration": i + 4,
                    "boundary_ns": 1000,
                    "thread_ns": 500,
                    "input_copy_ns": 100,
                    "run_ns": 500,
                    "postprocess_ns": 100,
                    "speech_probability_sum": 100,
                }
            )
        )
    return "\n".join(
        [
            "CPU_IDENTITY ort_version=1.15.1 provider=CPUExecutionProvider threads=1",
            *samples,
            "CPU_EXECUTION status=PASS mode=execute-once operation=fp32-marblenet-cpu iterations=36 input_bytes=128000 output_bytes=1600 graph_us=18000 output_hex="
            + actual.hex()
            + " cleanup=0",
        ]
    )


def test_cpu_identity_is_actual_ort_and_not_chip_query():
    actual = np.ones(400, dtype="<f4").tobytes()
    result = cpu.parse_cpu_result(output_stdout(actual))
    assert result["identity"] == {
        "ort_version": "1.15.1",
        "provider": "CPUExecutionProvider",
        "threads": 1,
    }
    assert (
        "chip_model" not in result["identity"] and result["output_hex"] == actual.hex()
    )
    for change in [
        lambda s: s + s.splitlines()[0],
        lambda s: s.replace("threads=1", "threads=2"),
        lambda s: s.replace("iterations=36", "iterations=35"),
        lambda s: s.replace("CPU_EXECUTION", "EXECUTION"),
    ]:
        with pytest.raises(cpu.CpuLabError):
            cpu.parse_cpu_result(change(output_stdout(actual)))


def test_one_cpu_invocation_retains_actual_output_and_nonce(tmp_path, monkeypatch):
    import re
    from types import SimpleNamespace

    actual = np.ones(400, dtype="<f4").tobytes()
    calls = []

    class Transport:
        def run(self, args, timeout):
            calls.append((args, timeout))
            command = args[1]
            assert "VIVANTE" not in command and "VSI_NN" not in command
            nonce = re.search(r"__WAVE_EXEC_EXIT_[0-9a-f]{32}__", command)[0]
            return SimpleNamespace(
                returncode=0,
                timed_out=False,
                stdout=output_stdout(actual) + "\n" + nonce + ":0\n",
                stderr="",
            )

    manifest = SimpleNamespace(
        policy=SimpleNamespace(experiment="CPU", poc_id="18CPU"),
        sha256="a" * 64,
        environment={"LD_LIBRARY_PATH": "/tmp/cpu/lib:/usr/lib"},
        remote_executable="/tmp/cpu/bin/wave-npu-onnx-execute-v1",
    )
    monkeypatch.setattr(cpu.lab.execution, "DEFAULT_RUN_ROOT", tmp_path / "runs")
    monkeypatch.setattr(
        cpu.lab.execution, "collect_snapshot", lambda *a: {"errors": []}
    )
    monkeypatch.setattr(cpu.lab.execution, "verify_preflight", lambda *a: None)
    monkeypatch.setattr(
        cpu.lab.compile_lab, "align_native_diagnostics", lambda *a: None
    )
    monkeypatch.setattr(cpu.lab, "health", lambda *a: ([], []))
    result, directory = cpu._run_once(Transport(), manifest, "cpu-lifecycle-test")
    assert len(calls) == 1 and calls[0][1] == 12000
    assert result["status"] == "PASS" and result["transport"]["remote_exit"] == 0
    assert result["native_runs"] == 0 and result["result"]["output_hex"] == actual.hex()
    assert (directory / "preflight.json").is_file() and (
        directory / "postflight.json"
    ).is_file()
    assert (
        json.loads((directory / "run.json").read_text())["result"]["identity"]
        == cpu.CPU_IDENTITY
    )


@pytest.fixture
def admission(tmp_path, monkeypatch):
    from types import SimpleNamespace

    monkeypatch.setattr(cpu.lab, "REPOSITORY_ROOT", tmp_path)
    directory = tmp_path / "artifacts/npu/poc18/cpu"
    (directory / "build").mkdir(parents=True)
    record = {
        "source_model_sha256": cpu.baseline.FULL_MODEL_SHA256,
        "lowering_sha256": "a" * 64,
        "profile_id": cpu.lab.GRAPH_PROFILE,
    }
    package = {
        "record": record,
        "package_sha256": "b" * 64,
        "directory": str(tmp_path / "package"),
    }
    files = {
        "cpu.onnx": b"original CPU model",
        "cpu-input.bin": bytes(128000),
        "cpu-reference.bin": np.ones(400, dtype="<f4").tobytes(),
    }
    original = files["cpu-reference.bin"]
    remote = cpu.lab.remote_root(record, "execute")
    for name, payload in files.items():
        (directory / name).write_bytes(payload)
    (directory / "execute.cc").write_text("fixed CPU source")
    (directory / "build/wave-npu-onnx-execute-v1").write_bytes(b"fixed CPU binary")
    (directory / "build/execute.elf.txt").write_text(elf(remote))
    (directory / "build/execute.log").write_text(elf(remote))
    build = {
        "image_id": cpu.lab.IMAGE,
        "ort_header_sha256": cpu.baseline.HEADER_SHA256,
        "network": "none",
        "repository_mount": "read-only",
        **{
            key: cpu.lab._digest((directory / name).read_bytes())
            for name, key in [
                ("execute.cc", "source_sha256"),
                ("build/wave-npu-onnx-execute-v1", "binary_sha256"),
                ("build/execute.elf.txt", "inspection_sha256"),
                ("build/execute.log", "log_sha256"),
            ]
        },
    }
    receipt = {
        "schema_version": 1,
        "operation": "fp32-marblenet-cpu",
        "package_sha256": package["package_sha256"],
        "frozen_preparation_sha256": cpu.baseline.PREPARATION_SHA256,
        "full_model_sha256": cpu.baseline.FULL_MODEL_SHA256,
        "ort_library_sha256": cpu.baseline.ORT_SHA256,
        "host_ort_version": "1.27.0",
        "native_runs": 0,
        "cpu_iterations": 36,
        "warmup_iterations": 4,
        "measured_iterations": 32,
        "files": {
            name: {"sha256": cpu.lab._digest(payload), "bytes": len(payload)}
            for name, payload in files.items()
        },
        "host_original_vad": cpu._vad(original, original),
        "build": build,
    }
    cpu.lab._write_json(directory / "preparation.json", receipt)
    monkeypatch.setattr(cpu, "_baseline", lambda *a: (package, files, original))
    monkeypatch.setattr(
        cpu, "_source", lambda: SimpleNamespace(validate_cpu_source=lambda *a: None)
    )
    monkeypatch.setattr(
        cpu.lab.npu_lab,
        "AdbTransport",
        lambda: pytest.fail("transport constructed during admission"),
    )
    return directory, receipt, files


@pytest.mark.parametrize("changed", ["cpu.onnx", "cpu-input.bin", "cpu-reference.bin"])
def test_resealed_local_receipt_cannot_change_independent_baseline(admission, changed):
    directory, receipt, _ = admission
    payload = (directory / changed).read_bytes()
    payload = b"\x01" + payload[1:]
    (directory / changed).write_bytes(payload)
    receipt["files"][changed] = {
        "sha256": cpu.lab._digest(payload),
        "bytes": len(payload),
    }
    (directory / "preparation.json").write_bytes(cpu.lab._canonical(receipt))
    digest = cpu.lab._digest((directory / "preparation.json").read_bytes())
    with pytest.raises(cpu.CpuLabError):
        cpu.run_cpu_benchmark(directory, directory, directory, "cpu-tamper", digest)
    assert not (directory / "attempt.json").exists()


def test_changed_review_receipt_refuses_before_any_baseline_or_transport(
    admission, monkeypatch
):
    directory, receipt, _ = admission
    digest = cpu.lab._digest((directory / "preparation.json").read_bytes())
    receipt["build"]["binary_sha256"] = "0" * 64
    (directory / "preparation.json").write_bytes(cpu.lab._canonical(receipt))
    monkeypatch.setattr(
        cpu, "_baseline", lambda *a: pytest.fail("baseline ran for stale review hash")
    )
    with pytest.raises(cpu.CpuLabError):
        cpu.run_cpu_benchmark(directory, directory, directory, "cpu-tamper", digest)


@pytest.mark.parametrize("extra", ["attempt.json", "unknown.bin"])
def test_extra_file_or_prior_attempt_refuses_before_transport(admission, extra):
    directory, _, _ = admission
    digest = cpu.lab._digest((directory / "preparation.json").read_bytes())
    (directory / extra).write_bytes(b"keep")
    with pytest.raises(cpu.CpuLabError):
        cpu.run_cpu_benchmark(directory, directory, directory, "cpu-tamper", digest)


def test_cpu_manifest_only_stages_four_cpu_payloads(admission):
    directory, receipt, _ = admission
    package, _, _ = cpu._baseline(None, None)
    manifest, payloads = cpu._manifest(package, directory, receipt)
    assert len(payloads) == 4 and not any(
        "model.nbg" in p or "libtim" in p or "libGAL" in p for p in payloads
    )
    assert manifest.expected_result["identity"] == cpu.CPU_IDENTITY
    assert set(manifest.environment) == {"LD_LIBRARY_PATH"}
    assert manifest.required_artifacts[cpu.baseline.ORT_PATH] == cpu.baseline.ORT_SHA256
    assert manifest.policy.remote_nbg.endswith("/model/cpu.onnx")


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", True),
        ("native_runs", 1),
        ("cpu_iterations", 37),
        ("warmup_iterations", 3),
        ("measured_iterations", 31),
    ],
)
def test_resealed_cpu_iteration_metadata_refuses(admission, field, value):
    directory, receipt, _ = admission
    receipt[field] = value
    (directory / "preparation.json").write_bytes(cpu.lab._canonical(receipt))
    digest = cpu.lab._digest((directory / "preparation.json").read_bytes())
    with pytest.raises(cpu.CpuLabError):
        cpu._admit(directory, directory, directory, digest)
