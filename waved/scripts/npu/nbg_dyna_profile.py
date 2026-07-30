#!/usr/bin/env python3
"""Offline Wave NBG DYNAIO and serializer file-checksum qualification.

The module never loads or executes an NBG.  It combines one controlled
constant differential, the generated stock-corpus inventory, and bounded
Ghidra facts from the exact Wave libOpenVX runtime.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
import tempfile
import zlib
from pathlib import Path
from typing import Any, Mapping, Sequence

try:
    from . import nbg_format
except ImportError:
    import nbg_format  # type: ignore


SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_RUN_ROOT = (
    REPOSITORY_ROOT
    / "artifacts/npu/runs/poc12b-erf-constant-diff-20260729"
)
DEFAULT_INVENTORY = (
    REPOSITORY_ROOT / "docs/npu/format/stock-nbg-inventory.json"
)
DEFAULT_OUTPUT = (
    REPOSITORY_ROOT / "docs/npu/format/dyna-io-crc-profile.json"
)
DEFAULT_RUNTIME = (
    REPOSITORY_ROOT / "waved/third_party/device-sdk/lib/libOpenVX.so"
)
DEFAULT_SAVE_EVIDENCE = (
    REPOSITORY_ROOT
    / "artifacts/npu/analysis/poc12c-libopenvx-string-xrefs.json"
)
DEFAULT_CRC_EVIDENCE = (
    REPOSITORY_ROOT
    / "artifacts/npu/analysis/poc12c-libopenvx-crc-function.json"
)
SOURCE_A = bytes((0, 1, 2, 3))
SOURCE_B = bytes((0, 1, 2, 4))
HEADER_CRC_OFFSET = 12
CHECKSUM_SECTION_TYPE = 40
CHECKSUM_RECORD_BYTES = 8
EXPECTED_CONTROL_ARTIFACTS = {
    "a1.nbg": (
        5904,
        "4a84faf4da4604429522dfa6f0a8eb988"
        "7287eecb0f97d697843e0beb78c8a8b",
    ),
    "a2.nbg": (
        5904,
        "4a84faf4da4604429522dfa6f0a8eb988"
        "7287eecb0f97d697843e0beb78c8a8b",
    ),
    "b.nbg": (
        5904,
        "c9e3e18414f5bc674d4204921966ba53"
        "b08918ddcef73112003b065b9599f227",
    ),
}
EXPECTED_DEFINITION_SHA256 = (
    "8fc97f217e33daf0ee8e96a87bd8ba3b"
    "9d1b96b98055db0f0207849f3c3176b5"
)
EXPECTED_INVENTORY_SHA256 = (
    "7103775e6659117bea2eafef353aece3"
    "ff75c5046a7fcb198a8a50e8d3dc1c6c"
)
EXPECTED_RUNTIME_SHA256 = (
    "f9cd917d4232316ab53a443ec93cd121"
    "7893f4de96fc113811b800b9ae8ca0bb"
)
EXPECTED_SAVE_EVIDENCE_SHA256 = (
    "177e1a78191bdde750dec8b21e2cf260"
    "fde94b7d0232377642fdece2b601f3e9"
)
EXPECTED_CRC_EVIDENCE_SHA256 = (
    "b1e80fd54ea3be3c183336fccbd78b7e"
    "81bd4ba12ec77717b227396609fa420f"
)
EXPECTED_SAVE_BODY_BYTES = 10428
EXPECTED_CRC_BODY_BYTES = 160


class ProfileError(Exception):
    """A bounded offline qualification precondition failed."""


def crc32(payload: bytes, initial: int = 0) -> int:
    """Return the unsigned IEEE CRC-32 used by the recovered runtime."""

    return zlib.crc32(payload, initial) & 0xFFFFFFFF


def canonical_json(value: Any) -> bytes:
    """Return deterministic UTF-8 JSON without binary payload material."""

    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _hex32(value: int) -> str:
    return f"0x{value:08x}"


def _exact_regular_bytes(
    path: Path,
    *,
    label: str,
    expected_sha256: str,
    expected_bytes: int | None = None,
) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ProfileError(f"expected regular non-symlink {label}: {path}")
    payload = path.read_bytes()
    if expected_bytes is not None and len(payload) != expected_bytes:
        raise ProfileError(
            f"{label} byte count changed: expected {expected_bytes}, "
            f"observed {len(payload)}"
        )
    observed_sha256 = _sha256(payload)
    if observed_sha256 != expected_sha256:
        raise ProfileError(
            f"{label} SHA-256 changed: expected {expected_sha256}, "
            f"observed {observed_sha256}"
        )
    return payload


def _strict_json(path: Path) -> tuple[dict[str, Any], bytes]:
    if path.is_symlink() or not path.is_file():
        raise ProfileError(f"expected regular non-symlink JSON: {path}")
    payload = path.read_bytes()

    def without_duplicates(
        pairs: list[tuple[str, Any]],
    ) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ProfileError(f"duplicate JSON key {key!r} in {path}")
            result[key] = value
        return result

    try:
        value = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=without_duplicates,
        )
    except ProfileError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ProfileError(f"invalid JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ProfileError(f"JSON root must be an object: {path}")
    return value, payload


def serializer_file_crc32(
    payload: bytes, parsed: Mapping[str, Any]
) -> int:
    """Reproduce the Wave serializer's header ``crc_sum``.

    Section payloads are processed in directory order.  Alignment gaps,
    headers, and directory bytes are excluded.  Type-40 record type words
    are included, while their checksum-value words are treated as zero
    because the producer fills those values after saving ``crc_sum``.
    """

    if not isinstance(payload, bytes):
        raise ProfileError("payload must be immutable bytes")
    checksum = 0
    sections = parsed.get("sections")
    if not isinstance(sections, list):
        raise ProfileError("parsed NBG has no section list")
    for section in sections:
        if not isinstance(section, dict):
            raise ProfileError("parsed section must be an object")
        offset = section.get("offset")
        size = section.get("size")
        section_type = section.get("type")
        if (
            isinstance(offset, bool)
            or not isinstance(offset, int)
            or isinstance(size, bool)
            or not isinstance(size, int)
            or offset < 0
            or size < 0
            or offset > len(payload)
            or size > len(payload) - offset
        ):
            raise ProfileError("parsed section range is invalid")
        section_data = payload[offset : offset + size]
        if section_type == CHECKSUM_SECTION_TYPE:
            if len(section_data) % CHECKSUM_RECORD_BYTES:
                raise ProfileError("checksum section geometry is invalid")
            normalized = bytearray(section_data)
            for value_offset in range(
                4, len(normalized), CHECKSUM_RECORD_BYTES
            ):
                normalized[value_offset : value_offset + 4] = b"\0" * 4
            section_data = bytes(normalized)
        checksum = crc32(section_data, checksum)
    return checksum


def _parse_control(payload: bytes, label: str) -> dict[str, Any]:
    try:
        parsed = nbg_format.parse_bytes(payload, display_path=f"<{label}>")
    except nbg_format.NbgParseError as exc:
        raise ProfileError(f"{label} failed strict NBG admission: {exc}") from exc
    if (
        parsed.get("version") != nbg_format.V2_VERSION
        or parsed.get("target") != 0x1000000E
    ):
        raise ProfileError(f"{label} is not an exact Wave v2 NBG")
    if not parsed["file_checksum"]["matches"]:
        raise ProfileError(f"{label} file checksum did not validate")
    return parsed


def _section(
    parsed: Mapping[str, Any], section_type: int
) -> Mapping[str, Any]:
    matches = [
        section
        for section in parsed["sections"]
        if section["type"] == section_type
    ]
    if len(matches) != 1:
        raise ProfileError(
            f"expected one section type {section_type}, observed "
            f"{len(matches)}"
        )
    return matches[0]


def _first_difference(left: bytes, right: bytes) -> int | None:
    for index, (old, new) in enumerate(zip(left, right)):
        if old != new:
            return index
    return min(len(left), len(right)) if len(left) != len(right) else None


def analyze_controlled_payloads(
    a1_payload: bytes,
    a2_payload: bytes,
    b_payload: bytes,
    *,
    source_a: bytes,
    source_b: bytes,
) -> dict[str, Any]:
    """Prove the controlled source mapping and rebuild B in memory."""

    if (
        not isinstance(source_a, bytes)
        or not isinstance(source_b, bytes)
        or not source_a
        or len(source_a) != len(source_b)
    ):
        raise ProfileError("source constants must be equal nonzero byte arrays")
    source_differences = [
        index
        for index, (old, new) in enumerate(zip(source_a, source_b))
        if old != new
    ]
    if len(source_differences) != 1:
        raise ProfileError("source constants must differ at exactly one byte")

    a1 = _parse_control(a1_payload, "A1")
    _parse_control(a2_payload, "A2")
    b = _parse_control(b_payload, "B")
    if a1_payload != a2_payload:
        raise ProfileError(
            "A1/A2 determinism gate failed before B interpretation"
        )
    if len(a1_payload) != len(b_payload):
        raise ProfileError("A/B file sizes differ")
    if a1["structural_signature"] != b["structural_signature"]:
        raise ProfileError("A/B structural signatures differ")
    if [
        section["directory_words"] for section in a1["sections"]
    ] != [section["directory_words"] for section in b["sections"]]:
        raise ProfileError("A/B section directories differ")

    a_dynamic = a1.get("dynamic_io")
    b_dynamic = b.get("dynamic_io")
    if not isinstance(a_dynamic, dict) or not isinstance(b_dynamic, dict):
        raise ProfileError("controlled NBGs have no qualified DYNAIO profile")
    if a_dynamic != b_dynamic:
        raise ProfileError("A/B DYNAIO table geometry differs")
    a_data_section = _section(a1, 20)
    b_data_section = _section(b, 20)
    candidates: list[dict[str, int]] = []
    for entry in a_dynamic["entries"]:
        relative = entry["offset"]
        size = entry["size"]
        if size != len(source_a):
            continue
        a_start = a_data_section["offset"] + relative
        b_start = b_data_section["offset"] + relative
        if (
            a1_payload[a_start : a_start + size] == source_a
            and b_payload[b_start : b_start + size] == source_b
        ):
            candidates.append(
                {
                    "entry_index": entry["index"],
                    "entry_offset": relative,
                    "entry_size": size,
                    "file_offset": a_start,
                }
            )
    if len(candidates) != 1:
        raise ProfileError(
            f"expected one direct DYNAIO source mapping, observed "
            f"{len(candidates)}"
        )
    mapping = candidates[0]

    reconstructed = bytearray(a1_payload)
    start = mapping["file_offset"]
    reconstructed[start : start + len(source_b)] = source_b
    reconstructed_crc = serializer_file_crc32(bytes(reconstructed), a1)
    struct.pack_into(
        "<I", reconstructed, HEADER_CRC_OFFSET, reconstructed_crc
    )
    reconstructed_bytes = bytes(reconstructed)
    if reconstructed_bytes != b_payload:
        difference = _first_difference(reconstructed_bytes, b_payload)
        raise ProfileError(
            "A/B changed outside DYNAIO source bytes and recovered "
            f"file checksum; first mismatch {difference}"
        )

    changed_index = source_differences[0]
    a_recorded = a1["header"]["crc32"]
    b_recorded = b["header"]["crc32"]
    a_computed = serializer_file_crc32(a1_payload, a1)
    b_computed = serializer_file_crc32(b_payload, b)
    return {
        "artifacts": {
            "a1": {
                "bytes": len(a1_payload),
                "sha256": _sha256(a1_payload),
            },
            "a2": {
                "bytes": len(a2_payload),
                "sha256": _sha256(a2_payload),
            },
            "b": {
                "bytes": len(b_payload),
                "sha256": _sha256(b_payload),
            },
        },
        "classification": "direct-dyna-io-entry-mapping",
        "determinism": {
            "a1_sha256": _sha256(a1_payload),
            "a2_sha256": _sha256(a2_payload),
            "exact_match": True,
            "first_difference": None,
        },
        "dynamic_io": {
            "changed_file_offset": start + changed_index,
            "changed_region_relative_offset": (
                mapping["entry_offset"] + changed_index
            ),
            "data_section_index": a_data_section["index"],
            "data_section_offset": a_data_section["offset"],
            "data_section_size": a_data_section["size"],
            "entry_index": mapping["entry_index"],
            "entry_offset": mapping["entry_offset"],
            "entry_size": mapping["entry_size"],
            "new_byte_hex": f"{source_b[changed_index]:02x}",
            "old_byte_hex": f"{source_a[changed_index]:02x}",
            "table_section_index": a_dynamic["table_section_index"],
        },
        "file_checksum": {
            "a_computed_crc32_hex": _hex32(a_computed),
            "a_matches": a_computed == a_recorded,
            "a_recorded_crc32_hex": _hex32(a_recorded),
            "algorithm": "crc32-ieee",
            "b_computed_crc32_hex": _hex32(b_computed),
            "b_matches": b_computed == b_recorded,
            "b_recorded_crc32_hex": _hex32(b_recorded),
            "initial_value": 0,
            "normalized_checksum_value_words": a1["file_checksum"][
                "normalized_checksum_value_words"
            ],
            "scope": "section-payloads-in-directory-order",
        },
        "reconstruction": {
            "exact_match": True,
            "first_difference": None,
            "reconstructed_sha256": _sha256(reconstructed_bytes),
            "written_to_disk": False,
        },
        "schema_version": SCHEMA_VERSION,
        "source": {
            "a_u8": list(source_a),
            "b_u8": list(source_b),
            "changed_index": changed_index,
        },
    }


def aggregate_inventory(
    inventory: Mapping[str, Any],
) -> dict[str, Any]:
    """Aggregate already generated strict-parser results."""

    files = inventory.get("files")
    corpus = inventory.get("corpus")
    if not isinstance(files, list) or not isinstance(corpus, dict):
        raise ProfileError("inventory has invalid root structure")
    configured = corpus.get("configured_file_count")
    if configured != len(files):
        raise ProfileError("inventory configured-file count is inconsistent")

    v2_files = 0
    legacy_files = 0
    matched = 0
    mismatched = 0
    covered_bytes = 0
    dyna_files = 0
    dyna_entries = 0
    dyna_region_bytes = 0
    dyna_payload_bytes = 0
    dyna_padding_bytes = 0
    entry_counts: dict[int, int] = {}
    for item in files:
        if not isinstance(item, dict):
            raise ProfileError("inventory file entry must be an object")
        if item.get("version") != nbg_format.V2_VERSION:
            legacy_files += 1
            continue
        v2_files += 1
        integrity = item.get("file_checksum")
        if not isinstance(integrity, dict):
            raise ProfileError("v2 inventory entry lacks file checksum")
        if integrity.get("matches") is True:
            matched += 1
        else:
            mismatched += 1
        covered_bytes += int(integrity["covered_section_bytes"])
        dynamic = item.get("dynamic_io")
        if dynamic is None:
            continue
        if not isinstance(dynamic, dict):
            raise ProfileError("dynamic_io must be an object or null")
        if dynamic.get("padding_all_zero") is not True:
            raise ProfileError("inventory contains nonzero DYNAIO padding")
        entries = dynamic.get("entries")
        if not isinstance(entries, list):
            raise ProfileError("dynamic_io entries must be an array")
        prior_end = 0
        for entry in entries:
            if (
                not isinstance(entry, dict)
                or entry.get("offset_alignment")
                != nbg_format.V2_DYNA_IO_ALIGNMENT
                or entry["offset"] < prior_end
                or entry["size"] <= 0
                or entry["end"] != entry["offset"] + entry["size"]
                or entry["end"] > dynamic["region_bytes"]
            ):
                raise ProfileError("inventory DYNAIO entry is inconsistent")
            prior_end = entry["end"]
        count = len(entries)
        if count != dynamic.get("entry_count"):
            raise ProfileError("inventory DYNAIO entry count is inconsistent")
        dyna_files += 1
        dyna_entries += count
        dyna_region_bytes += int(dynamic["region_bytes"])
        dyna_payload_bytes += int(dynamic["payload_bytes"])
        dyna_padding_bytes += int(dynamic["padding_bytes"])
        entry_counts[count] = entry_counts.get(count, 0) + 1

    return {
        "configured_files": len(files),
        "covered_section_bytes": covered_bytes,
        "dyna_io": {
            "entry_count_distribution": [
                {"entries": count, "files": entry_counts[count]}
                for count in sorted(entry_counts)
            ],
            "entries": dyna_entries,
            "files": dyna_files,
            "padding_bytes": dyna_padding_bytes,
            "padding_nonzero_bytes": 0,
            "payload_bytes": dyna_payload_bytes,
            "region_bytes": dyna_region_bytes,
        },
        "legacy_files_skipped": legacy_files,
        "v2_file_checksums_matched": matched,
        "v2_file_checksums_mismatched": mismatched,
        "v2_files": v2_files,
    }


def _find_function(
    evidence: Mapping[str, Any], entry: str
) -> Mapping[str, Any]:
    functions: list[Any] = []
    if isinstance(evidence.get("functions"), list):
        functions.extend(evidence["functions"])
    targets = evidence.get("targets")
    if isinstance(targets, list):
        for target in targets:
            if isinstance(target, dict) and isinstance(
                target.get("functions"), list
            ):
                functions.extend(target["functions"])
    matches = [
        function
        for function in functions
        if isinstance(function, dict) and function.get("entry") == entry
    ]
    if len(matches) != 1:
        raise ProfileError(
            f"expected one Ghidra function at {entry}, observed "
            f"{len(matches)}"
        )
    return matches[0]


def _runtime_profile(
    runtime_path: Path,
    save_evidence_path: Path,
    crc_evidence_path: Path,
) -> dict[str, Any]:
    runtime = _exact_regular_bytes(
        runtime_path,
        label="Wave libOpenVX runtime",
        expected_sha256=EXPECTED_RUNTIME_SHA256,
    )
    runtime_sha = _sha256(runtime)
    save_evidence, save_payload = _strict_json(save_evidence_path)
    crc_evidence, crc_payload = _strict_json(crc_evidence_path)
    if _sha256(save_payload) != EXPECTED_SAVE_EVIDENCE_SHA256:
        raise ProfileError("save-function Ghidra evidence SHA-256 changed")
    if _sha256(crc_payload) != EXPECTED_CRC_EVIDENCE_SHA256:
        raise ProfileError("CRC-function Ghidra evidence SHA-256 changed")
    for label, evidence in (
        ("save", save_evidence),
        ("crc", crc_evidence),
    ):
        program = evidence.get("program")
        if (
            not isinstance(program, dict)
            or program.get("sha256") != runtime_sha
        ):
            raise ProfileError(
                f"{label} Ghidra evidence does not match libOpenVX"
            )

    save_function = _find_function(save_evidence, "002d2f20")
    crc_function = _find_function(crc_evidence, "002c8160")
    save_code = save_function.get("decompiled_c")
    crc_code = crc_function.get("decompiled_c")
    if not isinstance(save_code, str) or not isinstance(crc_code, str):
        raise ProfileError("Ghidra evidence has no decompiled functions")
    save_markers = (
        "nbg_save_file_checksum",
        "FUN_002c8160",
        "+ 0x7f92c",
        "+ 0x85648",
    )
    crc_markers = (
        "0xedb88320",
        "param_1 = ~param_1",
        "return ~param_1",
    )
    if not all(marker in save_code for marker in save_markers):
        raise ProfileError("save-function evidence markers are incomplete")
    if not all(marker in crc_code for marker in crc_markers):
        raise ProfileError("CRC-function evidence markers are incomplete")
    if (
        save_function.get("decompile_success") is not True
        or save_function.get("body_bytes") != EXPECTED_SAVE_BODY_BYTES
    ):
        raise ProfileError("save-function bounds or decompilation changed")
    if (
        crc_function.get("decompile_success") is not True
        or crc_function.get("body_bytes") != EXPECTED_CRC_BODY_BYTES
    ):
        raise ProfileError("CRC-function bounds or decompilation changed")
    return {
        "crc_function": {
            "body_bytes": crc_function.get("body_bytes"),
            "decompile_success": crc_function.get("decompile_success"),
            "ghidra_entry": "002c8160",
            "polynomial_hex": "0xedb88320",
            "reflected": True,
            "updates_accept_prior_crc": True,
        },
        "evidence": {
            "crc_export_sha256": _sha256(crc_payload),
            "save_export_sha256": _sha256(save_payload),
        },
        "libopenvx_sha256": runtime_sha,
        "save_function": {
            "body_bytes": save_function.get("body_bytes"),
            "decompile_success": save_function.get("decompile_success"),
            "ghidra_entry": "002d2f20",
            "header_crc_field_offset": HEADER_CRC_OFFSET,
            "initial_crc": 0,
            "type40_values_written_after_file_crc": True,
        },
    }


def _relative(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPOSITORY_ROOT).as_posix()
    except ValueError as exc:
        raise ProfileError(f"path is outside repository: {path}") from exc


def build_evidence(
    *,
    run_root: Path = DEFAULT_RUN_ROOT,
    inventory_path: Path = DEFAULT_INVENTORY,
    runtime_path: Path = DEFAULT_RUNTIME,
    save_evidence_path: Path = DEFAULT_SAVE_EVIDENCE,
    crc_evidence_path: Path = DEFAULT_CRC_EVIDENCE,
) -> dict[str, Any]:
    """Build the complete bounded PoC 12C evidence document."""

    control_payloads = {
        name: _exact_regular_bytes(
            run_root / name,
            label=f"controlled artifact {name}",
            expected_bytes=expected_bytes,
            expected_sha256=expected_sha256,
        )
        for name, (
            expected_bytes,
            expected_sha256,
        ) in EXPECTED_CONTROL_ARTIFACTS.items()
    }
    definition, definition_payload = _strict_json(
        run_root / "definition.json"
    )
    if _sha256(definition_payload) != EXPECTED_DEFINITION_SHA256:
        raise ProfileError("controlled definition SHA-256 changed")
    candidates = definition.get("candidates")
    if candidates != {
        "A1": list(SOURCE_A),
        "A2": list(SOURCE_A),
        "B": list(SOURCE_B),
    }:
        raise ProfileError("controlled definition constants changed")
    inventory, inventory_payload = _strict_json(inventory_path)
    if _sha256(inventory_payload) != EXPECTED_INVENTORY_SHA256:
        raise ProfileError("strict corpus inventory SHA-256 changed")
    controlled = analyze_controlled_payloads(
        control_payloads["a1.nbg"],
        control_payloads["a2.nbg"],
        control_payloads["b.nbg"],
        source_a=SOURCE_A,
        source_b=SOURCE_B,
    )
    corpus = aggregate_inventory(inventory)
    runtime = _runtime_profile(
        runtime_path, save_evidence_path, crc_evidence_path
    )
    if corpus["v2_file_checksums_mismatched"] != 0:
        raise ProfileError("stock corpus contains a file-checksum mismatch")
    return {
        "classification": (
            "dyna-io-read-and-file-crc-qualified"
        ),
        "controlled_differential": controlled,
        "corpus_validation": corpus,
        "non_claims": [
            "no NBG was loaded or executed",
            "no command stream or coefficient encoder was recovered",
            "no general ONNX-to-NBG compiler was recovered",
            "no arbitrary DYNAIO mutation is declared runtime-safe",
            "no noise-cancellation model was executed in this PoC",
        ],
        "poc_id": "poc12c-dyna-io-crc-profile",
        "runtime_recovery": runtime,
        "schema_version": SCHEMA_VERSION,
        "serializer_capabilities": {
            "compute_header_crc_sum": True,
            "decode_dyna_io_table": True,
            "rebuild_controlled_dyna_io_entry_in_memory": True,
            "write_mutated_nbg_to_disk": False,
        },
        "sources": {
            "controlled_definition": {
                "path": _relative(run_root / "definition.json"),
                "sha256": _sha256(definition_payload),
            },
            "corpus_inventory": {
                "path": _relative(inventory_path),
                "sha256": _sha256(inventory_payload),
            },
            "crc_ghidra_export": {
                "path": _relative(crc_evidence_path),
                "sha256": runtime["evidence"]["crc_export_sha256"],
            },
            "libopenvx": {
                "path": _relative(runtime_path),
                "sha256": runtime["libopenvx_sha256"],
            },
            "profiler": {
                "path": _relative(Path(__file__)),
                "sha256": _sha256(Path(__file__).read_bytes()),
            },
            "save_ghidra_export": {
                "path": _relative(save_evidence_path),
                "sha256": runtime["evidence"]["save_export_sha256"],
            },
        },
    }


def _atomic_write(path: Path, payload: bytes) -> None:
    if path.is_symlink():
        raise ProfileError(f"output may not be a symlink: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        temporary = Path(handle.name)
        handle.write(payload)
        handle.flush()
    temporary.replace(path)


def generate(output: Path = DEFAULT_OUTPUT) -> None:
    _relative(output)
    _atomic_write(output, canonical_json(build_evidence()))


def verify_generated(output: Path = DEFAULT_OUTPUT) -> None:
    expected = canonical_json(build_evidence())
    try:
        observed = output.read_bytes()
    except OSError as exc:
        raise ProfileError(f"cannot read generated evidence: {exc}") from exc
    if observed != expected:
        raise ProfileError(
            "generated DYNAIO/CRC evidence is stale; run generate"
        )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Offline Wave NBG DYNAIO and serializer file-CRC qualification"
        )
    )
    commands = parser.add_subparsers(dest="command", required=True)
    generate_command = commands.add_parser("generate")
    generate_command.add_argument(
        "--output", type=Path, default=DEFAULT_OUTPUT
    )
    verify_command = commands.add_parser("verify-generated")
    verify_command.add_argument(
        "--output", type=Path, default=DEFAULT_OUTPUT
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "generate":
            generate(args.output)
            print(f"wrote DYNAIO/CRC evidence to {args.output}")
            return 0
        if args.command == "verify-generated":
            verify_generated(args.output)
            print("generated DYNAIO/CRC evidence is current")
            return 0
        raise AssertionError(f"unhandled command {args.command}")
    except (ProfileError, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
