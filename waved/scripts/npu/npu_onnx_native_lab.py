#!/usr/bin/env python3
"""Hash-bound ONNX native packages and bounded Wave compile/execution."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
from contextvars import ContextVar
from dataclasses import replace
from functools import wraps
from pathlib import Path
from typing import Any

import nbg_format
import npu_compile_lab as compile_lab
import npu_erf_execution_lab as execution
import npu_lab
import npu_native_phase
import npu_op_dtype_lab
import npu_op_dtype_stage as stage_tools

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
SOURCE_ROOT = REPOSITORY_ROOT / "waved/tools/onnx-hybrid/src"
RUNTIME = REPOSITORY_ROOT / "artifacts/npu/build/poc16/runtime/libtim-vx.so"
WRAPPER_SHA256 = "c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc"
RUNTIME_PROVENANCE_SHA256 = (
    "58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade"
)
IMAGE = "sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a"
TIM_COMMIT = "702715d714277376d7ff466b7b72833ff57564c1"
ROOT_RE = re.compile(
    r"/tmp/wave-npu-onnx(?:17|18|20)-[0-9a-f]{16}-(?:compile|execute)\Z"
)
GRAPH_PROFILE = "fp16-marblenet-v1"
VISION_PROFILE = "fp16-squeezenet-v1"
GRAPH_PROFILES = frozenset({GRAPH_PROFILE, "fp16-marblenet-pad-v1", VISION_PROFILE})
PHASE_PROFILES = frozenset({"fp16-marblenet-pad-v1", VISION_PROFILE})
MAXIMUM_GRAPH_TENSOR_BYTES = 8 * 1024 * 1024
SYSTEM_HASHES = {
    path: digest
    for path, digest in json.loads(execution.POC7_POLICY.manifest_path.read_text())[
        "required_artifacts"
    ].items()
    if not path.startswith("/tmp/")
}
DEVICE = json.loads(npu_op_dtype_lab.POLICIES["fp16-conv"].manifest_path.read_text())[
    "device"
]


class NativeLabError(Exception):
    """A local package, transport, native result or health gate failed."""


class CleanCompileNegative(NativeLabError):
    """One canonical unsupported compile, with verified healthy cleanup."""

    def __init__(self, message, cleanup):
        super().__init__(message)
        self.cleanup = cleanup


_ACTIVE_PHASE = ContextVar("onnx_native_phase", default=None)


def _phase_guarded(function):
    @wraps(function)
    def guarded(package_dir, *args, **kwargs):
        if _ACTIVE_PHASE.get() is not None:
            return function(package_dir, *args, **kwargs)
        token = _ACTIVE_PHASE.set(None)
        try:
            result = function(package_dir, *args, **kwargs)
            phase = _ACTIVE_PHASE.get()
            if phase is not None:
                phase.advance(phase.pending_cleanup)
            return result
        except CleanCompileNegative as exc:
            phase = _ACTIVE_PHASE.get()
            if phase is not None:
                phase.advance(exc.cleanup)
            raise
        except Exception as exc:
            phase = _ACTIVE_PHASE.get()
            if phase is not None:
                phase.halt(str(exc))
            raise
        finally:
            _ACTIVE_PHASE.reset(token)

    return guarded


def _activate_phase(package):
    if package.get("record", {}).get("profile_id") not in PHASE_PROFILES:
        return
    path = Path(package["directory"]).parent / "native-phase.json"
    current = _ACTIVE_PHASE.get()
    if current is not None:
        if current.path != path:
            raise NativeLabError(
                "nested native operation changed the shared phase path"
            )
        return
    manifest = _system_manifest(_compile_manifest(package))
    _ACTIVE_PHASE.set(npu_native_phase.NativePhase(path, manifest, SYSTEM_HASHES))


def _phase_runner_kwargs(error_type):
    phase = _ACTIVE_PHASE.get()
    return (
        {}
        if phase is None
        else {
            "preflight_validator": phase.callback(
                error_type, "immediate-native-preflight"
            )
        }
    )


def _host():
    if str(SOURCE_ROOT) not in sys.path:
        sys.path.insert(0, str(SOURCE_ROOT))
    from wave_onnx_hybrid import native_import, native_sources

    return native_import, native_sources


def _canonical(value: Any) -> bytes:
    return (
        json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + "\n"
    ).encode()


def _digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _regular(path: Path, maximum: int = 64 * 1024 * 1024) -> bytes:
    if not path.is_file() or path.is_symlink() or path.stat().st_size > maximum:
        raise NativeLabError(
            f"missing, linked, oversized or nonregular artifact: {path}"
        )
    with path.open("rb") as handle:
        data = handle.read(maximum + 1)
    if len(data) > maximum:
        raise NativeLabError(f"artifact exceeds limit: {path}")
    return data


def _json(path: Path) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise NativeLabError(f"duplicate key in {path}: {key}")
            result[key] = value
        return result

    try:
        document = json.loads(_regular(path), object_pairs_hook=pairs)
    except (ValueError, UnicodeError, OSError) as error:
        raise NativeLabError(f"invalid JSON artifact: {path}") from error
    if not isinstance(document, dict):
        raise NativeLabError(f"JSON artifact must be an object: {path}")
    return document


def _write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as handle:
        handle.write(payload)


def _write_json(path: Path, value: Any) -> None:
    _write(path, _canonical(value))


def _poc_id(record: dict) -> str:
    if record.get("profile_id") == VISION_PROFILE:
        return "20"
    return "18" if record.get("profile_id") in GRAPH_PROFILES else "17"


def remote_root(record: dict, kind: str) -> str:
    digest = record.get("lowering_sha256", "")
    if kind not in ("compile", "execute") or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise NativeLabError("invalid native root identity")
    poc = _poc_id(record)
    return f"/tmp/wave-npu-onnx{poc}-{digest[:16]}-{kind}"


def _tensor_bytes(record: dict, key: str) -> int:
    size = record[key]["byte_size"]
    maximum = (
        MAXIMUM_GRAPH_TENSOR_BYTES if record.get("profile_id") in GRAPH_PROFILES else 32
    )
    if type(size) is not int or not 0 < size <= maximum:
        raise NativeLabError("native tensor exceeds its profile byte bound")
    return size


def _compare_output(record: dict, expected: bytes, observed: bytes) -> dict:
    if len(expected) != _tensor_bytes(record, "onnx_output") or len(observed) != len(
        expected
    ):
        raise NativeLabError("native output length differs from its tensor contract")
    if record.get("profile_id") in GRAPH_PROFILES:
        _host()
        if record.get("profile_id") == VISION_PROFILE:
            from wave_onnx_hybrid.vision_graph import validate_graph_output
        else:
            from wave_onnx_hybrid.marblenet_graph import validate_graph_output

        try:
            return validate_graph_output(record, expected, observed)
        except Exception as error:
            raise NativeLabError(
                f"native numerical qualification failed: {error}"
            ) from error
    if expected != observed:
        raise NativeLabError("native output differs from byte-exact CPU oracle")
    return {
        "status": "PASS",
        "comparison": "byte-exact",
        "reference_sha256": _digest(expected),
        "observed_sha256": _digest(observed),
    }


def import_package(model_path: Path | str, package_dir: Path | str) -> dict:
    importer, sources = _host()
    model = Path(model_path)
    payload = _regular(model)
    record = importer.import_native_model(model)
    if _digest(payload) != record["source_model_sha256"]:
        raise NativeLabError("model changed during import")
    root = Path(package_dir)
    if root.exists() or root.is_symlink():
        raise NativeLabError("package path already exists")
    root.mkdir(parents=True)
    _write(root / "model.onnx", payload)
    _write_json(root / "lowering.json", record)
    source = sources.generate_compile_source(record, remote_root(record, "compile"))
    _write(root / "compile.cc", source.encode())
    package = {
        "schema_version": 1,
        "model_sha256": record["source_model_sha256"],
        "lowering_sha256": record["lowering_sha256"],
        "files": {
            name: _digest(_regular(root / name))
            for name in ("model.onnx", "lowering.json", "compile.cc")
        },
    }
    _write_json(root / "package.json", package)
    return load_package(root)


def load_package(package_dir: Path | str, require_qualification: bool = False) -> dict:
    root = Path(package_dir)
    if not root.is_dir() or root.is_symlink():
        raise NativeLabError("package directory is missing or linked")
    root = root.resolve()
    package = _json(root / "package.json")
    if (
        set(package) != {"schema_version", "model_sha256", "lowering_sha256", "files"}
        or package["schema_version"] != 1
    ):
        raise NativeLabError("package schema differs from native policy")
    if not isinstance(package["files"], dict) or set(package["files"]) != {
        "model.onnx",
        "lowering.json",
        "compile.cc",
    }:
        raise NativeLabError("package artifact set differs from native policy")
    for name, digest in package["files"].items():
        if (
            not isinstance(digest, str)
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or _digest(_regular(root / name)) != digest
        ):
            raise NativeLabError(f"package artifact changed: {name}")
    importer, sources = _host()
    try:
        record = importer.import_native_model(root / "model.onnx")
        if record != _json(root / "lowering.json"):
            raise NativeLabError("stored lowering differs from imported ONNX")
        sources.validate_compile_source(
            record,
            remote_root(record, "compile"),
            _regular(root / "compile.cc").decode(),
        )
    except NativeLabError:
        raise
    except Exception as error:
        raise NativeLabError(
            f"native package import/source validation failed: {error}"
        ) from error
    if (
        record["source_model_sha256"] != package["model_sha256"]
        or record["lowering_sha256"] != package["lowering_sha256"]
    ):
        raise NativeLabError("package model/lowering identity mismatch")
    loaded = {
        "directory": str(root),
        "record": record,
        "package": package,
        "package_sha256": _digest(_regular(root / "package.json")),
        "build": None,
        "compiled": None,
        "qualification": None,
    }
    if (root / "compile-build.json").exists():
        loaded["build"] = _load_build(loaded, "compile")
    if (root / "compiled.json").exists():
        compiled = _json(root / "compiled.json")
        _bound(loaded, compiled)
        if compiled.get("status") != "PASS" or compiled.get("nn_target") not in (
            {"VXNNE_OPERATION_TARGET_NN", "VXNNE_OPERATION_TARGET_TP"}
            if record["profile_id"] == VISION_PROFILE and record["control_id"] == "20b"
            else {"VXNNE_OPERATION_TARGET_NN"}
        ):
            raise NativeLabError("native compile evidence is not a qualified NN export")
        if (
            not loaded["build"]
            or compiled.get("compiler_sha256") != loaded["build"]["binary_sha256"]
        ):
            raise NativeLabError("compiler differs from compiled artifact provenance")
        if _digest(_regular(root / "model.nbg")) != compiled.get("nbg_sha256"):
            raise NativeLabError("native NBG changed")
        report = nbg_format.parse_bytes(
            _regular(root / "model.nbg"), display_path="model.nbg"
        )
        compile_lab.validate_nbg_report(report, _compile_manifest(loaded))
        if report["size"] != compiled.get("nbg_bytes"):
            raise NativeLabError("NBG size differs from compile ledger")
        run = _json(root / "evidence/compile/run.json")
        _successful_run(run)
        manifest = _compile_manifest(loaded)
        if (
            run.get("manifest_sha256") != manifest.sha256
            or compiled.get("manifest_sha256") != manifest.sha256
            or run.get("run_id") != compiled.get("run_id")
        ):
            raise NativeLabError("compile run/manifest identity differs")
        if _digest(_regular(root / "evidence/compile/run.json")) != compiled.get(
            "run_sha256"
        ):
            raise NativeLabError("compile run evidence changed")
        _verify_evidence_files(root / "evidence/compile", compiled)
        compile_lab.verify_preflight(
            manifest,
            manifest.candidates["candidate-1"],
            _json(root / "evidence/compile/preflight.json"),
        )
        artifact = run.get("candidate_artifact") or {}
        if (
            artifact.get("sha256") != compiled["nbg_sha256"]
            or artifact.get("bytes") != compiled["nbg_bytes"]
        ):
            raise NativeLabError("compile envelope does not bind this NBG")
        stdout = _regular(root / "evidence/compile/stdout.txt").decode()
        if _digest(stdout.encode()) != compiled.get(
            "stdout_sha256"
        ) or _qualified_target(record, stdout) != compiled.get("nn_target"):
            raise NativeLabError("NN target evidence changed or absent")
        cleanup = _json(root / "evidence/compile/cleanup.json")
        if (
            cleanup.get("status") != "CLEAN"
            or cleanup.get("health_differences")
            or cleanup.get("kernel_alerts")
        ):
            raise NativeLabError("compile cleanup evidence is not clean")
        loaded["compiled"] = compiled
    if (root / "qualification.json").exists():
        qualification = _json(root / "qualification.json")
        _bound(loaded, qualification)
        if qualification.get("status") != "PASS" or not loaded["compiled"]:
            raise NativeLabError("native model has no successful qualification")
        if qualification.get("nbg_sha256") != loaded["compiled"]["nbg_sha256"]:
            raise NativeLabError("qualified NBG differs from compiler output")
        build = _load_build(loaded, "execute")
        if build["binary_sha256"] != qualification.get("executor_sha256"):
            raise NativeLabError("qualified executor changed")
        run = _json(root / "evidence/qualification/run.json")
        _successful_run(run)
        if run.get("execution_attempts") != 1 or _digest(
            _regular(root / "evidence/qualification/run.json")
        ) != qualification.get("run_sha256"):
            raise NativeLabError("qualification execution evidence changed")
        expected = bytes.fromhex(qualification["output_hex"])
        input_bytes = bytes.fromhex(qualification["input_hex"])
        if (
            bytes(run["result"]["output"]) != expected
            or run["result"].get("iterations") != 1
            or bytes(run["result"]["input"]) != input_bytes
            or run.get("run_id") != qualification.get("run_id")
            or qualification.get("input_sha256") != _digest(input_bytes)
            or qualification.get("output_sha256") != _digest(expected)
        ):
            raise NativeLabError("qualification oracle and execution differ")
        _verify_evidence_files(root / "evidence/qualification", qualification)
        if compile_lab.RUN_ID_RE.fullmatch(qualification["run_id"]) is None:
            raise NativeLabError("qualification run ID is invalid")
        stage_dir = root / "evidence" / (qualification["run_id"] + "-stage")
        if (
            _regular(stage_dir / "input.bin", _tensor_bytes(record, "onnx_input"))
            != input_bytes
        ):
            raise NativeLabError("qualification input file changed")
        import numpy as np

        spec = record["onnx_input"]
        dtype = "<f2" if spec["dtype"] == "FLOAT16" else "u1"
        if len(input_bytes) != spec["byte_size"]:
            raise NativeLabError("qualification input size mismatch")
        inputs = {
            spec["name"]: np.frombuffer(input_bytes, dtype=dtype)
            .reshape(spec["shape"])
            .copy()
        }
        _, oracle = _cpu_expected(loaded, inputs, verify_onnx_runtime=False)
        _compare_output(record, oracle, expected)
        manifest_reference = expected
        cpu_output_digest = _digest(expected)
        if record["profile_id"] in GRAPH_PROFILES:
            independent_bytes = _regular(
                root / "qualification-independent-output.bin",
                _tensor_bytes(record, "onnx_output"),
            )
            cpu_bytes = _regular(
                root / "qualification-cpu-output.bin",
                _tensor_bytes(record, "onnx_output"),
            )
            if qualification.get("independent_output_sha256") != _digest(
                independent_bytes
            ) or qualification.get("cpu_output_sha256") != _digest(cpu_bytes):
                raise NativeLabError("retained numerical reference bytes changed")
            _compare_output(record, oracle, independent_bytes)
            _compare_output(record, independent_bytes, cpu_bytes)
            comparison = _compare_output(record, independent_bytes, expected)
            comparison_path = root / "evidence/qualification/numerical-comparison.json"
            if (
                qualification.get("comparison_sha256")
                != _digest(_regular(comparison_path))
                or _json(comparison_path) != comparison
            ):
                raise NativeLabError("retained native numerical comparison changed")
            manifest_reference = independent_bytes
            cpu_output_digest = _digest(cpu_bytes)
        manifest = _execute_manifest(
            loaded, stage_dir / "input.bin", manifest_reference, stage_dir, create=False
        )
        if (
            run.get("manifest_sha256") != manifest.sha256
            or qualification.get("manifest_sha256") != manifest.sha256
        ):
            raise NativeLabError("qualification execution manifest differs")
        execution.verify_preflight(
            manifest, _json(root / "evidence/qualification/preflight.json")
        )
        identity = _json(root / "evidence/qualification/native-identity.json")
        expected_identity = {
            "package_sha256": loaded["package_sha256"],
            "source_model_sha256": record["source_model_sha256"],
            "lowering_sha256": record["lowering_sha256"],
            "nbg_sha256": loaded["compiled"]["nbg_sha256"],
            "input_sha256": _digest(input_bytes),
            "output_sha256": _digest(expected),
        }
        if identity != expected_identity:
            raise NativeLabError("qualification native identity differs")
        cpu_evidence_path = root / "qualification-cpu-oracle.json"
        cpu_evidence = _json(cpu_evidence_path)
        if (
            qualification.get("cpu_evidence_sha256")
            != _digest(_regular(cpu_evidence_path))
            or cpu_evidence.get("model_sha256") != record["source_model_sha256"]
            or cpu_evidence.get("graph_signature") != record["graph_signature"]
            or cpu_evidence.get("providers") != ["CPUExecutionProvider"]
            or cpu_evidence.get("inputs", {}).get(spec["name"], {}).get("sha256")
            != _digest(input_bytes)
            or cpu_evidence.get("outputs", {})
            .get(record["onnx_output"]["name"], {})
            .get("sha256")
            != cpu_output_digest
        ):
            raise NativeLabError("retained ONNX CPU oracle evidence differs")
        if _digest(_regular(root / "evidence/qualification/output.bin")) != _digest(
            expected
        ):
            raise NativeLabError("qualified readback changed")
        cleanup = _json(root / "evidence/qualification/cleanup.json")
        if (
            cleanup.get("status") != "CLEAN"
            or cleanup.get("health_differences")
            or cleanup.get("kernel_alerts")
        ):
            raise NativeLabError("qualification cleanup evidence is not clean")
        loaded["qualification"] = qualification
    if require_qualification and loaded["qualification"] is None:
        raise NativeLabError("ONNX model has no exact native qualification")
    return loaded


def _bound(package: dict, document: dict) -> None:
    if any(
        document.get(key) != value
        for key, value in {
            "model_sha256": package["record"]["source_model_sha256"],
            "lowering_sha256": package["record"]["lowering_sha256"],
            "package_sha256": package["package_sha256"],
        }.items()
    ):
        raise NativeLabError("evidence differs from native package identity")


def _evidence_hashes(directory: Path) -> dict:
    return {
        name + "_sha256": _digest(_regular(directory / (name + ".json")))
        for name in ("preflight", "postflight", "cleanup")
    }


def _verify_evidence_files(directory: Path, ledger: dict) -> None:
    if any(
        ledger.get(key) != digest for key, digest in _evidence_hashes(directory).items()
    ):
        raise NativeLabError("native health/cleanup evidence changed")


def _identity(package: dict) -> dict:
    return {
        "model_sha256": package["record"]["source_model_sha256"],
        "lowering_sha256": package["record"]["lowering_sha256"],
        "package_sha256": package["package_sha256"],
    }


def _successful_run(envelope: dict) -> None:
    if (
        envelope.get("status") != "PASS"
        or envelope.get("errors")
        or envelope.get("health_differences")
        or envelope.get("kernel_alerts")
        or envelope.get("transport", {}).get("remote_exit") != 0
        or envelope.get("transport", {}).get("timed_out")
    ):
        raise NativeLabError("native run is incomplete, unsuccessful or unhealthy")


def _verify_runtime() -> None:
    if _digest(_regular(RUNTIME)) != WRAPPER_SHA256:
        raise NativeLabError("exact native wrapper is unavailable")
    sdk = REPOSITORY_ROOT / "waved/third_party/device-sdk"
    provenance_path = (
        REPOSITORY_ROOT / "artifacts/npu/build/poc16/runtime-provenance.json"
    )
    if _digest(_regular(provenance_path)) != RUNTIME_PROVENANCE_SHA256:
        raise NativeLabError("exact native runtime provenance changed")
    provenance = _json(provenance_path)
    original = _json(REPOSITORY_ROOT / "artifacts/npu/build/poc16/runtime-inputs.json")
    input_document = json.dumps(
        {"source": original["source_files"], "sdk": original["sdk_files"]},
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    if _digest(input_document) != provenance["input_manifest_sha256"]:
        raise NativeLabError("exact source/SDK input manifest changed")
    for name, expected in original["sdk_files"].items():
        path = sdk / name
        observed = (
            {"kind": "symlink", "target": os.readlink(path)}
            if path.is_symlink()
            else {
                "kind": "file",
                "bytes": path.stat().st_size,
                "sha256": _digest(_regular(path)),
            }
        )
        if observed != expected:
            raise NativeLabError(f"device SDK changed: {name}")
    source = REPOSITORY_ROOT / "waved/third_party/TIM-VX"
    if (
        subprocess.check_output(
            ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
        ).strip()
        != TIM_COMMIT
    ):
        raise NativeLabError("TIM-VX source commit changed")
    if subprocess.check_output(
        ["git", "-C", str(source), "status", "--porcelain"], text=True
    ).strip():
        raise NativeLabError("TIM-VX source has uncommitted changes")


def validate_elf(report: str, root: str) -> None:
    patterns = (
        r"Class:\s+ELF64",
        r"Data:\s+2's complement, little endian",
        r"Machine:\s+AArch64",
        r"Type:\s+DYN",
        r"GNU_RELRO",
        r"BIND_NOW",
        r"\bgcoHAL_QueryChipIdentityEx\b",
    )
    if not ROOT_RE.fullmatch(root) or any(
        not re.search(pattern, report) for pattern in patterns
    ):
        raise NativeLabError("ELF architecture or protection/identity policy failed")
    stack = next((line for line in report.splitlines() if "GNU_STACK" in line), "")
    if (
        not re.search(r"\sRW\s", stack)
        or "RWE" in stack
        or "(TEXTREL)" in report
        or "(RPATH)" in report
    ):
        raise NativeLabError("ELF stack/relocation policy failed")
    needed = set(re.findall(r"\(NEEDED\)\s+Shared library: \[([^]]+)\]", report))
    allowed = {
        "libtim-vx.so",
        "libGAL.so",
        "libstdc++.so.6",
        "libgcc_s.so.1",
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "ld-linux-aarch64.so.1",
    }
    if not {"libtim-vx.so", "libGAL.so", "libc.so.6"} <= needed <= allowed:
        raise NativeLabError("ELF dependency policy failed")
    if re.findall(r"\(RUNPATH\)\s+Library runpath: \[([^]]+)\]", report) != [
        root + "/lib:/usr/lib"
    ]:
        raise NativeLabError("ELF RUNPATH differs from fixed native root")
    limits = {
        "GLIBC": (2, 17),
        "GLIBCXX": (3, 4, 21),
        "CXXABI": (1, 3, 9),
        "GCC": (3, 0),
    }
    for namespace, version in re.findall(
        r"\b(GLIBCXX|GLIBC|CXXABI|GCC)_([0-9.]+)", report
    ):
        if tuple(map(int, version.split("."))) > limits[namespace]:
            raise NativeLabError(
                f"ELF ABI floor exceeds device baseline: {namespace}_{version}"
            )


def _build(package: dict, kind: str) -> dict:
    _verify_runtime()
    root = Path(package["directory"])
    output = root / "build"
    output.mkdir(exist_ok=True)
    binary = output / f"wave-npu-onnx-{kind}-v1"
    source = root / ("compile.cc" if kind == "compile" else "execute.cc")
    remote = remote_root(package["record"], kind)
    for path in (
        binary,
        output / f"{kind}.log",
        output / f"{kind}.elf.txt",
        root / f"{kind}-build.json",
    ):
        if path.exists() or path.is_symlink():
            raise NativeLabError(f"build output already exists: {path}")
    script = """export LC_ALL=C SOURCE_DATE_EPOCH=1785196800
