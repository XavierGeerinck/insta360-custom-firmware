"""Four deterministic PoC 17 controls, each checked by ORT CPU and NumPy."""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

import numpy as np
import onnx
import onnxruntime as ort
from onnx import TensorProto, helper, numpy_helper

from .audit import canonical_json
from .errors import ErrorCode, HybridError
from .fixtures import _write_exclusive, deterministic_npz_bytes
from .native_import import import_native_model, independent_oracle

CONTROLS = ("17a1", "17a2", "17b1", "17b2")


def _model(control: str) -> tuple[onnx.ModelProto, dict[str, np.ndarray]]:
    if control.startswith("17a"):
        alternate = control == "17a2"
        shape = [1, 1, 1, 16] if alternate else [1, 1, 16]
        weights = np.array(
            [0.5, 0.25, -0.25] if alternate else [0.25, 0.5, 0.25], dtype=np.float16
        )
        weight_shape = [1, 1, 1, 3] if alternate else [1, 1, 3]
        constants = [numpy_helper.from_array(weights.reshape(weight_shape), "weight")]
        node_inputs = ["input", "weight"]
        if alternate:
            constants.append(
                numpy_helper.from_array(np.array([0.125], dtype=np.float16), "bias")
            )
            node_inputs.append("bias")
        node = helper.make_node(
            "Conv",
            node_inputs,
            ["output"],
            name=f"{control}.conv",
            pads=[0, 1, 0, 1] if alternate else [1, 1],
        )
        values = (
            (
                (np.arange(16, dtype=np.float32) - 8) / 8
                if alternate
                else np.where(np.arange(16) % 2 == 0, 1, -1)
            )
            .astype(np.float16)
            .reshape(shape)
        )
        dtype = TensorProto.FLOAT16
        output_shape = shape
    else:
        weights = np.zeros((16, 4), dtype=np.uint8)
        if control == "17b1":
            for j, k in enumerate((0, 5, 10, 15)):
                weights[k, j] = 1
        else:
            for k in range(16):
                for j in range(4):
                    weights[k, j] = int((k + 2 * j) % 5 == 0)
        constants = [numpy_helper.from_array(weights, "weight")]
        for prefix in ("a", "b", "y"):
            constants.extend(
                [
                    numpy_helper.from_array(
                        np.array(1, dtype=np.float32), f"{prefix}_scale"
                    ),
                    numpy_helper.from_array(
                        np.array(0, dtype=np.uint8), f"{prefix}_zero_point"
                    ),
                ]
            )
        node = helper.make_node(
            "QLinearMatMul",
            [
                "input",
                "a_scale",
                "a_zero_point",
                "weight",
                "b_scale",
                "b_zero_point",
                "y_scale",
                "y_zero_point",
            ],
            ["output"],
            name=f"{control}.matmul",
        )
        shape, output_shape = [1, 16], [1, 4]
        values = np.arange(16, dtype=np.uint8).reshape(shape)
        dtype = TensorProto.UINT8
    graph = helper.make_graph(
        [node],
        f"wave-native-{control}",
        [helper.make_tensor_value_info("input", dtype, shape)],
        [helper.make_tensor_value_info("output", dtype, output_shape)],
        initializer=constants,
    )
    model = helper.make_model(
        graph,
        ir_version=10,
        opset_imports=[helper.make_opsetid("", 13)],
        producer_name="wave-native-poc17-fixtures",
        producer_version="1",
    )
    onnx.checker.check_model(model, full_check=True)
    return model, {"input": values}


def generate_native_fixtures(path: Path) -> dict[str, Any]:
    """Write the frozen controls exclusively, after exact CPU/oracle agreement.

    ZIP timestamps and entry order are fixed; no wall-clock data enters hashes.
    Rollback removes only files created by this invocation.
    """
    path = Path(path)
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "generator": "wave_onnx_hybrid.native_fixtures.generate_native_fixtures",
        "files": {},
        "controls": [],
    }
    expected = [
        "manifest.json",
        *(
            f"{control}{suffix}"
            for control in CONTROLS
            for suffix in (".onnx", "_input.npz", "_expected.npz")
        ),
    ]
    if any((path / name).exists() for name in expected):
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS, "native fixture output already exists"
        )
    written: list[Path] = []
    try:
        for control in CONTROLS:
            model, inputs = _model(control)
            model_name = f"{control}.onnx"
            payload = model.SerializeToString(deterministic=True)
            model_path = path / model_name
            _write_exclusive(model_path, payload)
            written.append(model_path)
            record = import_native_model(model_path)
            expected_output = independent_oracle(record, inputs)
            session = ort.InferenceSession(payload, providers=["CPUExecutionProvider"])
            actual = session.run(None, inputs)[0]
            oracle = expected_output[record["onnx_output"]["name"]]
            if (
                actual.dtype != oracle.dtype
                or actual.shape != oracle.shape
                or actual.tobytes() != oracle.tobytes()
            ):
                raise HybridError(
                    ErrorCode.NPU_OUTPUT_NUMERICAL_MISMATCH,
                    f"ORT CPU/independent oracle disagree for {control}",
                )
            files = {
                model_name: payload,
                f"{control}_input.npz": deterministic_npz_bytes(inputs),
                f"{control}_expected.npz": deterministic_npz_bytes(expected_output),
            }
            for name, content in sorted(files.items()):
                if name != model_name:
                    file_path = path / name
                    _write_exclusive(file_path, content)
                    written.append(file_path)
                manifest["files"][name] = {
                    "sha256": hashlib.sha256(content).hexdigest(),
                    "byte_size": len(content),
                }
            manifest["controls"].append(
                {
                    "control_id": control.upper(),
                    "model": model_name,
                    "inputs": f"{control}_input.npz",
                    "expected": f"{control}_expected.npz",
                    "ir_version": 10,
                    "opset": 13,
                    "profile_id": record["profile_id"],
                    "source_model_sha256": record["source_model_sha256"],
                    "graph_signature": record["graph_signature"],
                    "lowering_sha256": record["lowering_sha256"],
                    "weights_sha256": record["weights"]["sha256"],
                    "bias_sha256": record["bias"]["sha256"],
                    "input_payload_sha256": hashlib.sha256(
                        inputs["input"].tobytes()
                    ).hexdigest(),
                    "expected_payload_sha256": hashlib.sha256(
                        oracle.tobytes()
                    ).hexdigest(),
                    "cpu_oracle_byte_agreement": True,
                }
            )
        _write_exclusive(path / "manifest.json", canonical_json(manifest))
        return manifest
    except BaseException:
        for created in reversed(written):
            created.unlink(missing_ok=True)
        raise
