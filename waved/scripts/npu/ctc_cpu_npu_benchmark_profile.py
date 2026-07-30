#!/usr/bin/env python3
"""Build the fixed PoC 15A stock CTC CPU/NPU benchmark profile."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any, Sequence

import onnx
from onnx import TensorProto


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
NPU_SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(NPU_SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_SCRIPT_DIRECTORY))

import nbg_format  # noqa: E402


ONNX_PATH = (
    REPOSITORY_ROOT
    / "models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx"
)
NBG_PATH = (
    REPOSITORY_ROOT
    / "models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc_online.nb"
)
ORT_LIBRARY_PATH = REPOSITORY_ROOT / "models/ai-models/runtime/libonnxruntime.so.1.15.1"
ORT_HEADER_PATH = (
    REPOSITORY_ROOT / "artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/"
    "onnxruntime_c_api.h"
)
ORT_LICENSE_PATH = (
    REPOSITORY_ROOT / "artifacts/npu/build/poc15a/onnxruntime-v1.15.1/LICENSE"
)
EXPECTED_HASHES = {
    "onnx": "8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219",
    "nbg": "3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5",
    "ort_library": ("457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639"),
    "ort_header": ("cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91"),
    "ort_license": ("2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c"),
}
EXPECTED_ONNX_BYTES = 10_258_790
EXPECTED_NBG_BYTES = 5_263_360
EXPECTED_ORT_BYTES = 8_299_512
EXPECTED_ORT_HEADER_BYTES = 185_543
EXPECTED_INPUT_SHAPE = (1, 16, 512)
EXPECTED_OUTPUT_SHAPE = (1, 16, 4999)
EXPECTED_NBG_INPUT_SHAPE = (512, 16, 1)
EXPECTED_NBG_OUTPUT_SHAPE = (4999, 16, 1)
EXPECTED_NODE_TYPES = ("MatMul", "Add", "LogSoftmax")
EXPECTED_OPSET = 14
EXPECTED_MACS = 16 * 512 * 4999


class ProfileError(ValueError):
    """The fixed source/binary anchor differs from the reviewed contract."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65_536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _relative(path: Path) -> str:
    return path.relative_to(REPOSITORY_ROOT).as_posix()


def _shape(value_info: onnx.ValueInfoProto) -> tuple[int, ...]:
    tensor_type = value_info.type.tensor_type
    if tensor_type.elem_type != TensorProto.FLOAT:
        raise ProfileError(f"{value_info.name} is not FLOAT32")
    dimensions: list[int] = []
    for dimension in tensor_type.shape.dim:
        if not dimension.HasField("dim_value") or dimension.dim_value <= 0:
            raise ProfileError(f"{value_info.name} has a dynamic/invalid shape")
        dimensions.append(dimension.dim_value)
    return tuple(dimensions)


