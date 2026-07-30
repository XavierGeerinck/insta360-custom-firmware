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

import npu_erf_mask_execution_lab as lab  # noqa: E402


class ErfMaskManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()

    def _document(self) -> dict:
        return json.loads(lab.DEFAULT_MANIFEST.read_text(encoding="utf-8"))

    def _load_document(self, document: dict) -> lab.ExecutionManifest:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path)

    def test_manifest_and_pack_are_fixed(self) -> None:
        self.assertEqual(self.manifest.remote_executable, lab.REMOTE_EXECUTABLE)
        self.assertEqual(self.manifest.remote_argument, "--qualify-six")
        self.assertEqual(self.manifest.timeout_ms, 30000)
        self.assertEqual(self.manifest.candidates, {})
        vectors = lab.load_qualification_vectors(self.manifest)
        self.assertEqual(len(vectors), 6)
        self.assertTrue(all(len(vector) == 481 for vector in vectors))

    def test_manifest_rejects_arbitrary_invocation(self) -> None:
        document = self._document()
        document["remote_executable"] = "/tmp/other"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)

        document = self._document()
        document["remote_argument"] = "--benchmark"
        with self.assertRaisesRegex(lab.ManifestError, "invocation"):
            self._load_document(document)

    def test_manifest_rejects_artifact_or_clc_policy_change(self) -> None:
        document = self._document()
        document["required_artifacts"][lab.REMOTE_NBG] = "0" * 64
        with self.assertRaisesRegex(lab.ManifestError, "fixed artifact"):
            self._load_document(document)

        document = self._document()
        document["absent_artifacts"].remove("/usr/lib/libCLC.so")
        with self.assertRaisesRegex(lab.ManifestError, "CLC"):
            self._load_document(document)

    def test_manifest_rejects_vector_contract_change(self) -> None:
        document = self._document()
        document["expected_result"]["width"] = 480
        with self.assertRaisesRegex(lab.ManifestError, "vector contract"):
            self._load_document(document)


class ErfMaskOracleAndResultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest()
        self.vectors = lab.load_qualification_vectors(self.manifest)
        self.identity = (
            "IDENTITY chip_model=0x00009000 chip_revision=0x00009002 "
            "chip_product_id=0x05090001 chip_eco_id=0x04000000 "
            "customer_id=0x1000000e reserved=0x00000000 "
            "flags=0x0000000000000000"
        )

    def _stdout(self) -> str:
        table = lab.build_independent_erf_table()
        lines = [self.identity]
        for index, (vector_id, vector) in enumerate(
            zip(lab.VECTOR_IDS, self.vectors, strict=True)
        ):
            output = vector.translate(table)
            lines.append(
                f"VECTOR index={index} id={vector_id} "
                f"duration_us={100 + index} "
                f"input_fnv1a64=0x{lab.fnv1a64(vector):016x} "
                f"output_fnv1a64=0x{lab.fnv1a64(output):016x} "
                f"output_hex={output.hex()}"
            )
        lines.append(lab.SUMMARY_LINE)
        return "\n".join(lines)

    def test_independent_oracle_is_stable(self) -> None:
        table = lab.build_independent_erf_table()
        self.assertEqual(len(table), 256)
        self.assertEqual(table[0], 0)
        self.assertEqual(table[96], 20)
        self.assertEqual(table[128], 128)
        self.assertEqual(table[160], 236)
        self.assertEqual(table[255], 255)

    def test_exact_six_vector_result_passes(self) -> None:
        parsed = lab.parse_result(
            self._stdout(), self.manifest, self.vectors
        )
        self.assertTrue(parsed["numeric_match"]["passed"])
        self.assertEqual(parsed["numeric_match"]["compared_bytes"], 2886)
        self.assertEqual(parsed["numeric_match"]["mismatched_bytes"], 0)
        self.assertEqual(parsed["run_count"], 6)
        self.assertEqual(parsed["vectors"][5]["id"], "learned-fixture")
        self.assertGreater(parsed["vectors"][5]["output_unique"], 1)

    def test_one_wrong_device_byte_is_rejected(self) -> None:
        stdout = self._stdout()
        marker = "output_hex="
        start = stdout.index(marker) + len(marker)
        changed = "01" if stdout[start : start + 2] != "01" else "02"
        corrupted = stdout[:start] + changed + stdout[start + 2 :]
        with self.assertRaises(lab.ResultError):
            lab.parse_result(corrupted, self.manifest, self.vectors)

    def test_duplicate_or_foreign_identity_is_rejected(self) -> None:
        with self.assertRaisesRegex(lab.ResultError, "exactly one"):
            lab.parse_result(
                f"{self.identity}\n{self._stdout()}",
                self.manifest,
                self.vectors,
            )
        foreign = self._stdout().replace(
            "customer_id=0x1000000e",
            "customer_id=0x10000020",
            1,
        )
        with self.assertRaisesRegex(lab.ResultError, "identity"):
            lab.parse_result(foreign, self.manifest, self.vectors)

    def test_remote_command_is_fixed_and_has_no_retry(self) -> None:
        sentinel = (
            "__WAVE_ERF_MASK_EXIT_"
            "0123456789abcdef0123456789abcdef__"
        )
        command = lab.build_remote_command(self.manifest, sentinel)
        self.assertIn("env -i", command)
        self.assertEqual(command.count("--qualify-six"), 1)
        self.assertEqual(command.count(lab.REMOTE_EXECUTABLE), 1)
        self.assertNotIn("retry", command.lower())
        self.assertNotIn("--benchmark", command)

    def test_remote_exit_sentinel_is_unique(self) -> None:
        sentinel = (
            "__WAVE_ERF_MASK_EXIT_"
            "0123456789abcdef0123456789abcdef__"
        )
        cleaned, status = lab.parse_remote_exit(
            f"hello\n{sentinel}:0\n", sentinel
        )
        self.assertEqual(cleaned, "hello")
        self.assertEqual(status, 0)
        with self.assertRaises(lab.ResultError):
            lab.parse_remote_exit(
                f"{sentinel}:0\n{sentinel}:0\n", sentinel
            )


class ErfMaskHealthGateTests(unittest.TestCase):
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
            "dmesg_tail": ["[1.0] baseline"],
            "errors": [],
        }

    def test_clean_preflight_passes(self) -> None:
        lab.verify_preflight(self.manifest, self.snapshot)

    def test_preflight_rejects_holder_or_wrong_vector_size(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["galcore_holders"] = [{"pid": 9, "fd": 4}]
        with self.assertRaisesRegex(lab.PreflightError, "already held"):
            lab.verify_preflight(self.manifest, changed)

        changed = copy.deepcopy(self.snapshot)
        changed["staged_artifact_states"][lab.REMOTE_VECTORS][
            "bytes"
        ] = 2901
        with self.assertRaisesRegex(lab.PreflightError, "byte count"):
            lab.verify_preflight(self.manifest, changed)

    def test_preflight_rejects_clc_or_competing_executor(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["absent_artifact_states"][
            f"{lab.REMOTE_ROOT}/lib/libCLC.so"
        ] = {"status": "present"}
        with self.assertRaisesRegex(lab.PreflightError, "CLC"):
            lab.verify_preflight(self.manifest, changed)

        changed = copy.deepcopy(self.snapshot)
        changed["processes"] = [
            f"123 root {lab.REMOTE_EXECUTABLE} --qualify-six"
        ]
        with self.assertRaisesRegex(lab.PreflightError, "already present"):
            lab.verify_preflight(self.manifest, changed)

    def test_health_gate_detects_new_kernel_fault(self) -> None:
        changed = copy.deepcopy(self.snapshot)
        changed["uptime"] = "101.0 91.0"
        changed["dmesg_tail"].append("[2.0] galcore MMU fault")
        differences, alerts = lab.compare_health(
            self.manifest, self.snapshot, changed
        )
        self.assertTrue(alerts)
        self.assertIn(
            "new kernel warning/error diagnostics were observed",
            differences,
        )


if __name__ == "__main__":
    unittest.main()
