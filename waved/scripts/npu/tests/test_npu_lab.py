#!/usr/bin/env python3
"""Contract tests for the Wave NPU experiment harness."""

from __future__ import annotations

import copy
import contextlib
import io
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import npu_lab  # noqa: E402


PROBE_SHA = "fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0"
NNSDK_SHA = "36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe"
OVXLIB_SHA = "09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9"
MODEL_SHA = "5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66"
OUTPUT_SHA = "e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4"

ARTIFACT_HASHES = {
    "/data/waved/waved-npu-probe.poc": PROBE_SHA,
    "/usr/lib/libnnsdk.so": NNSDK_SHA,
    "/usr/lib/libovxlib.so": OVXLIB_SHA,
    "/model/asr_de/de_online/ctc_online.nb": MODEL_SHA,
}

PASS_LINE = (
    "RESULT: PASS — NPU executed 3 firmware-graph iterations; "
    f"output_bytes=332800, output_sha256={OUTPUT_SHA}, "
    "average=12.345 ms, cleanup=0."
)


def valid_document() -> dict:
    return {
        "schema_version": 1,
        "experiments": {
            "stock-ctc": {
                "description": "Firmware-matched CTC inference control",
                "remote_argv": [
                    "/data/waved/waved-npu-probe.poc",
                    "--run-stock-ctc",
                ],
                "environment": {
                    "LD_LIBRARY_PATH": "/data/waved/lib:/usr/lib",
                    "VIVANTE_SDK_DIR": "/usr",
                },
                "timeout_ms": 20000,
                "expected_remote_exit": 0,
                "required_artifacts": copy.deepcopy(ARTIFACT_HASHES),
                "expected_result": {
                    "status": "PASS",
                    "iterations": 3,
                    "output_bytes": 332800,
                    "output_sha256": OUTPUT_SHA,
                    "cleanup": 0,
                },
            }
        },
    }


def result(
    stdout: str = "",
    stderr: str = "",
    returncode: int = 0,
    *,
    timed_out: bool = False,
    duration_ms: int = 1,
) -> npu_lab.CommandResult:
    return npu_lab.CommandResult(
        stdout=stdout,
        stderr=stderr,
        returncode=returncode,
        duration_ms=duration_ms,
        timed_out=timed_out,
    )


class FakeTransport:
    def __init__(self, handler):
        self.handler = handler
        self.calls: list[tuple[tuple[str, ...], int]] = []

    def run(self, args, timeout_ms):
        normalized = tuple(args)
        self.calls.append((normalized, timeout_ms))
        return self.handler(normalized, timeout_ms)


