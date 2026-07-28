#!/usr/bin/env python3
"""Strict, non-executing VeriSilicon NBG parser and corpus inventory."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import struct
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, Mapping, Sequence


SCHEMA_VERSION = 1
INVENTORY_SCHEMA_VERSION = 1
SUMMARY_SCHEMA_VERSION = 1
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CORPUS_PATH = Path(__file__).with_name("nbg_corpus.json")
DEFAULT_INVENTORY_PATH = (
    REPOSITORY_ROOT / "docs/npu/format/stock-nbg-inventory.json"
)
DEFAULT_SUMMARY_PATH = REPOSITORY_ROOT / "docs/npu/format/corpus-summary.md"

ROOT_KEYS = frozenset({"schema_version", "collections", "files"})
COLLECTION_KEYS = frozenset(
    {"id", "root", "glob", "role", "required", "expected_count"}
)
FILE_KEYS = frozenset(
    {"id", "path", "role", "required", "expected_sha256"}
)
ROLES = frozenset({"stock", "reference", "negative_control"})
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")

MAGIC = b"VPMN"
COMMON_HEADER_BYTES = 12
MINIMUM_RUNTIME_NBG_BYTES = 513
V1_MIN_VERSION = 0x00010000
V2_VERSION = 0x00020000
V2_HEADER_BYTES = 48
V2_DIRECTORY_RECORD_BYTES = 44
V2_IO_RECORD_BYTES = 112
CONNECTION_TAIL_BYTES = 512
MAX_DIRECTORY_RECORDS = 256
MAX_IO_RECORDS = 64
MAX_DIMENSIONS = 6
MAX_LAYER_OR_OPERATION_COUNT = 1_000_000
MAX_TENSOR_BYTES = 1 << 40
MAX_U64 = (1 << 64) - 1
FINGERPRINT_CHUNK_BYTES = 64 * 1024
EDGE_FINGERPRINT_BYTES = 4096

DATA_FORMATS: Mapping[int, tuple[str, int]] = {
    0: ("fp32", 4),
    1: ("fp16", 2),
    2: ("uint8", 1),
    3: ("int8", 1),
    4: ("uint16", 2),
    5: ("int16", 2),
}

V1_SECTION_NAMES = (
    "input_table",
    "output_table",
    "layer_table",
    "operation_table",
    "lcd_table",
    "lcd",
    "nn_operation_data",
    "tp_operation_data",
    "shader_operation_data",
    "patch_data",
    "layer_parameter_table",
    "software_operation_data",
)
V1_EXTENDED_0C_SECTIONS = (
    "hardware_init_operation_table",
    "icd_table",
    "icd",
)
V1_EXTENDED_0E_SECTIONS = ("ppu_parameter_table",)


class NbgError(Exception):
    """Base class for expected NBG research-tool errors."""


class NbgCorpusError(NbgError):
    """The source-controlled corpus definition or discovery is invalid."""


class NbgInventoryError(NbgError):
    """A corpus could not be inventoried reproducibly."""


class NbgParseError(NbgError):
    """A model failed a strict, bounded parse."""

    def __init__(
        self,
        code: str,
        stage: str,
        message: str,
        *,
        offset: int | None = None,
        context: str | None = None,
    ) -> None:
        self.code = code
        self.stage = stage
        self.message = message
        self.offset = offset
        self.context = context
        pieces = [f"{code} at {stage}"]
        if offset is not None:
            pieces.append(f"offset 0x{offset:x}")
        if context:
            pieces.append(context)
        super().__init__(": ".join(pieces) + f": {message}")

    def as_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "code": self.code,
            "message": self.message,
            "stage": self.stage,
        }
        if self.context is not None:
            result["context"] = self.context
        if self.offset is not None:
            result["offset"] = self.offset
            result["offset_hex"] = _hex32(self.offset)
        return result


@dataclass(frozen=True)
class CollectionSpec:
    collection_id: str
    root: Path
    pattern: str
    role: str
    required: bool
    expected_count: int


@dataclass(frozen=True)
class FileSpec:
    file_id: str
    path: Path
    role: str
    required: bool
    expected_sha256: str


@dataclass(frozen=True)
class Corpus:
    repository_root: Path
    source_path: Path
    source_sha256: str
    collections: tuple[CollectionSpec, ...]
    files: tuple[FileSpec, ...]


@dataclass(frozen=True)
class CorpusFile:
    corpus_id: str
    path: Path
    display_path: str
    role: str
    collection_id: str | None
    model_family: str
    expected_sha256: str | None


@dataclass(frozen=True)
class Discovery:
    files: tuple[CorpusFile, ...]
    collections: tuple[dict[str, Any], ...]


class MemorySource:
    """Random-access immutable source used by parser tests."""

    def __init__(self, data: bytes) -> None:
        self._data = data
        self.size = len(data)

    def read_at(self, offset: int, size: int) -> bytes:
        return self._data[offset : offset + size]

    def chunks(
        self, offset: int, size: int, chunk_size: int
    ) -> Iterator[bytes]:
        end = offset + size
        cursor = offset
        while cursor < end:
            chunk = self._data[cursor : min(end, cursor + chunk_size)]
            if not chunk:
                raise OSError("memory source returned an empty in-range chunk")
            yield chunk
            cursor += len(chunk)


class FileSource:
    """Bounded random-access source that never loads a complete NBG."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._handle = path.open("rb")
        self.size = path.stat().st_size

    def close(self) -> None:
        self._handle.close()

    def __enter__(self) -> FileSource:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()

    def read_at(self, offset: int, size: int) -> bytes:
        self._handle.seek(offset)
        return self._handle.read(size)

    def chunks(
        self, offset: int, size: int, chunk_size: int
    ) -> Iterator[bytes]:
        self._handle.seek(offset)
        remaining = size
        while remaining:
            chunk = self._handle.read(min(chunk_size, remaining))
            if not chunk:
                raise OSError(
                    f"unexpected EOF while streaming {self.path} "
                    f"with {remaining} bytes remaining"
                )
            yield chunk
            remaining -= len(chunk)


class CheckedReader:
    """All file-controlled offsets pass through this checked reader."""

    def __init__(self, source: MemorySource | FileSource) -> None:
        self.source = source
        self.size = source.size

    def checked_range(
        self,
        offset: int,
        size: int,
        field: str,
        *,
        code: str = "section-range-overflow",
        stage: str = "range",
    ) -> tuple[int, int]:
        if (
            isinstance(offset, bool)
            or isinstance(size, bool)
            or not isinstance(offset, int)
            or not isinstance(size, int)
            or offset < 0
            or size < 0
        ):
            raise NbgParseError(
                code,
                stage,
                "offset and size must be nonnegative integers",
                offset=offset if isinstance(offset, int) else None,
                context=field,
            )
        if offset > MAX_U64 or size > MAX_U64:
            raise NbgParseError(
                code,
                stage,
                "offset or size exceeds the 64-bit format boundary",
                offset=offset,
                context=field,
            )
        if offset > self.size or size > self.size - offset:
            raise NbgParseError(
                code,
                stage,
                f"range [{offset}, {offset + size}) exceeds file size "
                f"{self.size}",
                offset=offset,
                context=field,
            )
        return offset, offset + size

    def bytes(
        self,
        offset: int,
        size: int,
        field: str,
        *,
        code: str = "truncated-header",
        stage: str = "header",
    ) -> bytes:
        self.checked_range(
            offset, size, field, code=code, stage=stage
        )
        try:
            value = self.source.read_at(offset, size)
        except OSError as exc:
            raise NbgParseError(
                code,
                stage,
                f"read failed: {exc}",
                offset=offset,
                context=field,
            ) from exc
        if len(value) != size:
            raise NbgParseError(
                code,
                stage,
                f"short read: expected {size}, received {len(value)}",
                offset=offset,
                context=field,
            )
        return value

    def u32(
        self,
        offset: int,
        field: str,
        *,
        code: str = "truncated-header",
        stage: str = "header",
    ) -> int:
        return struct.unpack(
            "<I",
            self.bytes(
                offset, 4, field, code=code, stage=stage
            ),
        )[0]

    def cstring(
        self,
        offset: int,
        size: int,
        field: str,
        *,
        stage: str,
        allow_empty: bool,
    ) -> str:
        raw = self.bytes(
            offset,
            size,
            field,
            code="invalid-name",
            stage=stage,
        )
        terminator = raw.find(b"\0")
        if terminator < 0:
            raise NbgParseError(
                "invalid-name",
                stage,
                f"{size}-byte field is not NUL terminated",
                offset=offset,
                context=field,
            )
        value = raw[:terminator]
        if not value and not allow_empty:
            raise NbgParseError(
                "invalid-name",
                stage,
                "name is empty",
                offset=offset,
                context=field,
            )
        try:
            decoded = value.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise NbgParseError(
                "invalid-name",
                stage,
                "name is not valid UTF-8",
                offset=offset,
                context=field,
            ) from exc
        if CONTROL_RE.search(decoded):
            raise NbgParseError(
                "invalid-name",
                stage,
                "name contains a control character",
                offset=offset,
                context=field,
            )
        return decoded


