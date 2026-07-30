from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_compile_lab as lab  # noqa: E402


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = lab.ERF_AFFINE_POLICY
        self.manifest = lab.load_manifest(policy=self.policy)

    def _document(self) -> dict:
        return json.loads(self.policy.manifest_path.read_text(encoding="utf-8"))

    def _load(self, document: dict):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return lab.load_manifest(path, policy=self.policy)

    def test_manifest_is_exact_six_candidate_affine_compile_only(self) -> None:
        self.assertEqual(self.policy.poc_id, "14B")
        self.assertEqual(self.policy.remote_root, "/tmp/wave-npu-poc14b")
        self.assertEqual(
            tuple(self.manifest.candidates),
            ("a1", "a2", "b1", "b2", "c1", "c2"),
        )
        self.assertEqual(self.policy.inputs[0].dimensions, (481,))
        self.assertEqual(self.policy.inputs[0].quantization_scale, 0.0625)
        self.assertEqual(
            self.policy.outputs[0].quantization_scale,
            0.0078125,
        )
        self.assertEqual(
            self.manifest.required_artifacts[self.policy.remote_executable],
            "eab129f936cd3fc22142acd7abe55d9ecd276d1a2d7e76d0c647acf04af75fe9",
        )

    def test_manifest_rejects_other_argument_output_and_binary(self) -> None:
        document = self._document()
        document["candidates"]["b1"]["argument"] = "--weights"
        with self.assertRaises(lab.ManifestError):
            self._load(document)
        document = self._document()
        document["candidates"]["c2"]["output"] = "/tmp/other.nbg"
        with self.assertRaises(lab.ManifestError):
            self._load(document)
        document = self._document()
        document["required_artifacts"][self.policy.remote_executable] = "not-a-digest"
        with self.assertRaises(lab.ManifestError):
            self._load(document)


class NativeAndNbgContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = lab.load_manifest(policy=lab.ERF_AFFINE_POLICY)
        self.candidate = self.manifest.candidates["b1"]

    def _stdout(self, backend: str = "OPENVX") -> str:
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
                f'D [selector]Instance {backend} node with kernel "multiply"',
                f'D [selector]Instance {backend} node with kernel "add"',
                f'D [selector]Instance {backend} node with kernel "erf"',
                (
                    "RESULT status=PASS mode=compile-only target=wave "
                    "candidate=b1 bytes=8192 version=0x00020000 "
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
                        "scale": 0.0625,
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

    def test_exact_native_result_and_external_nbg_contract_pass(self) -> None:
        parsed = lab.parse_native_result(
            self._stdout(),
            self.manifest,
            self.candidate,
        )
        self.assertEqual(parsed["result"]["candidate"], "b1")
        self.assertEqual(
            parsed["selector"],
            {"backend": "OPENVX", "kernel": "erf"},
        )
        lab.validate_nbg_report(self._report(), self.manifest)

    def test_wrong_input_scale_or_erf_backend_fails(self) -> None:
        report = self._report()
        report["inputs"][0]["quantization"]["scale"] = 0.03125
        with self.assertRaises(lab.ResultError):
            lab.validate_nbg_report(report, self.manifest)
        with self.assertRaisesRegex(lab.ResultError, "selected CPU"):
            lab.parse_native_result(
                self._stdout("CPU"),
                self.manifest,
                self.candidate,
            )


class PairComparisonTests(unittest.TestCase):
    def test_candidate_pairs_are_closed_and_differential(self) -> None:
        candidates = lab.ERF_AFFINE_POLICY.candidates
        self.assertEqual(
            {name: values["argument"] for name, values in candidates.items()},
            {
                "a1": "--a1",
                "a2": "--a2",
                "b1": "--b1",
                "b2": "--b2",
                "c1": "--c1",
                "c2": "--c2",
            },
        )
        for name, values in candidates.items():
            self.assertEqual(
                values["output"],
                f"/tmp/wave-npu-poc14b/output/erf-affine-{name}.nbg",
            )


if __name__ == "__main__":
    unittest.main()
