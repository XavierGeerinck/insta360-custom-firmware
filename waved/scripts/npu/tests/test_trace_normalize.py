#!/usr/bin/env python3
"""Contract tests for metadata-only Wave NPU trace normalization."""

from __future__ import annotations

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIRECTORY))

import trace_normalize  # noqa: E402


OUTPUT_SHA = "e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4"


def gal_pair(sequence: int, command: int) -> list[str]:
    shared = (
        f"seq={sequence} request=30000 envelope=ok command={command} "
        "hardware=4 core=0 interface_bytes=496"
    )
    return [
        f"WAVE_GAL_TRACE v=1 event=enter {shared}",
        f"WAVE_GAL_TRACE v=1 event=exit {shared} result=0 errno=0 status=0",
    ]


def synthetic_gal(*, final_entry_only: bool = False) -> str:
    lines = gal_pair(1, 6) + gal_pair(2, 18)
    if final_entry_only:
        lines = lines[:-1]
    return "\n".join(lines)


def synthetic_strace(
    *, extra_line: str | None = None, final_entry_only: bool = False
) -> str:
    lines = [
        '101 1753700000.000001 execve("/data/waved/waved-npu-probe.poc", '
        '["waved-npu-probe.poc"], 0xffff) = 0 <0.001000>',
        '101 1753700000.000002 openat(AT_FDCWD, "/usr/lib/libnnsdk.so", '
        'O_RDONLY|O_CLOEXEC) = 3</usr/lib/libnnsdk.so> <0.000010>',
        '101 1753700000.000003 openat(AT_FDCWD, "/dev/galcore", '
        'O_RDWR|O_CLOEXEC) = 4</dev/galcore> <0.000010>',
    ]
    for phase in range(1, 9):
        lines.append(
            f'101 1753700000.{phase:06d} write(1<pipe:[1]>, '
            f'"[npu:{phase}] phase {phase}\\\\n", 20) = 20 <0.000010>'
        )
        if phase in {5, 7}:
            sequence = 1 if phase == 5 else 2
            for event in ("enter", "exit"):
                if final_entry_only and sequence == 2 and event == "exit":
                    continue
                lines.append(
                    f'101 1753700000.{phase:06d} '
                    'write(2</tmp/wave-npu-lab/gal.raw>, '
                    f'"WAVE_GAL_TRACE v=1 event={event} seq={sequence}"'
                    '..., 120) = 120 <0.000010>'
                )
    lines.extend(
        [
            "101 1753700001.000001 ioctl(4</dev/galcore>, "
            "0x7530, 0xffff12345678) = 0 <0.001000>",
            "101 1753700001.000002 ioctl(4</dev/galcore>, "
            "0x7530, 0xffff87654321) = 0 <0.001000>",
            "101 1753700001.000003 close(4</dev/galcore>) = 0 <0.000010>",
            "101 1753700001.000004 exit_group(0) = ?",
        ]
    )
    if extra_line is not None:
        lines.insert(-1, extra_line)
    return "\n".join(lines) + "\n"


def envelope() -> dict:
    return {
        "experiment_id": "stock-ctc-trace",
        "definition_sha256": "a" * 64,
        "required_artifacts": {"/usr/bin/strace": "b" * 64},
        "execution": {"classification": "success"},
        "semantic_result": {
            "status": "PASS",
            "iterations": 3,
            "output_bytes": 332800,
            "output_sha256": OUTPUT_SHA,
            "average_ms": 1.23,
            "cleanup": 0,
        },
        "health": {
            "adb_responsive": True,
            "post_snapshot_complete": True,
            "npu_parameters_unchanged": True,
            "required_artifacts_unchanged": True,
            "galcore_present": True,
            "timeout_requires_manual_holder_review": False,
        },
        "artifacts": [
            {"kind": "stdout", "sha256": "c" * 64, "bytes": 1},
            {"kind": "stderr", "sha256": "d" * 64, "bytes": 1},
        ],
    }


