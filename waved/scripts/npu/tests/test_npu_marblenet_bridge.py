import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_onnx_native_lab as lab


class MarbleNetBridgeTests(unittest.TestCase):
    def test_new_profile_root_is_separate_from_existing_packages(self):
        record = {
            "profile_id": "fp16-marblenet-v1",
            "lowering_sha256": "ab" * 32,
        }
        self.assertEqual(
            lab.remote_root(record, "compile"),
            "/tmp/wave-npu-onnx18-" + "ab" * 8 + "-compile",
        )

    def test_numeric_result_parser_retains_measured_not_reference_bytes(self):
        policy = mock.Mock(
            input_values=(1, 2),
            output_values=(0, 0),
            execution_re=re.compile(
                r"EXECUTION status=PASS graph_us=(?P<graph_us>[0-9]+) "
                r"output_hex=(?P<output_hex>[0-9a-f]{4})"
            ),
        )
        manifest = mock.Mock(
            policy=policy,
            expected_result={"identity": lab.execution.EXPECTED_IDENTITY},
        )
        identity = "IDENTITY " + " ".join(
            f"{key}={value}" for key, value in lab.execution.EXPECTED_IDENTITY.items()
        )
        result = lab.execution.parse_result(
            identity + "\nEXECUTION status=PASS graph_us=23 output_hex=1234\n",
            manifest,
        )
        self.assertEqual(result["output"], [0x12, 0x34])
        self.assertEqual(result["output_hex"], "1234")

    def test_poc18_service_respawn_refuses(self):
        manifest = mock.Mock(serial="wave", policy=mock.Mock(poc_id="18"))
        before = {
            "serials": ["wave"],
            "dmesg_tail": ["marker"],
            "processes": ["2197 root /data/waved/waved"],
        }
        after = {**before, "processes": ["2201 root /data/waved/waved"]}
        differences, _ = lab.compile_lab.compare_health(manifest, before, after)
        self.assertIn("service process identities changed", differences)

    def test_graph_tensor_byte_bound_does_not_relax_old_profile(self):
        graph = {"profile_id": "fp16-marblenet-v1", "onnx_input": {"byte_size": 64000}}
        self.assertEqual(lab._tensor_bytes(graph, "onnx_input"), 64000)
        for profile, size in (
            ("fp16-conv-v1", 64000),
            ("fp16-marblenet-v1", 8 * 1024 * 1024 + 1),
        ):
            with (
                self.subTest(profile=profile, size=size),
                self.assertRaises(lab.NativeLabError),
            ):
                lab._tensor_bytes(
                    {"profile_id": profile, "onnx_input": {"byte_size": size}},
                    "onnx_input",
                )

    def test_tp_target_is_only_allowed_with_explicit_graph_policy(self):
        stdout = (
            "operation target VXNNE_OPERATION_TARGET_NN, conv\n"
            "operation target VXNNE_OPERATION_TARGET_TP, add\n"
        )
        self.assertFalse(lab._nn_target(stdout))
        self.assertTrue(lab._nn_target(stdout, allow_tp=True))
        self.assertFalse(
            lab._nn_target(stdout.replace("TARGET_NN", "TARGET_CL"), allow_tp=True)
        )

    def test_old_profile_remains_byte_exact(self):
        record = {"profile_id": "fp16-conv-v1", "onnx_output": {"byte_size": 32}}
        with self.assertRaises(lab.NativeLabError):
            lab._compare_output(record, bytes(32), bytes([1]) + bytes(31))

    def test_graph_manifest_accepts_real_feature_size_and_variable_readback(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            input_path = directory / "input.bin"
            input_path.write_bytes(bytes(64000))
            record = {
                "profile_id": "fp16-marblenet-v1",
                "lowering_sha256": "ab" * 32,
                "onnx_input": {"byte_size": 64000},
                "onnx_output": {"byte_size": 800},
            }
            package = {
                "directory": str(directory),
                "record": record,
                "compiled": {"nbg_bytes": 1800, "nbg_sha256": "c" * 64},
            }
            with mock.patch.object(
                lab, "_load_build", return_value={"binary_sha256": "d" * 64}
            ):
                manifest = lab._execute_manifest(
                    package, input_path, bytes(800), directory
                )
            self.assertEqual(manifest.policy.poc_id, "18")
            measured = bytes([1]) + bytes(799)
            line = (
                "EXECUTION status=PASS mode=execute-once target=wave operation=fp16-marblenet "
                "iterations=1 input_bytes=64000 output_bytes=800 graph_us=99 output_hex="
                + measured.hex()
                + " cleanup=0"
            )
            self.assertIsNotNone(manifest.policy.execution_re.fullmatch(line))

    def test_numeric_cpu_evidence_retains_actual_cpu_and_independent_bytes(self):
        import numpy as np

        expected = np.zeros((1, 2, 200), dtype=np.float16)
        cpu_output = np.full((1, 2, 200), 0.001, dtype=np.float16)
        inputs = {"features": np.zeros((1, 80, 400), dtype=np.float16)}
        record = {
            "profile_id": "fp16-marblenet-v1",
            "onnx_input": {"name": "features", "byte_size": 64000},
            "onnx_output": {"name": "logits", "byte_size": 800},
        }
        importer = mock.Mock()
        importer.independent_oracle.return_value = {"logits": expected}
        cpu = mock.Mock(
            outputs={"logits": cpu_output},
            evidence={"providers": ["CPUExecutionProvider"]},
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with (
                mock.patch.object(lab, "_host", return_value=(importer, None)),
                mock.patch.object(
                    lab, "_compare_output", return_value={"status": "PASS"}
                ),
                mock.patch("wave_onnx_hybrid.audit.audit_model_path"),
                mock.patch(
                    "wave_onnx_hybrid.runtime.execute_cpu_model", return_value=cpu
                ),
            ):
                _, reference = lab._cpu_expected(
                    {"directory": str(root), "record": record},
                    inputs,
                    evidence_path=root / "qualification-cpu-oracle.json",
                )
            self.assertEqual(reference, expected.tobytes())
            self.assertEqual(
                (root / "qualification-cpu-output.bin").read_bytes(),
                cpu_output.tobytes(),
            )
            self.assertEqual(
                (root / "qualification-independent-output.bin").read_bytes(),
                expected.tobytes(),
            )


if __name__ == "__main__":
    unittest.main()