def _object_without_duplicates(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise NbgCorpusError(f"duplicate JSON key: {key!r}")
        result[key] = value
    return result


def _require_dict(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise NbgCorpusError(f"{context} must be a JSON object")
    return value


def _require_list(value: Any, context: str) -> list[Any]:
    if not isinstance(value, list):
        raise NbgCorpusError(f"{context} must be a JSON array")
    return value


def _require_string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise NbgCorpusError(f"{context} must be a nonempty string")
    if CONTROL_RE.search(value):
        raise NbgCorpusError(f"{context} contains a control character")
    return value


def _require_bool(value: Any, context: str) -> bool:
    if not isinstance(value, bool):
        raise NbgCorpusError(f"{context} must be a boolean")
    return value


def _require_int(
    value: Any,
    context: str,
    *,
    minimum: int | None = None,
    maximum: int | None = None,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise NbgCorpusError(f"{context} must be an integer")
    if minimum is not None and value < minimum:
        raise NbgCorpusError(f"{context} must be at least {minimum}")
    if maximum is not None and value > maximum:
        raise NbgCorpusError(f"{context} must be at most {maximum}")
    return value


def _strict_keys(
    value: Mapping[str, Any],
    expected: frozenset[str],
    context: str,
) -> None:
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing:
        raise NbgCorpusError(
            f"{context} missing keys: {', '.join(missing)}"
        )
    if unknown:
        raise NbgCorpusError(
            f"{context} has unknown keys: {', '.join(unknown)}"
        )


def _relative_path(value: Any, context: str) -> Path:
    text = _require_string(value, context)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts:
        raise NbgCorpusError(
            f"{context} may not escape the repository: {text!r}"
        )
    if path == Path("."):
        raise NbgCorpusError(
            f"{context} must identify a path below the repository"
        )
    return path


def _resolve_below(repository: Path, relative: Path, context: str) -> Path:
    candidate = repository / relative
    resolved_repository = repository.resolve()
    resolved_candidate = candidate.resolve()
    try:
        resolved_candidate.relative_to(resolved_repository)
    except ValueError as exc:
        raise NbgCorpusError(
            f"{context} resolves outside the repository boundary"
        ) from exc
    return candidate


def _reject_symlink_components(
    repository: Path, path: Path, context: str
) -> None:
    resolved_repository = repository.resolve()
    try:
        relative = path.absolute().relative_to(repository.absolute())
    except ValueError as exc:
        raise NbgCorpusError(
            f"{context} is outside the repository boundary"
        ) from exc
    cursor = resolved_repository
    for part in relative.parts:
        cursor = cursor / part
        if cursor.is_symlink():
            raise NbgCorpusError(
                f"{context} contains a symlink component: {cursor}"
            )


def load_corpus(
    path: Path = DEFAULT_CORPUS_PATH,
    *,
    repository_root: Path | None = None,
) -> Corpus:
    """Load a strict NBG corpus definition without scanning model data."""

    repository = (
        repository_root.resolve()
        if repository_root is not None
        else REPOSITORY_ROOT
    )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise NbgCorpusError(f"cannot read corpus {path}: {exc}") from exc
    try:
        document = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except NbgCorpusError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise NbgCorpusError(
            f"invalid corpus JSON at {path}: {exc}"
        ) from exc

    root = _require_dict(document, "root")
    _strict_keys(root, ROOT_KEYS, "root")
    schema_version = _require_int(
        root["schema_version"], "schema_version"
    )
    if schema_version != SCHEMA_VERSION:
        raise NbgCorpusError(
            f"unsupported schema_version {schema_version}; "
            f"expected {SCHEMA_VERSION}"
        )

    raw_collections = _require_list(root["collections"], "collections")
    raw_files = _require_list(root["files"], "files")
    if not raw_collections and not raw_files:
        raise NbgCorpusError("corpus must contain a collection or file")

    ids: set[str] = set()
    collections: list[CollectionSpec] = []
    for index, raw in enumerate(raw_collections):
        context = f"collections[{index}]"
        item = _require_dict(raw, context)
        _strict_keys(item, COLLECTION_KEYS, context)
        collection_id = _require_string(item["id"], f"{context}.id")
        if ID_RE.fullmatch(collection_id) is None:
            raise NbgCorpusError(
                f"{context}.id is invalid: {collection_id!r}"
            )
        if collection_id in ids:
            raise NbgCorpusError(f"duplicate corpus ID: {collection_id}")
        ids.add(collection_id)
        relative_root = _relative_path(
            item["root"], f"{context}.root"
        )
        _resolve_below(repository, relative_root, f"{context}.root")
        pattern = _require_string(item["glob"], f"{context}.glob")
        if pattern != "**/*.nb":
            raise NbgCorpusError(
                f"{context}.glob must be exactly '**/*.nb'"
            )
        role = _require_string(item["role"], f"{context}.role")
        if role not in ROLES:
            raise NbgCorpusError(
                f"{context}.role must be one of {sorted(ROLES)}"
            )
        required = _require_bool(
            item["required"], f"{context}.required"
        )
        expected_count = _require_int(
            item["expected_count"],
            f"{context}.expected_count",
            minimum=0,
            maximum=100_000,
        )
        if required and expected_count == 0:
            raise NbgCorpusError(
                f"{context}.expected_count must be positive when required"
            )
        collections.append(
            CollectionSpec(
                collection_id=collection_id,
                root=relative_root,
                pattern=pattern,
                role=role,
                required=required,
                expected_count=expected_count,
            )
        )

    files: list[FileSpec] = []
    for index, raw in enumerate(raw_files):
        context = f"files[{index}]"
        item = _require_dict(raw, context)
        _strict_keys(item, FILE_KEYS, context)
        file_id = _require_string(item["id"], f"{context}.id")
        if ID_RE.fullmatch(file_id) is None:
            raise NbgCorpusError(
                f"{context}.id is invalid: {file_id!r}"
            )
        if file_id in ids:
            raise NbgCorpusError(f"duplicate corpus ID: {file_id}")
        ids.add(file_id)
        relative_path = _relative_path(
            item["path"], f"{context}.path"
        )
        if relative_path.suffix != ".nb":
            raise NbgCorpusError(
                f"{context}.path must end in .nb"
            )
        _resolve_below(repository, relative_path, f"{context}.path")
        role = _require_string(item["role"], f"{context}.role")
        if role not in ROLES:
            raise NbgCorpusError(
                f"{context}.role must be one of {sorted(ROLES)}"
            )
        required = _require_bool(
            item["required"], f"{context}.required"
        )
        digest = _require_string(
            item["expected_sha256"], f"{context}.expected_sha256"
        )
        if SHA256_RE.fullmatch(digest) is None:
            raise NbgCorpusError(
                f"{context}.expected_sha256 must be lowercase SHA-256"
            )
        files.append(
            FileSpec(
                file_id=file_id,
                path=relative_path,
                role=role,
                required=required,
                expected_sha256=digest,
            )
        )

    return Corpus(
        repository_root=repository,
        source_path=path.resolve(),
        source_sha256=hashlib.sha256(payload).hexdigest(),
        collections=tuple(
            sorted(collections, key=lambda item: item.collection_id)
        ),
        files=tuple(sorted(files, key=lambda item: item.file_id)),
    )


def discover_corpus(corpus: Corpus) -> Discovery:
    """Resolve configured paths, reject symlinks, and pin file ordering."""

    discovered: list[CorpusFile] = []
    collection_results: list[dict[str, Any]] = []
    physical_paths: dict[Path, str] = {}

    def add_file(item: CorpusFile) -> None:
        resolved = item.path.resolve()
        prior = physical_paths.get(resolved)
        if prior is not None:
            raise NbgCorpusError(
                f"duplicate physical NBG path in {prior} and "
                f"{item.corpus_id}: {item.display_path}"
            )
        physical_paths[resolved] = item.corpus_id
        discovered.append(item)

    for spec in corpus.collections:
        root = corpus.repository_root / spec.root
        if not root.exists():
            if spec.required:
                raise NbgCorpusError(
                    f"required collection {spec.collection_id} is missing: "
                    f"{spec.root.as_posix()}"
                )
            collection_results.append(
                {
                    "expected_count": spec.expected_count,
                    "id": spec.collection_id,
                    "observed_count": 0,
                    "present": False,
                    "role": spec.role,
                    "root": spec.root.as_posix(),
                }
            )
            continue
        _reject_symlink_components(
            corpus.repository_root,
            root,
            f"collection {spec.collection_id}",
        )
        if not root.is_dir():
            raise NbgCorpusError(
                f"collection {spec.collection_id} is not a directory"
            )
        candidates = sorted(
            root.glob(spec.pattern),
            key=lambda value: value.relative_to(root).as_posix().encode(),
        )
        if len(candidates) != spec.expected_count:
            raise NbgCorpusError(
                f"collection {spec.collection_id} count mismatch: "
                f"expected {spec.expected_count}, observed "
                f"{len(candidates)}"
            )
        for path in candidates:
            relative_to_collection = path.relative_to(root)
            _reject_symlink_components(
                corpus.repository_root,
                path,
                f"collection {spec.collection_id} file",
            )
            if not path.is_file() or path.suffix != ".nb":
                raise NbgCorpusError(
                    f"collection {spec.collection_id} contains a "
                    f"non-regular .nb candidate: {path}"
                )
            display = path.relative_to(
                corpus.repository_root
            ).as_posix()
            family = relative_to_collection.parent.as_posix()
            if family == ".":
                family = "(root)"
            add_file(
                CorpusFile(
                    corpus_id=(
                        f"{spec.collection_id}:"
                        f"{relative_to_collection.as_posix()}"
                    ),
                    path=path,
                    display_path=display,
                    role=spec.role,
                    collection_id=spec.collection_id,
                    model_family=family,
                    expected_sha256=None,
                )
            )
        collection_results.append(
            {
                "expected_count": spec.expected_count,
                "id": spec.collection_id,
                "observed_count": len(candidates),
                "present": True,
                "role": spec.role,
                "root": spec.root.as_posix(),
            }
        )

    for spec in corpus.files:
        path = corpus.repository_root / spec.path
        if not path.exists():
            if spec.required:
                raise NbgCorpusError(
                    f"required file {spec.file_id} is missing: "
                    f"{spec.path.as_posix()}"
                )
            continue
        _reject_symlink_components(
            corpus.repository_root, path, f"file {spec.file_id}"
        )
        if not path.is_file():
            raise NbgCorpusError(
                f"file {spec.file_id} is not a regular file"
            )
        add_file(
            CorpusFile(
                corpus_id=spec.file_id,
                path=path,
                display_path=spec.path.as_posix(),
                role=spec.role,
                collection_id=None,
                model_family=f"_{spec.role}",
                expected_sha256=spec.expected_sha256,
            )
        )

    return Discovery(
        files=tuple(
            sorted(
                discovered,
                key=lambda item: (
                    item.collection_id or "~",
                    item.display_path.encode(),
                ),
            )
        ),
        collections=tuple(
            sorted(collection_results, key=lambda item: item["id"])
        ),
    )


def _hex32(value: int) -> str:
    width = max(8, (value.bit_length() + 3) // 4)
    return f"0x{value:0{width}x}"


def _signed_u32(value: int) -> int:
    return value - (1 << 32) if value & (1 << 31) else value


def _float_u32(value: int) -> float:
    return struct.unpack("<f", struct.pack("<I", value))[0]


def _checked_u64(low: int, high: int, field: str) -> int:
    value = (high << 32) | low
    if value > MAX_U64:
        raise NbgParseError(
            "section-range-overflow",
            "directory",
            "combined field exceeds u64",
            context=field,
        )
    return value


def _sha256_range(
    source: MemorySource | FileSource, offset: int, size: int
) -> str:
    digest = hashlib.sha256()
    try:
        for chunk in source.chunks(
            offset, size, FINGERPRINT_CHUNK_BYTES
        ):
            digest.update(chunk)
    except OSError as exc:
        raise NbgParseError(
            "section-range-overflow",
            "fingerprint",
            f"streaming read failed: {exc}",
            offset=offset,
        ) from exc
    return digest.hexdigest()


def _fingerprint_range(
    reader: CheckedReader,
    offset: int,
    size: int,
    field: str,
) -> dict[str, Any]:
    reader.checked_range(
        offset,
        size,
        field,
        code="section-range-overflow",
        stage="fingerprint",
    )
    digest = hashlib.sha256()
    chunk_digest = hashlib.sha256()
    counts = [0] * 256
    chunk_count = 0
    try:
        for chunk in reader.source.chunks(
            offset, size, FINGERPRINT_CHUNK_BYTES
        ):
            digest.update(chunk)
            chunk_digest.update(hashlib.sha256(chunk).digest())
            for value, count in Counter(chunk).items():
                counts[value] += count
            chunk_count += 1
    except OSError as exc:
        raise NbgParseError(
            "section-range-overflow",
            "fingerprint",
            f"streaming read failed: {exc}",
            offset=offset,
            context=field,
        ) from exc

    edge_size = min(size, EDGE_FINGERPRINT_BYTES)
    first = reader.bytes(
        offset,
        edge_size,
        f"{field}.first",
        code="section-range-overflow",
        stage="fingerprint",
    )
    last = reader.bytes(
        offset + size - edge_size,
        edge_size,
        f"{field}.last",
        code="section-range-overflow",
        stage="fingerprint",
    )
    if size:
        entropy = -sum(
            (count / size) * math.log2(count / size)
            for count in counts
            if count
        )
        zero_ratio = counts[0] / size
    else:
        entropy = 0.0
        zero_ratio = 0.0
    return {
        "chunk_count": chunk_count,
        "chunk_hashes_sha256": chunk_digest.hexdigest(),
        "chunk_size": FINGERPRINT_CHUNK_BYTES,
        "entropy_bits_per_byte": round(entropy, 6),
        "first_sha256": hashlib.sha256(first).hexdigest(),
        "last_sha256": hashlib.sha256(last).hexdigest(),
        "sample_edge_bytes": edge_size,
        "sha256": digest.hexdigest(),
        "zero_ratio": round(zero_ratio, 6),
    }


def _tensor_size(
    dimensions: Sequence[int], data_format: int, context: str
) -> tuple[str | None, int | None, int | None]:
    format_spec = DATA_FORMATS.get(data_format)
    if format_spec is None:
        return None, None, None
    format_name, element_bytes = format_spec
    elements = 1
    for dimension in dimensions:
        if dimension <= 0:
            raise NbgParseError(
                "invalid-dimension",
                "tensor",
                f"dimension must be positive, observed {dimension}",
                context=context,
            )
        elements *= dimension
        if elements > MAX_TENSOR_BYTES:
            raise NbgParseError(
                "tensor-size-overflow",
                "tensor",
                "tensor element product exceeds parser limit",
                context=context,
            )
    byte_size = elements * element_bytes
    if byte_size > MAX_TENSOR_BYTES:
        raise NbgParseError(
            "tensor-size-overflow",
            "tensor",
            "derived tensor storage exceeds parser limit",
            context=context,
        )
    return format_name, element_bytes, byte_size


def _quantization(
    quantization_format: int,
    word_0: int,
    word_1: int,
    *,
    has_explicit_fixed_word: bool,
    fixed_word: int | None = None,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "format": quantization_format,
        "word_0": word_0,
        "word_0_hex": _hex32(word_0),
        "word_1": word_1,
        "word_1_hex": _hex32(word_1),
    }
    if has_explicit_fixed_word:
        assert fixed_word is not None
        result["fixed_position"] = _signed_u32(fixed_word)
        result["interpretation"] = (
            "fixed-point" if quantization_format == 1 else "affine-or-none"
        )
        result["scale"] = _float_u32(word_0)
        result["zero_point"] = _signed_u32(word_1)
    elif quantization_format == 1:
        result["fixed_position"] = _signed_u32(word_0)
        result["interpretation"] = "fixed-point"
    else:
        result["interpretation"] = "affine-or-none"
        result["scale"] = _float_u32(word_0)
        result["zero_point"] = _signed_u32(word_1)
    return result


def _parse_io_record_v1(
    reader: CheckedReader, offset: int, index: int, direction: str
) -> dict[str, Any]:
    context = f"{direction}[{index}]"
    dimension_count = reader.u32(
        offset,
        f"{context}.dimension_count",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    if not 1 <= dimension_count <= MAX_DIMENSIONS:
        raise NbgParseError(
            "invalid-dimension-count",
            "v1-io",
            f"expected 1..{MAX_DIMENSIONS}, observed {dimension_count}",
            offset=offset,
            context=context,
        )
    all_dimensions = [
        reader.u32(
            offset + 4 + dimension_index * 4,
            f"{context}.dimensions[{dimension_index}]",
            code="invalid-io-record-size",
            stage="v1-io",
        )
        for dimension_index in range(MAX_DIMENSIONS)
    ]
    dimensions = all_dimensions[:dimension_count]
    data_format = reader.u32(
        offset + 28,
        f"{context}.data_format",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    data_type = reader.u32(
        offset + 32,
        f"{context}.data_type",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    quantization_format = reader.u32(
        offset + 36,
        f"{context}.quantization_format",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    fixed_word = reader.u32(
        offset + 40,
        f"{context}.fixed_position",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    scale_word = reader.u32(
        offset + 44,
        f"{context}.scale",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    zero_word = reader.u32(
        offset + 48,
        f"{context}.zero_point",
        code="invalid-io-record-size",
        stage="v1-io",
    )
    name = reader.cstring(
        offset + 52,
        64,
        f"{context}.name",
        stage="v1-io",
        allow_empty=True,
    )
    format_name, element_bytes, byte_size = _tensor_size(
        dimensions, data_format, context
    )
    return {
        "byte_size": byte_size,
        "data_format": data_format,
        "data_format_name": format_name,
        "data_type": data_type,
        "dimension_count": dimension_count,
        "dimensions": dimensions,
        "element_bytes": element_bytes,
        "index": index,
        "name": name,
        "quantization": _quantization(
            quantization_format,
            scale_word,
            zero_word,
            has_explicit_fixed_word=True,
            fixed_word=fixed_word,
        ),
    }


def _parse_io_record_v2(
    reader: CheckedReader, offset: int, index: int, direction: str
) -> dict[str, Any]:
    context = f"{direction}[{index}]"
    dimension_count = reader.u32(
        offset,
        f"{context}.dimension_count",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    if not 1 <= dimension_count <= MAX_DIMENSIONS:
        raise NbgParseError(
            "invalid-dimension-count",
            "v2-io",
            f"expected 1..{MAX_DIMENSIONS}, observed {dimension_count}",
            offset=offset,
            context=context,
        )
    all_dimensions = [
        reader.u32(
            offset + 4 + dimension_index * 4,
            f"{context}.dimensions[{dimension_index}]",
            code="invalid-io-record-size",
            stage="v2-io",
        )
        for dimension_index in range(MAX_DIMENSIONS)
    ]
    dimensions = all_dimensions[:dimension_count]
    data_format = reader.u32(
        offset + 28,
        f"{context}.data_format",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    data_type = reader.u32(
        offset + 32,
        f"{context}.data_type",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    quantization_format = reader.u32(
        offset + 36,
        f"{context}.quantization_format",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    word_0 = reader.u32(
        offset + 40,
        f"{context}.quantization_word_0",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    word_1 = reader.u32(
        offset + 44,
        f"{context}.quantization_word_1",
        code="invalid-io-record-size",
        stage="v2-io",
    )
    name = reader.cstring(
        offset + 48,
        64,
        f"{context}.name",
        stage="v2-io",
        allow_empty=True,
    )
    format_name, element_bytes, byte_size = _tensor_size(
        dimensions, data_format, context
    )
    return {
        "byte_size": byte_size,
        "data_format": data_format,
        "data_format_name": format_name,
        "data_type": data_type,
        "dimension_count": dimension_count,
        "dimensions": dimensions,
        "element_bytes": element_bytes,
        "index": index,
        "name": name,
        "quantization": _quantization(
            quantization_format,
            word_0,
            word_1,
            has_explicit_fixed_word=False,
        ),
    }


def _parse_v1(
    reader: CheckedReader, version: int
) -> dict[str, Any]:
    if version < V1_MIN_VERSION or version >= V2_VERSION:
        raise NbgParseError(
            "unsupported-version-family",
            "version",
            f"unsupported legacy version {_hex32(version)}",
            offset=4,
        )
    network_name = reader.cstring(
        12,
        64,
        "network_name",
        stage="v1-header",
        allow_empty=True,
    )
    layer_count = reader.u32(76, "layer_count", stage="v1-header")
    operation_count = reader.u32(
        80, "operation_count", stage="v1-header"
    )
    input_count = reader.u32(84, "input_count", stage="v1-header")
    output_count = reader.u32(88, "output_count", stage="v1-header")
    if (
        layer_count > MAX_LAYER_OR_OPERATION_COUNT
        or operation_count > MAX_LAYER_OR_OPERATION_COUNT
    ):
        raise NbgParseError(
            "invalid-header-field",
            "v1-header",
            "layer or operation count exceeds parser limit",
            offset=76,
        )
    if input_count > MAX_IO_RECORDS or output_count > MAX_IO_RECORDS:
        raise NbgParseError(
            "io-count-limit",
            "v1-header",
            "input or output count exceeds parser limit",
            offset=84,
        )

    cursor = 92
    feature_database: dict[str, Any] | None = None
    if version >= 0x00010003:
        feature = reader.bytes(
            cursor, 256, "feature_database", stage="v1-header"
        )
        feature_database = {
            "core_count": feature[8],
            "flags": struct.unpack("<I", feature[:4])[0],
            "num_pixel_pipes": struct.unpack("<I", feature[4:8])[0],
            "sha256": hashlib.sha256(feature).hexdigest(),
            "size": len(feature),
        }
        cursor += 256

    pool = {
        "size": reader.u32(cursor, "pool.size", stage="v1-header"),
        "alignment": reader.u32(
            cursor + 4, "pool.alignment", stage="v1-header"
        ),
        "base": reader.u32(
            cursor + 8, "pool.base", stage="v1-header"
        ),
    }
    cursor += 12
    sram: dict[str, int | None] = {
        "axi_base": reader.u32(
            cursor, "sram.axi_base", stage="v1-header"
        ),
        "axi_size": reader.u32(
            cursor + 4, "sram.axi_size", stage="v1-header"
        ),
        "vip_base": None,
        "vip_size": None,
    }
    cursor += 8
    if version >= 0x00010008:
        sram["vip_base"] = reader.u32(
            cursor, "sram.vip_base", stage="v1-header"
        )
        sram["vip_size"] = reader.u32(
            cursor + 4, "sram.vip_size", stage="v1-header"
        )
        cursor += 8

    section_names = list(V1_SECTION_NAMES)
    if version >= 0x0001000C:
        section_names.extend(V1_EXTENDED_0C_SECTIONS)
    if version >= 0x0001000E:
        section_names.extend(V1_EXTENDED_0E_SECTIONS)

    raw_sections: list[dict[str, Any]] = []
    for section_index, name in enumerate(section_names):
        section_offset = reader.u32(
            cursor,
            f"sections.{name}.offset",
            stage="v1-header",
        )
        section_size = reader.u32(
            cursor + 4,
            f"sections.{name}.size",
            stage="v1-header",
        )
        raw_sections.append(
            {
                "classification": (
                    "io-metadata"
                    if name in {"input_table", "output_table"}
                    else "opaque"
                ),
                "index": section_index,
                "name": name,
                "offset": section_offset,
                "size": section_size,
            }
        )
        cursor += 8

    nonempty_offsets: list[int] = []
    for section in raw_sections:
        offset = section["offset"]
        size = section["size"]
        if size:
            reader.checked_range(
                offset,
                size,
                f"sections.{section['name']}",
                code="section-range-overflow",
                stage="v1-section",
            )
            if offset < cursor:
                raise NbgParseError(
                    "section-overlap",
                    "v1-section",
                    "section overlaps the evidence-backed fixed prefix",
                    offset=offset,
                    context=section["name"],
                )
            nonempty_offsets.append(offset)

    first_section_offset = (
        min(nonempty_offsets) if nonempty_offsets else cursor
    )
    if first_section_offset < cursor:
        raise NbgParseError(
            "section-overlap",
            "v1-header",
            "first section begins inside the fixed prefix",
            offset=first_section_offset,
        )
    fixed_extension = {
        "offset": cursor,
        "size": first_section_offset - cursor,
        "fingerprint": _fingerprint_range(
            reader,
            cursor,
            first_section_offset - cursor,
            "opaque_fixed_extension",
        ),
    }

    inputs: list[dict[str, Any]] = []
    outputs: list[dict[str, Any]] = []
    for section in raw_sections:
        name = section["name"]
        offset = section["offset"]
        size = section["size"]
        section["fingerprint"] = _fingerprint_range(
            reader, offset, size, f"sections.{name}"
        )
        if name not in {"input_table", "output_table"}:
            continue
        count = input_count if name == "input_table" else output_count
        direction = "inputs" if name == "input_table" else "outputs"
        if count == 0:
            if size:
                raise NbgParseError(
                    "invalid-io-record-size",
                    "v1-io",
                    f"{name} is nonempty while header count is zero",
                    offset=offset,
                )
            stride = None
        else:
            if size % count:
                raise NbgParseError(
                    "invalid-io-record-size",
                    "v1-io",
                    f"{name} size {size} is not divisible by count {count}",
                    offset=offset,
                )
            stride = size // count
            allowed_strides = (
                {116, 128}
                if version >= 0x0001000B
                else ({108, 120} if version >= 0x00010004 else {44, 56})
            )
            if stride not in allowed_strides:
                raise NbgParseError(
                    "invalid-io-record-size",
                    "v1-io",
                    f"{name} stride {stride} is not one of "
                    f"{sorted(allowed_strides)}",
                    offset=offset,
                )
            if stride < 116:
                raise NbgParseError(
                    "unsupported-version-family",
                    "v1-io",
                    "legacy IO records below the six-dimension named "
                    "layout are recognized but not semantically decoded",
                    offset=offset,
                )
            parsed = [
                _parse_io_record_v1(
                    reader, offset + index * stride, index, direction
                )
                for index in range(count)
            ]
            if name == "input_table":
                inputs.extend(parsed)
            else:
                outputs.extend(parsed)
        section["record_count"] = count
        section["record_stride"] = stride

    return {
        "coverage": {
            "fixed_prefix_bytes": cursor,
            "io_metadata_bytes": sum(
                section["size"]
                for section in raw_sections
                if section["name"] in {"input_table", "output_table"}
            ),
            "opaque_fixed_extension_bytes": fixed_extension["size"],
        },
        "fixed_extension": fixed_extension,
        "format_family": "v1-fixed",
        "header": {
            "feature_database": feature_database,
            "input_count": input_count,
            "layer_count": layer_count,
            "network_name": network_name,
            "operation_count": operation_count,
            "output_count": output_count,
            "pool": pool,
            "public_prefix_end": cursor,
            "sram": sram,
        },
        "inputs": inputs,
        "outputs": outputs,
        "sections": raw_sections,
        "warnings": [
            "version exceeds pinned public TIM-VX parser maximum"
        ]
        if version > 0x00010014
        else [],
    }


def _parse_v2(
    reader: CheckedReader, version: int
) -> dict[str, Any]:
    if version != V2_VERSION:
        raise NbgParseError(
            "unsupported-version-family",
            "version",
            f"only exact Wave v2 version {_hex32(V2_VERSION)} is "
            f"supported, observed {_hex32(version)}",
            offset=4,
        )
    words = [
        reader.u32(
            12 + index * 4,
            f"header.word_{index}",
            stage="v2-header",
        )
        for index in range(9)
    ]
    declared_size = words[1]
    if declared_size != reader.size:
        raise NbgParseError(
            "invalid-header-field",
            "v2-header",
            f"declared file size {declared_size} does not match "
            f"actual size {reader.size}",
            offset=16,
            context="declared_file_size",
        )
    directory_locator = words[3]
    if directory_locator < V2_HEADER_BYTES:
        raise NbgParseError(
            "invalid-directory-locator",
            "directory",
            "directory preamble begins inside the v2 header",
            offset=directory_locator,
        )
    reader.checked_range(
        directory_locator,
        12,
        "directory_preamble",
        code="invalid-directory-locator",
        stage="directory",
    )
    records_offset = reader.u32(
        directory_locator,
        "directory.records_offset",
        code="invalid-directory-locator",
        stage="directory",
    )
    record_size = reader.u32(
        directory_locator + 4,
        "directory.record_size",
        code="invalid-directory-locator",
        stage="directory",
    )
    record_count = reader.u32(
        directory_locator + 8,
        "directory.record_count",
        code="invalid-directory-locator",
        stage="directory",
    )
    if record_size != V2_DIRECTORY_RECORD_BYTES:
        raise NbgParseError(
            "invalid-directory-record-size",
            "directory",
            f"expected {V2_DIRECTORY_RECORD_BYTES}, observed "
            f"{record_size}",
            offset=directory_locator + 4,
        )
    if not 1 <= record_count <= MAX_DIRECTORY_RECORDS:
        raise NbgParseError(
            "directory-count-limit",
            "directory",
            f"expected 1..{MAX_DIRECTORY_RECORDS}, observed "
            f"{record_count}",
            offset=directory_locator + 8,
        )
    directory_bytes = record_count * record_size
    if directory_bytes > MAX_U64:
        raise NbgParseError(
            "directory-range-overflow",
            "directory",
            "directory byte product exceeds u64",
            offset=records_offset,
        )
    _, directory_end = reader.checked_range(
        records_offset,
        directory_bytes,
        "directory_records",
        code="directory-range-overflow",
        stage="directory",
    )
    if records_offset < directory_locator + 12:
        raise NbgParseError(
            "section-overlap",
            "directory",
            "directory records overlap their preamble",
            offset=records_offset,
        )

    sections: list[dict[str, Any]] = []
    nonempty_ranges: list[tuple[int, int, int]] = []
    for index in range(record_count):
        record_offset = records_offset + index * record_size
        directory_words = [
            reader.u32(
                record_offset + word_index * 4,
                f"directory[{index}].word_{word_index}",
                code="directory-range-overflow",
                stage="directory",
            )
            for word_index in range(11)
        ]
        section_type = directory_words[0]
        data_offset = _checked_u64(
            directory_words[1],
            directory_words[2],
            f"directory[{index}].data_offset",
        )
        data_size = _checked_u64(
            directory_words[3],
            directory_words[4],
            f"directory[{index}].data_size",
        )
        stride = directory_words[8]
        reader.checked_range(
            data_offset,
            data_size,
            f"directory[{index}].section",
            code="section-range-overflow",
            stage="v2-section",
        )
        if data_size and data_offset < directory_end:
            raise NbgParseError(
                "section-overlap",
                "v2-section",
                "section overlaps the v2 header or directory",
                offset=data_offset,
                context=f"directory[{index}]",
            )
        if data_size:
            nonempty_ranges.append(
                (data_offset, data_offset + data_size, index)
            )
        if stride and data_size % stride == 0:
            section_record_count: int | None = data_size // stride
        else:
            section_record_count = None
        section = {
            "classification": (
                "input-metadata"
                if section_type == 1
                else (
                    "output-metadata"
                    if section_type == 2
                    else f"opaque-section-type-{section_type}"
                )
            ),
            "directory_words": directory_words,
            "fingerprint": _fingerprint_range(
                reader,
                data_offset,
                data_size,
                f"directory[{index}].section",
            ),
            "index": index,
            "offset": data_offset,
            "record_count": section_record_count,
            "size": data_size,
            "stride": stride,
            "type": section_type,
        }
        sections.append(section)

    ordered_ranges = sorted(nonempty_ranges)
    for prior, current in zip(ordered_ranges, ordered_ranges[1:]):
        if current[0] < prior[1]:
            raise NbgParseError(
                "section-overlap",
                "v2-section",
                f"directory records {prior[2]} and {current[2]} overlap",
                offset=current[0],
            )

    inputs: list[dict[str, Any]] = []
    outputs: list[dict[str, Any]] = []
    seen_io_types: set[int] = set()
    for section in sections:
        section_type = section["type"]
        if section_type not in {1, 2}:
            continue
        if section_type in seen_io_types:
            raise NbgParseError(
                "invalid-io-record-size",
                "v2-io",
                f"duplicate IO section type {section_type}",
                offset=section["offset"],
            )
        seen_io_types.add(section_type)
        if section["stride"] != V2_IO_RECORD_BYTES:
            raise NbgParseError(
                "invalid-io-record-size",
                "v2-io",
                f"expected stride {V2_IO_RECORD_BYTES}, observed "
                f"{section['stride']}",
                offset=section["offset"],
            )
        count = section["record_count"]
        if count is None:
            raise NbgParseError(
                "invalid-io-record-size",
                "v2-io",
                "IO section size is not divisible by its stride",
                offset=section["offset"],
            )
        if not 1 <= count <= MAX_IO_RECORDS:
            raise NbgParseError(
                "io-count-limit",
                "v2-io",
                f"expected 1..{MAX_IO_RECORDS}, observed {count}",
                offset=section["offset"],
            )
        direction = "inputs" if section_type == 1 else "outputs"
        parsed = [
            _parse_io_record_v2(
                reader,
                section["offset"] + index * V2_IO_RECORD_BYTES,
                index,
                direction,
            )
            for index in range(count)
        ]
        if section_type == 1:
            inputs.extend(parsed)
        else:
            outputs.extend(parsed)

    if 1 not in seen_io_types or 2 not in seen_io_types:
        raise NbgParseError(
            "invalid-io-record-size",
            "v2-io",
            "v2 graph must contain one input and one output section",
        )

    io_bytes = sum(
        section["size"]
        for section in sections
        if section["type"] in {1, 2}
    )
    opaque_bytes = sum(
        section["size"]
        for section in sections
        if section["type"] not in {1, 2}
    )
    return {
        "coverage": {
            "directory_bytes": directory_bytes + 12,
            "header_bytes": V2_HEADER_BYTES,
            "io_metadata_bytes": io_bytes,
            "opaque_section_bytes": opaque_bytes,
        },
        "directory": {
            "end": directory_end,
            "locator": directory_locator,
            "record_count": record_count,
            "record_size": record_size,
            "records_offset": records_offset,
        },
        "format_family": "v2-directory",
        "header": {
            "declared_file_size": declared_size,
            "directory_locator": directory_locator,
            "flags": {
                "bit_0": words[6] & 1,
                "bits_1_through_4": (words[6] >> 1) & 0xF,
                "raw": words[6],
            },
            "words_0_through_8": words,
        },
        "inputs": inputs,
        "outputs": outputs,
        "sections": sections,
        "warnings": [],
    }


def _structural_signature(result: Mapping[str, Any]) -> str:
    structure = {
        "format_family": result["format_family"],
        "inputs": [
            {
                "data_format": tensor["data_format"],
                "data_type": tensor["data_type"],
                "dimensions": tensor["dimensions"],
                "quantization_format": tensor["quantization"]["format"],
            }
            for tensor in result["inputs"]
        ],
        "outputs": [
            {
                "data_format": tensor["data_format"],
                "data_type": tensor["data_type"],
                "dimensions": tensor["dimensions"],
                "quantization_format": tensor["quantization"]["format"],
            }
            for tensor in result["outputs"]
        ],
        "product": result["product"],
        "sections": [
            {
                "record_count": section.get("record_count"),
                "size": section["size"],
                "stride": section.get("stride")
                or section.get("record_stride"),
                "type": section.get("type") or section.get("name"),
            }
            for section in result["sections"]
        ],
        "version": result["version"],
    }
    return hashlib.sha256(canonical_json(structure)).hexdigest()


def parse_source(
    source: MemorySource | FileSource,
    *,
    display_path: str,
) -> dict[str, Any]:
    """Parse one immutable source without invoking any native model code."""

    reader = CheckedReader(source)
    if reader.size < MINIMUM_RUNTIME_NBG_BYTES:
        raise NbgParseError(
            "file-too-small",
            "envelope",
            f"runtime-compatible NBG must be at least "
            f"{MINIMUM_RUNTIME_NBG_BYTES} bytes, observed {reader.size}",
            offset=0,
        )
    magic = reader.bytes(
        0, 4, "magic", code="file-too-small", stage="envelope"
    )
    if magic != MAGIC:
        raise NbgParseError(
            "bad-magic",
            "envelope",
            "file does not start with VPMN",
            offset=0,
        )
    version = reader.u32(4, "version", stage="envelope")
    product = reader.u32(8, "product", stage="envelope")
    if V1_MIN_VERSION <= version < V2_VERSION:
        family = _parse_v1(reader, version)
    elif version >= V2_VERSION:
        family = _parse_v2(reader, version)
    else:
        raise NbgParseError(
            "unsupported-version-family",
            "version",
            f"unsupported NBG version {_hex32(version)}",
            offset=4,
        )

    connection_offset = reader.size - CONNECTION_TAIL_BYTES
    connection_tail = {
        "offset": connection_offset,
        "size": CONNECTION_TAIL_BYTES,
        "fingerprint": _fingerprint_range(
            reader,
            connection_offset,
            CONNECTION_TAIL_BYTES,
            "connection_tail",
        ),
    }
    result: dict[str, Any] = {
        "connection_tail": connection_tail,
        "magic": MAGIC.decode("ascii"),
        "path": display_path,
        "product": product,
        "product_hex": _hex32(product),
        "sha256": _sha256_range(source, 0, reader.size),
        "size": reader.size,
        "version": version,
        "version_hex": _hex32(version),
    }
    result.update(family)
    result["structural_signature"] = _structural_signature(result)
    return result


def parse_bytes(data: bytes, *, display_path: str = "<memory>") -> dict[str, Any]:
    """Parse test-owned bytes with the same checked implementation."""

    return parse_source(MemorySource(data), display_path=display_path)


def parse_path(
    path: Path,
    *,
    display_path: str | None = None,
) -> dict[str, Any]:
    """Parse one regular non-symlink file through streaming reads."""

    if path.is_symlink():
        raise NbgParseError(
            "symlink-rejected",
            "file",
            "NBG path may not be a symlink",
            context=str(path),
        )
    if not path.is_file():
        raise NbgParseError(
            "not-regular-file",
            "file",
            "NBG path is not a regular file",
            context=str(path),
        )
    with FileSource(path) as source:
        return parse_source(
            source, display_path=display_path or path.as_posix()
        )


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


def _source_identity(path: Path) -> dict[str, str]:
    payload = path.read_bytes()
    return {
        "path": path.relative_to(REPOSITORY_ROOT).as_posix(),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def build_inventory(
    corpus: Corpus,
    *,
    progress: Callable[[int, int, CorpusFile], None] | None = None,
) -> dict[str, Any]:
    """Parse and fingerprint every configured NBG deterministically."""

    discovery = discover_corpus(corpus)
    files: list[dict[str, Any]] = []
    total = len(discovery.files)
    for index, corpus_file in enumerate(discovery.files, start=1):
        if progress is not None:
            progress(index, total, corpus_file)
        try:
            parsed = parse_path(
                corpus_file.path,
                display_path=corpus_file.display_path,
            )
        except NbgParseError as exc:
            raise NbgInventoryError(
                f"cannot parse {corpus_file.display_path}: {exc}"
            ) from exc
        if (
            corpus_file.expected_sha256 is not None
            and parsed["sha256"] != corpus_file.expected_sha256
        ):
            raise NbgInventoryError(
                f"corpus hash mismatch for {corpus_file.corpus_id}: "
                f"expected {corpus_file.expected_sha256}, observed "
                f"{parsed['sha256']}"
            )
        parsed["collection_id"] = corpus_file.collection_id
        parsed["corpus_id"] = corpus_file.corpus_id
        parsed["model_family"] = corpus_file.model_family
        parsed["role"] = corpus_file.role
        files.append(parsed)

    return {
        "collections": list(discovery.collections),
        "corpus": {
            "configured_file_count": total,
            "path": corpus.source_path.relative_to(
                corpus.repository_root
            ).as_posix(),
            "sha256": corpus.source_sha256,
        },
        "files": files,
        "generator": _source_identity(Path(__file__).resolve()),
        "limits": {
            "connection_tail_bytes": CONNECTION_TAIL_BYTES,
            "maximum_dimensions": MAX_DIMENSIONS,
            "maximum_directory_records": MAX_DIRECTORY_RECORDS,
            "maximum_io_records": MAX_IO_RECORDS,
            "maximum_tensor_bytes": MAX_TENSOR_BYTES,
            "v2_directory_record_bytes": V2_DIRECTORY_RECORD_BYTES,
            "v2_io_record_bytes": V2_IO_RECORD_BYTES,
        },
        "schema_version": INVENTORY_SCHEMA_VERSION,
    }


def _counter_rows(counter: Counter[Any]) -> list[tuple[str, int]]:
    return sorted(
        ((str(key), count) for key, count in counter.items()),
        key=lambda item: item[0],
    )


def _markdown_table(
    headers: Sequence[str], rows: Iterable[Sequence[Any]]
) -> list[str]:
    lines = [
        "| " + " | ".join(headers) + " |",
        "|" + "|".join("---" for _ in headers) + "|",
    ]
    for row in rows:
        lines.append(
            "| "
            + " | ".join(str(value).replace("|", "\\|") for value in row)
            + " |"
        )
    return lines


def render_summary(inventory: Mapping[str, Any]) -> str:
    """Render a deterministic, generated Markdown corpus summary."""

    if inventory.get("schema_version") != INVENTORY_SCHEMA_VERSION:
        raise NbgInventoryError("inventory schema version is invalid")
    raw_files = inventory.get("files")
    if not isinstance(raw_files, list):
        raise NbgInventoryError("inventory files must be an array")
    files: list[dict[str, Any]] = []
    for index, value in enumerate(raw_files):
        if not isinstance(value, dict):
            raise NbgInventoryError(
                f"inventory file {index} must be an object"
            )
        files.append(value)

    total_bytes = sum(int(item["size"]) for item in files)
    role_counts = Counter(item["role"] for item in files)
    versions = Counter(
        (item["version_hex"], item["product_hex"]) for item in files
    )
    families: dict[str, tuple[int, int]] = {}
    for item in files:
        family = item["model_family"]
        old_count, old_bytes = families.get(family, (0, 0))
        families[family] = (old_count + 1, old_bytes + item["size"])

    full_hash_groups: dict[str, list[str]] = {}
    structural_groups: dict[str, list[str]] = {}
    section_types: dict[str, dict[str, Any]] = {}
    io_shapes: Counter[str] = Counter()
    io_formats: Counter[str] = Counter()
    quantization_formats: Counter[int] = Counter()
    for item in files:
        full_hash_groups.setdefault(item["sha256"], []).append(item["path"])
        structural_groups.setdefault(
            item["structural_signature"], []
        ).append(item["path"])
        for section in item["sections"]:
            section_key = str(section.get("type", section.get("name")))
            stats = section_types.setdefault(
                section_key,
                {"bytes": 0, "files": set(), "records": 0, "strides": set()},
            )
            stats["bytes"] += section["size"]
            stats["files"].add(item["path"])
            if section.get("record_count") is not None:
                stats["records"] += section["record_count"]
            stride = section.get("stride") or section.get("record_stride")
            if stride is not None:
                stats["strides"].add(stride)
        for direction in ("inputs", "outputs"):
            for tensor in item[direction]:
                io_shapes[
                    f"{direction[:-1]} {tensor['dimensions']}"
                ] += 1
                io_formats[
                    f"{direction[:-1]} "
                    f"{tensor['data_format_name'] or tensor['data_format']}"
                ] += 1
                quantization_formats[
                    tensor["quantization"]["format"]
                ] += 1

    duplicate_groups = sorted(
        (
            (digest, sorted(paths))
            for digest, paths in full_hash_groups.items()
            if len(paths) > 1
        ),
        key=lambda item: (-len(item[1]), item[0]),
    )
    structure_duplicates = sorted(
        (
            (digest, sorted(paths))
            for digest, paths in structural_groups.items()
            if len(paths) > 1
        ),
        key=lambda item: (-len(item[1]), item[0]),
    )
    inventory_digest = hashlib.sha256(
        canonical_json(inventory)
    ).hexdigest()

    lines = [
        "# Generated Wave NBG Corpus Summary",
        "",
        "This file is generated by `waved/scripts/npu/nbg_format.py`; "
        "do not edit it manually.",
        "",
        "## Corpus identity",
        "",
        f"- Inventory schema: `{INVENTORY_SCHEMA_VERSION}`",
        f"- Inventory SHA-256: `{inventory_digest}`",
        f"- Corpus config SHA-256: `{inventory['corpus']['sha256']}`",
        f"- Files: {len(files)}",
        f"- Bytes: {total_bytes}",
        f"- Unique full-file SHA-256 values: {len(full_hash_groups)}",
        f"- Unique structural signatures: {len(structural_groups)}",
        "",
        "## Roles",
        "",
    ]
    lines.extend(
        _markdown_table(
            ("Role", "Files"), _counter_rows(role_counts)
        )
    )
    lines.extend(["", "## Version and target clusters", ""])
    lines.extend(
        _markdown_table(
            ("Version", "Product", "Files"),
            (
                (version, product, count)
                for (version, product), count in sorted(versions.items())
            ),
        )
    )
    lines.extend(["", "## Model path families", ""])
    lines.extend(
        _markdown_table(
            ("Family", "Files", "Bytes"),
            (
                (family, values[0], values[1])
                for family, values in sorted(families.items())
            ),
        )
    )
    lines.extend(["", "## Section directory distributions", ""])
    lines.extend(
        _markdown_table(
            ("Type/name", "Files", "Bytes", "Records", "Strides"),
            (
                (
                    key,
                    len(value["files"]),
                    value["bytes"],
                    value["records"],
                    ", ".join(
                        str(stride)
                        for stride in sorted(value["strides"])
                    )
                    or "—",
                )
                for key, value in sorted(section_types.items())
            ),
        )
    )
    lines.extend(["", "## Tensor formats", ""])
    lines.extend(
        _markdown_table(
            ("Direction/format", "Records"), _counter_rows(io_formats)
        )
    )
    lines.extend(["", "## Tensor shapes", ""])
    lines.extend(
        _markdown_table(
            ("Direction/shape", "Records"), _counter_rows(io_shapes)
        )
    )
    lines.extend(["", "## Quantization formats", ""])
    lines.extend(
        _markdown_table(
            ("Raw format", "Records"),
            _counter_rows(quantization_formats),
        )
    )
    lines.extend(["", "## Exact duplicate groups", ""])
    if duplicate_groups:
        for digest, paths in duplicate_groups:
            lines.append(
                f"- `{digest}` — {len(paths)} files: "
                + ", ".join(f"`{path}`" for path in paths)
            )
    else:
        lines.append("No exact duplicate groups.")
    lines.extend(["", "## Structural duplicate groups", ""])
    if structure_duplicates:
        for digest, paths in structure_duplicates[:30]:
            examples = paths[:4]
            suffix = (
                f", and {len(paths) - len(examples)} more"
                if len(paths) > len(examples)
                else ""
            )
            lines.append(
                f"- `{digest}` — {len(paths)} files: "
                + ", ".join(f"`{path}`" for path in examples)
                + suffix
            )
        if len(structure_duplicates) > 30:
            lines.append(
                f"- {len(structure_duplicates) - 30} additional groups are "
                "available in the machine-readable inventory."
            )
    else:
        lines.append("No structural duplicate groups.")
    lines.extend(
        [
            "",
            "## Interpretation boundary",
            "",
            "An exact hash proves byte identity. A structural signature "
            "covers only parsed envelope, section-layout, and IO metadata; "
            "it does not prove equal operations, weights, or NPU commands.",
            "",
        ]
    )
    return "\n".join(lines)


def _atomic_write(path: Path, payload: bytes) -> None:
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


def _read_inventory(path: Path) -> dict[str, Any]:
    try:
        payload = path.read_bytes()
        value = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except NbgCorpusError as exc:
        raise NbgInventoryError(str(exc)) from exc
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise NbgInventoryError(
            f"cannot read inventory {path}: {exc}"
        ) from exc
    if not isinstance(value, dict):
        raise NbgInventoryError("inventory root must be an object")
    return value


def _progress(index: int, total: int, item: CorpusFile) -> None:
    print(
        f"[{index:03d}/{total:03d}] {item.display_path}",
        file=sys.stderr,
        flush=True,
    )


def verify_generated(corpus: Corpus) -> None:
    inventory = build_inventory(corpus, progress=_progress)
    expected_inventory = canonical_json(inventory)
    try:
        observed_inventory = DEFAULT_INVENTORY_PATH.read_bytes()
    except OSError as exc:
        raise NbgInventoryError(
            f"cannot read generated inventory: {exc}"
        ) from exc
    if observed_inventory != expected_inventory:
        raise NbgInventoryError(
            "generated NBG inventory is stale; run the inventory command"
        )
    expected_summary = render_summary(inventory).encode("utf-8")
    try:
        observed_summary = DEFAULT_SUMMARY_PATH.read_bytes()
    except OSError as exc:
        raise NbgInventoryError(
            f"cannot read generated summary: {exc}"
        ) from exc
    if observed_summary != expected_summary:
        raise NbgInventoryError(
            "generated NBG summary is stale; run the summary command"
        )


def _repository_path(text: str) -> tuple[Path, str]:
    candidate = Path(text)
    path = candidate if candidate.is_absolute() else Path.cwd() / candidate
    resolved = path.resolve()
    try:
        display = resolved.relative_to(REPOSITORY_ROOT).as_posix()
    except ValueError as exc:
        raise NbgCorpusError(
            "parse path must stay inside the repository"
        ) from exc
    _reject_symlink_components(REPOSITORY_ROOT, path, "parse path")
    return path, display


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Strict host-only NBG parser. This tool never invokes ADB or "
            "native firmware code."
        )
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    parse_command = subparsers.add_parser("parse")
    parse_command.add_argument("path")

    inventory_command = subparsers.add_parser("inventory")
    inventory_command.add_argument(
        "--output", type=Path, default=DEFAULT_INVENTORY_PATH
    )

    summary_command = subparsers.add_parser("summary")
    summary_command.add_argument(
        "--inventory", type=Path, default=DEFAULT_INVENTORY_PATH
    )
    summary_command.add_argument(
        "--output", type=Path, default=DEFAULT_SUMMARY_PATH
    )

    subparsers.add_parser("verify-generated")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "parse":
            path, display = _repository_path(args.path)
            result = parse_path(path, display_path=display)
            sys.stdout.buffer.write(canonical_json(result))
            return 0
        if args.command == "inventory":
            corpus = load_corpus()
            inventory = build_inventory(corpus, progress=_progress)
            output = args.output
            if not output.is_absolute():
                output = Path.cwd() / output
            resolved_output = output.resolve()
            try:
                resolved_output.relative_to(REPOSITORY_ROOT)
            except ValueError as exc:
                raise NbgCorpusError(
                    "inventory output must stay inside the repository"
                ) from exc
            _atomic_write(output, canonical_json(inventory))
            print(
                f"wrote {len(inventory['files'])} NBG records to {output}"
            )
            return 0
        if args.command == "summary":
            inventory = _read_inventory(args.inventory)
            summary = render_summary(inventory).encode("utf-8")
            output = args.output
            if not output.is_absolute():
                output = Path.cwd() / output
            resolved_output = output.resolve()
            try:
                resolved_output.relative_to(REPOSITORY_ROOT)
            except ValueError as exc:
                raise NbgCorpusError(
                    "summary output must stay inside the repository"
                ) from exc
            _atomic_write(output, summary)
            print(f"wrote NBG corpus summary to {output}")
            return 0
        if args.command == "verify-generated":
            verify_generated(load_corpus())
            print("generated NBG inventory and summary are current")
            return 0
        raise AssertionError(f"unhandled command {args.command}")
    except NbgParseError as exc:
        sys.stderr.buffer.write(
            canonical_json({"error": exc.as_dict(), "status": "error"})
        )
        return 2
    except NbgError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
