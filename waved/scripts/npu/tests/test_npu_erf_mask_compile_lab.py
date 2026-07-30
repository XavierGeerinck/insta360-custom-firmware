from __future__ import annotations

import json
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_compile_lab as lab  # noqa: E402
import npu_lab  # noqa: E402


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = lab.ERF_MASK_POLICY
        self.manifest = lab.load_manifest(policy=self.policy)

    def _document(self) -> dict:
        return json.loads(
            self.policy.manifest_path.read_text(encoding="utf-8")
        )

    def _load(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path, policy=self.policy)

    def test_manifest_is_exact_external_erf_481_compile_only(self) -> None:
        self.assertEqual(self.policy.poc_id, "13B")
        self.assertEqual(self.policy.remote_root, "/tmp/wave-npu-poc13b")
        self.assertEqual(tuple(self.manifest.candidates), ("a1", "a2"))
        self.assertEqual(
            [value.argument for value in self.manifest.candidates.values()],
            ["--a1", "--a2"],
        )
        self.assertEqual(self.policy.inputs[0].dimensions, (481,))
        self.assertEqual(self.policy.inputs[0].byte_size, 481)
        self.assertEqual(
            self.policy.inputs[0].quantization_scale, 0.03125
        )
        self.assertEqual(
            self.policy.inputs[0].quantization_zero_point, 128
        )
        self.assertEqual(
            self.policy.outputs[0].quantization_scale, 0.0078125
        )
        self.assertEqual(
            self.policy.outputs[0].quantization_zero_point, 128
        )
        binary_hash = self.manifest.required_artifacts[
            "/tmp/wave-npu-poc13b/bin/wave-npu-compile-erf-mask-v1"
        ]
        self.assertNotEqual(binary_hash, "0" * 64)
        self.assertRegex(binary_hash, r"^[0-9a-f]{64}$")

    def test_manifest_rejects_other_root_argument_and_output(self) -> None:
        mutations = []
        document = self._document()
        document["remote_executable"] = "/tmp/other/bin/compiler"
        mutations.append(document)
        document = self._document()
        document["candidates"]["a1"]["argument"] = "--model"
        mutations.append(document)
        document = self._document()
        document["candidates"]["a2"]["output"] = "/tmp/other.nbg"
        mutations.append(document)
        for changed in mutations:
            with self.assertRaises(lab.ManifestError):
                self._load(changed)

    def test_manifest_rejects_loader_interposition_and_placeholder_hash(
        self,
    ) -> None:
        document = self._document()
        document["environment"]["LD_PRELOAD"] = "/tmp/hook.so"
        with self.assertRaises(lab.ManifestError):
            self._load(document)

        document = self._document()
        binary = self.policy.remote_executable
        document["required_artifacts"][binary] = "not-a-digest"
        with self.assertRaises(lab.ManifestError):
            self._load(document)


class NativeAndNbgContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest(policy=lab.ERF_MASK_POLICY)
        self.candidate = self.manifest.candidates["a1"]

    def _stdout(self) -> str:
        return "\n".join(
            [
                (
                    "IDENTITY chip_model=0x00009000 "
                    "chip_revision=0x00009002 "
                    "chip_product_id=0x05090001 "
                    "chip_eco_id=0x04000000 "
                    "customer_id=0x1000000e "
                    "reserved=0x00000000 "
                    "flags=0x0000000000000000"
                ),
                'D [selector]Instance OPENVX node with kernel "erf"',
                (
                    "RESULT status=PASS mode=compile-only target=wave "
                    "candidate=a1 bytes=8192 version=0x00020000 "
                    "product=0x1000000e "
                    f"output={self.candidate.output}"
                ),
            ]
        )

    def _report(self) -> dict:
        return {
            "magic": "VPMN",
            "version_hex": "0x00020000",
            "product_hex": "0x1000000e",
            "size": 8192,
            "inputs": [
                {
                    "dimensions": [481],
                    "data_format_name": "uint8",
                    "byte_size": 481,
                    "quantization": {
                        "format_name": "affine-asymmetric",
                        "scale": 0.03125,
                        "zero_point": 128,
                    },
                }
            ],
            "outputs": [
                {
                    "dimensions": [481],
                    "data_format_name": "uint8",
                    "byte_size": 481,
                    "quantization": {
                        "format_name": "affine-asymmetric",
                        "scale": 0.0078125,
                        "zero_point": 128,
                    },
                }
            ],
        }

    def test_exact_native_result_and_nbg_contract_pass(self) -> None:
        parsed = lab.parse_native_result(
            self._stdout(), self.manifest, self.candidate
        )
        self.assertEqual(parsed["result"]["candidate"], "a1")
        self.assertEqual(
            parsed["selector"], {"backend": "OPENVX", "kernel": "erf"}
        )
        lab.validate_nbg_report(self._report(), self.manifest)

    def test_wrong_shape_scale_zero_point_and_backend_fail(self) -> None:
        for direction, field, value in (
            ("inputs", "dimensions", [480]),
            ("inputs", "quantization.scale", 0.0625),
            ("outputs", "quantization.zero_point", 127),
        ):
            report = self._report()
            if field == "dimensions":
                report[direction][0][field] = value
            else:
                _, leaf = field.split(".")
                report[direction][0]["quantization"][leaf] = value
            with self.assertRaises(lab.ResultError):
                lab.validate_nbg_report(report, self.manifest)
        with self.assertRaisesRegex(lab.ResultError, "selected EVIS"):
            lab.parse_native_result(
                self._stdout().replace("Instance OPENVX", "Instance EVIS"),
                self.manifest,
                self.candidate,
            )


class PreflightAndTransportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest(policy=lab.ERF_MASK_POLICY)
        self.candidate = self.manifest.candidates["a1"]

    def _snapshot(self, *, output_present: bool = False) -> dict:
        outputs = {}
        for candidate in self.manifest.candidates.values():
            outputs[candidate.output] = {"status": "absent"}
            outputs[candidate.partial_output] = {"status": "absent"}
        if output_present:
            outputs[self.candidate.output] = {
                "status": "present",
                "bytes": 8192,
                "permissions": "-r--r--r--",
                "sha256": "a" * 64,
            }
        return {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "01234567-89ab-cdef-0123-456789abcdef",
            "uptime": "10.0 2.0",
            "galcore_device": (
                "crw-rw---- 1 0 0 199, 0 Jul 29 00:00 /dev/galcore"
            ),
            "npu_parameters": dict(self.manifest.npu_parameters),
            "galcore_holders": [],
            "processes": [],
            "artifact_hashes": dict(self.manifest.required_artifacts),
            "absent_artifact_states": {
                path: {"status": "absent"}
                for path in self.manifest.policy.absent_artifacts
            },
            "output_states": outputs,
            "dmesg_tail": ["baseline"],
            "errors": [],
        }

    def test_preflight_rejects_holder_clc_and_existing_output(self) -> None:
        lab.verify_preflight(
            self.manifest, self.candidate, self._snapshot()
        )
        changed = self._snapshot()
        changed["galcore_holders"] = [{"pid": 9, "fd": 4}]
        with self.assertRaises(lab.PreflightError):
            lab.verify_preflight(self.manifest, self.candidate, changed)
        changed = self._snapshot()
        clc = self.manifest.policy.absent_artifacts[0]
        changed["absent_artifact_states"][clc] = {
            "status": "present"
        }
        with self.assertRaises(lab.PreflightError):
            lab.verify_preflight(self.manifest, self.candidate, changed)
        with self.assertRaises(lab.PreflightError):
            lab.verify_preflight(
                self.manifest,
                self.candidate,
                self._snapshot(output_present=True),
            )

    def test_one_run_invokes_one_native_process_and_timeout_never_pulls(
        self,
    ) -> None:
        identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        candidate_output = self.candidate.output

        class FakeTransport:
            def __init__(self, *, timed_out: bool = False) -> None:
                self.calls: list[tuple[str, ...]] = []
                self.timed_out = timed_out

            def run(
                self, argv: tuple[str, ...], _timeout_ms: int
            ) -> npu_lab.CommandResult:
                self.calls.append(argv)
                sentinel = re.search(
                    r"__WAVE_COMPILE_EXIT_[0-9a-f]{32}__", argv[1]
                )
                assert sentinel is not None
                stdout = "\n".join(
                    [
                        identity,
                        'D [selector]Instance OPENVX node with kernel "erf"',
                        (
                            "RESULT status=PASS mode=compile-only "
                            "target=wave candidate=a1 bytes=8192 "
                            "version=0x00020000 product=0x1000000e "
                            f"output={candidate_output}"
                        ),
                        f"{sentinel.group(0)}:0",
                    ]
                )
                return npu_lab.CommandResult(
                    stdout=stdout,
                    stderr="",
                    returncode=0,
                    duration_ms=1,
                    timed_out=self.timed_out,
                )

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            transport = FakeTransport()
            with (
                mock.patch.object(lab, "DEFAULT_RUN_ROOT", root),
                mock.patch.object(
                    lab,
                    "collect_snapshot",
                    side_effect=[
                        self._snapshot(),
                        self._snapshot(output_present=True),
                    ],
                ),
                mock.patch.object(
                    lab,
                    "pull_and_parse",
                    return_value=(
                        {
                            "path": "artifact.nbg",
                            "bytes": 8192,
                            "sha256": "a" * 64,
                        },
                        self._report_fixture(),
                    ),
                ) as pull,
            ):
                result, _ = lab.run_candidate(
                    transport, self.manifest, "a1", "fake-a1"
                )
            self.assertEqual(result["status"], "PASS")
            self.assertEqual(len(transport.calls), 1)
            pull.assert_called_once()

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            transport = FakeTransport(timed_out=True)
            with (
                mock.patch.object(lab, "DEFAULT_RUN_ROOT", root),
                mock.patch.object(
                    lab,
                    "collect_snapshot",
                    side_effect=[
                        self._snapshot(),
                        self._snapshot(),
                    ],
                ),
                mock.patch.object(lab, "pull_and_parse") as pull,
            ):
                result, _ = lab.run_candidate(
                    transport, self.manifest, "a1", "fake-timeout"
                )
            self.assertEqual(result["status"], "FAIL")
            pull.assert_not_called()

    @staticmethod
    def _report_fixture() -> dict:
        return {"fixture": True}