aarch64-linux-gnu-g++ -std=c++17 -O2 -g0 -Wall -Wextra -Werror -Wformat=2 -Wshadow \
 -D_FORTIFY_SOURCE=2 -fPIE -fstack-protector-strong \
 -I /work/waved/third_party/TIM-VX/include "$1" \
 -L /work/artifacts/npu/build/poc16/runtime -L /work/waved/third_party/device-sdk/lib \
 -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib -Wl,--allow-shlib-undefined \
 -Wl,--as-needed -Wl,--build-id=sha1 -Wl,--enable-new-dtags "-Wl,-rpath,$3/lib:/usr/lib" \
 -Wl,-z,noexecstack -Wl,-z,now -Wl,-z,relro -pie -ltim-vx -lGAL -ldl -lpthread -o "$2"
aarch64-linux-gnu-readelf -W -h -l -d -V "$2"
aarch64-linux-gnu-nm -D --undefined-only "$2" | c++filt
"""
    command = [
        "docker",
        "run",
        "--rm",
        "--pull=never",
        "--network=none",
        "--platform=linux/amd64",
        "--volume",
        f"{REPOSITORY_ROOT}:/work:ro",
        "--volume",
        f"{root}:/package:ro",
        "--volume",
        f"{output}:/output",
        "--workdir",
        "/work",
        IMAGE,
        "bash",
        "-euo",
        "pipefail",
        "-c",
        script,
        "bash",
        "/package/" + source.name,
        "/output/" + binary.name,
        remote,
    ]
    result = subprocess.run(command, capture_output=True, text=True, timeout=90)
    _write(output / f"{kind}.log", (result.stdout + result.stderr).encode())
    if result.returncode:
        raise NativeLabError(
            f"offline native build failed ({result.returncode}); retained {output / (kind + '.log')}"
        )
    _write(output / f"{kind}.elf.txt", result.stdout.encode())
    validate_elf(result.stdout, remote)
    _verify_runtime()
    build = {
        **_identity(package),
        "schema_version": 1,
        "kind": kind,
        "image_id": IMAGE,
        "tim_vx_commit": TIM_COMMIT,
        "wrapper_sha256": WRAPPER_SHA256,
        "source_sha256": _digest(_regular(source)),
        "binary_sha256": _digest(_regular(binary)),
        "inspection_sha256": _digest(_regular(output / f"{kind}.elf.txt")),
        "log_sha256": _digest(_regular(output / f"{kind}.log")),
        "network": "none",
        "source_sdk_mount": "read-only",
    }
    _write_json(root / f"{kind}-build.json", build)
    return build


def _load_build(package: dict, kind: str) -> dict:
    root = Path(package["directory"])
    build = _json(root / f"{kind}-build.json")
    _bound(package, build)
    expected = {
        "kind": kind,
        "image_id": IMAGE,
        "tim_vx_commit": TIM_COMMIT,
        "wrapper_sha256": WRAPPER_SHA256,
        "network": "none",
        "source_sdk_mount": "read-only",
    }
    if any(build.get(key) != value for key, value in expected.items()):
        raise NativeLabError("native build provenance differs from fixed policy")
    source = root / ("compile.cc" if kind == "compile" else "execute.cc")
    for path, key in (
        (source, "source_sha256"),
        (root / f"build/wave-npu-onnx-{kind}-v1", "binary_sha256"),
        (root / f"build/{kind}.elf.txt", "inspection_sha256"),
        (root / f"build/{kind}.log", "log_sha256"),
    ):
        if _digest(_regular(path)) != build.get(key):
            raise NativeLabError(f"native build artifact changed: {path.name}")
    validate_elf(
        _regular(root / f"build/{kind}.elf.txt").decode(),
        remote_root(package["record"], kind),
    )
    if kind == "execute":
        _, sources = _host()
        sources.validate_execute_source(
            package["record"],
            remote_root(package["record"], kind),
            package["compiled"]["nbg_bytes"],
            _regular(source).decode(),
        )
    return build


def build_package(package_dir: Path | str) -> dict:
    package = load_package(package_dir)
    _build(package, "compile")
    return load_package(package_dir)


def _tensor_expectation(tensor: dict) -> compile_lab.TensorExpectation:
    quantized = tensor["dtype"] == "UINT8"
    return compile_lab.TensorExpectation(
        tuple(tensor["shape"]),
        "uint8" if quantized else "fp16",
        tensor["byte_size"],
        ("affine-asymmetric",) if quantized else ("none",),
        1.0 if quantized else None,
        0 if quantized else None,
    )


def _compile_policy(package: dict) -> npu_op_dtype_lab.DiagnosticCompilePolicy:
    record = package["record"]
    root = remote_root(record, "compile")
    poc = _poc_id(record)
    return npu_op_dtype_lab.DiagnosticCompilePolicy(
        poc_id=poc,
        experiment=f"wave-npu-poc{poc}-onnx-compile",
        operation="OnnxNative",
        manifest_path=Path(package["directory"]) / "compile-manifest.json",
        remote_executable=root + "/bin/wave-npu-onnx-compile-v1",
        candidates={
            "candidate-1": {
                "argument": "--candidate-1",
                "output": root + "/output/model.nbg",
                "partial_output": root + "/output/.model.nbg.partial",
            }
        },
        inputs=(_tensor_expectation(record["native_input"]),),
        outputs=(_tensor_expectation(record["native_output"]),),
        selector_backend="OPENVX" if record["profile_id"] == "fp16-conv-v1" else None,
        selector_kernel="conv2d" if record["profile_id"] == "fp16-conv-v1" else None,
        absent_artifacts=(
            "/lib/libCLC.so",
            "/usr/lib/libCLC.so",
            "/usr/lib64/libCLC.so",
            root + "/lib/libCLC.so",
        ),
    )


def _compile_manifest(package: dict) -> compile_lab.CompileManifest:
    policy = _compile_policy(package)
    root = policy.remote_root
    build = package["build"]
    if not build:
        raise NativeLabError("package has no inspected native compiler")
    header = REPOSITORY_ROOT / "waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h"
    document = {
        "schema_version": 1,
        "device": DEVICE,
        "environment": policy.expected_environment,
        "remote_executable": policy.remote_executable,
        "timeout_ms": 20000,
        "required_artifacts": {
            **SYSTEM_HASHES,
            policy.remote_executable: build["binary_sha256"],
            root + "/lib/libtim-vx.so": WRAPPER_SHA256,
            root + "/sdk/include/CL/cl_viv_vx_ext.h": _digest(_regular(header)),
        },
        "candidates": dict(policy.candidates),
        "expected_nbg": {
            "magic": "VPMN",
            "minimum_bytes": 513,
            "maximum_bytes": 67108864,
            "version": "0x00020000",
            "product": "0x1000000e",
            "chip_model": "0x00009000",
        },
    }
    if policy.manifest_path.exists():
        if _json(policy.manifest_path) != document:
            raise NativeLabError("compiler manifest differs from package")
    else:
        _write_json(policy.manifest_path, document)
    return compile_lab.load_manifest(policy=policy)


def health(manifest, before: dict, after: dict) -> tuple[list, list]:
    differences, alerts = compile_lab.compare_health(manifest, before, after)

    def services(snapshot):
        return sorted(
            re.sub(r"\s+", " ", line.strip())
            for line in snapshot.get("processes", [])
            if re.search(r"\bwaved\b|S60waved|audioserver|pspapp", line)
        )

    if (
        services(before) != services(after)
        and "service process identities changed" not in differences
    ):
        differences.append("service process identities changed")
    return differences, alerts


def expected_inventory(root: str, payloads: dict) -> dict:
    if not ROOT_RE.fullmatch(root):
        raise NativeLabError("unapproved staging root")
    paths = {root, root + "/output"}
    inventory = {}
    for remote, (_, mode) in payloads.items():
        if not remote.startswith(root + "/") or ".." in Path(remote).parts:
            raise NativeLabError("staged payload path outside fixed root")
        path = Path(remote).parent
        while str(path) != root:
            paths.add(str(path))
            path = path.parent
        inventory[remote] = "F " + {"0555": "-r-xr-xr-x", "0444": "-r--r--r--"}[mode]
    inventory.update({path: "D drwxr-xr-x" for path in paths})
    return inventory


def validate_inventory(expected: dict, observed: dict) -> None:
    if observed != expected:
        raise NativeLabError(
            "staged inventory differs: unexpected file, directory, mode or link"
        )


def _adb(manifest, *args: str) -> str:
    return stage_tools._adb(manifest, *args)


def _inventory(manifest, root: str) -> dict:
    text = _adb(
        manifest,
        "shell",
        f"find {shlex.quote(root)} -print | sort | while IFS= read -r wave_path; do "
        'metadata=$(ls -ldn "$wave_path") || exit 23; permissions=${metadata%% *}; '
        'if [ -L "$wave_path" ]; then kind=L; elif [ -d "$wave_path" ]; then kind=D; '
        'elif [ -f "$wave_path" ]; then kind=F; else kind=X; fi; '
        'printf "%s %s %s\\n" "$kind" "$permissions" "$wave_path"; done',
    )
    result = {}
    for line in text.splitlines():
        match = re.fullmatch(r"([DFLX]) ([^\s]+) (.+)", line)
        if not match or match[3] in result:
            raise NativeLabError("invalid staged inventory response")
        result[match[3]] = match[1] + " " + match[2]
    return result


def _system_manifest(manifest):
    return replace(
        manifest,
        required_artifacts={
            path: digest
            for path, digest in manifest.required_artifacts.items()
            if not path.startswith("/tmp/")
        },
    )


def _snapshot(transport, manifest, execute: bool = False) -> dict:
    snapshot = (
        execution.collect_snapshot(transport, manifest)
        if execute
        else compile_lab.collect_snapshot(transport, manifest)
    )
    return snapshot


def _stage(
    manifest,
    payloads: dict,
    root: str,
    run_dir: Path,
    execute: bool = False,
    *,
    phase=None,
) -> dict:
    _verify_runtime()
    for remote, (local, _) in payloads.items():
        if _digest(_regular(Path(local))) != manifest.required_artifacts.get(remote):
            raise NativeLabError(
                "local stage payload hash differs before device contact"
            )
    baseline = _system_manifest(manifest)
    transport = npu_lab.AdbTransport()
    before = compile_lab.collect_snapshot(transport, baseline)
    _write_json(run_dir / "stage-before.json", before)
    phase = phase or _ACTIVE_PHASE.get()
    if phase is not None:
        phase.check(before, "before-staging-mutations")
    candidate = next(iter(baseline.candidates.values()))
    compile_lab.verify_preflight(baseline, candidate, before)
    if (
        _adb(
            manifest,
            "shell",
            f"if [ -e {root} ] || [ -L {root} ]; then printf PRESENT; else printf ABSENT; fi",
        )
        != "ABSENT"
    ):
        raise NativeLabError("staging root already exists")
    if _adb(
        manifest, "shell", "ps | grep -E '[w]ave-npu-onnx-(compile|execute)-v1' || true"
    ):
        raise NativeLabError("a native ONNX process is already present")
    inventory = expected_inventory(root, payloads)
    directories = sorted(
        (path for path, mode in inventory.items() if mode.startswith("D ")),
        key=lambda path: (len(Path(path).parts), path),
    )
    _adb(
        manifest,
        "shell",
        "umask 022; "
        + " && ".join("mkdir " + shlex.quote(path) for path in directories),
    )
    for remote, (local, mode) in payloads.items():
        incoming = remote + ".incoming"
        _adb(manifest, "push", str(local), incoming)
        q_incoming, q_remote = shlex.quote(incoming), shlex.quote(remote)
        observed = _adb(
            manifest,
            "shell",
            f"test ! -L {q_incoming} && test -f {q_incoming} && sha256sum {q_incoming}",
        ).split()
        if not observed or observed[0] != manifest.required_artifacts[remote]:
            raise NativeLabError("pushed payload hash mismatch")
        _adb(
            manifest,
            "shell",
            f"chmod {mode} {q_incoming} && ln {q_incoming} {q_remote} && rm {q_incoming}",
        )
    validate_inventory(inventory, _inventory(manifest, root))
    after = _snapshot(transport, manifest, execute)
    if execute:
        execution.verify_preflight(manifest, after)
        compile_lab.verify_preflight(manifest, candidate, after)
    else:
        compile_lab.verify_preflight(manifest, candidate, after)
    current = compile_lab.collect_snapshot(transport, baseline)
    stage_tools.align_cleanup_diagnostics(transport, before, current)
    differences, alerts = health(baseline, before, current)
    _write_json(
        run_dir / "stage-after.json",
        {"snapshot": after, "health_differences": differences, "kernel_alerts": alerts},
    )
    if differences or alerts:
        raise NativeLabError("staging changed device health")
    if phase is not None:
        phase.check(current, "after-staging")
    return inventory


def _cleanup(
    manifest,
    payloads: dict,
    root: str,
    envelope: dict,
    run_dir: Path,
    execute: bool = False,
) -> dict:
    transport_state = envelope.get("transport", {})
    if (
        transport_state.get("timed_out")
        or transport_state.get("remote_exit") not in ((0,) if execute else (0, 5))
        or envelope.get("health_differences")
        or envelope.get("kernel_alerts")
        or (execute and envelope.get("status") != "PASS")
    ):
        raise NativeLabError(
            "cleanup refused after ambiguous, unsuccessful or unhealthy native operation"
        )
    transport = npu_lab.AdbTransport()
    before = _snapshot(transport, manifest, execute)
    postflight = _json(run_dir / "postflight.json")
    stage_tools.align_cleanup_diagnostics(transport, postflight, before)
    differences, alerts = health(manifest, postflight, before)
    if differences or alerts:
        raise NativeLabError("device health changed before cleanup")
    expected = expected_inventory(root, payloads)
    files = {remote: manifest.required_artifacts[remote] for remote in payloads}
    for candidate in manifest.candidates.values():
        for remote in (candidate.output, candidate.partial_output):
            state = before.get("output_states", {}).get(remote, {})
            original = postflight.get("output_states", {}).get(remote, {})
            if state != original or state.get("status") not in ("present", "absent"):
                raise NativeLabError("owned output changed before cleanup")
            if state["status"] == "present":
                files[remote] = state["sha256"]
                expected[remote] = "F " + state["permissions"]
    validate_inventory(expected, _inventory(manifest, root))
    for remote, digest in files.items():
        quoted = shlex.quote(remote)
        values = _adb(
            manifest,
            "shell",
            f"test ! -L {quoted} && test -f {quoted} && sha256sum {quoted}",
        ).split()
        if not values or values[0] != digest:
            raise NativeLabError("cleanup refuses changed owned artifact")
    baseline = _system_manifest(manifest)
    system_before = compile_lab.collect_snapshot(transport, baseline)
    projected_before = {
        **before,
        "artifact_hashes": {
            path: before.get("artifact_hashes", {}).get(path)
            for path in baseline.required_artifacts
        },
    }
    stage_tools.align_cleanup_diagnostics(transport, projected_before, system_before)
    differences, alerts = health(baseline, projected_before, system_before)
    if differences or alerts:
        raise NativeLabError(
            "device health changed during cleanup inventory validation"
        )
    _adb(manifest, "shell", "rm " + " ".join(shlex.quote(path) for path in files))
    directories = sorted(
        (path for path, mode in expected.items() if mode.startswith("D ")),
        key=lambda path: (len(Path(path).parts), path),
        reverse=True,
    )
    _adb(
        manifest,
        "shell",
        "rmdir " + " ".join(shlex.quote(path) for path in directories),
    )
    after = compile_lab.collect_snapshot(transport, baseline)
    stage_tools.align_cleanup_diagnostics(transport, system_before, after)
    differences, alerts = health(baseline, system_before, after)
    compile_lab.verify_preflight(
        baseline, next(iter(baseline.candidates.values())), after
    )
    if (
        _adb(
            manifest,
            "shell",
            f"if [ -e {root} ] || [ -L {root} ]; then printf PRESENT; else printf ABSENT; fi",
        )
        != "ABSENT"
    ):
        differences.append("staging root remains after cleanup")
    result = {
        "status": "CLEAN" if not differences and not alerts else "FAIL",
        "before": before,
        "after": after,
        "inventory_before": expected,
        "root": root,
        "health_differences": differences,
        "kernel_alerts": alerts,
    }
    _write_json(run_dir / "cleanup.json", result)
    if differences or alerts:
        raise NativeLabError("cleanup health gate failed")
    if _ACTIVE_PHASE.get() is not None:
        _ACTIVE_PHASE.get().pending_cleanup = result
    return result


def _nn_target(stdout: str, allow_tp: bool = False, *, require_nn: bool = True) -> bool:
    lines = [line for line in stdout.splitlines() if "operation target " in line]
    targets = (
        ("VXNNE_OPERATION_TARGET_NN", "VXNNE_OPERATION_TARGET_TP")
        if allow_tp
        else ("VXNNE_OPERATION_TARGET_NN",)
    )
    return (
        bool(lines)
        and (
            not require_nn
            or any(
                "operation target VXNNE_OPERATION_TARGET_NN," in line for line in lines
            )
        )
        and all(
            any(f"operation target {target}," in line for target in targets)
            for line in lines
        )
    )


def _qualified_target(record: dict, stdout: str) -> str | None:
    pool_control = (
        record.get("profile_id") == VISION_PROFILE and record.get("control_id") == "20b"
    )
    if not _nn_target(
        stdout,
        allow_tp=record.get("profile_id") in GRAPH_PROFILES,
        require_nn=not pool_control,
    ):
        return None
    return (
        "VXNNE_OPERATION_TARGET_NN"
        if "operation target VXNNE_OPERATION_TARGET_NN," in stdout
        else "VXNNE_OPERATION_TARGET_TP"
    )


def _copy_evidence(source: Path, target: Path) -> None:
    target.mkdir(parents=True, exist_ok=False)
    for path in source.iterdir():
        if path.is_file() and not path.is_symlink():
            _write(target / path.name, _regular(path, 8 * 1024 * 1024))


def _run_id(run_id: str) -> None:
    if not isinstance(run_id, str) or compile_lab.RUN_ID_RE.fullmatch(run_id) is None:
        raise NativeLabError("invalid exclusive run ID")
    if (compile_lab.DEFAULT_RUN_ROOT / run_id).exists():
        raise NativeLabError("run evidence directory already exists")


@_phase_guarded
def compile_package(package_dir: Path | str, run_id: str) -> dict:
    _run_id(run_id)
    package = load_package(package_dir)
    root = Path(package["directory"])
    if package["compiled"] or (root / "compile-attempt.json").exists():
        raise NativeLabError("this package already has a compile attempt; no retry")
    _activate_phase(package)
    manifest = _compile_manifest(package)
    remote = manifest.policy.remote_root
    payloads = {
        manifest.remote_executable: (root / "build/wave-npu-onnx-compile-v1", "0555"),
        remote + "/lib/libtim-vx.so": (RUNTIME, "0444"),
        remote + "/sdk/include/CL/cl_viv_vx_ext.h": (
            REPOSITORY_ROOT / "waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h",
            "0444",
        ),
    }
    stage_dir = root / "evidence" / (run_id + "-stage")
    stage_dir.mkdir(parents=True, exist_ok=False)
    _stage(manifest, payloads, remote, stage_dir)
    _write_json(
        root / "compile-attempt.json",
        {**_identity(package), "run_id": run_id, "manifest_sha256": manifest.sha256},
    )
    envelope, run_dir = compile_lab.run_candidate(
        npu_lab.AdbTransport(),
        manifest,
        "candidate-1",
        run_id,
        **_phase_runner_kwargs(compile_lab.PreflightError),
    )
    if envelope.get("status") != "PASS":
        if (
            envelope.get("transport", {}).get("remote_exit") == 5
            and not envelope.get("health_differences")
            and not envelope.get("kernel_alerts")
        ):
            state = envelope.get("transport", {})
            canonical = (
                state.get("timed_out") is False
                and state.get("adb_returncode") == 0
                and envelope.get("errors") == ["native compiler exited 5"]
            )
            if _ACTIVE_PHASE.get() is None or canonical:
                cleanup = _cleanup(manifest, payloads, remote, envelope, run_dir)
                if _ACTIVE_PHASE.get() is not None:
                    raise CleanCompileNegative(
                        "native compiler cleanly rejected this candidate; "
                        f"evidence: {run_dir}",
                        cleanup,
                    )
        raise NativeLabError(f"native compilation did not pass; evidence: {run_dir}")
    _successful_run(envelope)
    stdout = _regular(run_dir / "stdout.txt").decode()
    target = _qualified_target(package["record"], stdout)
    if target is None:
        raise NativeLabError(
            "native compiler did not establish exclusive NN target selection"
        )
    _cleanup(manifest, payloads, remote, envelope, run_dir)
    _copy_evidence(run_dir, root / "evidence/compile")
    nbg = _regular(run_dir / "candidate-1.nbg")
    _write(root / "model.nbg", nbg)
    compiled = {
        **_identity(package),
        "status": "PASS",
        "run_id": run_id,
        **_evidence_hashes(run_dir),
        "compiler_sha256": package["build"]["binary_sha256"],
        "nbg_sha256": _digest(nbg),
        "nbg_bytes": len(nbg),
        "nn_target": target,
        "manifest_sha256": manifest.sha256,
        "run_sha256": _digest(_regular(run_dir / "run.json")),
        "stdout_sha256": _digest(stdout.encode()),
    }
    _write_json(root / "compiled.json", compiled)
    return load_package(root)


def _cpu_expected(
    package: dict,
    inputs: dict,
    verify_onnx_runtime: bool = True,
    evidence_path: Path | None = None,
) -> tuple[bytes, bytes]:
    import numpy as np

    importer, _ = _host()
    expected = importer.independent_oracle(package["record"], inputs)
    input_spec = package["record"]["onnx_input"]
    output_spec = package["record"]["onnx_output"]
    if set(inputs) != {input_spec["name"]} or not isinstance(
        inputs[input_spec["name"]], np.ndarray
    ):
        raise NativeLabError("wrong native input tensor set")
    if not inputs[input_spec["name"]].flags.c_contiguous:
        raise NativeLabError("native input tensor must be contiguous")
    if any(not np.isfinite(array).all() for array in expected.values()):
        raise NativeLabError("independent oracle output is non-finite")
    output_bytes = expected[output_spec["name"]].tobytes()
    if len(output_bytes) != _tensor_bytes(package["record"], "onnx_output"):
        raise NativeLabError("independent oracle output has the wrong length")
    if output_bytes == bytes([0xA5]) * len(output_bytes):
        raise NativeLabError(
            "oracle output equals the output canary; use CPU inference"
        )
    if verify_onnx_runtime:
        from wave_onnx_hybrid.audit import audit_model_path, load_policy
        from wave_onnx_hybrid.runtime import ExecutionMode, execute_cpu_model

        policy = load_policy(
            REPOSITORY_ROOT / "docs/npu/onnx/hybrid-policy.json",
            repository_root=REPOSITORY_ROOT,
        )
        model = Path(package["directory"]) / "model.onnx"
        audit = audit_model_path(model, policy=policy)
        cpu_run = execute_cpu_model(model, audit, inputs, mode=ExecutionMode.CPU)
        cpu = cpu_run.outputs
        if set(expected) != set(cpu) or any(
            expected[name].dtype != cpu[name].dtype
            or expected[name].shape != cpu[name].shape
            for name in expected
        ):
            raise NativeLabError("ONNX Runtime CPU output tensor contract differs")
        for name in expected:
            _compare_output(
                package["record"], expected[name].tobytes(), cpu[name].tobytes()
            )
        if evidence_path is not None:
            _write_json(evidence_path, cpu_run.evidence)
            if package["record"]["profile_id"] in GRAPH_PROFILES:
                _write(
                    evidence_path.with_name("qualification-cpu-output.bin"),
                    cpu[output_spec["name"]].tobytes(),
                )
                _write(
                    evidence_path.with_name("qualification-independent-output.bin"),
                    output_bytes,
                )
    return inputs[input_spec["name"]].tobytes(), output_bytes


def _execute_manifest(
    package: dict, input_path: Path, expected: bytes, run_dir: Path, create: bool = True
):
    record = package["record"]
    root = remote_root(record, "execute")
    build = _load_build(package, "execute")
    input_bytes = _regular(input_path, _tensor_bytes(record, "onnx_input"))
    if len(input_bytes) != _tensor_bytes(record, "onnx_input") or len(
        expected
    ) != _tensor_bytes(record, "onnx_output"):
        raise NativeLabError("execution tensor file length differs from profile")
    artifacts = {
        **SYSTEM_HASHES,
        root + "/bin/wave-npu-onnx-execute-v1": build["binary_sha256"],
        root + "/lib/libtim-vx.so": WRAPPER_SHA256,
        root + "/model/model.nbg": package["compiled"]["nbg_sha256"],
        root + "/input/input.bin": _digest(input_bytes),
    }
    modes = {
        path: "-r-xr-xr-x" if "/bin/" in path else "-r--r--r--"
        for path in artifacts
        if path.startswith(root + "/")
    }
    numerical = record["profile_id"] in GRAPH_PROFILES
    if record["profile_id"] == VISION_PROFILE:
        operation = "fp16-squeezenet"
    elif record["profile_id"] == "fp16-marblenet-pad-v1":
        operation = "fp16-marblenet-pad"
    else:
        operation = (
            "fp16-marblenet"
            if numerical
            else ("fp16-conv" if record["profile_id"] == "fp16-conv-v1" else "u8-fc")
        )
    result_re = re.compile(
        r"^EXECUTION status=PASS mode=execute-once target=wave operation="
        + operation
        + rf" iterations=1 input_bytes={len(input_bytes)} output_bytes={len(expected)}"
        + r" graph_us=(?P<graph_us>[0-9]+) output_hex=(?P<output_hex>"
        + (rf"[0-9a-f]{{{2 * len(expected)}}}" if numerical else expected.hex())
        + r") cleanup=0$"
    )
    policy = execution.ExecutionPolicy(
        poc_id=_poc_id(record),
        experiment=f"wave-npu-poc{_poc_id(record)}-onnx-execute",
        manifest_path=run_dir / "execution-manifest.json",
        remote_root=root,
        remote_executable=root + "/bin/wave-npu-onnx-execute-v1",
        remote_tim_vx=root + "/lib/libtim-vx.so",
        remote_nbg=root + "/model/model.nbg",
        fixed_artifacts=artifacts,
        absent_artifacts=(
            "/lib/libCLC.so",
            "/usr/lib/libCLC.so",
            "/usr/lib64/libCLC.so",
            root + "/lib/libCLC.so",
        ),
        staged_modes=modes,
        nbg_bytes=package["compiled"]["nbg_bytes"],
        input_values=tuple(input_bytes),
        output_values=tuple(expected),
        execution_re=result_re,
    )
    document = {
        "schema_version": 1,
        "device": DEVICE,
        "environment": {
            "LD_LIBRARY_PATH": root + "/lib:/usr/lib",
            "VIVANTE_SDK_DIR": "/usr",
            "VSI_NN_LOG_LEVEL": "5",
        },
        "remote_executable": policy.remote_executable,
        "remote_argument": "--execute-once",
        "timeout_ms": 12000,
        "required_artifacts": artifacts,
        "absent_artifacts": list(policy.absent_artifacts),
        "expected_result": {
            "identity": execution.EXPECTED_IDENTITY,
            "input": list(input_bytes),
            "output": list(expected),
            "bytes": len(expected),
            "iterations": 1,
        },
    }
    if policy.manifest_path.exists():
        if _json(policy.manifest_path) != document:
            raise NativeLabError(
                "execution manifest differs from package/current input"
            )
    elif create:
        _write_json(policy.manifest_path, document)
    else:
        raise NativeLabError("qualification execution manifest is missing")
    manifest = execution.load_manifest(policy=policy)
    candidate = compile_lab.Candidate(
        "output",
        "--execute-once",
        root + "/output/output.bin",
        root + "/output/.output.bin.partial",
    )
    return replace(manifest, candidates={"output": candidate})


@_phase_guarded
def execute_package(
    package_dir: Path | str,
    inputs: dict,
    run_id: str,
    require_qualification: bool = True,
) -> dict:
    _run_id(run_id)
    package = load_package(package_dir, require_qualification=require_qualification)
    if package["compiled"] is None:
        raise NativeLabError("package has no qualified native compile artifact")
    _activate_phase(package)
    root = Path(package["directory"])
    input_bytes, expected = _cpu_expected(
        package, inputs, verify_onnx_runtime=not require_qualification
    )
    if not (root / "execute-build.json").exists():
        _, sources = _host()
        _write(
            root / "execute.cc",
            sources.generate_execute_source(
                package["record"],
                remote_root(package["record"], "execute"),
                package["compiled"]["nbg_bytes"],
            ).encode(),
        )
        _build(package, "execute")
    attempt_dir = root / "evidence" / (run_id + "-stage")
    attempt_dir.mkdir(parents=True, exist_ok=False)
    input_path = attempt_dir / "input.bin"
    _write(input_path, input_bytes)
    manifest = _execute_manifest(package, input_path, expected, attempt_dir)
    remote = manifest.policy.remote_root
    payloads = {
        manifest.remote_executable: (root / "build/wave-npu-onnx-execute-v1", "0555"),
        remote + "/lib/libtim-vx.so": (RUNTIME, "0444"),
        remote + "/model/model.nbg": (root / "model.nbg", "0444"),
        remote + "/input/input.bin": (input_path, "0444"),
    }
    # Output directory is intentionally empty before execution.
    _stage(manifest, payloads, remote, attempt_dir, execute=True)
    envelope, directory = execution.run_once(
        npu_lab.AdbTransport(),
        manifest,
        run_id,
        **_phase_runner_kwargs(execution.ExecutionPreflightError),
    )
    _successful_run(envelope)
    before = _json(directory / "preflight.json")
    after = _json(directory / "postflight.json")
    differences, alerts = health(manifest, before, after)
    if differences or alerts:
        raise NativeLabError("execution changed service/device health")
    candidate = manifest.candidates["output"]
    state = after["output_states"][candidate.output]
    if (
        state.get("status") != "present"
        or state.get("bytes") != len(expected)
        or state.get("permissions") != "-r--r--r--"
    ):
        raise NativeLabError("native output file differs from tensor contract")
    _adb(manifest, "pull", candidate.output, str(directory / "output.bin"))
    output = _regular(
        directory / "output.bin", _tensor_bytes(package["record"], "onnx_output")
    )
    if (
        _digest(output) != state.get("sha256")
        or envelope["result"].get("output_hex") != output.hex()
    ):
        raise NativeLabError("readback file differs from captured native execution")
    try:
        comparison = _compare_output(package["record"], expected, output)
    except NativeLabError as error:
        _write_json(
            directory / "numerical-comparison.json",
            {
                "status": "FAIL",
                "error": str(error),
                "reference_sha256": _digest(expected),
                "observed_sha256": _digest(output),
            },
        )
        _cleanup(manifest, payloads, remote, envelope, directory, execute=True)
        raise
    if package["record"]["profile_id"] in GRAPH_PROFILES:
        _write_json(directory / "numerical-comparison.json", comparison)
    _cleanup(manifest, payloads, remote, envelope, directory, execute=True)
    native_identity = {
        "package_sha256": package["package_sha256"],
        "source_model_sha256": package["record"]["source_model_sha256"],
        "lowering_sha256": package["record"]["lowering_sha256"],
        "nbg_sha256": package["compiled"]["nbg_sha256"],
        "input_sha256": _digest(input_bytes),
        "output_sha256": _digest(output),
    }
    _write_json(directory / "native-identity.json", native_identity)
    return {
        "output_bytes": output,
        "envelope": {**envelope, "native_identity": native_identity},
        "run_directory": str(directory),
    }


@_phase_guarded
def qualify_package(
    package_dir: Path | str, inputs_path: Path | str, run_id: str
) -> dict:
    _run_id(run_id)
    import numpy as np

    package = load_package(package_dir)
    root = Path(package["directory"])
    if package["qualification"] or (root / "qualification-attempt.json").exists():
        raise NativeLabError(
            "this package already has a qualification attempt; no retry"
        )
    if (
        package["record"]["profile_id"] in PHASE_PROFILES
        and package["compiled"] is None
    ):
        raise NativeLabError("package has no qualified native compile artifact")
    _activate_phase(package)
    with np.load(inputs_path, allow_pickle=False) as archive:
        inputs = {
            name: np.array(archive[name], copy=True, order="C")
            for name in archive.files
        }
    input_bytes, expected = _cpu_expected(
        package, inputs, evidence_path=root / "qualification-cpu-oracle.json"
    )
    _write_json(
        root / "qualification-attempt.json",
        {
            **_identity(package),
            "run_id": run_id,
            "input_sha256": _digest(input_bytes),
            "output_sha256": _digest(expected),
        },
    )
    result = execute_package(root, inputs, run_id, require_qualification=False)
    directory = Path(result["run_directory"])
    _copy_evidence(directory, root / "evidence/qualification")
    build = _load_build(package, "execute")
    qualification = {
        **_identity(package),
        "status": "PASS",
        "run_id": run_id,
        **_evidence_hashes(directory),
        "manifest_sha256": result["envelope"]["manifest_sha256"],
        "nbg_sha256": package["compiled"]["nbg_sha256"],
        "executor_sha256": build["binary_sha256"],
        "run_sha256": _digest(_regular(directory / "run.json")),
        "input_hex": input_bytes.hex(),
        "output_hex": result["output_bytes"].hex(),
        "input_sha256": _digest(input_bytes),
        "output_sha256": _digest(result["output_bytes"]),
        "cpu_evidence_sha256": _digest(
            _regular(root / "qualification-cpu-oracle.json")
        ),
    }
    if package["record"]["profile_id"] in GRAPH_PROFILES:
        qualification.update(
            {
                "independent_output_sha256": _digest(
                    _regular(
                        root / "qualification-independent-output.bin",
                        _tensor_bytes(package["record"], "onnx_output"),
                    )
                ),
                "cpu_output_sha256": _digest(
                    _regular(
                        root / "qualification-cpu-output.bin",
                        _tensor_bytes(package["record"], "onnx_output"),
                    )
                ),
                "comparison_sha256": _digest(
                    _regular(directory / "numerical-comparison.json")
                ),
            }
        )
    _write_json(root / "qualification.json", qualification)
    return load_package(root, require_qualification=True)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    imp = commands.add_parser("import")
    imp.add_argument("model", type=Path)
    imp.add_argument("package_dir", type=Path)
    for kind in ("build", "compile", "qualify", "inspect"):
        command = commands.add_parser(kind)
        command.add_argument("package_dir", type=Path)
        if kind in ("compile", "qualify"):
            command.add_argument("--run-id", required=True)
        if kind == "qualify":
            command.add_argument("--inputs", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "import":
            package = import_package(args.model, args.package_dir)
        elif args.command == "build":
            package = build_package(args.package_dir)
        elif args.command == "compile":
            package = compile_package(args.package_dir, args.run_id)
        elif args.command == "qualify":
            package = qualify_package(args.package_dir, args.inputs, args.run_id)
        else:
            package = load_package(args.package_dir)
        print(
            json.dumps(
                {
                    "directory": package["directory"],
                    "profile": package["record"]["profile_id"],
                    "model_sha256": package["record"]["source_model_sha256"],
                    "compiled": package["compiled"],
                    "qualification": package["qualification"],
                },
                indent=2,
            )
        )
        return 0
    except (
        NativeLabError,
        compile_lab.CompileLabError,
        execution.ExecutionLabError,
        nbg_format.NbgError,
        OSError,
        ValueError,
    ) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
