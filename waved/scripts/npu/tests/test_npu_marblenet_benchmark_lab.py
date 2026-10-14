import json
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_erf_execution_lab as execution
import npu_marblenet_benchmark_lab as benchmark


class MarbleNetBenchmarkTests(unittest.TestCase):
    def test_arbitrary_full_model_hash_is_not_a_benchmark_candidate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = b"arbitrary model with a self-consistent hash"
            (root / "model.onnx").write_bytes(payload)
            package = {
                "directory": str(root),
                "record": {
                    "profile_id": "fp16-marblenet-v1",
                    "source_model_sha256": benchmark.lab._digest(payload),
                },
            }
            with (
                mock.patch.object(benchmark.lab.npu_lab, "AdbTransport") as transport,
                self.assertRaises(benchmark.BenchmarkError),
            ):
                benchmark._full_model_identity(package)
            transport.assert_not_called()

    def test_iterations_are_explicit_and_old_policy_stays_one(self):
        self.assertEqual(execution.POC7_POLICY.expected_iterations, 1)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            document = json.loads(execution.POC7_POLICY.manifest_path.read_text())
            document["expected_result"]["iterations"] = 36
            path.write_text(json.dumps(document))
            policy = replace(
                execution.POC7_POLICY, manifest_path=path, expected_iterations=36
            )
            self.assertEqual(
                execution.load_manifest(policy=policy).expected_result["iterations"], 36
            )
            with self.assertRaises(execution.ExecutionManifestError):
                execution.load_manifest(policy=replace(policy, expected_iterations=1))

    def test_native_vad_gate_uses_actual_probabilities_and_decisions(self):
        reference = np.zeros((1, 2, 200), dtype="<f4")
        reference[:, 1, :] = 2
        native = reference.astype("<f2")
        result = benchmark.compare_vad(reference.tobytes(), native.tobytes())
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["native_speech_frames"], 200)
        self.assertEqual(result["decision_agreement"], 1.0)
        native[:, 1, :] = -2
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.compare_vad(reference.tobytes(), native.tobytes())

    def test_nonfinite_or_wrong_length_output_fails(self):
        reference = np.zeros((1, 2, 200), dtype="<f4").tobytes()
        for output in (bytes(799), np.full((1, 2, 200), np.nan, dtype="<f2").tobytes()):
            with (
                self.subTest(output_length=len(output)),
                self.assertRaises(benchmark.BenchmarkError),
            ):
                benchmark.compare_vad(reference, output)

    def test_ir8_change_preserves_every_other_model_field(self):
        import onnx
        from onnx import helper

        model = helper.make_model(helper.make_graph([], "empty", [], []))
        model.ir_version = 10
        derived = benchmark.ir8_model(model.SerializeToString())
        restored = onnx.load_model_from_string(derived)
        self.assertEqual(restored.ir_version, 8)
        restored.ir_version = 10
        self.assertEqual(restored.SerializeToString(), model.SerializeToString())

    def test_already_ir8_model_is_identical(self):
        from onnx import helper

        model = helper.make_model(helper.make_graph([], "empty", [], []))
        model.ir_version = 8
        self.assertEqual(
            benchmark.ir8_model(model.SerializeToString()), model.SerializeToString()
        )

    def test_resealed_preparation_refuses_before_transport(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            gate_path = directory / "gate.json"
            gate = {
                "clips": {
                    clip: {"run_id": "frozen-" + clip} for clip in benchmark.CLIPS
                }
            }
            gate_path.write_text(json.dumps(gate))
            preparation_path = directory / "preparation.json"
            preparation_path.write_text('{"original":true}')
            reviewed_digest = benchmark.lab._digest(preparation_path.read_bytes())
            preparation_path.write_text('{"resealed":true}')
            with (
                mock.patch.object(benchmark.lab, "_run_id"),
                mock.patch.object(benchmark.lab, "load_package", return_value={}),
                mock.patch.object(benchmark, "_preparation"),
                mock.patch.object(benchmark, "vad_gate", return_value=gate),
                mock.patch.object(benchmark.lab.npu_lab, "AdbTransport") as transport,
                self.assertRaises(benchmark.BenchmarkError),
            ):
                benchmark.run_benchmark(
                    directory,
                    directory,
                    gate_path,
                    directory,
                    "host-only-reseal",
                    reviewed_digest,
                )
            transport.assert_not_called()
            self.assertFalse((directory / "attempt.json").exists())


if __name__ == "__main__":
    unittest.main()
