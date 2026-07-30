#!/usr/bin/env python3
"""Correlate the PoC 14B rejection with the Wave elementwise type checker."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any, Sequence


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
ELTWISE_SOURCE = (
    REPOSITORY_ROOT / "waved/third_party/TIM-VX/src/tim/vx/internal/src/ops/"
    "vsi_nn_op_eltwise.c"
)
OPENVX_SOURCE = (
    REPOSITORY_ROOT / "waved/third_party/TIM-VX/src/tim/vx/internal/src/kernel/vx/"
    "eltwise_vx.c"
)
WAVE_OVXLIB = REPOSITORY_ROOT / "waved/third_party/device-sdk/lib/libovxlib.so"
DEFAULT_STDOUT = (
    REPOSITORY_ROOT / "artifacts/npu/runs/poc14b-a1-erf-affine-20260729/stdout.txt"
)
EXPECTED_HASHES = {
    "elementwise_source": (
        "3d2e3c991d0bb53545605b3d88310ed78487d57f72e7ff87e1f4cce86962bee2"
    ),
    "openvx_source": (
        "c4951165d155d1ab45c1c460aa8c6245903b62f4332e372d97b8f9a68011b4dd"
    ),
    "wave_ovxlib": ("09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9"),
}
REJECTION_RE = re.compile(
    r"op_check_add:(?P<line>[0-9]+)\]"
    r"Inputs/Outputs data type not support: "
    r"(?P<input_0>[^,\r\n]+), (?P<input_1>[^\r\n]+)"
)
IO_TYPE_RE = re.compile(r"IO_TYPE\((?P<input_0>[^,]+),(?P<input_1>[^)]+)\)")


class ProfileError(ValueError):
    """The fixed source or capture does not satisfy the profile contract."""


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _normalized(value: str) -> str:
    return re.sub(r"\s+", "", value)


def extract_add_type_table(source: str) -> tuple[tuple[str, str], ...]:
    start_marker = "BEGIN_IO_TYPE_DECL(ADD, 2, 0)"
    end_marker = "END_IO_TYPE_DECL(ADD)"
    if source.count(start_marker) != 1 or source.count(end_marker) != 1:
        raise ProfileError("ADD type-table markers are not unique")
    block = source.split(start_marker, 1)[1].split(end_marker, 1)[0]
    entries = tuple(
        (
            _normalized(match["input_0"]),
            _normalized(match["input_1"]),
        )
        for match in IO_TYPE_RE.finditer(block)
    )
    if len(entries) != 59:
        raise ProfileError(f"expected 59 ADD type pairs, observed {len(entries)}")
    return entries


def multiply_delegates_to_add(source: str) -> bool:
    match = re.search(
        r"static vsi_bool op_check_mul\b(?P<body>.*?)"
        r"#ifdef __cplusplus",
        source,
        re.DOTALL,
    )
    if match is None:
        raise ProfileError("cannot isolate op_check_mul")
    body = match["body"]
    return (
        body.count("vsi_nn_OpCheck(VSI_NN_OP_ADD") == 1
        and "BEGIN_IO_TYPE_DECL" not in body
    )


def parse_rejection(stdout: str) -> dict[str, Any]:
    matches = list(REJECTION_RE.finditer(stdout))
    if len(matches) != 1:
        raise ProfileError(
            f"expected one op_check_add rejection, observed {len(matches)}"
        )
    match = matches[0]
    return {
        "function": "op_check_add",
        "line": int(match["line"]),
        "input_descriptions": [
            match["input_0"].strip(),
            match["input_1"].strip(),
        ],
        "normalized_signature": [
            "D_U8|Q_ASYM",
            "D_I8|Q_SYM_PC",
        ],
    }


def build_profile(stdout_path: Path = DEFAULT_STDOUT) -> dict[str, Any]:
    paths = {
        "elementwise_source": ELTWISE_SOURCE,
        "openvx_source": OPENVX_SOURCE,
        "wave_ovxlib": WAVE_OVXLIB,
    }
    observed_hashes = {name: _sha256(path) for name, path in paths.items()}
    if observed_hashes != EXPECTED_HASHES:
        raise ProfileError(
            "fixed source/runtime hashes differ from the reviewed profile"
        )
    source = ELTWISE_SOURCE.read_text(encoding="utf-8")
    openvx = OPENVX_SOURCE.read_text(encoding="utf-8")
    table = extract_add_type_table(source)
    if not multiply_delegates_to_add(source):
        raise ProfileError("Multiply no longer delegates type checks to ADD")
    if (
        openvx.count("REGISTER_ELTWISE_OPENVX_KERNEL( add )") != 1
        or openvx.count("REGISTER_ELTWISE_OPENVX_KERNEL( mul )") != 1
        or openvx.count("vxTensorAddNode(") != 1
        or openvx.count("vxTensorMultiplyNode(") != 1
    ):
        raise ProfileError("OpenVX Add/Multiply registration differs")

    rejection_bytes = stdout_path.read_bytes()
    rejection = parse_rejection(rejection_bytes.decode("utf-8"))
    rejected_signature = tuple(rejection["normalized_signature"])
    source_error_line = next(
        index
        for index, line in enumerate(source.splitlines(), start=1)
        if 'VSILOGE("Inputs/Outputs data type not support: %s", desc);' in line
        and 380 <= index <= 480
    )
    per_channel_entries = [
        entry for entry in table if any("PC" in value for value in entry)
    ]
    return {
        "schema_version": 1,
        "poc": "14B",
        "status": "clean-negative",
        "scope": "Wave affine-plus-Erf compile-only baseline A1",
        "provenance": {
            name: {
                "path": path.relative_to(REPOSITORY_ROOT).as_posix(),
                "sha256": observed_hashes[name],
            }
            for name, path in paths.items()
        },
        "capture": {
            "path": stdout_path.relative_to(REPOSITORY_ROOT).as_posix(),
            "sha256": hashlib.sha256(rejection_bytes).hexdigest(),
            "rejection": rejection,
            "source_error_line": source_error_line,
            "source_line_delta": source_error_line - rejection["line"],
        },
        "checker": {
            "multiply_delegates_to_add": True,
            "add_declared_input_count": 2,
            "add_declared_output_count": 0,
            "accepted_pair_count": len(table),
            "accepted_pairs": [list(entry) for entry in table],
            "per_channel_pair_count": len(per_channel_entries),
            "rejected_signature_present": rejected_signature in table,
        },
        "openvx_path": {
            "add_registered": True,
            "multiply_registered": True,
            "add_constructor": "vxTensorAddNode",
            "multiply_constructor": "vxTensorMultiplyNode",
        },
        "interpretation": {
            "first_blocking_stage": "TIM-VX operation type check",
            "nbg_emitted": False,
            "coefficient_differential_available": False,
            "current_target_a_eligible_for_semantic_emission": False,
            "compatible_type_example": [
                "D_U8|Q_ASYM",
                "D_U8|Q_ASYM",
            ],
            "compatible_type_example_caveat": (
                "type-check compatibility is not NBG-export evidence; "
                "PoC 5B already observed OpenVX Add selection followed by "
                "vxGenerateNBG failure"
            ),
        },
        "next_gate": (
            "host-only lowering and quality qualification plus new static "
            "producer evidence; no retry of the failed A1 artifact"
        ),
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Build the fixed PoC 14B elementwise compiler profile"
    )
    parser.add_argument("--stdout", type=Path, default=DEFAULT_STDOUT)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    try:
        profile = build_profile(_parser().parse_args(argv).stdout.resolve())
    except (OSError, UnicodeError, ProfileError, StopIteration) as exc:
        print(f"error: {exc}")
        return 2
    print(json.dumps(profile, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