def snapshot_handler(
    artifact_hashes: dict[str, str] | None = None,
    *,
    execution_mode: str = "pass",
):
    hashes = artifact_hashes or ARTIFACT_HASHES

    def handler(args: tuple[str, ...], timeout_ms: int) -> npu_lab.CommandResult:
        del timeout_ms
        if args == ("version",):
            return result("Android Debug Bridge version 1.0.41\n")
        if args == ("get-state",):
            return result("device\n")

        if len(args) != 2 or args[0] != "shell":
            return result(stderr="unexpected adb arguments", returncode=2)

        command = args[1]
        if command == "uname -a":
            return result(
                "Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT "
                "Thu Jun 19 07:06:39 UTC 2025 aarch64 GNU/Linux\n"
            )
        if command == "cat /proc/uptime":
            return result("1234.56 789.00\n")
        if "/sys/module/galcore/parameters/nanoqFreq" in command:
            return result("838860800\n")
        if "/sys/module/galcore/parameters/enableNN" in command:
            return result("255\n")
        if "/sys/module/galcore/parameters/recovery" in command:
            return result("__UNAVAILABLE__\n")
        if "ls -ln /dev/galcore" in command:
            return result("crw-rw---- 1 0 0 199, 0 Jan 1 00:00 /dev/galcore\n")
        if command.startswith("ps ") or "\nps " in command:
            return result(
                "  330 root     {S60waved} /bin/sh /etc/init.d/S60waved start\n"
                "  442 root     /usr/bin/audioserver\n"
            )
        if "/proc/[0-9]*/fd/*" in command:
            return result("442:9\n442:7\n330:4\n442:9\n")
        if command.startswith("sha256sum "):
            matched = re.search(r"sha256sum '([^']+)'", command)
            if matched is None:
                matched = re.search(r"sha256sum ([^ ]+)", command)
            if matched is None:
                return result("__MISSING__\n")
            path = matched.group(1)
            digest = hashes.get(path)
            if digest is None:
                return result("__MISSING__\n")
            return result(f"{digest}  {path}\n")

        if "__NPU_LAB_REMOTE_EXIT_" in command:
            sentinel_match = re.search(
                r"(__NPU_LAB_REMOTE_EXIT_[0-9a-f]+__)", command
            )
            assert sentinel_match is not None
            sentinel = sentinel_match.group(1)
            if execution_mode == "timeout":
                return result(
                    "probe started\n",
                    "host timeout\n",
                    returncode=-9,
                    timed_out=True,
                    duration_ms=20000,
                )
            if execution_mode == "native-failure":
                return result(f"probe rejected\n{sentinel}:7\n")
            if execution_mode == "missing-result":
                return result(f"probe ended without result\n{sentinel}:0\n")
            if execution_mode == "missing-sentinel":
                return result(PASS_LINE + "\n")
            return result(f"probe log\n{PASS_LINE}\n{sentinel}:0\n")

        return result(stderr=f"unexpected shell command: {command}", returncode=2)

    return handler


class DefinitionTests(unittest.TestCase):
    def load(self, document: dict) -> npu_lab.ExperimentRegistry:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "experiments.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return npu_lab.load_registry(path)

    def test_valid_definition_loads(self):
        registry = self.load(valid_document())
        experiment = registry.get("stock-ctc")
        self.assertEqual(experiment.remote_argv[0], "/data/waved/waved-npu-probe.poc")
        self.assertEqual(experiment.expected_result.output_sha256, OUTPUT_SHA)
        self.assertRegex(registry.definition_sha256("stock-ctc"), r"^[0-9a-f]{64}$")

    def test_unknown_schema_version_is_rejected(self):
        document = valid_document()
        document["schema_version"] = 2
        with self.assertRaisesRegex(npu_lab.SchemaError, "schema_version"):
            self.load(document)

    def test_unknown_root_and_experiment_keys_are_rejected(self):
        document = valid_document()
        document["surprise"] = True
        with self.assertRaisesRegex(npu_lab.SchemaError, "unknown"):
            self.load(document)

        document = valid_document()
        document["experiments"]["stock-ctc"]["shell"] = "id"
        with self.assertRaisesRegex(npu_lab.SchemaError, "unknown"):
            self.load(document)

    def test_unknown_experiment_name_is_rejected(self):
        registry = self.load(valid_document())
        with self.assertRaisesRegex(npu_lab.ExperimentNotFound, "unknown"):
            registry.get("unknown")

    def test_environment_outside_allowlist_is_rejected(self):
        document = valid_document()
        document["experiments"]["stock-ctc"]["environment"]["PATH"] = "/tmp"
        with self.assertRaisesRegex(npu_lab.SchemaError, "environment"):
            self.load(document)

    def test_executable_outside_remote_policy_is_rejected(self):
        document = valid_document()
        document["experiments"]["stock-ctc"]["remote_argv"][0] = "/tmp/probe"
        with self.assertRaisesRegex(npu_lab.SchemaError, "executable"):
            self.load(document)

    def test_npu_lab_executable_is_allowed(self):
        document = valid_document()
        document["experiments"]["stock-ctc"]["remote_argv"][0] = (
            "/data/waved/npu-lab/probe"
        )
        registry = self.load(document)
        self.assertEqual(
            registry.get("stock-ctc").remote_argv[0],
            "/data/waved/npu-lab/probe",
        )

    def test_empty_artifacts_invalid_hash_and_timeout_are_rejected(self):
        document = valid_document()
        document["experiments"]["stock-ctc"]["required_artifacts"] = {}
        with self.assertRaisesRegex(npu_lab.SchemaError, "required_artifacts"):
            self.load(document)

        document = valid_document()
        document["experiments"]["stock-ctc"]["required_artifacts"][
            "/usr/lib/libnnsdk.so"
        ] = "not-a-hash"
        with self.assertRaisesRegex(npu_lab.SchemaError, "SHA-256"):
            self.load(document)

        document = valid_document()
        document["experiments"]["stock-ctc"]["timeout_ms"] = 0
        with self.assertRaisesRegex(npu_lab.SchemaError, "timeout_ms"):
            self.load(document)

    def test_control_characters_are_rejected(self):
        document = valid_document()
        document["experiments"]["stock-ctc"]["remote_argv"][1] = "--ok\nid"
        with self.assertRaisesRegex(npu_lab.SchemaError, "control"):
            self.load(document)

    def test_run_id_validation_and_non_overwrite(self):
        for valid in ("a", "poc0-first-control-20260728", "a1-b2"):
            self.assertEqual(npu_lab.validate_run_id(valid), valid)
        for invalid in ("A", "-bad", "bad_underscore", "", "a" * 65, "../escape"):
            with self.assertRaises(npu_lab.RunDirectoryError):
                npu_lab.validate_run_id(invalid)

        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            created = npu_lab.create_run_directory(root, "run-a")
            self.assertEqual(created, root / "run-a")
            with self.assertRaisesRegex(npu_lab.RunDirectoryError, "exists"):
                npu_lab.create_run_directory(root, "run-a")

    def test_cli_does_not_accept_mutable_remote_arguments(self):
        parser = npu_lab._build_argument_parser()
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                parser.parse_args(
                    [
                        "run",
                        "stock-ctc",
                        "--remote-arg",
                        "--different-command",
                    ]
                )


