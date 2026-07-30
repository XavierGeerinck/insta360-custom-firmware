from __future__ import annotations

import copy
import hashlib
import json
import struct
import sys
import unittest
import zlib
from pathlib import Path


NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import nbg_erf_mask_profile as profile  # noqa: E402
from tests.test_nbg_format import (  # noqa: E402
    float_word,
    make_v2,
    v2_io,
    v2_network_info,
)


SIGNATURE = "1" * 64


def model_manifest() -> bytes:
    return profile.canonical_json(
        {
            "execution_approved": False,
            "compiler_qualified": False,
            "npu_partition": {
                "signature": SIGNATURE,
                "contract": {},
                "nbg": None,
                "numerical_evidence_sha256": None,
            },
        }
    )


def nbg_fixture() -> bytes:
    input_record = v2_io(
        "input/output[0]",
        (481,),
        data_format=2,
        quantization_format=2,
        word_0=float_word(0.03125),
        word_1=128,
    )
    output_record = v2_io(
        "uid_1_out_0",
        (481,),
        data_format=2,
        quantization_format=2,
        word_0=float_word(0.0078125),
        word_1=128,
    )
    opaque = b"\0" * 64
    checksum_record = struct.pack(
        "<II",
        24,
        zlib.crc32(opaque) & 0xFFFFFFFF,
    )
    return make_v2(
        input_record=input_record,
        output_record=output_record,
        opaque=opaque,
        extra_sections=[
            (35, v2_network_info(), 220),
            (40, checksum_record, 8),
        ],
    )


class ProfileTests(unittest.TestCase):
    def _qualify(
        self,
        left: bytes,
        right: bytes | None = None,
        model: bytes | None = None,
    ) -> dict:
        return profile.qualify_erf_mask_nbg(
            left,
            left if right is None else right,
            model_manifest() if model is None else model,
            expected_nbg_sha256=hashlib.sha256(left).hexdigest(),
            expected_model_manifest_sha256=hashlib.sha256(
                model_manifest() if model is None else model
            ).hexdigest(),
            expected_partition_signature=SIGNATURE,
            expected_section_types=(1, 2, 35, 40, 24),
        )

    def test_exact_profile_has_tensor_crc_roundtrip_and_nonclaims(self) -> None:
        result = self._qualify(nbg_fixture())
        self.assertEqual(
            result["classification"],
            "sealed-unapproved-erf-mask-partition",
        )
        self.assertTrue(result["artifact"]["a1_a2_byte_identical"])
        self.assertEqual(
            result["tensors"]["inputs"][0]["quantization"],
            {
                "format": "affine-asymmetric",
                "scale": 0.03125,
                "zero_point": 128,
            },
        )
        self.assertEqual(
            result["tensors"]["outputs"][0]["quantization"]["scale"],
            0.0078125,
        )
        self.assertTrue(result["file_checksum"]["matches"])
        self.assertTrue(result["roundtrip"]["exact_match"])
        self.assertEqual(result["roundtrip"]["padding_nonzero_bytes"], 0)
        self.assertFalse(result["package"]["execution_approved"])
        self.assertEqual(result["device_calls"], 0)

    def test_a1_a2_mismatch_and_truncation_fail(self) -> None:
        payload = nbg_fixture()
        changed = bytearray(payload)
        changed[-1] ^= 1
        with self.assertRaisesRegex(profile.ProfileError, "determinism"):
            self._qualify(payload, bytes(changed))
        with self.assertRaises(profile.ProfileError):
            self._qualify(payload[:-1])

    def test_every_section_corruption_fails_file_crc(self) -> None:
        payload = nbg_fixture()
        parsed = profile.nbg_format.parse_bytes(payload)
        for section in parsed["sections"]:
            changed = bytearray(payload)
            changed[section["offset"]] ^= 1
            corrupted = bytes(changed)
            with self.subTest(section=section["type"]):
                with self.assertRaises(profile.ProfileError):
                    self._qualify(corrupted)

    def test_shape_type_scale_zero_point_name_and_target_fail(self) -> None:
        mutations = [
            v2_io(
                "input/output[0]",
                (480,),
                data_format=2,
                quantization_format=2,
                word_0=float_word(0.03125),
                word_1=128,
            ),
            v2_io(
                "wrong",
                (481,),
                data_format=2,
                quantization_format=2,
                word_0=float_word(0.03125),
                word_1=128,
            ),
            v2_io(
                "input/output[0]",
                (481,),
                data_format=1,
                quantization_format=2,
                word_0=float_word(0.03125),
                word_1=128,
            ),
            v2_io(
                "input/output[0]",
                (481,),
                data_format=2,
                quantization_format=2,
                word_0=float_word(0.0625),
                word_1=128,
            ),
            v2_io(
                "input/output[0]",
                (481,),
                data_format=2,
                quantization_format=2,
                word_0=float_word(0.03125),
                word_1=127,
            ),
        ]
        reference = nbg_fixture()
        for input_record in mutations:
            opaque = b"\0" * 64
            payload = make_v2(
                input_record=input_record,
                output_record=v2_io(
                    "uid_1_out_0",
                    (481,),
                    data_format=2,
                    quantization_format=2,
                    word_0=float_word(0.0078125),
                    word_1=128,
                ),
                opaque=opaque,
                extra_sections=[
                    (35, v2_network_info(), 220),
                    (
                        40,
                        struct.pack(
                            "<II",
                            24,
                            zlib.crc32(opaque) & 0xFFFFFFFF,
                        ),
                        8,
                    ),
                ],
            )
            with self.assertRaises(profile.ProfileError):
                self._qualify(payload)

        wrong_target = bytearray(reference)
        struct.pack_into("<I", wrong_target, 8, 0x1000000D)
        with self.assertRaises(profile.ProfileError):
            self._qualify(bytes(wrong_target))

    def test_package_hash_signature_and_approval_state_fail_closed(self) -> None:
        payload = nbg_fixture()
        document = json.loads(model_manifest())
        cases = []
        wrong = copy.deepcopy(document)
        wrong["npu_partition"]["signature"] = "2" * 64
        cases.append(wrong)
        wrong = copy.deepcopy(document)
        wrong["execution_approved"] = True
        cases.append(wrong)
        wrong = copy.deepcopy(document)
        wrong["compiler_qualified"] = True
        cases.append(wrong)
        wrong = copy.deepcopy(document)
        wrong["npu_partition"]["nbg"] = {"sha256": "3" * 64}
        cases.append(wrong)
        for changed in cases:
            encoded = profile.canonical_json(changed)
            with self.assertRaises(profile.ProfileError):
                self._qualify(payload, model=encoded)

    def test_canonical_output_is_deterministic_and_has_no_raw_payload(self) -> None:
        payload = nbg_fixture()
        first = profile.canonical_json(self._qualify(payload))
        second = profile.canonical_json(self._qualify(payload))
        self.assertEqual(first, second)
        self.assertNotIn(payload, first)


if __name__ == "__main__":
    unittest.main()
