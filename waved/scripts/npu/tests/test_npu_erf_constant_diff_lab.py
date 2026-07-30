#!/usr/bin/env python3
"""Host-policy tests for the PoC 12B fixed constant differential."""

from __future__ import annotations

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_erf_constant_diff_lab as lab  # noqa: E402


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path)

    def test_real_manifest_is_fixed_and_compile_only(self) -> None:
        self.assertEqual(self.manifest.poc_id, "poc12b-erf-constant-diff")
        self.assertEqual(self.manifest.remote_root, "/tmp/wave-npu-poc12b")
        self.assertEqual(
            tuple(self.manifest.candidates),
            ("a1", "a2", "b"),
        )
        self.assertEqual(self.manifest.candidates["a1"].constant_u8, (0, 1, 2, 3))
        self.assertEqual(self.manifest.candidates["a2"].constant_u8, (0, 1, 2, 3))
        self.assertEqual(self.manifest.candidates["b"].constant_u8, (0, 1, 2, 4))
        self.assertEqual(self.manifest.graph.operation, "Erf")
        self.assertEqual(self.manifest.graph.attribute, "CONSTANT")
        self.assertTrue(
            all(value == 0 for value in self.manifest.prohibited_actions.values())
        )
        self.assertEqual(
            self.manifest.approved_build_provenance["container_runs"], 1
        )

    def test_duplicate_key_is_rejected(self) -> None:
        payload = lab.DEFAULT_MANIFEST.read_text(encoding="utf-8")
        payload = payload.replace(
            '"schema_version": 1,',
            '"schema_version": 1, "schema_version": 1,',
            1,
        )
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(payload, encoding="utf-8")
            with self.assertRaisesRegex(lab.ManifestError, "duplicate JSON key"):
                lab.load_manifest(path)

    def test_unknown_key_and_environment_interposition_are_rejected(self) -> None:
        document = self._document()
        document["unknown"] = True
        with self.assertRaisesRegex(lab.ManifestError, "unknown keys"):
            self._load(document)

        document = self._document()
        document["environment"]["LD_PRELOAD"] = "/tmp/hook.so"
        with self.assertRaisesRegex(lab.ManifestError, "environment"):
            self._load(document)

    def test_candidate_order_constants_and_paths_are_fixed(self) -> None:
        document = self._document()
        document["candidates"][1]["constant_u8"][-1] = 9
        with self.assertRaisesRegex(lab.ManifestError, "fixed constant"):
            self._load(document)

        document = self._document()
        document["candidates"][2]["output"] = "/tmp/other.nbg"
        with self.assertRaisesRegex(lab.ManifestError, "fixed output"):
            self._load(document)

        document = self._document()
        document["candidates"].reverse()
        with self.assertRaisesRegex(lab.ManifestError, "candidate order"):
            self._load(document)

    def test_nonzero_prohibited_action_is_rejected(self) -> None:
        document = self._document()
        document["policy"]["prohibited_actions"]["graph_execution_calls"] = 1
        with self.assertRaisesRegex(lab.ManifestError, "must be zero"):
            self._load(document)

    def test_manifest_is_frozen(self) -> None:
        with self.assertRaises(TypeError):
            self.manifest.required_artifacts["/tmp/new"] = "0" * 64


class NativeContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.candidate = self.manifest.candidates["a1"]

    def _stdout(self, *, candidate: str = "A1") -> str:
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
                    "RESULT status=PASS mode=compile-only "
                    f"candidate={candidate} bytes=5840 "
                    "version=0x00020000 product=0x1000000e "
                    f"output={self.candidate.output}"
                ),
            ]
        )

    def test_exact_native_result_parses(self) -> None:
        result = lab.parse_native_result(
            self._stdout(), self.manifest, self.candidate
        )
        self.assertEqual(result.bytes, 5840)
        self.assertEqual(result.candidate_id, "a1")
        self.assertEqual(result.selector_backend, "OPENVX")

    def test_wrong_candidate_and_duplicate_result_are_rejected(self) -> None:
        with self.assertRaisesRegex(lab.ResultError, "candidate"):
            lab.parse_native_result(
                self._stdout(candidate="B"), self.manifest, self.candidate
            )
        with self.assertRaisesRegex(lab.ResultError, "one identity and result"):
            text = self._stdout()
            lab.parse_native_result(
                f"{text}\n{text.splitlines()[-1]}",
                self.manifest,
                self.candidate,
            )

    def test_remote_command_is_fixed_and_has_no_execution_or_retry(self) -> None:
        command = lab.build_remote_command(
            self.manifest,
            self.candidate,
            "__WAVE_POC12B_EXIT_0123456789abcdef0123456789abcdef__",
        )
        self.assertIn(self.manifest.remote_executable, command)
        self.assertIn("--a1", command)
        self.assertNotIn("Run", command)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("LD_PRELOAD", command)
        self.assertNotIn("/dev/galcore", command)


class SnapshotContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _snapshot(self, *, staged: bool) -> dict:
        artifact_states = {}
        for path, digest in self.manifest.required_artifacts.items():
            under_stage = path.startswith(
                f"{self.manifest.remote_root}/"
            )
            if under_stage and not staged:
                artifact_states[path] = {"status": "absent"}
            else:
                artifact_states[path] = {
                    "bytes": 1,
                    "permissions": "-r--r--r--",
                    "sha256": digest,
                    "status": "present",
                }
        output_states = {}
        for candidate in self.manifest.candidates.values():
            output_states[candidate.output] = {"status": "absent"}
            output_states[candidate.partial_output] = {
                "status": "absent"
            }
        return {
            "artifact_states": artifact_states,
            "boot_id": "01234567-89ab-cdef-0123-456789abcdef",
            "dmesg_tail": ["baseline"],
            "errors": [],
            "galcore_device": (
                "crw-rw---- 1 0 0 199, 0 Jul 29 00:00 /dev/galcore"
            ),
            "galcore_holders": [],
            "kernel": self.manifest.device.kernel,
            "npu_parameters": dict(
                self.manifest.device.npu_parameters
            ),
            "output_states": output_states,
            "probe_processes": [],
            "remote_root_state": "directory" if staged else "absent",
            "serials": [self.manifest.device.serial],
            "tmp_available_bytes": lab.MINIMUM_TMP_BYTES,
            "uptime": "10.00 2.00",
        }

    def test_unstaged_and_staged_snapshots_pass(self) -> None:
        lab.verify_snapshot(
            self.manifest, self._snapshot(staged=False), staged=False
        )
        lab.verify_snapshot(
            self.manifest, self._snapshot(staged=True), staged=True
        )

    def test_wrong_serial_holder_root_space_and_hash_fail(self) -> None:
        cases = []
        snapshot = self._snapshot(staged=False)
        snapshot["serials"] = []
        cases.append(snapshot)
        snapshot = self._snapshot(staged=False)
        snapshot["galcore_holders"] = [{"pid": 1, "fd": 2}]
        cases.append(snapshot)
        snapshot = self._snapshot(staged=False)
        snapshot["remote_root_state"] = "directory"
        cases.append(snapshot)
        snapshot = self._snapshot(staged=False)
        snapshot["tmp_available_bytes"] = lab.MINIMUM_TMP_BYTES - 1
        cases.append(snapshot)
        snapshot = self._snapshot(staged=False)
        system_path = next(
            path
            for path in self.manifest.required_artifacts
            if not path.startswith(f"{self.manifest.remote_root}/")
        )
        snapshot["artifact_states"][system_path]["sha256"] = "0" * 64
        cases.append(snapshot)
        for candidate in cases:
            with self.assertRaises(lab.PreflightError):
                lab.verify_snapshot(
                    self.manifest, candidate, staged=False
                )

    def test_new_kernel_alert_fails_health_comparison(self) -> None:
        before = self._snapshot(staged=True)
        after = copy.deepcopy(before)
        after["dmesg_tail"].append("galcore MMU fault")
        differences, alerts = lab.compare_health(
            self.manifest, before, after, require_staged=True
        )
        self.assertTrue(alerts)
        self.assertTrue(differences)

    def test_remote_exit_is_exact_and_bounded(self) -> None:
        sentinel = (
            "__WAVE_POC12B_EXIT_"
            "0123456789abcdef0123456789abcdef__"
        )
        cleaned, status = lab.parse_remote_exit(
            f"native output\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "native output")
        self.assertEqual(status, 0)
        with self.assertRaisesRegex(lab.ResultError, "sentinel"):
            lab.parse_remote_exit("native output", sentinel)


class SequenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def test_sequence_stops_before_b_on_nondeterminism(self) -> None:
        calls: list[str] = []

        def capture(candidate: lab.CandidateSpec) -> lab.CandidateCapture:
            calls.append(candidate.candidate_id)
            payload = b"A" if candidate.candidate_id == "a1" else b"X"
            return lab.CandidateCapture.fixture(candidate, payload)

        result = lab.run_sequence_with_capture(self.manifest, capture)
        self.assertEqual(calls, ["a1", "a2"])
        self.assertEqual(result.classification, "serializer-nondeterministic")

    def test_sequence_runs_b_only_after_exact_a_controls(self) -> None:
        calls: list[str] = []

        def capture(candidate: lab.CandidateSpec) -> lab.CandidateCapture:
            calls.append(candidate.candidate_id)
            payload = b"A" if candidate.candidate_id != "b" else b"B"
            return lab.CandidateCapture.fixture(candidate, payload)

        result = lab.run_sequence_with_capture(self.manifest, capture)
        self.assertEqual(calls, ["a1", "a2", "b"])
        self.assertEqual(result.classification, "captured")


class SourcePolicyTests(unittest.TestCase):
    def test_native_source_has_one_fixed_compile_only_graph(self) -> None:
        source = (
            lab.REPOSITORY_ROOT
            / "waved/tools/npu-compiler/"
            "tim_vx_compile_erf_constant_diff.cc"
        ).read_text(encoding="utf-8")
        self.assertEqual(source.count("CompileToBinary("), 2)
        self.assertEqual(
            source.count("CreateOperation<tim::vx::ops::Erf>()"), 1
        )
        self.assertEqual(source.count("TensorAttribute::CONSTANT"), 1)
        self.assertNotIn("CopyDataToTensor(", source)
        self.assertNotIn("Graph::Run", source)
        self.assertNotIn("/dev/galcore", source)
        self.assertIn("kConstantA{{0, 1, 2, 3}}", source)
        self.assertIn("kConstantB{{0, 1, 2, 4}}", source)

    def test_build_is_one_pinned_offline_container(self) -> None:
        script = (
            lab.REPOSITORY_ROOT
            / "waved/tools/npu-compiler/"
            "build-device-erf-constant-diff.sh"
        ).read_text(encoding="utf-8")
        self.assertEqual(script.count("docker run "), 1)
        self.assertIn("--network=none", script)
        self.assertIn(lab.EXPECTED_BUILD_IMAGE, script)
        self.assertIn("docker image inspect", script)


if __name__ == "__main__":
    unittest.main()