def captured_stdout(
    *,
    gal_text: str | None = None,
    strace_text: str | None = None,
) -> str:
    probe = "\n".join(
        f"[npu:{number}] phase {number}" for number in range(1, 9)
    )
    return (
        probe
        + "\nRESULT: PASS — synthetic\n\n"
        + trace_normalize.GAL_BEGIN
        + "\n"
        + (gal_text if gal_text is not None else synthetic_gal())
        + "\n"
        + trace_normalize.GAL_END
        + "\n"
        + trace_normalize.STRACE_BEGIN
        + "\n"
        + (strace_text if strace_text is not None else synthetic_strace())
        + trace_normalize.STRACE_END
        + "\n"
    )


class GalLineTests(unittest.TestCase):
    def test_parses_metadata_only_pair(self):
        enter, exit_record = [
            trace_normalize.parse_gal_line(line) for line in gal_pair(1, 18)
        ]
        self.assertEqual(enter["command"], 18)
        self.assertEqual(exit_record["status"], 0)

    def test_rejects_duplicate_token(self):
        line = gal_pair(1, 18)[0] + " command=18"
        with self.assertRaisesRegex(trace_normalize.TraceError, "duplicate"):
            trace_normalize.parse_gal_line(line)

    def test_rejects_unreadable_envelope(self):
        line = (
            "WAVE_GAL_TRACE v=1 event=enter seq=1 request=30000 "
            "envelope=unreadable"
        )
        with self.assertRaisesRegex(trace_normalize.TraceError, "keys differ"):
            trace_normalize.parse_gal_line(line)

    def test_rejects_oversized_interface(self):
        line = gal_pair(1, 18)[0].replace(
            "interface_bytes=496", "interface_bytes=65537"
        )
        with self.assertRaisesRegex(trace_normalize.TraceError, "outside"):
            trace_normalize.parse_gal_line(line)

    def test_rejects_pointer_token(self):
        line = gal_pair(1, 18)[0] + " input=0xffff12345678"
        with self.assertRaises(trace_normalize.TraceError):
            trace_normalize.parse_gal_line(line)


