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

import npu_erf_mask_stream_execution_lab as lab  # noqa: E402


class StreamManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load_document(self, document: dict) -> lab.StreamManifest:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path)

    def test_manifest_is_fixed_full_stream(self) -> None:
        self.assertEqual(self.manifest.remote_argument, "--fixture-stream")
        self.assertEqual(self.manifest.remote_output, lab.REMOTE_OUTPUT)
        self.assertEqual(self.manifest.timeout_ms, 40000)
        self.assertEqual(self.manifest.expected_result["frame_count"], 1201)
        self.assertEqual(self.manifest.expected_result["width"], 481)

    def test_manifest_rejects_invocation_or_output_change(self) -> None:
        document = self._document()
        document["remote_argument"] = "--benchmark"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)

        document = self._document()
        document["remote_output"] = "/tmp/other"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)

    def test_manifest_rejects_input_hash_or_clc_change(self) -> None:
        document = self._document()
        document["required_artifacts"][lab.REMOTE_INPUT] = "0" * 64
        with self.assertRaisesRegex(lab.ManifestError, "fixed artifact"):
            self._load_document(document)

        document = self._document()
        document["absent_artifacts"].remove("/usr/lib/libCLC.so")
        with self.assertRaisesRegex(lab.ManifestError, "CLC"):
            self._load_document(document)


class StreamResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )

    def _native_stdout(self, output_fnv: str) -> str:
        return (
            f"{self.identity}\n"
            "ERF_STREAM status=PASS mode=fixture-stream frames=1201 "
            "bytes_per_frame=481 compile_count=1 run_count=1201 "
            "canaries=PASS total_run_us=240200 min_run_us=100 "
            "max_run_us=1500 output_bytes=577697 "
            f"output_fnv1a64={output_fnv} "
            f"output={lab.REMOTE_OUTPUT}"
        )

    def test_exact_full_stream_matches_independent_oracle(self) -> None:
        input_payload = lab.LOCAL_INPUT.read_bytes()
        table = lab.build_independent_erf_table()
        output_payload = (
            lab.STREAM_HEADER.pack(b"ERO1", 1, 1201, 481)
            + input_payload[lab.STREAM_HEADER.size :].translate(table)
        )
        output_fnv = f"0x{lab.fnv1a64(output_payload):016x}"
        native = lab.parse_native_result(
            self._native_stdout(output_fnv), self.manifest
        )
        with tempfile.TemporaryDirectory(
            dir=lab.REPOSITORY_ROOT
        ) as temporary:
            path = Path(temporary) / "output.bin"
            path.write_bytes(output_payload)
            result = lab.verify_pulled_output(path, native)
        self.assertTrue(result["numeric_match"]["passed"])
        self.assertEqual(
            result["numeric_match"]["compared_bytes"], 1201 * 481
        )
        self.assertEqual(result["numeric_match"]["mismatched_bytes"], 0)
        self.assertLess(
            result["latency_us"]["graph_run_realtime_fraction"], 1.0
        )

    def test_one_wrong_output_byte_is_rejected(self) -> None:
        input_payload = lab.LOCAL_INPUT.read_bytes()
        table = lab.build_independent_erf_table()
        output = bytearray(
            lab.STREAM_HEADER.pack(b"ERO1", 1, 1201, 481)
            + input_payload[lab.STREAM_HEADER.size :].translate(table)
        )
        output[-1] ^= 1
        payload = bytes(output)
        output_fnv = f"0x{lab.fnv1a64(payload):016x}"
        native = lab.parse_native_result(
            self._native_stdout(output_fnv), self.manifest
        )
        with tempfile.TemporaryDirectory(
            dir=lab.REPOSITORY_ROOT
        ) as temporary:
            path = Path(temporary) / "output.bin"
            path.write_bytes(payload)
            with self.assertRaisesRegex(lab.ResultError, "mismatch"):
                lab.verify_pulled_output(path, native)

    def test_native_summary_and_remote_command_are_fixed(self) -> None:
        with self.assertRaises(lab.ResultError):
            lab.parse_native_result(
                self._native_stdout("0x0000000000000000").replace(
                    "run_count=1201", "run_count=1200"
                ),
                self.manifest,
            )
        sentinel = (
            "__WAVE_ERF_STREAM_EXIT_"
            "0123456789abcdef0123456789abcdef__"
        )
        command = lab.build_remote_command(self.manifest, sentinel)
        self.assertEqual(command.count("--fixture-stream"), 1)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("--benchmark", command)


class StreamHealthGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.snapshot = {
            "serials": [self.manifest.serial],
            "kernel": self.manifest.kernel,
            "boot_id": "0b8091ee-557d-4857-8019-af1aaab9d665",
            "uptime": "100.0 90.0",
            "galcore_device": (
                "crw-rw---- 1 0 0 199, 0 Jul 29 12:00 /dev/galcore"
            ),
            "npu_parameters": dict(self.manifest.npu_parameters),
            "galcore_holders": [],
            "processes": [],
            "artifact_hashes": dict(self.manifest.required_artifacts),
            "output_states": {},
            "absent_artifact_states": {
                path: {"status": "absent"}
                for path in self.manifest.absent_artifacts
            },
            "staged_artifact_states": {
                path: {
                    "status": "present",
                    "permissions": mode,
                    "bytes": lab.EXPECTED_STAGED_BYTES.get(path, 100),
                    "sha256": self.manifest.required_artifacts[path],
                }
                for path, mode in lab.EXPECTED_STAGED_MODES.items()
            },
            "stream_output_states": {
                lab.REMOTE_OUTPUT: {"status": "absent"},
                lab.REMOTE_PARTIAL_OUTPUT: {"status": "absent"},
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self) -> None:
        lab.verify_preflight(self.manifest, self.snapshot)

    def test_preflight_rejects_output_or_holder(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["stream_output_states"][lab.REMOTE_OUTPUT] = {
            "status": "present"
        }
        with self.assertRaisesRegex(lab.PreflightError, "not absent"):
            lab.verify_preflight(self.manifest, changed)

        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 8, "fd": 3}]
        with self.assertRaisesRegex(lab.PreflightError, "already held"):
            lab.verify_preflight(self.manifest, changed)

    def test_health_gate_rejects_partial_and_kernel_fault(self) -> None:
        after = copy.deepcopy(self.snapshot)
        after["uptime"] = "101.0 91.0"
        after["dmesg_tail"].append("[2.0] galcore timeout")
        after["stream_output_states"][lab.REMOTE_PARTIAL_OUTPUT] = {
            "status": "present"
        }
        differences, alerts = lab.compare_health(
            self.manifest, self.snapshot, after
        )
        self.assertTrue(alerts)
        self.assertIn("partial stream output remains", differences)


if __name__ == "__main__":
    unittest.main()