class SourceAndBuildPolicyTests(unittest.TestCase):
    def test_native_source_is_closed_compile_only_graph(self) -> None:
        source = (
            lab.REPOSITORY_ROOT
            / "waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc"
        ).read_text(encoding="utf-8")
        self.assertEqual(source.count("CompileToBinary("), 2)
        self.assertEqual(
            source.count("CreateOperation<tim::vx::ops::Erf>()"), 1
        )
        self.assertIn("const tim::vx::ShapeType shape{481};", source)
        self.assertIn(
            "tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128", source
        )
        self.assertIn(
            "tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128", source
        )
        for forbidden in (
            "CopyDataToTensor(",
            "CopyDataFromTensor(",
            "Graph::Run",
            "ioctl(",
            "libCLC",
        ):
            self.assertNotIn(forbidden, source)

    def test_build_and_stage_policies_are_pinned_and_placeholder_free(
        self,
    ) -> None:
        root = lab.REPOSITORY_ROOT / "waved/tools/npu-compiler"
        build = (root / "build-device-erf-mask.sh").read_text(
            encoding="utf-8"
        )
        policy = (root / "verify-device-erf-mask-policy.sh").read_text(
            encoding="utf-8"
        )
        stage = (root / "stage-device-erf-mask.sh").read_text(
            encoding="utf-8"
        )
        self.assertEqual(build.count("docker run "), 1)
        self.assertIn("--network=none", build)
        self.assertIn("docker image inspect", build)
        self.assertIn("Graph::Run", policy)
        self.assertIn("libCLC", policy)
        self.assertIn("refusing pre-existing remote root", stage)
        self.assertNotIn("__BINARY_SHA256__", stage)
        self.assertNotIn("reboot", stage)
        self.assertNotIn("reset", stage)


if __name__ == "__main__":
    unittest.main()
