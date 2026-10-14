"""Optional host integrity checks against retained native qualification packages."""

import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_onnx_native_lab as lab


@unittest.skipUnless(
    os.environ.get("WAVE_ONNX_QUALIFIED_PACKAGES"),
    "set WAVE_ONNX_QUALIFIED_PACKAGES to retained PoC 17 packages",
)
class QualifiedPackageIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.packages = Path(os.environ["WAVE_ONNX_QUALIFIED_PACKAGES"])
        self.controls = tuple(
            os.environ.get("WAVE_ONNX_QUALIFIED_CONTROLS", "17a1,17a2,17b1,17b2").split(
                ","
            )
        )
        if not self.controls or any(
            control not in {"17a1", "17a2", "17b1", "17b2", "18a", "18b"}
            for control in self.controls
        ):
            self.fail("unknown qualified control selection")

    def test_four_real_strict_sessions_load_without_cpu_session_or_transport(self):
        lab._host()
        from wave_onnx_hybrid.native_backend import NativeSession
        from wave_onnx_hybrid.runtime import ExecutionMode

        cpu_factory = mock.Mock(
            side_effect=AssertionError("strict session created a CPU fallback")
        )
        with (
            mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
            mock.patch(
                "wave_onnx_hybrid.runtime.execute_cpu_model",
                side_effect=AssertionError("strict loading created a CPU session"),
            ) as cpu,
        ):
            for control in self.controls:
                with self.subTest(control=control):
                    package = lab.load_package(
                        self.packages / control, require_qualification=True
                    )
                    self.assertEqual(package["qualification"]["status"], "PASS")
                    session = NativeSession(
                        self.packages / control / "model.onnx",
                        self.packages / control,
                        run_id="host-strict-integration-" + control,
                        mode=ExecutionMode.STRICT,
                        cpu_factory=cpu_factory,
                    )
                    self.assertEqual(session.providers, ["WaveNativeBackend"])
                    self.assertEqual(session.evidence["cpu_sessions"], 0)
                    self.assertEqual(session.evidence["native_attempts"], 0)
            transport.assert_not_called()
            cpu.assert_not_called()
            cpu_factory.assert_not_called()

    def test_changed_real_artifacts_refuse_before_cpu_session_or_transport(self):
        source = self.packages / self.controls[0]
        qualification = lab._json(source / "qualification.json")
        stage_input = "evidence/" + qualification["run_id"] + "-stage/input.bin"
        artifacts = (
            "model.onnx",
            "compile.cc",
            "build/wave-npu-onnx-compile-v1",
            "model.nbg",
            "qualification.json",
            "qualification-cpu-oracle.json",
            "evidence/qualification/run.json",
            "evidence/qualification/output.bin",
            "evidence/qualification/cleanup.json",
            stage_input,
        )
        if (source / "qualification-independent-output.bin").exists():
            artifacts += (
                "qualification-independent-output.bin",
                "qualification-cpu-output.bin",
                "evidence/qualification/numerical-comparison.json",
            )
        with (
            mock.patch.object(lab.npu_lab, "AdbTransport") as transport,
            mock.patch(
                "wave_onnx_hybrid.runtime.execute_cpu_model",
                side_effect=AssertionError("refusal created a CPU session"),
            ) as cpu,
        ):
            for artifact in artifacts:
                with (
                    self.subTest(artifact=artifact),
                    tempfile.TemporaryDirectory() as tmp,
                ):
                    package = Path(tmp) / "package"
                    shutil.copytree(source, package)
                    path = package / artifact
                    payload = path.read_bytes()
                    path.chmod(0o600)
                    path.write_bytes(bytes([payload[0] ^ 1]) + payload[1:])
                    with self.assertRaises(lab.NativeLabError):
                        lab.load_package(package, require_qualification=True)
            transport.assert_not_called()
            cpu.assert_not_called()


if __name__ == "__main__":
    unittest.main()
