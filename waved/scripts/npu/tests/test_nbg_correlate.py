#!/usr/bin/env python3
"""Bounded transform and exact-section correlation contract tests."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

import numpy as np

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import nbg_correlate as correlate  # noqa: E402
import nbg_serializer_recovery as recovery  # noqa: E402


def limits(**overrides: int) -> correlate.CorrelationLimits:
    values = {
        "max_candidate_bytes": 1024 * 1024,
        "max_candidates_per_tensor": 128,
        "max_initializer_bytes": 1024 * 1024,
        "max_search_bytes_per_tensor": 64 * 1024 * 1024,
        "max_search_sections": 16,
    }
    values.update(overrides)
    return correlate.CorrelationLimits.from_mapping(values)


def tensor(
    values: list[float] | np.ndarray,
    *,
    name: str = "weight",
    shape: tuple[int, ...] | None = None,
) -> correlate.TensorSource:
    array = np.asarray(values, dtype=np.float32)
    if shape is not None:
        array = array.reshape(shape)
    return correlate.TensorSource.from_array(name, array)


def candidate(
    source: correlate.TensorSource,
    transform_id: str,
    **parameters: object,
) -> correlate.TransformCandidate:
    matches = [
        item
        for item in correlate.iter_transform_candidates(source, limits())
        if item.transform_id == transform_id
        and all(item.parameters.get(key) == value for key, value in parameters.items())
    ]
    if len(matches) != 1:
        raise AssertionError(
            f"expected one {transform_id} candidate, observed {len(matches)}"
        )
    return matches[0]


def section(
    payload: bytes,
    *,
    index: int = 7,
    offset: int = 0x1000,
) -> correlate.SearchSection:
    return correlate.SearchSection(
        artifact_id="fixture",
        section_index=index,
        section_type=24,
        type_name="NBG_SECTION_COEF_DATA",
        file_offset=offset,
        payload=payload,
    )


class TransformTests(unittest.TestCase):
    def test_registry_is_finite_deterministic_and_bounded(self) -> None:
        source = tensor(np.arange(32, dtype=np.float32), shape=(4, 8))
        first = list(correlate.iter_transform_candidates(source, limits()))
        second = list(correlate.iter_transform_candidates(source, limits()))
        self.assertEqual(
            [item.to_summary_document() for item in first],
            [item.to_summary_document() for item in second],
        )
        self.assertEqual(len(first), 50)
        self.assertLessEqual(len(first), 128)
        self.assertEqual(
            {item.transform_id for item in first},
            recovery.TRANSFORM_IDS,
        )

    def test_identity_unique_match_is_direct(self) -> None:
        source = tensor([1.0, 2.0, 3.0])
        item = candidate(source, "identity-fp32-le")
        result = correlate.search_candidate(
            item,
            [section(b"prefix" + item.payload + b"suffix")],
            limits(),
        )
        self.assertEqual(result.status, "unique")
        self.assertEqual(result.evidence_class, "direct-match")
        self.assertEqual(result.occurrence_count, 1)
        self.assertEqual(
            result.occurrences[0].absolute_offset,
            0x1000 + len(b"prefix"),
        )

    def test_duplicate_match_is_ambiguous(self) -> None:
        source = tensor([1.0, 2.0])
        item = candidate(source, "identity-fp32-le")
        result = correlate.search_candidate(
            item,
            [section(item.payload + b"x" + item.payload)],
            limits(),
        )
        self.assertEqual(result.status, "ambiguous")
        self.assertEqual(result.error_code, "CORRELATION_AMBIGUOUS")
        self.assertEqual(result.occurrence_count, 2)
        self.assertEqual(result.evidence_class, "unresolved")

    def test_fp16_and_bf16_matches_are_named_transforms(self) -> None:
        source = tensor([1.0, -2.25, 0.333251953125])
        for transform_id in (
            "cast-fp16-rne-le",
            "cast-bf16-rne-le",
        ):
            with self.subTest(transform_id=transform_id):
                item = candidate(source, transform_id)
                result = correlate.search_candidate(
                    item, [section(item.payload)], limits()
                )
                self.assertEqual(result.status, "unique")
                self.assertEqual(
                    result.evidence_class, "transformed-match"
                )
                self.assertFalse(item.inverse["available"])
                self.assertIn("precision", item.inverse["reason"])

    def test_transpose_then_fp16_match(self) -> None:
        source = tensor(
            np.arange(6, dtype=np.float32),
            shape=(2, 3),
        )
        item = candidate(
            source, "transpose-2d+cast-fp16-rne-le"
        )
        expected = (
            np.arange(6, dtype=np.float32)
            .reshape(2, 3)
            .T.astype("<f2")
            .tobytes()
        )
        self.assertEqual(item.payload, expected)
        result = correlate.search_candidate(
            item, [section(expected)], limits()
        )
        self.assertEqual(result.status, "unique")

    def test_fixed_block_permutation_match(self) -> None:
        source = tensor(np.arange(8, dtype=np.float32))
        item = candidate(
            source,
            "fixed-block-permute",
            block_elements=4,
            pattern="reverse",
        )
        expected = np.asarray(
            [3, 2, 1, 0, 7, 6, 5, 4], dtype="<f4"
        ).tobytes()
        self.assertEqual(item.payload, expected)
        self.assertEqual(
            correlate.search_candidate(
                item, [section(expected)], limits()
            ).status,
            "unique",
        )

    def test_byte_and_halfword_lane_permutations_match(self) -> None:
        source = tensor([1.0, 2.0, 3.0, 4.0])
        for transform_id, parameter in (
            ("byte-lane-permute", {"lane_bytes": 4}),
            ("halfword-lane-permute", {"lane_halfwords": 4}),
        ):
            with self.subTest(transform_id=transform_id):
                item = candidate(
                    source,
                    transform_id,
                    pattern="adjacent-pair-swap",
                    **parameter,
                )
                result = correlate.search_candidate(
                    item, [section(item.payload)], limits()
                )
                self.assertEqual(result.status, "unique")
                self.assertEqual(
                    result.evidence_class, "transformed-match"
                )

    def test_unrelated_bytes_do_not_match(self) -> None:
        source = tensor([1.0, 2.0, 3.0])
        item = candidate(source, "identity-fp32-le")
        result = correlate.search_candidate(
            item, [section(b"\xff" * len(item.payload))], limits()
        )
        self.assertEqual(result.status, "no-match")
        self.assertEqual(result.evidence_class, "unresolved")
        self.assertEqual(result.occurrence_count, 0)

    def test_size_section_and_candidate_limits_fail_closed(self) -> None:
        source = tensor(np.arange(16, dtype=np.float32))
        item = candidate(source, "identity-fp32-le")
        with self.assertRaisesRegex(
            recovery.SerializerError, "CORRELATION_LIMIT_EXCEEDED"
        ):
            correlate.search_candidate(
                item,
                [section(item.payload), section(item.payload, index=8)],
                limits(max_search_sections=1),
            )
        with self.assertRaisesRegex(
            recovery.SerializerError, "CORRELATION_LIMIT_EXCEEDED"
        ):
            list(
                correlate.iter_transform_candidates(
                    source, limits(max_candidate_bytes=4)
                )
            )
        with self.assertRaisesRegex(
            recovery.SerializerError, "CORRELATION_LIMIT_EXCEEDED"
        ):
            correlate.correlate_tensor(
                source,
                [section(b"\0" * 100)],
                limits(max_candidates_per_tensor=1),
            )

    def test_tensor_correlation_document_is_deterministic(self) -> None:
        source = tensor([1.0, 2.0, 3.0])
        sections = [section(b"\xff" * 128)]
        first = correlate.correlate_tensor(source, sections, limits())
        second = correlate.correlate_tensor(source, sections, limits())
        self.assertEqual(first, second)
        self.assertEqual(first["relationship"]["class"], "unresolved")
        self.assertEqual(
            first["plain_fp16"]["transform_id"],
            "cast-fp16-rne-le",
        )


class CorrelationEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = recovery.load_manifest()
        recovery.validate_artifacts(cls.manifest)

    def test_real_anchor_is_strict_and_unresolved_without_guessing(self):
        document = correlate.build_correlation_evidence(self.manifest)
        self.assertEqual(document["decision"]["poc12b"], "not-ready")
        self.assertEqual(
            document["anchor"]["onnx"]["node_types"],
            ["MatMul", "Add", "LogSoftmax"],
        )
        self.assertEqual(
            [item["source"]["name"] for item in document["tensors"]],
            ["ctc_lo.bias", "onnx::MatMul_7"],
        )
        self.assertEqual(
            [item["candidate_count"] for item in document["tensors"]],
            [47, 50],
        )
        for item in document["tensors"]:
            with self.subTest(tensor=item["source"]["name"]):
                self.assertEqual(
                    item["relationship"]["class"], "unresolved"
                )
                self.assertEqual(item["plain_fp16"]["status"], "no-match")
                self.assertEqual(
                    item["plain_fp16"]["occurrence_count"], 0
                )
        self.assertTrue(
            all(value == 0 for value in document["safety"].values())
        )
        self.assertFalse(document["tools"]["onnx_runtime_imported"])
        self.assertEqual(len(document["anchor"]["search_sections"]), 9)

    def test_real_evidence_is_byte_deterministic_and_payload_free(self):
        first = correlate.build_correlation_evidence(self.manifest)
        second = correlate.build_correlation_evidence(self.manifest)
        encoded = recovery.canonical_json(first)
        self.assertEqual(encoded, recovery.canonical_json(second))
        self.assertNotIn(str(self.manifest.repository_root).encode(), encoded)
        self.assertNotIn(b"raw_payload", encoded)
        self.assertNotIn(b"source_bytes\": \"", encoded)

    def test_multilingual_controls_remain_differential_only(self):
        document = correlate.build_correlation_evidence(self.manifest)
        differential = document["differential_controls"]
        self.assertEqual(
            differential["relationship_class"],
            "differentially-supported",
        )
        self.assertEqual(len(differential["profiles"]), 4)
        self.assertEqual(len(differential["comparisons"]), 3)
        output_widths = {
            item["artifact_id"]: item["output_dimensions"][0][0]
            for item in differential["profiles"]
        }
        self.assertEqual(
            output_widths,
            {
                "ctc-de": 5200,
                "ctc-en": 4999,
                "ctc-fr": 5432,
                "ctc-ja": 5132,
            },
        )
        self.assertIn("confounded", differential["warning"])

    def test_tracked_real_evidence_verifies(self):
        correlate.verify_generated(self.manifest)


if __name__ == "__main__":
    unittest.main()