class ParsingTests(unittest.TestCase):
    def test_remote_exit_sentinel_is_parsed_and_removed(self):
        sentinel = "__NPU_LAB_REMOTE_EXIT_0123abcd__"
        cleaned, remote_exit = npu_lab.parse_remote_exit(
            f"log line\n{sentinel}:7\n", sentinel
        )
        self.assertEqual(remote_exit, 7)
        self.assertEqual(cleaned, "log line\n")

    def test_missing_or_duplicate_sentinel_is_rejected(self):
        sentinel = "__NPU_LAB_REMOTE_EXIT_0123abcd__"
        for output in (
            "no sentinel\n",
            f"{sentinel}:0\n{sentinel}:0\n",
            f"{sentinel}:not-a-number\n",
        ):
            with self.assertRaises(npu_lab.ResultParseError):
                npu_lab.parse_remote_exit(output, sentinel)

    def test_stock_result_is_parsed(self):
        parsed = npu_lab.parse_stock_result("prefix\n" + PASS_LINE + "\n")
        self.assertEqual(parsed["status"], "PASS")
        self.assertEqual(parsed["iterations"], 3)
        self.assertEqual(parsed["output_bytes"], 332800)
        self.assertEqual(parsed["output_sha256"], OUTPUT_SHA)
        self.assertEqual(parsed["cleanup"], 0)
        self.assertEqual(parsed["average_ms"], 12.345)

    def test_missing_duplicate_and_malformed_result_are_rejected(self):
        for output in (
            "no result\n",
            PASS_LINE + "\n" + PASS_LINE + "\n",
            "RESULT: PASS — malformed\n",
        ):
            with self.assertRaises(npu_lab.ResultParseError):
                npu_lab.parse_stock_result(output)

    def test_bounded_text_marks_truncation(self):
        bounded = npu_lab.bounded_text("abcdefghij", limit=6)
        self.assertLessEqual(len(bounded.encode("utf-8")), 128)
        self.assertIn("TRUNCATED", bounded)
        self.assertTrue(bounded.startswith("abc"))