class NormalizeTests(unittest.TestCase):
    def create_run(
        self,
        root: Path,
        *,
        envelope_value: dict | None = None,
        stdout_value: str | None = None,
        stderr_value: str | None = None,
    ) -> Path:
        run = root / "run"
        run.mkdir(parents=True)
        (run / "envelope.json").write_text(
            json.dumps(envelope_value or envelope()), encoding="utf-8"
        )
        (run / "stdout.txt").write_text(
            stdout_value if stdout_value is not None else captured_stdout(),
            encoding="utf-8",
        )
        (run / "stderr.txt").write_text(
            stderr_value if stderr_value is not None else "",
            encoding="utf-8",
        )
        return run

    def test_normalizes_and_redacts_volatile_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = trace_normalize.normalize_run_directory(
                self.create_run(Path(temporary))
            )
        self.assertEqual(report["lifecycle"]["traced_phase_order"], list(range(1, 9)))
        self.assertEqual(report["gal"]["call_count"], 2)
        self.assertEqual(report["gal"]["calls"][1]["command_name_adjacent"], "COMMIT")
        self.assertEqual(
            report["gal"]["calls"][1]["hardware_name_adjacent"], "VIP"
        )
        self.assertEqual(
            [call["phase"] for call in report["gal"]["calls"]], [5, 7]
        )
        self.assertIn(
            {"kind": "device", "name": "/dev/galcore"}, report["paths"]
        )
        self.assertEqual(
            report["driver_events"],
            [
                {
                    "operation": "open",
                    "path": "/dev/galcore",
                    "phase": None,
                    "result": "success",
                },
                {
                    "operation": "close",
                    "path": "/dev/galcore",
                    "phase": 8,
                    "result": "success",
                },
            ],
        )
        rendered = json.dumps(report, sort_keys=True)
        self.assertNotIn("1753700000", rendered)
        self.assertNotIn("0xffff12345678", rendered)
        self.assertIsNone(trace_normalize.POINTER_RE.search(rendered))

    def test_rejects_failed_experiment(self):
        bad_envelope = envelope()
        bad_envelope["execution"]["classification"] = "native-timeout"
        with tempfile.TemporaryDirectory() as temporary:
            run = self.create_run(
                Path(temporary), envelope_value=bad_envelope
            )
            with self.assertRaisesRegex(trace_normalize.TraceError, "did not succeed"):
                trace_normalize.normalize_run_directory(run)

    def test_rejects_missing_phase(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = self.create_run(
                Path(temporary),
                stdout_value=captured_stdout().replace(
                    "[npu:6] phase 6\n", ""
                ),
            )
            with self.assertRaisesRegex(trace_normalize.TraceError, "phases 1..8"):
                trace_normalize.normalize_run_directory(run)

    def test_rejects_nonzero_gal_status(self):
        bad_stdout = captured_stdout().replace(
            "status=0", "status=-1", 1
        )
        with tempfile.TemporaryDirectory() as temporary:
            run = self.create_run(Path(temporary), stdout_value=bad_stdout)
            with self.assertRaisesRegex(trace_normalize.TraceError, "nonzero"):
                trace_normalize.normalize_run_directory(run)

    def test_rejects_unknown_gal_diagnostic(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = self.create_run(
                Path(temporary),
                stdout_value=captured_stdout(
                    gal_text=synthetic_gal() + "\nunexpected diagnostic"
                ),
            )
            with self.assertRaisesRegex(trace_normalize.TraceError, "unrecognized"):
                trace_normalize.normalize_run_directory(run)

    def test_rejects_sequence_gap(self):
        bad_stdout = captured_stdout().replace("seq=2", "seq=3")
        with tempfile.TemporaryDirectory() as temporary:
            run = self.create_run(Path(temporary), stdout_value=bad_stdout)
            with self.assertRaisesRegex(trace_normalize.TraceError, "contiguous"):
                trace_normalize.normalize_run_directory(run)

    def test_accepts_one_final_entry_when_strace_proves_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = trace_normalize.normalize_run_directory(
                self.create_run(
                    Path(temporary),
                    stdout_value=captured_stdout(
                        gal_text=synthetic_gal(final_entry_only=True),
                        strace_text=synthetic_strace(final_entry_only=True),
                    ),
                )
            )
        self.assertEqual(
            report["gal"]["completion"]["entry_only_process_exit_ordinals"],
            [2],
        )
        self.assertTrue(
            report["gal"]["completion"]["all_strace_ioctl_results_zero"]
        )

    def test_equivalent_reports_ignore_syscall_count_variation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = trace_normalize.normalize_run_directory(
                self.create_run(root / "first")
            )
            second_root = root / "second"
            second = trace_normalize.normalize_run_directory(
                self.create_run(
                    second_root,
                    stdout_value=captured_stdout(
                        strace_text=synthetic_strace(
                            extra_line=(
                                "101 1753700001.000003 futex(0, 0, 0) "
                                "= 0 <0.000010>"
                            )
                        )
                    ),
                )
            )
            comparison = trace_normalize.compare_reports(first, second)
        self.assertTrue(comparison["equivalent"])
        self.assertFalse(comparison["checks"]["syscall_counts_equal"])

    def test_gal_difference_fails_repeatability(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = trace_normalize.normalize_run_directory(
                self.create_run(Path(temporary))
            )
        changed = copy.deepcopy(report)
        changed["gal"]["calls"][1]["command"] = 19
        comparison = trace_normalize.compare_reports(report, changed)
        self.assertFalse(comparison["equivalent"])
        self.assertFalse(comparison["checks"]["gal_sequence_equal"])


if __name__ == "__main__":
    unittest.main()
