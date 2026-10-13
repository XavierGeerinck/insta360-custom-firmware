#!/usr/bin/env python3
"""Fixed PoC 16A/16B compiler policies and independent numerical oracles."""

from __future__ import annotations

import argparse
import json
import math
import struct
from dataclasses import dataclass
from typing import Sequence

import npu_compile_lab
import npu_lab


@dataclass(frozen=True)
class DiagnosticCompilePolicy(npu_compile_lab.CompilePolicy):
    @property
    def expected_environment(self) -> dict[str, str]:
        return {
            **super().expected_environment,
            "VIV_VX_DEBUG_LEVEL": "1",
            "VIV_VX_ENABLE_PRINT_TARGET": "1",
        }


def _policy(model: str) -> DiagnosticCompilePolicy:
    conv = model == "fp16-conv"
    root = f"/tmp/wave-npu-compile16{'a' if conv else 'b'}"
    inputs = npu_compile_lab.TensorExpectation(
        (16, 1, 1, 1) if conv else (16, 1),
        "fp16" if conv else "uint8", 32 if conv else 16,
        ("none",) if conv else ("affine-asymmetric",),
        None if conv else 1.0, None if conv else 0,
    )
    outputs = inputs if conv else npu_compile_lab.TensorExpectation(
        (4, 1), "uint8", 4, ("affine-asymmetric",), 1.0, 0,
    )
    return DiagnosticCompilePolicy(
        poc_id="16A" if conv else "16B",
        experiment=f"wave-npu-poc16{'a' if conv else 'b'}-{model}-compile-only",
        operation="Fp16Conv" if conv else "U8FullyConnected",
        manifest_path=npu_compile_lab.MANIFEST_DIRECTORY / f"{model.replace('-', '_')}_compile_manifest.json",
        remote_executable=f"{root}/bin/wave-npu-compile-{model}-v1",
        candidates={
            "candidate-1": {
                "argument": "--candidate-1",
                "output": f"{root}/output/wave-{model}-candidate-1.nbg",
                "partial_output": f"{root}/output/.wave-{model}-candidate-1.nbg.partial",
            },
        },
        inputs=(inputs,), outputs=(outputs,),
        selector_backend="OPENVX" if conv else None,
        selector_kernel="conv2d" if conv else None,
        absent_artifacts=(
            "/lib/libCLC.so", "/usr/lib/libCLC.so", "/usr/lib64/libCLC.so",
            f"{root}/lib/libCLC.so",
        ),
    )


POLICIES = {model: _policy(model) for model in ("fp16-conv", "u8-fc")}


def fp16_fir_oracle(samples: Sequence[float] | None = None) -> bytes:
    values = list(samples) if samples is not None else [1.0, -1.0] * 8
    if len(values) != 16 or any(not math.isfinite(value) for value in values):
        raise ValueError("FP16 FIR requires 16 finite samples")
    result = []
    for index in range(16):
        result.append(sum(
            coefficient * (values[index + tap - 1] if 0 <= index + tap - 1 < 16 else 0.0)
            for tap, coefficient in enumerate((0.25, 0.5, 0.25))
        ))
    return struct.pack("<16e", *result)


def u8_fc_oracle(samples: Sequence[int] | None = None) -> bytes:
    values = list(samples) if samples is not None else list(range(16))
    if len(values) != 16 or any(type(value) is not int or not 0 <= value <= 255 for value in values):
        raise ValueError("U8 FC requires 16 integers in 0..255")
    return bytes(sum(
        value * int(index == output * 5)
        for index, value in enumerate(values)
    ) for output in range(4))


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
    policy = POLICIES[args.model]
    try:
        manifest = npu_compile_lab.load_manifest(policy=policy)
        if args.command == "manifest":
            print(json.dumps({"model": args.model, "manifest_sha256": manifest.sha256,
                              "mode": "compile-only", "environment": manifest.environment}, indent=2))
            return 0
        if args.command in ("stage", "cleanup"):
            import npu_op_dtype_stage
            if args.command == "stage":
                result = npu_op_dtype_stage.stage(manifest)
            else:
                result = npu_op_dtype_stage.cleanup(manifest, args.run_id)
            print(json.dumps(result, indent=2))
            return 0
        envelope, directory = npu_compile_lab.run_candidate(
            npu_lab.AdbTransport(), manifest, "candidate-1", args.run_id,
        )
        print(json.dumps({"status": envelope["status"], "errors": envelope["errors"],
                          "run_directory": directory.as_posix()}, indent=2))
        return 0 if envelope["status"] == "PASS" else 1
    except npu_compile_lab.CompileLabError as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