class SnapshotTests(unittest.TestCase):
    def registry(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "experiments.json"
            path.write_text(json.dumps(valid_document()), encoding="utf-8")
            return npu_lab.load_registry(path)

    def test_snapshot_is_bounded_and_parses_fields(self):
        experiment = self.registry().get("stock-ctc")
        transport = FakeTransport(snapshot_handler())
        snapshot = npu_lab.collect_snapshot(transport, experiment)

        self.assertEqual(snapshot["adb_state"], "device")
        self.assertIn("5.4.242-ab575", snapshot["kernel"])
        self.assertEqual(snapshot["uptime_seconds"], 1234.56)
        self.assertEqual(
            snapshot["npu_parameters"]["nanoqFreq"],
            {"status": "observed", "value": "838860800"},
        )
        self.assertEqual(
            snapshot["npu_parameters"]["recovery"],
            {"status": "unavailable", "value": None},
        )
        self.assertEqual(snapshot["artifact_hashes"], ARTIFACT_HASHES)
        self.assertEqual(
            snapshot["galcore_holders"],
            [
                {"pid": 330, "fd": 4},
                {"pid": 442, "fd": 7},
                {"pid": 442, "fd": 9},
            ],
        )
        self.assertLessEqual(len(snapshot["processes"]), npu_lab.MAX_SNAPSHOT_LINES)

    def test_missing_or_mismatched_artifact_is_rejected(self):
        experiment = self.registry().get("stock-ctc")

        missing = dict(ARTIFACT_HASHES)
        missing.pop("/usr/lib/libnnsdk.so")
        snapshot = npu_lab.collect_snapshot(
            FakeTransport(snapshot_handler(missing)), experiment
        )
        with self.assertRaisesRegex(npu_lab.PreflightError, "missing"):
            npu_lab.verify_preflight(experiment, snapshot)

        mismatched = dict(ARTIFACT_HASHES)
        mismatched["/usr/lib/libnnsdk.so"] = "0" * 64
        snapshot = npu_lab.collect_snapshot(
            FakeTransport(snapshot_handler(mismatched)), experiment
        )
        with self.assertRaisesRegex(npu_lab.PreflightError, "mismatch"):
            npu_lab.verify_preflight(experiment, snapshot)


class RunTests(unittest.TestCase):
    def registry(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "experiments.json"
            path.write_text(json.dumps(valid_document()), encoding="utf-8")
            return npu_lab.load_registry(path)

    def run_with(self, handler, run_id="test-run"):
        transport = FakeTransport(handler)
        with tempfile.TemporaryDirectory() as temp_dir:
            envelope, run_dir = npu_lab.run_experiment(
                registry=self.registry(),
                experiment_id="stock-ctc",
                transport=transport,
                artifact_root=Path(temp_dir),
                run_id=run_id,
                git_commit="test-commit",
                host_metadata={
                    "platform": "test-host",
                    "python": "test-python",
                    "adb": "test-adb",
                },
            )
            persisted = json.loads(
                (run_dir / "envelope.json").read_text(encoding="utf-8")
            )
            stdout = (run_dir / "stdout.txt").read_text(encoding="utf-8")
            stderr = (run_dir / "stderr.txt").read_text(encoding="utf-8")
            return envelope, persisted, stdout, stderr, transport.calls

    def test_success_writes_complete_envelope(self):
        envelope, persisted, stdout, stderr, calls = self.run_with(
            snapshot_handler()
        )
        self.assertEqual(envelope, persisted)
        self.assertEqual(envelope["execution"]["classification"], "success")
        self.assertEqual(envelope["execution"]["remote_exit"], 0)
        self.assertEqual(envelope["semantic_result"]["output_sha256"], OUTPUT_SHA)
        self.assertEqual(envelope["preflight_hashes"], ARTIFACT_HASHES)
        self.assertEqual(
            envelope["command"]["environment_allowlist"],
            {
                "LD_LIBRARY_PATH": "/data/waved/lib:/usr/lib",
                "VIVANTE_SDK_DIR": "/usr",
            },
        )
        self.assertEqual(
            set(envelope["command"]["environment_allowlist"]),
            {"LD_LIBRARY_PATH", "VIVANTE_SDK_DIR"},
        )
        self.assertIn(PASS_LINE, stdout)
        self.assertNotIn("__NPU_LAB_REMOTE_EXIT_", stdout)
        self.assertEqual(stderr, "")
        self.assertTrue(envelope["health"]["adb_responsive"])
        self.assertTrue(envelope["health"]["npu_parameters_unchanged"])
        execution_calls = [
            args for args, _ in calls if "__NPU_LAB_REMOTE_EXIT_" in " ".join(args)
        ]
        self.assertEqual(len(execution_calls), 1)

    def test_preflight_rejection_never_executes_probe(self):
        mismatched = dict(ARTIFACT_HASHES)
        mismatched["/usr/lib/libnnsdk.so"] = "0" * 64
        envelope, _, _, _, calls = self.run_with(snapshot_handler(mismatched))
        self.assertEqual(
            envelope["execution"]["classification"], "preflight-rejected"
        )
        self.assertFalse(
            any("__NPU_LAB_REMOTE_EXIT_" in " ".join(args) for args, _ in calls)
        )

    def test_native_failure_is_classified_and_post_snapshot_runs(self):
        envelope, _, stdout, _, calls = self.run_with(
            snapshot_handler(execution_mode="native-failure")
        )
        self.assertEqual(envelope["execution"]["classification"], "native-failure")
        self.assertEqual(envelope["execution"]["remote_exit"], 7)
        self.assertIn("probe rejected", stdout)
        self.assertIn("device_after", envelope)
        self.assertGreaterEqual(
            sum(1 for args, _ in calls if args == ("get-state",)), 2
        )

    def test_timeout_is_classified_and_post_snapshot_runs(self):
        envelope, _, _, stderr, calls = self.run_with(
            snapshot_handler(execution_mode="timeout")
        )
        self.assertEqual(envelope["execution"]["classification"], "native-timeout")
        self.assertTrue(envelope["execution"]["timed_out"])
        self.assertIn("host timeout", stderr)
        self.assertGreaterEqual(
            sum(1 for args, _ in calls if args == ("get-state",)), 2
        )

    def test_result_and_sentinel_failures_are_classified(self):
        envelope, *_ = self.run_with(
            snapshot_handler(execution_mode="missing-result")
        )
        self.assertEqual(
            envelope["execution"]["classification"], "result-parse-failure"
        )

        envelope, *_ = self.run_with(
            snapshot_handler(execution_mode="missing-sentinel"),
            run_id="second-run",
        )
        self.assertEqual(envelope["execution"]["classification"], "adb-failure")

    def test_adb_unavailable_is_classified_without_native_execution(self):
        base = snapshot_handler()

        def unavailable(args, timeout_ms):
            if args == ("get-state",):
                return result(stderr="no devices", returncode=1)
            return base(args, timeout_ms)

        envelope, _, _, _, calls = self.run_with(unavailable)
        self.assertEqual(envelope["execution"]["classification"], "adb-unavailable")
        self.assertFalse(
            any("__NPU_LAB_REMOTE_EXIT_" in " ".join(args) for args, _ in calls)
        )

    def test_incomplete_post_snapshot_has_distinct_classification(self):
        base = snapshot_handler()
        state_calls = 0

        def disconnect_after_execution(args, timeout_ms):
            nonlocal state_calls
            if args == ("get-state",):
                state_calls += 1
                if state_calls >= 2:
                    return result(stderr="device offline", returncode=1)
            return base(args, timeout_ms)

        envelope, *_ = self.run_with(disconnect_after_execution)
        self.assertEqual(
            envelope["execution"]["classification"], "post-snapshot-failure"
        )
        self.assertFalse(envelope["health"]["adb_responsive"])
        self.assertFalse(envelope["health"]["post_snapshot_complete"])

    def test_unexpected_transport_error_still_writes_internal_error_envelope(self):
        def broken_transport(args, timeout_ms):
            del args, timeout_ms
            raise RuntimeError("synthetic transport failure")

        envelope, persisted, *_ = self.run_with(broken_transport)
        self.assertEqual(envelope, persisted)
        self.assertEqual(envelope["execution"]["classification"], "internal-error")
        self.assertIn("synthetic transport failure", envelope["execution"]["reason"])

    def test_raw_output_files_are_bounded(self):
        base = snapshot_handler()

        def large_output(args, timeout_ms):
            response = base(args, timeout_ms)
            if len(args) == 2 and "__NPU_LAB_REMOTE_EXIT_" in args[1]:
                return npu_lab.CommandResult(
                    stdout=("x" * (npu_lab.MAX_CAPTURE_BYTES + 100))
                    + response.stdout,
                    stderr="y" * (npu_lab.MAX_CAPTURE_BYTES + 100),
                    returncode=response.returncode,
                    duration_ms=response.duration_ms,
                    timed_out=response.timed_out,
                )
            return response

        envelope, _, stdout, stderr, _ = self.run_with(large_output)
        self.assertEqual(envelope["execution"]["classification"], "success")
        self.assertIn("TRUNCATED", stdout)
        self.assertIn("TRUNCATED", stderr)
        self.assertLess(
            len(stdout.encode("utf-8")), npu_lab.MAX_CAPTURE_BYTES + 128
        )
        self.assertLess(
            len(stderr.encode("utf-8")), npu_lab.MAX_CAPTURE_BYTES + 128
        )


class ComparisonTests(unittest.TestCase):
    def envelope(self):
        return {
            "experiment_id": "stock-ctc",
            "run_id": "run-a",
            "timestamp_utc": "2026-07-28T00:00:00Z",
            "definition_sha256": "a" * 64,
            "required_artifacts": ARTIFACT_HASHES,
            "preflight_hashes": ARTIFACT_HASHES,
            "device_before": {
                "kernel": "Linux buildroot 5.4.242-ab575",
                "uptime_seconds": 100.0,
                "npu_parameters": {
                    "nanoqFreq": {"status": "observed", "value": "838860800"},
                    "enableNN": {"status": "observed", "value": "255"},
                    "recovery": {"status": "unavailable", "value": None},
                },
                "processes": ["1 old pid"],
            },
            "execution": {
                "classification": "success",
                "remote_exit": 0,
                "duration_ms": 100,
            },
            "semantic_result": {
                "status": "PASS",
                "iterations": 3,
                "output_bytes": 332800,
                "output_sha256": OUTPUT_SHA,
                "average_ms": 12.3,
                "cleanup": 0,
            },
            "device_after": {
                "kernel": "Linux buildroot 5.4.242-ab575",
                "uptime_seconds": 101.0,
                "npu_parameters": {
                    "nanoqFreq": {"status": "observed", "value": "838860800"},
                    "enableNN": {"status": "observed", "value": "255"},
                    "recovery": {"status": "unavailable", "value": None},
                },
            },
            "health": {
                "adb_responsive": True,
                "npu_parameters_unchanged": True,
            },
        }

    def test_volatile_fields_are_ignored(self):
        left = self.envelope()
        right = copy.deepcopy(left)
        right["run_id"] = "run-b"
        right["timestamp_utc"] = "later"
        right["device_before"]["uptime_seconds"] = 9000.0
        right["device_before"]["processes"] = ["999 new pid"]
        right["execution"]["duration_ms"] = 999
        right["semantic_result"]["average_ms"] = 99.9
        self.assertEqual(npu_lab.compare_envelopes(left, right), [])

    def test_semantic_difference_is_reported(self):
        left = self.envelope()
        right = copy.deepcopy(left)
        right["semantic_result"]["output_sha256"] = "0" * 64
        differences = npu_lab.compare_envelopes(left, right)
        self.assertEqual(len(differences), 1)
        self.assertIn("semantic_result.output_sha256", differences[0])


if __name__ == "__main__":
    unittest.main()