def _initializer_record(initializer: onnx.TensorProto) -> dict[str, Any]:
    if initializer.data_type != TensorProto.FLOAT:
        raise ProfileError(f"initializer {initializer.name} is not FLOAT32")
    if initializer.external_data or initializer.data_location != TensorProto.DEFAULT:
        raise ProfileError(f"initializer {initializer.name} uses external data")
    payload = initializer.raw_data
    if not payload:
        raise ProfileError(f"initializer {initializer.name} has no raw_data")
    return {
        "name": initializer.name,
        "shape": list(initializer.dims),
        "element_count": len(payload) // 4,
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def _validate_onnx() -> dict[str, Any]:
    model = onnx.load(ONNX_PATH, load_external_data=False)
    onnx.checker.check_model(model, full_check=True)
    if len(model.graph.input) != 1 or len(model.graph.output) != 1:
        raise ProfileError("ONNX graph does not have one input and one output")
    input_shape = _shape(model.graph.input[0])
    output_shape = _shape(model.graph.output[0])
    if input_shape != EXPECTED_INPUT_SHAPE:
        raise ProfileError(f"unexpected ONNX input shape: {input_shape}")
    if output_shape != EXPECTED_OUTPUT_SHAPE:
        raise ProfileError(f"unexpected ONNX output shape: {output_shape}")

    nodes = tuple(node.op_type for node in model.graph.node)
    if nodes != EXPECTED_NODE_TYPES:
        raise ProfileError(f"unexpected ONNX nodes: {nodes}")
    opsets = {entry.domain: entry.version for entry in model.opset_import}
    if opsets != {"": EXPECTED_OPSET}:
        raise ProfileError(f"unexpected ONNX opsets: {opsets}")

    initializers = sorted(
        (_initializer_record(value) for value in model.graph.initializer),
        key=lambda record: record["name"],
    )
    expected_initializers = {
        "ctc_lo.bias": ([4999], 19_996),
        "onnx::MatMul_7": ([512, 4999], 10_237_952),
    }
    observed = {
        record["name"]: (record["shape"], record["bytes"]) for record in initializers
    }
    if observed != expected_initializers:
        raise ProfileError(f"unexpected ONNX initializers: {observed}")

    return {
        "path": _relative(ONNX_PATH),
        "sha256": EXPECTED_HASHES["onnx"],
        "bytes": ONNX_PATH.stat().st_size,
        "ir_version": model.ir_version,
        "opset": EXPECTED_OPSET,
        "input": {
            "name": model.graph.input[0].name,
            "dtype": "float32",
            "shape": list(input_shape),
            "elements": 16 * 512,
            "bytes": 16 * 512 * 4,
        },
        "output": {
            "name": model.graph.output[0].name,
            "dtype": "float32",
            "shape": list(output_shape),
            "elements": 16 * 4999,
            "bytes": 16 * 4999 * 4,
        },
        "nodes": list(nodes),
        "initializers": initializers,
        "matrix_macs_per_invocation": EXPECTED_MACS,
    }


def _validate_nbg() -> dict[str, Any]:
    document = nbg_format.parse_path(NBG_PATH)
    if document["magic"] != "VPMN":
        raise ProfileError("NBG magic differs")
    if document["version"] != 0x00020000:
        raise ProfileError("NBG is not VPMN v2")
    if document["product"] != 0x1000000E:
        raise ProfileError("NBG product differs")
    if document["size"] != EXPECTED_NBG_BYTES:
        raise ProfileError("NBG byte count differs")
    if len(document["inputs"]) != 1 or len(document["outputs"]) != 1:
        raise ProfileError("NBG does not have one input and one output")
    input_record = document["inputs"][0]
    output_record = document["outputs"][0]
    if (
        tuple(input_record["dimensions"]) != EXPECTED_NBG_INPUT_SHAPE
        or input_record["data_format_name"] != "fp16"
        or input_record["byte_size"] != 16_384
    ):
        raise ProfileError("NBG input contract differs")
    if (
        tuple(output_record["dimensions"]) != EXPECTED_NBG_OUTPUT_SHAPE
        or output_record["data_format_name"] != "fp16"
        or output_record["byte_size"] != 159_968
    ):
        raise ProfileError("NBG output contract differs")
    if document["warnings"]:
        raise ProfileError(f"NBG parser warnings: {document['warnings']}")
    if not all(record["matches"] for record in document["checksums"]):
        raise ProfileError("NBG has an invalid stored checksum")
    return {
        "path": _relative(NBG_PATH),
        "sha256": EXPECTED_HASHES["nbg"],
        "bytes": document["size"],
        "format": document["magic"],
        "version": document["version_hex"],
        "target": document["target_hex"],
        "product": document["product_hex"],
        "input": {
            "name": input_record["name"],
            "dtype": input_record["data_format_name"],
            "shape": input_record["dimensions"],
            "elements": 16 * 512,
            "bytes": input_record["byte_size"],
        },
        "output": {
            "name": output_record["name"],
            "dtype": output_record["data_format_name"],
            "shape": output_record["dimensions"],
            "elements": 16 * 4999,
            "bytes": output_record["byte_size"],
        },
        "stored_checksum_count": len(document["checksums"]),
        "stored_checksums_valid": True,
        "structural_signature": document["structural_signature"],
    }


def build_profile() -> dict[str, Any]:
    paths = {
        "onnx": ONNX_PATH,
        "nbg": NBG_PATH,
        "ort_library": ORT_LIBRARY_PATH,
        "ort_header": ORT_HEADER_PATH,
        "ort_license": ORT_LICENSE_PATH,
    }
    observed_hashes = {name: sha256(path) for name, path in paths.items()}
    if observed_hashes != EXPECTED_HASHES:
        raise ProfileError(f"fixed artifact hashes differ: {observed_hashes}")
    expected_sizes = {
        "onnx": EXPECTED_ONNX_BYTES,
        "nbg": EXPECTED_NBG_BYTES,
        "ort_library": EXPECTED_ORT_BYTES,
        "ort_header": EXPECTED_ORT_HEADER_BYTES,
    }
    observed_sizes = {name: paths[name].stat().st_size for name in expected_sizes}
    if observed_sizes != expected_sizes:
        raise ProfileError(f"fixed artifact sizes differ: {observed_sizes}")

    onnx_record = _validate_onnx()
    nbg_record = _validate_nbg()
    if onnx_record["input"]["elements"] != nbg_record["input"]["elements"]:
        raise ProfileError("logical input element counts differ")
    if onnx_record["output"]["elements"] != nbg_record["output"]["elements"]:
        raise ProfileError("logical output element counts differ")

    return {
        "schema_version": 1,
        "poc": "15A",
        "status": "host-profile-qualified",
        "relationship": "source-binary-anchor-pending-live-numerical-proof",
        "onnx": onnx_record,
        "nbg": nbg_record,
        "layout_mapping": {
            "input": {
                "onnx": "[batch=1,time=16,feature=512]",
                "nbg": "[feature=512,time=16,batch=1]",
                "linear_memory_order_equal": True,
            },
            "output": {
                "onnx": "[batch=1,time=16,class=4999]",
                "nbg": "[class=4999,time=16,batch=1]",
                "linear_memory_order_equal": True,
            },
        },
        "cpu_runtime": {
            "path": _relative(ORT_LIBRARY_PATH),
            "sha256": EXPECTED_HASHES["ort_library"],
            "bytes": ORT_LIBRARY_PATH.stat().st_size,
            "version": "1.15.1",
            "required_provider": "CPUExecutionProvider",
        },
        "api_header": {
            "source": (
                "https://raw.githubusercontent.com/microsoft/onnxruntime/"
                "v1.15.1/include/onnxruntime/core/session/"
                "onnxruntime_c_api.h"
            ),
            "path": _relative(ORT_HEADER_PATH),
            "sha256": EXPECTED_HASHES["ort_header"],
            "bytes": ORT_HEADER_PATH.stat().st_size,
            "ort_api_version": 15,
            "license": {
                "spdx": "MIT",
                "source": (
                    "https://raw.githubusercontent.com/microsoft/"
                    "onnxruntime/v1.15.1/LICENSE"
                ),
                "path": _relative(ORT_LICENSE_PATH),
                "sha256": EXPECTED_HASHES["ort_license"],
            },
        },
        "live_gate": {
            "fixture_count": 4,
            "top1_matches_required": 64,
            "mean_absolute_error_max": 0.025,
            "maximum_absolute_error_max": 0.25,
            "cosine_similarity_min": 0.999,
            "cold_pairs": 1,
            "warmup_pairs": 20,
            "measured_pairs": 200,
            "performance_boundary": "fp32-bridge-total",
            "ratio_of_means_max": 0.9,
            "paired_ratio_p50_max": 0.9,
            "paired_ratio_p95_max_exclusive": 1.0,
        },
        "interpretation": {
            "host_profile_proves": (
                "fixed shapes, types, operation sequence, memory-order map, "
                "runtime ABI source, and artifact identity"
            ),
            "host_profile_does_not_prove": (
                "the NBG contains numerically identical weights or that the "
                "NPU beats the CPU"
            ),
        },
    }


def main(argv: Sequence[str] | None = None) -> int:
    if argv:
        print("error: this fixed generator accepts no arguments")
        return 2
    try:
        profile = build_profile()
    except (OSError, ProfileError, onnx.checker.ValidationError) as exc:
        print(f"error: {exc}")
        return 2
    print(json.dumps(profile, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
