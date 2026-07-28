#!/usr/bin/env python3
"""Deterministic static ABI inventory for the Insta360 Wave NPU stack."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping, Sequence


SCHEMA_VERSION = 1
INVENTORY_SCHEMA_VERSION = 1
MATRIX_SCHEMA_VERSION = 1
DEFAULT_CORPUS_PATH = Path(__file__).with_name("abi_corpus.json")
DEFAULT_INVENTORY_PATH = (
    Path(__file__).resolve().parents[3]
    / "docs/npu/abi/runtime-abi-inventory.json"
)
DEFAULT_MATRIX_PATH = (
    Path(__file__).resolve().parents[3] / "docs/npu/abi/firmware-matrix.md"
)

ROOT_KEYS = frozenset(
    {
        "schema_version",
        "reference_firmware",
        "firmwares",
        "binaries",
        "symbol_filters",
        "string_filters",
        "caller_scan",
        "project_ffi",
    }
)
FIRMWARE_KEYS = frozenset({"root", "live_match"})
BINARY_KEYS = frozenset({"path", "required", "roles"})
SYMBOL_FILTER_KEYS = frozenset({"exact", "prefixes", "contains"})
CALLER_SCAN_KEYS = frozenset(
    {"directories", "needles", "maximum_file_bytes"}
)
ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")
THUNK_MAX_BYTES = 16
MAX_DIAGNOSTIC_BYTES = 512
RZBIN_TIMEOUT_SECONDS = 180

RZBIN_QUERIES = MappingProxyType(
    {
        "info": "-Ij",
        "libs": "-lj",
        "imports": "-ij",
        "symbols": "-sj",
        "relocs": "-Rj",
        "strings": "-zj",
    }
)


class AbiInventoryError(Exception):
    """Base error for expected static inventory failures."""


class CorpusError(AbiInventoryError):
    """The source-controlled corpus is malformed or unsafe."""


class ToolError(AbiInventoryError):
    """An external static-analysis tool failed."""


class InventoryError(AbiInventoryError):
    """Firmware evidence could not be inventoried safely."""


@dataclass(frozen=True)
class FirmwareSpec:
    firmware_id: str
    root: Path
    live_match: bool


@dataclass(frozen=True)
class BinarySpec:
    binary_id: str
    path: Path
    required: bool
    roles: tuple[str, ...]


@dataclass(frozen=True)
class SymbolFilters:
    exact: tuple[str, ...]
    prefixes: tuple[str, ...]
    contains: tuple[str, ...]

    def matches(self, value: str) -> bool:
        return (
            value in self.exact
            or any(value.startswith(prefix) for prefix in self.prefixes)
            or any(fragment in value for fragment in self.contains)
        )


@dataclass(frozen=True)
class CallerScan:
    directories: tuple[Path, ...]
    needles: tuple[bytes, ...]
    maximum_file_bytes: int


@dataclass(frozen=True)
class Corpus:
    repository_root: Path
    source_path: Path
    source_sha256: str
    reference_firmware: str
    firmwares: Mapping[str, FirmwareSpec]
    binaries: Mapping[str, BinarySpec]
    symbol_filters: SymbolFilters
    string_filters: tuple[str, ...]
    caller_scan: CallerScan
    project_ffi: tuple[str, ...]


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CorpusError(f"duplicate JSON key: {key!r}")
        result[key] = value
    return result


def _require_dict(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CorpusError(f"{context} must be a JSON object")
    return value


def _require_list(value: Any, context: str) -> list[Any]:
    if not isinstance(value, list):
        raise CorpusError(f"{context} must be a JSON array")
    return value


def _require_string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise CorpusError(f"{context} must be a nonempty string")
    if CONTROL_RE.search(value):
        raise CorpusError(f"{context} contains a control character")
    return value


def _require_bool(value: Any, context: str) -> bool:
    if not isinstance(value, bool):
        raise CorpusError(f"{context} must be a boolean")
    return value


def _require_int(
    value: Any, context: str, *, minimum: int | None = None
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise CorpusError(f"{context} must be an integer")
    if minimum is not None and value < minimum:
        raise CorpusError(f"{context} must be at least {minimum}")
    return value


def _strict_keys(
    value: Mapping[str, Any], expected: frozenset[str], context: str
) -> None:
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing:
        raise CorpusError(f"{context} missing keys: {', '.join(missing)}")
    if unknown:
        raise CorpusError(f"{context} has unknown keys: {', '.join(unknown)}")


def _string_list(
    value: Any, context: str, *, nonempty: bool = True
) -> tuple[str, ...]:
    raw = _require_list(value, context)
    if nonempty and not raw:
        raise CorpusError(f"{context} must not be empty")
    strings = tuple(
        _require_string(item, f"{context}[{index}]")
        for index, item in enumerate(raw)
    )
    if len(set(strings)) != len(strings):
        raise CorpusError(f"{context} contains duplicate values")
    return tuple(sorted(strings))


def _relative_path(value: Any, context: str) -> Path:
    text = _require_string(value, context)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts:
        raise CorpusError(f"{context} may not escape the repository: {text!r}")
    if path == Path("."):
        raise CorpusError(f"{context} must identify a path below the repository")
    return path


def _validate_resolved_below(
    root: Path, candidate: Path, context: str
) -> Path:
    root_resolved = root.resolve()
    candidate_resolved = candidate.resolve()
    try:
        candidate_resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise CorpusError(
            f"{context} resolves outside the repository boundary"
        ) from exc
    return candidate_resolved


def load_corpus(
    path: Path = DEFAULT_CORPUS_PATH,
    *,
    repository_root: Path | None = None,
) -> Corpus:
    """Load the strict source-controlled static-analysis corpus."""

    repository = (
        repository_root.resolve()
        if repository_root is not None
        else Path(__file__).resolve().parents[3]
    )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise CorpusError(f"cannot read corpus {path}: {exc}") from exc
    try:
        document = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except CorpusError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise CorpusError(f"invalid corpus JSON at {path}: {exc}") from exc

    root = _require_dict(document, "root")
    _strict_keys(root, ROOT_KEYS, "root")
    schema_version = _require_int(root["schema_version"], "schema_version")
    if schema_version != SCHEMA_VERSION:
        raise CorpusError(
            f"unsupported schema_version {schema_version}; "
            f"expected {SCHEMA_VERSION}"
        )

    reference_firmware = _require_string(
        root["reference_firmware"], "reference_firmware"
    )
    raw_firmwares = _require_dict(root["firmwares"], "firmwares")
    if not raw_firmwares:
        raise CorpusError("firmwares must not be empty")
    firmwares: dict[str, FirmwareSpec] = {}
    for firmware_id, raw_value in raw_firmwares.items():
        if ID_RE.fullmatch(firmware_id) is None:
            raise CorpusError(f"invalid firmware ID: {firmware_id!r}")
        context = f"firmwares.{firmware_id}"
        item = _require_dict(raw_value, context)
        _strict_keys(item, FIRMWARE_KEYS, context)
        relative_root = _relative_path(item["root"], f"{context}.root")
        _validate_resolved_below(
            repository,
            repository / relative_root,
            f"{context}.root",
        )
        firmwares[firmware_id] = FirmwareSpec(
            firmware_id=firmware_id,
            root=relative_root,
            live_match=_require_bool(
                item["live_match"], f"{context}.live_match"
            ),
        )

    if reference_firmware not in firmwares:
        raise CorpusError(
            f"reference_firmware {reference_firmware!r} is not defined"
        )
    live_matches = [
        firmware_id
        for firmware_id, spec in firmwares.items()
        if spec.live_match
    ]
    if live_matches != [reference_firmware]:
        raise CorpusError(
            "exactly the reference firmware must have live_match=true; "
            f"observed {sorted(live_matches)!r}"
        )

    raw_binaries = _require_dict(root["binaries"], "binaries")
    if not raw_binaries:
        raise CorpusError("binaries must not be empty")
    binaries: dict[str, BinarySpec] = {}
    for binary_id, raw_value in raw_binaries.items():
        if ID_RE.fullmatch(binary_id) is None:
            raise CorpusError(f"invalid binary ID: {binary_id!r}")
        context = f"binaries.{binary_id}"
        item = _require_dict(raw_value, context)
        _strict_keys(item, BINARY_KEYS, context)
        relative_binary = _relative_path(item["path"], f"{context}.path")
        roles = _string_list(item["roles"], f"{context}.roles")
        binaries[binary_id] = BinarySpec(
            binary_id=binary_id,
            path=relative_binary,
            required=_require_bool(item["required"], f"{context}.required"),
            roles=roles,
        )

    raw_filters = _require_dict(root["symbol_filters"], "symbol_filters")
    _strict_keys(raw_filters, SYMBOL_FILTER_KEYS, "symbol_filters")
    symbol_filters = SymbolFilters(
        exact=_string_list(raw_filters["exact"], "symbol_filters.exact"),
        prefixes=_string_list(
            raw_filters["prefixes"], "symbol_filters.prefixes"
        ),
        contains=_string_list(
            raw_filters["contains"], "symbol_filters.contains"
        ),
    )
    string_filters = tuple(
        item.casefold()
        for item in _string_list(root["string_filters"], "string_filters")
    )

    raw_caller = _require_dict(root["caller_scan"], "caller_scan")
    _strict_keys(raw_caller, CALLER_SCAN_KEYS, "caller_scan")
    directories = tuple(
        sorted(
            _relative_path(item, f"caller_scan.directories[{index}]")
            for index, item in enumerate(
                _require_list(
                    raw_caller["directories"], "caller_scan.directories"
                )
            )
        )
    )
    if not directories:
        raise CorpusError("caller_scan.directories must not be empty")
    caller_needles = _string_list(
        raw_caller["needles"], "caller_scan.needles"
    )
    caller_scan = CallerScan(
        directories=directories,
        needles=tuple(value.encode("utf-8") for value in caller_needles),
        maximum_file_bytes=_require_int(
            raw_caller["maximum_file_bytes"],
            "caller_scan.maximum_file_bytes",
            minimum=64,
        ),
    )

    project_ffi = _string_list(root["project_ffi"], "project_ffi")
    for symbol in project_ffi:
        if not symbol_filters.matches(symbol):
            raise CorpusError(
                f"project_ffi symbol is outside symbol filters: {symbol}"
            )

    return Corpus(
        repository_root=repository,
        source_path=path.resolve(),
        source_sha256=hashlib.sha256(payload).hexdigest(),
        reference_firmware=reference_firmware,
        firmwares=MappingProxyType(dict(sorted(firmwares.items()))),
        binaries=MappingProxyType(dict(sorted(binaries.items()))),
        symbol_filters=symbol_filters,
        string_filters=tuple(sorted(string_filters)),
        caller_scan=caller_scan,
        project_ffi=project_ffi,
    )


def _bounded_error(text: str, limit: int = 2000) -> str:
    normalized = text.strip().replace("\x00", "\\0")
    if len(normalized) <= limit:
        return normalized
    return normalized[: limit // 2] + " ...[truncated]... " + normalized[-limit // 2 :]


class RzBin:
    """Small subprocess boundary for deterministic rz-bin JSON queries."""

    def __init__(self, executable: str | None = None):
        self.executable = executable or shutil.which("rz-bin") or "rz-bin"

    def version(self) -> str:
        try:
            result = subprocess.run(
                [self.executable, "-v"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=10,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise ToolError(f"cannot query rz-bin version: {exc}") from exc
        if result.returncode != 0:
            raise ToolError(
                f"rz-bin version failed ({result.returncode}): "
                f"{_bounded_error(result.stderr)}"
            )
        return next(
            (line.strip() for line in result.stdout.splitlines() if line.strip()),
            "rz-bin unknown",
        )

    def query(self, path: Path, query: str) -> dict[str, Any]:
        try:
            flag = RZBIN_QUERIES[query]
        except KeyError as exc:
            raise ToolError(f"unknown rz-bin query: {query}") from exc
        try:
            result = subprocess.run(
                [self.executable, flag, str(path)],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=RZBIN_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired as exc:
            raise ToolError(
                f"rz-bin {query} timed out for {path.name}"
            ) from exc
        except OSError as exc:
            raise ToolError(
                f"cannot execute rz-bin {query} for {path.name}: {exc}"
            ) from exc
        stderr = result.stderr.decode("utf-8", errors="replace")
        if result.returncode != 0:
            raise ToolError(
                f"rz-bin {query} failed for {path.name} "
                f"({result.returncode}): {_bounded_error(stderr)}"
            )
        try:
            value = json.loads(result.stdout.decode("utf-8", errors="strict"))
        except (UnicodeError, json.JSONDecodeError) as exc:
            raise ToolError(
                f"rz-bin {query} emitted malformed JSON for {path.name}: {exc}"
            ) from exc
        if not isinstance(value, dict):
            raise ToolError(
                f"rz-bin {query} JSON root is not an object for {path.name}"
            )
        return value


def _payload_list(
    payload: Mapping[str, Any], key: str, binary_id: str
) -> list[Any]:
    value = payload.get(key)
    if not isinstance(value, list):
        raise InventoryError(
            f"rz-bin payload for {binary_id} is missing {key!r} array"
        )
    return value


def _payload_dict(
    payload: Mapping[str, Any], key: str, binary_id: str
) -> dict[str, Any]:
    value = payload.get(key)
    if not isinstance(value, dict):
        raise InventoryError(
            f"rz-bin payload for {binary_id} is missing {key!r} object"
        )
    return value


def _integer_or_none(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return value


def _normalize_name(item: Mapping[str, Any]) -> str:
    for key in ("realname", "name"):
        value = item.get(key)
        if isinstance(value, str) and value:
            return value
    return ""


def _hash_function_bytes(
    payload: bytes, paddr: int | None, size: int | None
) -> tuple[str | None, str]:
    if size is None or size <= 0:
        return None, "size-unavailable"
    if paddr is None or paddr < 0:
        return None, "offset-unavailable"
    end = paddr + size
    if end < paddr or end > len(payload):
        return None, "invalid-range"
    return hashlib.sha256(payload[paddr:end]).hexdigest(), "hashed"


def _normalize_export(
    item: Mapping[str, Any], payload: bytes
) -> dict[str, Any]:
    name = _normalize_name(item)
    size = _integer_or_none(item.get("size"))
    paddr = _integer_or_none(item.get("paddr"))
    vaddr = _integer_or_none(item.get("vaddr"))
    function_sha256, hash_status = _hash_function_bytes(payload, paddr, size)
    return {
        "name": name,
        "bind": item.get("bind") if isinstance(item.get("bind"), str) else None,
        "type": item.get("type") if isinstance(item.get("type"), str) else None,
        "vaddr": vaddr,
        "paddr": paddr,
        "size": size,
        "function_sha256": function_sha256,
        "byte_hash_status": hash_status,
        "probable_thunk": bool(
            size is not None and 0 < size <= THUNK_MAX_BYTES
        ),
    }


def _normalize_import(item: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "name": _normalize_name(item),
        "bind": item.get("bind") if isinstance(item.get("bind"), str) else None,
        "type": item.get("type") if isinstance(item.get("type"), str) else None,
        "plt": _integer_or_none(item.get("plt")),
    }


def _deduplicate_dicts(
    values: Sequence[dict[str, Any]], keys: tuple[str, ...]
) -> list[dict[str, Any]]:
    seen: set[tuple[Any, ...]] = set()
    result: list[dict[str, Any]] = []
    for value in sorted(
        values,
        key=lambda item: tuple(
            (item.get(key) is None, str(item.get(key))) for key in keys
        ),
    ):
        identity = tuple(value.get(key) for key in keys)
        if identity in seen:
            continue
        seen.add(identity)
        result.append(value)
    return result


def _normalize_diagnostic(value: str) -> str:
    cleaned = "".join(
        character
        if character in "\t" or ord(character) >= 32
        else f"\\x{ord(character):02x}"
        for character in value
    )
    encoded = cleaned.encode("utf-8", errors="replace")
    if len(encoded) <= MAX_DIAGNOSTIC_BYTES:
        return cleaned
    clipped = encoded[:MAX_DIAGNOSTIC_BYTES].decode(
        "utf-8", errors="replace"
    )
    return clipped + "…"


def _analyze_binary(
    *,
    path: Path,
    relative_path: Path,
    binary_id: str,
    spec: BinarySpec,
    corpus: Corpus,
    tool: Any,
) -> dict[str, Any]:
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise InventoryError(f"cannot read {binary_id}: {exc}") from exc
    if not payload.startswith(b"\x7fELF"):
        raise InventoryError(f"configured binary is not ELF: {binary_id}")

    try:
        info_payload = tool.query(path, "info")
        libs_payload = tool.query(path, "libs")
        imports_payload = tool.query(path, "imports")
        symbols_payload = tool.query(path, "symbols")
        relocs_payload = tool.query(path, "relocs")
        strings_payload = tool.query(path, "strings")
    except ToolError as exc:
        raise InventoryError(f"{binary_id}: {exc}") from exc

    info = _payload_dict(info_payload, "info", binary_id)
    libraries = _payload_list(libs_payload, "libs", binary_id)
    import_values = _payload_list(imports_payload, "imports", binary_id)
    symbol_values = _payload_list(symbols_payload, "symbols", binary_id)
    relocation_values = _payload_list(relocs_payload, "relocs", binary_id)
    string_values = _payload_list(strings_payload, "strings", binary_id)

    normalized_imports = [
        _normalize_import(item)
        for item in import_values
        if isinstance(item, dict)
        and corpus.symbol_filters.matches(_normalize_name(item))
    ]
    imports = _deduplicate_dicts(
        normalized_imports, ("name", "plt", "type", "bind")
    )

    normalized_exports = [
        _normalize_export(item, payload)
        for item in symbol_values
        if isinstance(item, dict)
        and not bool(item.get("is_imported", False))
        and item.get("type") in {"FUNC", "NOTYPE"}
        and corpus.symbol_filters.matches(_normalize_name(item))
    ]
    exports = _deduplicate_dicts(
        normalized_exports, ("name", "vaddr", "paddr", "size")
    )

    relocations: list[dict[str, Any]] = []
    for item in relocation_values:
        if not isinstance(item, dict):
            continue
        name_value = item.get("name")
        if not isinstance(name_value, str) or not corpus.symbol_filters.matches(
            name_value
        ):
            continue
        relocations.append(
            {
                "name": name_value,
                "type": (
                    item.get("type")
                    if isinstance(item.get("type"), str)
                    else None
                ),
                "vaddr": _integer_or_none(item.get("vaddr")),
                "paddr": _integer_or_none(item.get("paddr")),
            }
        )
    relocations = _deduplicate_dicts(
        relocations, ("name", "type", "vaddr", "paddr")
    )

    diagnostics: list[dict[str, Any]] = []
    for item in string_values:
        if not isinstance(item, dict):
            continue
        string_value = item.get("string")
        if not isinstance(string_value, str):
            continue
        folded = string_value.casefold()
        if not any(fragment in folded for fragment in corpus.string_filters):
            continue
        diagnostics.append(
            {
                "string": _normalize_diagnostic(string_value),
                "vaddr": _integer_or_none(item.get("vaddr")),
                "paddr": _integer_or_none(item.get("paddr")),
                "length": _integer_or_none(item.get("length")),
                "section": (
                    item.get("section")
                    if isinstance(item.get("section"), str)
                    else None
                ),
            }
        )
    diagnostics = _deduplicate_dicts(
        diagnostics, ("string", "vaddr", "paddr")
    )

    needed = sorted(
        {
            value
            for value in libraries
            if isinstance(value, str) and value
        }
    )
    selected_info_keys = (
        "arch",
        "bits",
        "endian",
        "bintype",
        "class",
        "machine",
        "PIE",
        "stripped",
        "baddr",
    )
    elf_info = {key: info.get(key) for key in selected_info_keys}
    return {
        "present": True,
        "status": "analyzed",
        "path": relative_path.as_posix(),
        "roles": list(spec.roles),
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
        "elf": elf_info,
        "needed": needed,
        "imports": imports,
        "exports": exports,
        "relocations": relocations,
        "strings": diagnostics,
    }


def _missing_binary(spec: BinarySpec) -> dict[str, Any]:
    return {
        "present": False,
        "status": "optional-absent",
        "path": spec.path.as_posix(),
        "roles": list(spec.roles),
        "bytes": None,
        "sha256": None,
        "elf": None,
        "needed": [],
        "imports": [],
        "exports": [],
        "relocations": [],
        "strings": [],
    }


def _candidate_contains_needles(
    path: Path, scan: CallerScan
) -> tuple[bytes, bool]:
    try:
        size = path.stat().st_size
    except OSError:
        return b"", False
    if size < 4 or size > scan.maximum_file_bytes:
        return b"", False
    try:
        payload = path.read_bytes()
    except OSError:
        return b"", False
    if not payload.startswith(b"\x7fELF"):
        return payload, False
    return payload, any(needle in payload for needle in scan.needles)


def _discover_callers(
    corpus: Corpus,
    firmware_root: Path,
    configured_paths: set[str],
    tool: Any,
) -> list[dict[str, Any]]:
    candidates: dict[str, Path] = {}
    root_resolved = firmware_root.resolve()
    for relative_directory in corpus.caller_scan.directories:
        directory = firmware_root / relative_directory
        try:
            resolved_directory = directory.resolve()
            resolved_directory.relative_to(root_resolved)
        except (OSError, ValueError):
            continue
        if not resolved_directory.is_dir():
            continue
        for candidate in resolved_directory.rglob("*"):
            try:
                if not candidate.is_file():
                    continue
                resolved_candidate = candidate.resolve()
                relative = resolved_candidate.relative_to(root_resolved)
            except (OSError, ValueError):
                continue
            relative_text = relative.as_posix()
            if relative_text in configured_paths:
                continue
            candidates[relative_text] = resolved_candidate

    callers: list[dict[str, Any]] = []
    for relative_text, candidate in sorted(candidates.items()):
        payload, relevant_bytes = _candidate_contains_needles(
            candidate, corpus.caller_scan
        )
        if not relevant_bytes:
            continue
        try:
            imports_payload = tool.query(candidate, "imports")
        except ToolError as exc:
            raise InventoryError(
                f"caller scan failed for {relative_text}: {exc}"
            ) from exc
        import_values = _payload_list(
            imports_payload, "imports", relative_text
        )
        imports = _deduplicate_dicts(
            [
                _normalize_import(item)
                for item in import_values
                if isinstance(item, dict)
                and corpus.symbol_filters.matches(_normalize_name(item))
            ],
            ("name", "plt", "type", "bind"),
        )
        if not imports:
            continue
        callers.append(
            {
                "path": relative_text,
                "bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
                "imports": imports,
            }
        )
    return callers


def _build_symbol_index(
    firmwares: Mapping[str, Any], project_ffi: Sequence[str]
) -> dict[str, Any]:
    index: dict[str, dict[str, list[dict[str, Any]]]] = {
        symbol: {"providers": [], "callers": []} for symbol in project_ffi
    }
    for firmware_id, firmware in sorted(firmwares.items()):
        for binary_id, binary in sorted(firmware["binaries"].items()):
            if not binary.get("present"):
                continue
            for export in binary.get("exports", []):
                symbol = export["name"]
                entry = index.setdefault(
                    symbol, {"providers": [], "callers": []}
                )
                entry["providers"].append(
                    {
                        "firmware": firmware_id,
                        "binary": binary_id,
                        "path": binary["path"],
                        "binary_sha256": binary["sha256"],
                        "vaddr": export.get("vaddr"),
                        "size": export.get("size"),
                        "function_sha256": export.get("function_sha256"),
                        "byte_hash_status": export.get("byte_hash_status"),
                        "probable_thunk": export.get("probable_thunk"),
                    }
                )
            for imported in binary.get("imports", []):
                symbol = imported["name"]
                entry = index.setdefault(
                    symbol, {"providers": [], "callers": []}
                )
                entry["callers"].append(
                    {
                        "firmware": firmware_id,
                        "binary": binary_id,
                        "path": binary["path"],
                        "binary_sha256": binary["sha256"],
                        "kind": "configured-import",
                        "plt": imported.get("plt"),
                    }
                )
        for caller in firmware.get("discovered_callers", []):
            for imported in caller["imports"]:
                symbol = imported["name"]
                entry = index.setdefault(
                    symbol, {"providers": [], "callers": []}
                )
                entry["callers"].append(
                    {
                        "firmware": firmware_id,
                        "binary": None,
                        "path": caller["path"],
                        "binary_sha256": caller["sha256"],
                        "kind": "discovered-import",
                        "plt": imported.get("plt"),
                    }
                )

    for entry in index.values():
        entry["providers"] = sorted(
            entry["providers"],
            key=lambda item: (
                item["firmware"],
                item["path"],
                item.get("vaddr") or -1,
            ),
        )
        entry["callers"] = sorted(
            entry["callers"],
            key=lambda item: (
                item["firmware"],
                item["path"],
                item.get("plt") or -1,
            ),
        )
    return dict(sorted(index.items()))


def build_inventory(corpus: Corpus, tool: Any | None = None) -> dict[str, Any]:
    """Analyze the complete configured corpus and discover relevant callers."""

    analyzer = tool or RzBin()
    try:
        tool_version = analyzer.version()
    except ToolError as exc:
        raise InventoryError(str(exc)) from exc

    firmware_results: dict[str, Any] = {}
    configured_paths = {
        spec.path.as_posix() for spec in corpus.binaries.values()
    }
    for firmware_id, firmware in corpus.firmwares.items():
        firmware_root = _validate_resolved_below(
            corpus.repository_root,
            corpus.repository_root / firmware.root,
            f"firmware root {firmware_id}",
        )
        binary_results: dict[str, Any] = {}
        for binary_id, spec in corpus.binaries.items():
            candidate = firmware_root / spec.path
            resolved_candidate = _validate_resolved_below(
                corpus.repository_root,
                candidate,
                f"{firmware_id}/{binary_id}",
            )
            if not resolved_candidate.is_file():
                if spec.required:
                    raise InventoryError(
                        f"required binary is missing: "
                        f"{firmware_id}/{spec.path.as_posix()}"
                    )
                binary_results[binary_id] = _missing_binary(spec)
                continue
            binary_results[binary_id] = _analyze_binary(
                path=resolved_candidate,
                relative_path=spec.path,
                binary_id=binary_id,
                spec=spec,
                corpus=corpus,
                tool=analyzer,
            )

        discovered_callers = _discover_callers(
            corpus,
            firmware_root,
            configured_paths,
            analyzer,
        )
        firmware_results[firmware_id] = {
            "root": firmware.root.as_posix(),
            "live_match": firmware.live_match,
            "binaries": dict(sorted(binary_results.items())),
            "discovered_callers": discovered_callers,
        }

    inventory: dict[str, Any] = {
        "schema_version": INVENTORY_SCHEMA_VERSION,
        "reference_firmware": corpus.reference_firmware,
        "corpus_sha256": corpus.source_sha256,
        "tool": {
            "name": "rz-bin",
            "version": tool_version,
        },
        "project_ffi": list(corpus.project_ffi),
        "firmwares": dict(sorted(firmware_results.items())),
    }
    inventory["symbol_index"] = _build_symbol_index(
        inventory["firmwares"], corpus.project_ffi
    )
    return inventory


def canonical_json(value: Any) -> bytes:
    return (
        json.dumps(
            value,
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def _export_map(binary: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    exports = binary.get("exports", [])
    result: dict[str, Mapping[str, Any]] = {}
    if not isinstance(exports, list):
        return result
    for item in sorted(
        (value for value in exports if isinstance(value, dict)),
        key=lambda value: (
            str(value.get("name")),
            value.get("vaddr") if isinstance(value.get("vaddr"), int) else -1,
        ),
    ):
        name = item.get("name")
        if isinstance(name, str) and name not in result:
            result[name] = item
    return result


def _matrix_state(
    *,
    reference_binary: Mapping[str, Any] | None,
    current_binary: Mapping[str, Any] | None,
    symbol: str,
    current_is_reference: bool,
) -> dict[str, Any]:
    if not current_binary or not current_binary.get("present"):
        return {
            "state": "absent",
            "size": None,
            "function_sha256": None,
            "byte_hash_status": None,
        }
    current = _export_map(current_binary).get(symbol)
    reference = (
        _export_map(reference_binary).get(symbol)
        if reference_binary and reference_binary.get("present")
        else None
    )
    if current is None:
        state = "removed" if reference is not None else "absent"
        return {
            "state": state,
            "size": None,
            "function_sha256": None,
            "byte_hash_status": None,
        }
    if reference is None:
        state = "added" if not current_is_reference else "reference-only"
    else:
        current_hash = current.get("function_sha256")
        reference_hash = reference.get("function_sha256")
        if isinstance(current_hash, str) and isinstance(reference_hash, str):
            state = "identical" if current_hash == reference_hash else "changed"
        else:
            state = "size-only"
    return {
        "state": state,
        "size": current.get("size"),
        "function_sha256": current.get("function_sha256"),
        "byte_hash_status": current.get("byte_hash_status"),
    }


def build_matrix(inventory: Mapping[str, Any]) -> dict[str, Any]:
    reference = inventory.get("reference_firmware")
    firmwares = inventory.get("firmwares")
    if not isinstance(reference, str) or not isinstance(firmwares, dict):
        raise InventoryError("inventory is missing reference_firmware/firmwares")
    if reference not in firmwares:
        raise InventoryError(f"reference firmware is absent: {reference}")

    firmware_ids = sorted(firmwares)
    binary_ids = sorted(
        {
            binary_id
            for firmware in firmwares.values()
            if isinstance(firmware, dict)
            for binary_id in firmware.get("binaries", {})
        }
    )
    matrix_binaries: dict[str, list[dict[str, Any]]] = {}
    reference_binaries = firmwares[reference].get("binaries", {})
    for binary_id in binary_ids:
        symbols: set[str] = set()
        for firmware in firmwares.values():
            binary = firmware.get("binaries", {}).get(binary_id)
            if isinstance(binary, dict):
                symbols.update(_export_map(binary))
        rows: list[dict[str, Any]] = []
        reference_binary = reference_binaries.get(binary_id)
        for symbol in sorted(symbols):
            states: dict[str, Any] = {}
            for firmware_id in firmware_ids:
                current_binary = (
                    firmwares[firmware_id]
                    .get("binaries", {})
                    .get(binary_id)
                )
                states[firmware_id] = _matrix_state(
                    reference_binary=reference_binary,
                    current_binary=current_binary,
                    symbol=symbol,
                    current_is_reference=firmware_id == reference,
                )
            rows.append({"symbol": symbol, "firmwares": states})
        matrix_binaries[binary_id] = rows
    return {
        "schema_version": MATRIX_SCHEMA_VERSION,
        "reference_firmware": reference,
        "firmwares": firmware_ids,
        "binaries": matrix_binaries,
    }


def _matrix_cell(value: Mapping[str, Any]) -> str:
    state = str(value.get("state", "unknown"))
    digest = value.get("function_sha256")
    if isinstance(digest, str):
        return f"{state} `{digest[:10]}`"
    return state


def render_matrix_markdown(matrix: Mapping[str, Any]) -> str:
    reference = matrix["reference_firmware"]
    firmware_ids = list(matrix["firmwares"])
    lines = [
        "# Wave NPU Runtime Firmware Matrix",
        "",
        "Generated deterministically by `waved/scripts/npu/abi_inventory.py`.",
        "Function identity uses exact bytes at the ELF physical file offset;",
        "virtual address changes alone do not count as implementation changes.",
        "",
        f"Reference firmware: `{reference}`",
        "",
        "States: `identical`, `changed`, `added`, `removed`, `absent`,",
        "`size-only`, or `reference-only`. A short function SHA-256 follows",
        "when exact bytes were available.",
        "",
    ]
    for binary_id, rows in sorted(matrix["binaries"].items()):
        lines.extend(
            [
                f"## `{binary_id}`",
                "",
                "| Symbol | "
                + " | ".join(f"`{firmware}`" for firmware in firmware_ids)
                + " |",
                "|---|" + "|".join("---" for _ in firmware_ids) + "|",
            ]
        )
        if not rows:
            lines.append(
                "| _No relevant defined symbols_ | "
                + " | ".join("—" for _ in firmware_ids)
                + " |"
            )
        else:
            for row in rows:
                cells = [
                    _matrix_cell(row["firmwares"][firmware])
                    for firmware in firmware_ids
                ]
                lines.append(
                    f"| `{row['symbol']}` | " + " | ".join(cells) + " |"
                )
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def _write_atomic(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _read_json_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise InventoryError(f"cannot read JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise InventoryError(f"JSON root is not an object: {path}")
    return value


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Build deterministic host-only Wave NPU ABI evidence."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    inventory = subparsers.add_parser(
        "inventory", help="analyze the source-controlled firmware corpus"
    )
    inventory.add_argument(
        "--output", type=Path, default=DEFAULT_INVENTORY_PATH
    )

    matrix = subparsers.add_parser(
        "matrix", help="render a firmware matrix from an inventory"
    )
    matrix.add_argument(
        "--inventory", type=Path, default=DEFAULT_INVENTORY_PATH
    )
    matrix.add_argument("--output", type=Path, default=DEFAULT_MATRIX_PATH)

    subparsers.add_parser(
        "verify-generated",
        help="rebuild inventory and matrix and compare tracked bytes",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "inventory":
            corpus = load_corpus()
            inventory = build_inventory(corpus)
            _write_atomic(args.output, canonical_json(inventory))
            print(args.output)
            return 0

        if args.command == "matrix":
            inventory = _read_json_object(args.inventory)
            matrix = build_matrix(inventory)
            _write_atomic(
                args.output, render_matrix_markdown(matrix).encode("utf-8")
            )
            print(args.output)
            return 0

        if args.command == "verify-generated":
            corpus = load_corpus()
            inventory = build_inventory(corpus)
            inventory_bytes = canonical_json(inventory)
            matrix_bytes = render_matrix_markdown(
                build_matrix(inventory)
            ).encode("utf-8")
            expected = (
                (DEFAULT_INVENTORY_PATH, inventory_bytes),
                (DEFAULT_MATRIX_PATH, matrix_bytes),
            )
            mismatches: list[str] = []
            for path, generated in expected:
                try:
                    tracked = path.read_bytes()
                except OSError as exc:
                    mismatches.append(f"{path}: {exc}")
                    continue
                if tracked != generated:
                    mismatches.append(f"{path}: generated bytes differ")
            if mismatches:
                for mismatch in mismatches:
                    print(mismatch, file=sys.stderr)
                return 1
            print("generated ABI inventory and matrix are current")
            return 0

        parser.error(f"unsupported command: {args.command}")
        return 2
    except AbiInventoryError as exc:
        print(f"npu-abi: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
