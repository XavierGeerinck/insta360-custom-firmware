from __future__ import annotations

import json
import struct
import sys
import tempfile
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_ctc_cpu_npu_benchmark_lab as lab  # noqa: E402


def qualification(fixture_id: int) -> bytes:
    return lab.QUALIFICATION_STRUCT.pack(
        fixture_id,
        16,
        lab.OUTPUT_ELEMENTS,
        0,
        0x100 + fixture_id,
        0x200 + fixture_id,
        0x300 + fixture_id,
        0x300 + fixture_id,
        0.01 * lab.OUTPUT_ELEMENTS,
        0.02,
        100.0,
        100.0,
        100.0,
    )


def timing(sequence: int, order: int, *, bridge: int = 5_000_000) -> bytes:
    return lab.TIMING_STRUCT.pack(
        sequence,
        order,
        10_000_000,
        8_000_000,
        400_000,
        350_000,
        100_000,
        80_000,
        3_000_000,
        500_000,
        500_000,
        400_000,
        500_000,
        450_000,
        4_000_000,
        1_100_000,
        bridge,
        1_900_000,
    )


def valid_payload(*, bridge: int = 5_000_000) -> bytes:
    header = lab.HEADER_STRUCT.pack(
        lab.TIMING_MAGIC,
        lab.TIMING_SCHEMA_VERSION,
        lab.FIXTURE_COUNT,
        lab.MEASURED_PAIRS,
        lab.INPUT_ELEMENTS,
        lab.OUTPUT_ELEMENTS,
        lab.INPUT_ELEMENTS * 4,
        lab.INPUT_ELEMENTS * 2,
        lab.OUTPUT_ELEMENTS * 4,
        lab.OUTPUT_ELEMENTS * 2,
        lab.COLD_PAIRS,
        lab.WARMUP_PAIRS,
        lab.QUALIFICATION_SEED,
        lab.BENCHMARK_SEED,
        lab.TIMING_STRUCT.size,
        lab.QUALIFICATION_STRUCT.size,
    )
    return b"".join(
        (
            header,
            *(qualification(index) for index in range(lab.FIXTURE_COUNT)),
            timing(0, 0, bridge=bridge),
            *(
                timing(index, index & 1, bridge=bridge)
                for index in range(lab.MEASURED_PAIRS)
            ),
        )
    )


