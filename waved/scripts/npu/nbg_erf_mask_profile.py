#!/usr/bin/env python3
"""Host-only sealed profile for the exact PoC 13B Erf-mask NBG."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path
from typing import Any, Mapping, Sequence

try:
    from . import nbg_format, nbg_image
except ImportError:
    import nbg_format  # type: ignore
    import nbg_image  # type: ignore


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
EXPECTED_NBG_SHA256 = (
    "96447d6afd63a7c349bbd80f0af99150"
    "c89ef24ed7f1ef66ab285075362e6c94"
)
EXPECTED_NBG_BYTES = 5840
EXPECTED_MODEL_MANIFEST_SHA256 = (
    "bd7ffc58d253855e3ddc546def711f918"
    "12da8cb9ecd88dc658b56aada40ce5e"
)
EXPECTED_PARTITION_SIGNATURE = (
    "1c535b02a536a6c313bcaa443c0e82af"
    "167720c5bdc9c9fb3efa1938c8cddd4b"
)
EXPECTED_SECTION_TYPES = (
    35,
    38,
    40,
    1,
    2,
    3,
    4,
    8,
    10,
    23,
    27,
    24,
    21,
)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ProfileError(Exception):
    """The candidate cannot be admitted as the exact sealed partition."""


def canonical_json(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _strict_json(payload: bytes, *, label: str) -> dict[str, Any]:
    if not isinstance(payload, bytes):
        raise ProfileError(f"{label} must be immutable bytes")

    def without_duplicates(
        pairs: list[tuple[str, Any]],
    ) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ProfileError(f"duplicate JSON key in {label}: {key}")
            result[key] = value
        return result

    try:
        value = json.loads(
            payload,
            object_pairs_hook=without_duplicates,
            parse_constant=lambda token: (_ for _ in ()).throw(
                ValueError(f"non-finite JSON token: {token}")
            ),
        )
    except ProfileError:
        raise
    except (UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise ProfileError(f"invalid {label}: {exc}") from exc
    if not isinstance(value, dict):
        raise ProfileError(f"{label} root must be an object")
    return value


def _tensor_contract(
    tensor: Mapping[str, Any],
    *,
    direction: str,
    expected_name: str,
    scale: float,
) -> dict[str, Any]:
    quantization = tensor.get("quantization")
    if (
        tensor.get("dimensions") != [481]
        or tensor.get("dimension_count") != 1
        or tensor.get("data_type_name") != "tensor"
        or tensor.get("data_format_name") != "uint8"
        or tensor.get("byte_size") != 481
        or tensor.get("element_bits") != 8
        or tensor.get("name") != expected_name
        or not isinstance(quantization, dict)
        or quantization.get("format_name") != "affine-asymmetric"
        or quantization.get("scale") != scale
        or quantization.get("zero_point") != 128
    ):
        raise ProfileError(f"{direction} tensor contract mismatch")
    return {
        "byte_size": 481,
        "data_format": "uint8",
        "dimensions": [481],
        "name": expected_name,
        "quantization": {
            "format": "affine-asymmetric",
            "scale": scale,
            "zero_point": 128,
        },
    }


def _parse_nbg(payload: bytes, *, label: str) -> dict[str, Any]:
    try:
        parsed = nbg_format.parse_bytes(payload, display_path=f"<{label}>")
    except nbg_format.NbgError as exc:
        raise ProfileError(f"{label} strict parse failed: {exc}") from exc
    if (
        parsed.get("magic") != "VPMN"
        or parsed.get("version") != nbg_format.V2_VERSION
        or parsed.get("target") != 0x1000000E
        or parsed.get("format_family") != "v2-directory"
    ):
        raise ProfileError(f"{label} target/container mismatch")
    checksum = parsed.get("file_checksum")
    if not isinstance(checksum, dict) or checksum.get("matches") is not True:
        raise ProfileError(f"{label} serializer file CRC mismatch")
    reserved = parsed.get("header", {}).get("reserved")
    if (
        not isinstance(reserved, dict)
        or any(value != 0 for value in reserved.values())
    ):
        raise ProfileError(f"{label} header reserved fields are nonzero")
    return parsed


def qualify_erf_mask_nbg(
    a1_payload: bytes,
    a2_payload: bytes,
    model_manifest_payload: bytes,
    *,
    expected_nbg_sha256: str | None,
    expected_model_manifest_sha256: str | None,
    expected_partition_signature: str,
    expected_section_types: Sequence[int] | None,
    expected_input_name: str = "input/output[0]",
    expected_output_name: str = "uid_1_out_0",
) -> dict[str, Any]:
    """Qualify two immutable captures without loading or executing either."""

    if (
        not isinstance(a1_payload, bytes)
        or not isinstance(a2_payload, bytes)
        or not a1_payload
    ):
        raise ProfileError("NBG captures must be nonempty immutable bytes")
    if a1_payload != a2_payload:
        raise ProfileError("A1/A2 determinism gate failed")
    nbg_sha256 = _sha256(a1_payload)
    if (
        expected_nbg_sha256 is not None
        and nbg_sha256 != expected_nbg_sha256
    ):
        raise ProfileError("NBG digest differs from the approved capture")
    model_sha256 = _sha256(model_manifest_payload)
    if (
        expected_model_manifest_sha256 is not None
        and model_sha256 != expected_model_manifest_sha256
    ):
        raise ProfileError("model manifest digest differs")

    model = _strict_json(
        model_manifest_payload,
        label="model manifest",
    )
    partition = model.get("npu_partition")
    if (
        model.get("execution_approved") is not False
        or model.get("compiler_qualified") is not False
        or not isinstance(partition, dict)
        or partition.get("signature") != expected_partition_signature
        or partition.get("nbg") is not None
        or partition.get("numerical_evidence_sha256") is not None
    ):
        raise ProfileError("unsealed model package state differs")

    a1 = _parse_nbg(a1_payload, label="A1")
    a2 = _parse_nbg(a2_payload, label="A2")
    if a1["structural_signature"] != a2["structural_signature"]:
        raise ProfileError("A1/A2 structural signatures differ")
    section_types = tuple(section["type"] for section in a1["sections"])
    if (
        expected_section_types is not None
        and section_types != tuple(expected_section_types)
    ):
        raise ProfileError("section order or inventory differs")
    if a1.get("dynamic_io") is not None:
        raise ProfileError("external-input graph unexpectedly contains DYNAIO")
    inputs = a1.get("inputs")
    outputs = a1.get("outputs")
    if (
        not isinstance(inputs, list)
        or len(inputs) != 1
        or not isinstance(outputs, list)
        or len(outputs) != 1
    ):
        raise ProfileError("NBG input/output counts differ")
    input_contract = _tensor_contract(
        inputs[0],
        direction="input",
        expected_name=expected_input_name,
        scale=0.03125,
    )
    output_contract = _tensor_contract(
        outputs[0],
        direction="output",
        expected_name=expected_output_name,
        scale=0.0078125,
    )

    try:
        roundtrip = nbg_image.roundtrip_bytes(
            a1_payload,
            artifact_id="erf-mask-481",
            display_path="<sealed-erf-mask-481>",
        )
    except Exception as exc:
        raise ProfileError(f"lossless image admission failed: {exc}") from exc
    if not roundtrip.exact_match:
        raise ProfileError("unchanged NBG reconstruction differs")
    padding_ranges = [
        item
        for item in roundtrip.image.ranges
        if item.byte_class is nbg_image.ByteClass.PADDING
    ]
    padding_nonzero_bytes = sum(
        sum(byte != 0 for byte in item.raw) for item in padding_ranges
    )
    if padding_nonzero_bytes:
        raise ProfileError("NBG padding contains nonzero bytes")

    checksum = a1["file_checksum"]
    section_documents = [
        {
            "alignment": section["alignment"],
            "index": section["index"],
            "offset": section["offset"],
            "size": section["size"],
            "stride": section["stride"],
            "type": section["type"],
            "type_name": section["type_name"],
        }
        for section in a1["sections"]
    ]
    return {
        "schema_version": SCHEMA_VERSION,
        "poc": "13C",
        "classification": "sealed-unapproved-erf-mask-partition",
        "artifact": {
            "bytes": len(a1_payload),
            "sha256": nbg_sha256,
            "a1_a2_byte_identical": True,
            "structural_signature": a1["structural_signature"],
        },
        "target": {
            "container": "VPMN",
            "version": "0x00020000",
            "product": "0x1000000e",
        },
        "tensors": {
            "inputs": [input_contract],
            "outputs": [output_contract],
            "dynamic_io": None,
        },
        "file_checksum": {
            "algorithm": checksum["algorithm"],
            "computed_crc32_hex": checksum["computed_crc32_hex"],
            "matches": True,
            "recorded_crc32_hex": checksum["recorded_crc32_hex"],
            "scope": checksum["scope"],
        },
        "sections": section_documents,
        "roundtrip": {
            **roundtrip.to_document(),
            "padding_bytes": sum(item.length for item in padding_ranges),
            "padding_nonzero_bytes": padding_nonzero_bytes,
        },
        "package": {
            "model_manifest_sha256": model_sha256,
            "partition_signature": expected_partition_signature,
            "execution_approved": False,
            "compiler_qualified": False,
        },
        "device_calls": 0,
        "next_gate": "six-vector numerical NPU qualification",
        "non_claims": [
            "This profile does not execute or load the NBG.",
            "The exact partition is not execution-approved yet.",
            "The general compiler remains unqualified.",
        ],
    }


def _regular_bytes(path: Path, *, label: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ProfileError(f"{label} must be a regular non-symlink file")
    return path.read_bytes()


def _write_exclusive(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, 0o644)
    except FileExistsError as exc:
        raise ProfileError(f"output already exists: {path}") from exc
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Host-only exact Erf-mask NBG sealed profile"
    )
    parser.add_argument("--a1", required=True, type=Path)
    parser.add_argument("--a2", required=True, type=Path)
    parser.add_argument("--model-manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        result = qualify_erf_mask_nbg(
            _regular_bytes(args.a1, label="A1"),
            _regular_bytes(args.a2, label="A2"),
            _regular_bytes(args.model_manifest, label="model manifest"),
            expected_nbg_sha256=EXPECTED_NBG_SHA256,
            expected_model_manifest_sha256=(
                EXPECTED_MODEL_MANIFEST_SHA256
            ),
            expected_partition_signature=EXPECTED_PARTITION_SIGNATURE,
            expected_section_types=EXPECTED_SECTION_TYPES,
        )
        if result["artifact"]["bytes"] != EXPECTED_NBG_BYTES:
            raise ProfileError("NBG byte count differs from approved capture")
        _write_exclusive(args.output, canonical_json(result))
    except ProfileError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    print(f"qualified sealed Erf-mask NBG at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
