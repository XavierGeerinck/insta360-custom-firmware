from __future__ import annotations

import copy
import json
import struct
import sys
import tempfile
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_erf_cpu_npu_benchmark_lab as lab  # noqa: E402


class BenchmarkManifestAndOracleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load_document(self, document: dict) -> lab.BenchmarkManifest:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path)

    def test_manifest_and_independent_digests_are_fixed(self) -> None:
        self.assertEqual(self.manifest.remote_argument, "--benchmark")
        self.assertEqual(
            self.manifest.source_sha256,
            "07feee816c2c5958b38f4965a076d061ae7bca9fb8d381f5d1010fe109c8be91",
        )
        self.assertEqual(self.manifest.environment["VSI_NN_LOG_LEVEL"], "2")
        self.assertEqual(lab.REMOTE_ROOT, "/tmp/wave-npu-poc13g-r2")
        self.assertEqual(
            self.manifest.cpu_table_sha256,
            "c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296",
        )
        self.assertEqual(
            lab.expected_digests(),
            {
                "input_fnv1a64": "0x367c7eaad959d707",
                "output_fnv1a64": "0x742e3a62d58acd0d",
                "final_state": "0x6ae16fe0",
            },
        )
        self.assertEqual(lab.COLD_FRAMES, 1)
        self.assertEqual(lab.WARMUP_FRAMES, 100)
        self.assertEqual(lab.MEASURED_FRAMES, 10_000)
        self.assertEqual(lab.TOTAL_FRAMES, 10_101)

    def test_manifest_rejects_count_or_invocation_change(self) -> None:
        document = self._document()
        document["expected_result"]["measured_frames"] = 9_999
        with self.assertRaisesRegex(lab.ManifestError, "expected benchmark"):
            self._load_document(document)

        document = self._document()
        document["remote_argument"] = "--benchmark-forever"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)


class RawTimingTests(unittest.TestCase):
    @staticmethod
    def _payload(
        *,
        corrupt_order: bool = False,
        zero_cpu_thread: bool = False,
    ) -> bytes:
        records = []
        for sequence in range(lab.MEASURED_FRAMES):
            order = sequence & 1
            if corrupt_order and sequence == 17:
                order ^= 1
            records.append(
                lab.RECORD_STRUCT.pack(
                    sequence,
                    order,
                    100 + sequence,
                    0 if zero_cpu_thread and sequence == 17 else 50 + sequence,
                    10 + sequence,
                    200 + sequence,
                    20 + sequence,
                    230 + sequence,
                    100 + sequence,
                )
            )
        header = lab.HEADER_STRUCT.pack(
            lab.TIMING_MAGIC,
            lab.TIMING_SCHEMA_VERSION,
            lab.MEASURED_FRAMES,
            lab.RECORD_STRUCT.size,
            lab.CALIBRATION_SAMPLES,
            1_000_000,
            5,
        )
        return header + b"".join(records)

    def test_raw_timing_roundtrip_and_nearest_rank_statistics(self) -> None:
        parsed = lab.parse_timing_payload(self._payload())
        self.assertEqual(len(parsed.records), lab.MEASURED_FRAMES)
        self.assertEqual(parsed.calibration_min_wall_ns, 5)
        summary = lab.summarize_timings(parsed)
        self.assertEqual(summary["cpu_wall_ns"]["minimum"], 100)
        self.assertEqual(summary["cpu_wall_ns"]["p50"], 5_099)
        self.assertEqual(summary["cpu_wall_ns"]["p99"], 9_999)
        self.assertEqual(summary["npu_total_wall_ns"]["maximum"], 10_229)
        self.assertGreater(summary["ratios"]["npu_total_over_cpu_wall"], 1.0)
        limited = lab.parse_timing_payload(
            self._payload(zero_cpu_thread=True)
        )
        limited_summary = lab.summarize_timings(limited)
        self.assertEqual(
            limited_summary["cpu_thread_ns"]["resolution_limited_zeros"],
            1,
        )

    def test_raw_timing_rejects_order_zero_duration_and_trailing_bytes(
        self,
    ) -> None:
        with self.assertRaisesRegex(lab.ResultError, "alternating order"):
            lab.parse_timing_payload(self._payload(corrupt_order=True))

        payload = bytearray(self._payload())
        first_cpu_wall_offset = lab.HEADER_STRUCT.size + 8
        struct.pack_into("<Q", payload, first_cpu_wall_offset, 0)
        with self.assertRaisesRegex(lab.ResultError, "zero duration"):
            lab.parse_timing_payload(bytes(payload))

        with self.assertRaisesRegex(lab.ResultError, "byte count"):
            lab.parse_timing_payload(self._payload() + b"\0")


class NativeResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )

    def _stdout(self) -> str:
        return "\n".join(
            (
                self.identity,
                "COLD cpu_wall_ns=900 npu_copy_in_ns=40000 "
                "npu_run_ns=1500000 npu_copy_out_ns=30000 "
                "npu_total_ns=1570000",
                "PROCESS_CPU frames=10101 total_ns=987654321",
                "ERF_CPU_NPU_BENCH status=PASS mode=benchmark "
                "cold_frames=1 warmup_frames=100 measured_frames=10000 "
                "cpu_count=10101 npu_run_count=10101 compile_count=1 "
                "canaries=PASS timing_bytes=640036 "
                "cpu_thread_zero_count=17 "
                "timing_fnv1a64=0x1234567890abcdef "
                "input_fnv1a64=0x367c7eaad959d707 "
                "cpu_output_fnv1a64=0x742e3a62d58acd0d "
                "npu_output_fnv1a64=0x742e3a62d58acd0d "
                "final_state=0x6ae16fe0",
            )
        )

    def test_exact_native_result_passes(self) -> None:
        result = lab.parse_native_result(self._stdout(), self.manifest)
        self.assertEqual(result["run_count"], 10_101)
        self.assertEqual(result["cold"]["npu_total_ns"], 1_570_000)
        self.assertEqual(result["process_cpu_total_ns"], 987_654_321)
        self.assertEqual(result["cpu_thread_zero_count"], 17)
        self.assertTrue(result["numeric_digest_match"])

    def test_changed_digest_or_duplicate_summary_is_rejected(self) -> None:
        changed = self._stdout().replace(
            "cpu_output_fnv1a64=0x742e3a62d58acd0d",
            "cpu_output_fnv1a64=0x0000000000000000",
        )
        with self.assertRaisesRegex(lab.ResultError, "digest"):
            lab.parse_native_result(changed, self.manifest)

        with self.assertRaisesRegex(lab.ResultError, "expected one"):
            lab.parse_native_result(
                f"{self._stdout()}\n{self._stdout().splitlines()[-1]}",
                self.manifest,
            )


class BenchmarkHealthGateTests(unittest.TestCase):
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
                    "bytes": 5840 if path == lab.REMOTE_NBG else 100,
                    "sha256": self.manifest.required_artifacts[path],
                }
                for path, mode in lab.EXPECTED_STAGED_MODES.items()
            },
            "timing_output_states": {
                lab.REMOTE_TIMING: {"status": "absent"},
                lab.REMOTE_PARTIAL_TIMING: {"status": "absent"},
            },
            "system_context": {
                "cpu_online": "0-3",
                "cpu_model": "processor : 0\nCPU part : 0xd05",
                "cpu_frequency": "policy0 affected_cpus=0-3",
                "thermal": "thermal_zone0 type=soc temp=45000",
                "proc_stat": "cpu  1 2 3 4 5 6 7 8 9 10",
            },
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self) -> None:
        lab.verify_preflight(self.manifest, self.snapshot)

    def test_holder_or_partial_output_fails_gates(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 4, "fd": 8}]
        with self.assertRaisesRegex(lab.PreflightError, "already held"):
            lab.verify_preflight(self.manifest, changed)

        after = copy.deepcopy(self.snapshot)
        after["uptime"] = "101.0 91.0"
        after["timing_output_states"][lab.REMOTE_PARTIAL_TIMING] = {
            "status": "present"
        }
        differences, _ = lab.compare_health(
            self.manifest, self.snapshot, after
        )
        self.assertIn("partial timing output remains", differences)


if __name__ == "__main__":
    unittest.main()
