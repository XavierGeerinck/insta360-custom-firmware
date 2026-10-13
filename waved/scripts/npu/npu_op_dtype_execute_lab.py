#!/usr/bin/env python3
"""One-shot execution of the two exact PoC 16 native NN artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
from pathlib import Path
from typing import Sequence

import nbg_format
import npu_compile_lab
import npu_erf_execution_lab as execution
import npu_lab
import npu_op_dtype_lab


MODEL_ARTIFACTS = {
    "fp16-conv": {
        "poc_id": "16C", "root": "/tmp/wave-npu-exec16a",
        "compile_run": "rea-poc16a-fp16-conv-20261013-a1",
        "nbg_bytes": 1800,
        "nbg_sha256": "4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d",
        "executor_sha256": "81900c201236e06ba5c6b1232d90d236de569f494a6a5ab6a7daf77bfbe37d02",
    },
    "u8-fc": {
        "poc_id": "16D", "root": "/tmp/wave-npu-exec16b",
        "compile_run": "rea-poc16b-u8-fc-20261013-a1",
        "nbg_bytes": 2056,
        "nbg_sha256": "00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e",
        "executor_sha256": "771871c8e5aacccc3bc279d6947ac69268c045e755baee0d0022ce9cb8df4fa0",
    },
}
WRAPPER_SHA256 = "c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc"
SYSTEM_HASHES = {
    path: digest for path, digest in json.loads(execution.POC7_POLICY.manifest_path.read_text())["required_artifacts"].items()
    if not path.startswith("/tmp/")
}


def make_policy(model: str) -> execution.ExecutionPolicy:
    artifact = MODEL_ARTIFACTS[model]
    root = artifact["root"]
    executable = f"{root}/bin/wave-npu-execute-{model}-v1"
    wrapper = f"{root}/lib/libtim-vx.so"
    nbg = f"{root}/model/wave-{model}.nbg"
    input_bytes = struct.pack("<16H", *([0x3c00, 0xbc00] * 8)) if model == "fp16-conv" else bytes(range(16))
    output_bytes = npu_op_dtype_lab.fp16_fir_oracle() if model == "fp16-conv" else npu_op_dtype_lab.u8_fc_oracle()
    result_re = re.compile(
        r"^EXECUTION status=PASS mode=execute-once target=wave operation=" + re.escape(model)
        + rf" iterations=1 input_bytes={len(input_bytes)} output_bytes={len(output_bytes)}"
        + r" graph_us=(?P<graph_us>[0-9]+) output_hex=(?P<output_hex>"
        + output_bytes.hex() + r") cleanup=0$"
    )
    return execution.ExecutionPolicy(
        poc_id=artifact["poc_id"], experiment=f"wave-npu-poc{artifact['poc_id'].lower()}-{model}-execute-once",
        manifest_path=Path(__file__).with_name(f"{model.replace('-', '_')}_execution_manifest.json"),
        remote_root=root, remote_executable=executable, remote_tim_vx=wrapper, remote_nbg=nbg,
        fixed_artifacts={**SYSTEM_HASHES, executable: artifact["executor_sha256"], wrapper: WRAPPER_SHA256, nbg: artifact["nbg_sha256"]},
        absent_artifacts=("/lib/libCLC.so", "/usr/lib/libCLC.so", "/usr/lib64/libCLC.so", f"{root}/lib/libCLC.so"),
        staged_modes={executable: "-r-xr-xr-x", wrapper: "-r--r--r--", nbg: "-r--r--r--"},
        nbg_bytes=artifact["nbg_bytes"], input_values=tuple(input_bytes), output_values=tuple(output_bytes), execution_re=result_re,
    )


POLICIES = {model: make_policy(model) for model in MODEL_ARTIFACTS}


def validate_local_model(model: str) -> dict:
    artifact = MODEL_ARTIFACTS[model]
    path = execution.DEFAULT_RUN_ROOT / artifact["compile_run"] / "candidate-1.nbg"
    if not path.is_file() or path.is_symlink():
        raise execution.ExecutionPreflightError("qualified local NBG is missing or not regular")
    if hashlib.sha256(path.read_bytes()).hexdigest() != artifact["nbg_sha256"]:
        raise execution.ExecutionPreflightError("qualified local NBG hash differs from fixed policy")
    report = nbg_format.parse_path(path, display_path=path.relative_to(execution.REPOSITORY_ROOT).as_posix())
    compile_manifest = npu_compile_lab.load_manifest(policy=npu_op_dtype_lab.POLICIES[model])
    npu_compile_lab.validate_nbg_report(report, compile_manifest)
    if report["size"] != artifact["nbg_bytes"]:
        raise execution.ExecutionPreflightError("qualified NBG byte count mismatch")
    return report


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", choices=tuple(POLICIES))
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("manifest")
    commands.add_parser("stage")
    run = commands.add_parser("run")
    run.add_argument("--run-id", required=True)
    cleanup = commands.add_parser("cleanup")
    cleanup.add_argument("--run-id", required=True)
    args = parser.parse_args(argv)
    try:
        manifest = execution.load_manifest(policy=POLICIES[args.model])
        validate_local_model(args.model)
        if args.command == "manifest":
            print(json.dumps({"model": args.model, "manifest_sha256": manifest.sha256,
                              "nbg_bytes": manifest.policy.nbg_bytes, "execution_attempts": 1,
                              "expected_output_hex": bytes(manifest.policy.output_values).hex()}, indent=2))
            return 0
        if args.command in ("stage", "cleanup"):
            import npu_op_dtype_execute_stage
            result = (npu_op_dtype_execute_stage.stage(manifest, args.model) if args.command == "stage"
                      else npu_op_dtype_execute_stage.cleanup(manifest, args.model, args.run_id))
            print(json.dumps(result, indent=2))
            return 0
        envelope, directory = execution.run_once(npu_lab.AdbTransport(), manifest, args.run_id)
        print(json.dumps({"status": envelope["status"], "errors": envelope["errors"],
                          "result": envelope["result"], "run_directory": directory.as_posix()}, indent=2))
        return 0 if envelope["status"] == "PASS" else 1
    except (execution.ExecutionLabError, npu_compile_lab.CompileLabError, nbg_format.NbgError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