class CtcCpuNpuBenchmarkLabTests(unittest.TestCase):
    def test_manifest_locks_artifacts_and_exactly_once_contract(self) -> None:
        manifest = lab.load_manifest()
        self.assertEqual(manifest.remote_argument, "--benchmark")
        self.assertEqual(manifest.timeout_ms, 120_000)
        self.assertEqual(
            manifest.required_artifacts[lab.REMOTE_EXECUTABLE],
            lab.EXPECTED_ARTIFACTS[lab.REMOTE_EXECUTABLE],
        )
        self.assertEqual(manifest.candidates, {})

    def test_manifest_rejects_unknown_root_key(self) -> None:
        document = json.loads(lab.DEFAULT_MANIFEST.read_text())
        document["unexpected"] = True
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            path.write_text(json.dumps(document))
            with self.assertRaisesRegex(lab.ManifestError, "root keys"):
                lab.load_manifest(path)

    def test_host_generator_matches_locked_final_state_and_digest(self) -> None:
        self.assertEqual(
            lab.expected_measured_input_fnv(),
            "0xeea0b3af5a10188e",
        )

    def test_valid_payload_parses_and_passes_bridge_gate(self) -> None:
        payload = valid_payload()
        self.assertEqual(len(payload), lab.TIMING_BYTES)
        parsed = lab.parse_timing_payload(payload)
        self.assertEqual(len(parsed.qualifications), 4)
        self.assertEqual(len(parsed.measured), 200)
        summary = lab.summarize_timings(parsed)
        self.assertEqual(summary["decision"], "npu-faster")
        self.assertTrue(summary["performance_gate"]["pass"])
        self.assertEqual(
            summary["ratios"]["bridge_total_over_cpu_ratio_of_means"],
            0.5,
        )

    def test_bridge_gate_rejects_slower_npu_without_rejecting_evidence(self) -> None:
        parsed = lab.parse_timing_payload(valid_payload(bridge=11_000_000))
        summary = lab.summarize_timings(parsed)
        self.assertEqual(summary["decision"], "cpu-faster")
        self.assertFalse(summary["performance_gate"]["pass"])

    def test_parser_rejects_truncation(self) -> None:
        with self.assertRaisesRegex(lab.ResultError, "byte count"):
            lab.parse_timing_payload(valid_payload()[:-1])

    def test_parser_rejects_header_change(self) -> None:
        payload = bytearray(valid_payload())
        struct.pack_into("<I", payload, 4, 3)
        with self.assertRaisesRegex(lab.ResultError, "header"):
            lab.parse_timing_payload(bytes(payload))

    def test_parser_rejects_nondeterministic_npu_fixture(self) -> None:
        payload = bytearray(valid_payload())
        repeat_hash_offset = lab.HEADER_STRUCT.size + 40
        struct.pack_into("<Q", payload, repeat_hash_offset, 0x999)
        with self.assertRaisesRegex(lab.ResultError, "malformed"):
            lab.parse_timing_payload(bytes(payload))

    def test_parser_rejects_fixture_numerical_gate(self) -> None:
        payload = bytearray(valid_payload())
        sum_absolute_error_offset = lab.HEADER_STRUCT.size + 48
        struct.pack_into(
            "<d",
            payload,
            sum_absolute_error_offset,
            0.026 * lab.OUTPUT_ELEMENTS,
        )
        with self.assertRaisesRegex(lab.ResultError, "failed gates"):
            lab.parse_timing_payload(bytes(payload))

    def test_parser_rejects_sequence_change(self) -> None:
        payload = bytearray(valid_payload())
        first_measured = (
            lab.HEADER_STRUCT.size
            + lab.FIXTURE_COUNT * lab.QUALIFICATION_STRUCT.size
            + lab.TIMING_STRUCT.size
        )
        struct.pack_into("<I", payload, first_measured, 7)
        with self.assertRaisesRegex(lab.ResultError, "sequence"):
            lab.parse_timing_payload(bytes(payload))

    def test_parser_rejects_zero_duration(self) -> None:
        payload = bytearray(valid_payload())
        cold_cpu_wall = (
            lab.HEADER_STRUCT.size
            + lab.FIXTURE_COUNT * lab.QUALIFICATION_STRUCT.size
            + 8
        )
        struct.pack_into("<Q", payload, cold_cpu_wall, 0)
        with self.assertRaisesRegex(lab.ResultError, "zero duration"):
            lab.parse_timing_payload(bytes(payload))

    def test_parser_rejects_inconsistent_native_total(self) -> None:
        payload = bytearray(valid_payload())
        cold_start = (
            lab.HEADER_STRUCT.size + lab.FIXTURE_COUNT * lab.QUALIFICATION_STRUCT.size
        )
        native_total_wall = cold_start + 8 + 12 * 8
        struct.pack_into("<Q", payload, native_total_wall, 1)
        with self.assertRaisesRegex(lab.ResultError, "smaller"):
            lab.parse_timing_payload(bytes(payload))

    def test_remote_command_is_fixed_and_has_no_retry(self) -> None:
        manifest = lab.load_manifest()
        sentinel = "__WAVE_CTC_BENCH_EXIT_" + "a" * 32 + "__"
        command = lab.build_remote_command(manifest, sentinel)
        self.assertEqual(command.count(lab.REMOTE_EXECUTABLE), 1)
        self.assertIn("--benchmark", command)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("reboot", command.lower())

    def test_remote_exit_requires_exactly_one_sentinel(self) -> None:
        sentinel = "__WAVE_CTC_BENCH_EXIT_" + "b" * 32 + "__"
        cleaned, status = lab.parse_remote_exit(
            f"native output\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "native output")
        self.assertEqual(status, 0)
        with self.assertRaisesRegex(lab.ResultError, "sentinel"):
            lab.parse_remote_exit("native output", sentinel)

    def test_native_result_parser_accepts_exact_framing(self) -> None:
        manifest = lab.load_manifest()
        identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )
        fixtures = "\n".join(
            (
                f"CTC_QUAL fixture={name} fixture_id={index} status=PASS "
                "top1=16 finite=79984 mae=0.01 max_abs=0.02 "
                "cosine=1 repeat=PASS"
            )
            for index, name in enumerate(("zero", "impulse", "stripe", "xorshift"))
        )
        summary = (
            "CTC_CPU_NPU_BENCH status=PASS mode=benchmark cold_pairs=1 "
            "qualification_fixtures=4 warmup_pairs=20 measured_pairs=200 "
            "cpu_run_count=225 npu_run_count=229 module_create_count=1 "
            "session_create_count=1 canaries=PASS semantics=PASS "
            "timing_bytes=27752 timing_fnv1a64=0x1111111111111111 "
            f"measured_input_fnv1a64={lab.expected_measured_input_fnv()} "
            "cpu_output_fnv1a64=0x2222222222222222 "
            "npu_output_fnv1a64=0x3333333333333333 "
            "final_qualification_state=0x474c5d42 "
            "final_benchmark_state=0x98d70268 "
            "process_cpu_ns=10 process_wall_ns=20"
        )
        parsed = lab.parse_native_result(
            f"{identity}\n{fixtures}\n{summary}\n", manifest
        )
        self.assertEqual(parsed["cpu_run_count"], 225)
        self.assertEqual(parsed["npu_run_count"], 229)


if __name__ == "__main__":
    unittest.main()
