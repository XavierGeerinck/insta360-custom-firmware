#!/usr/bin/env python3
"""Run focused Ghidra analysis for the Wave NPU ABI evidence set."""

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
EVIDENCE_SCHEMA_VERSION = 1
DEFAULT_CONFIG_PATH = Path(__file__).with_name("ghidra_targets.json")
DEFAULT_OUTPUT_PATH = (
    Path(__file__).resolve().parents[3] / "docs/npu/abi/ghidra-evidence.json"
)
ROOT_KEYS = frozenset({"schema_version", "targets"})
TARGET_KEYS = frozenset(
    {
        "firmware",
        "path",
        "sha256",
        "roles",
        "functions",
        "string_filters",
    }
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")
HEADLESS_TIMEOUT_SECONDS = 1800
MAX_TOOL_LOG_CHARS = 12000


class GhidraAbiError(Exception):
    """Base error for focused Ghidra evidence generation."""


class GhidraConfigError(GhidraAbiError):
    """Target configuration is malformed or escapes the repository."""


class GhidraExecutionError(GhidraAbiError):
    """Ghidra failed or returned invalid evidence."""


@dataclass(frozen=True)
class GhidraTarget:
    evidence_id: str
    firmware: str
    path: Path
    sha256: str
    roles: tuple[str, ...]
    functions: tuple[str, ...]
    string_filters: tuple[str, ...]


@dataclass(frozen=True)
class GhidraConfig:
    repository_root: Path
    source_path: Path
    source_sha256: str
    targets: Mapping[str, GhidraTarget]


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise GhidraConfigError(f"duplicate JSON key: {key!r}")
        value[key] = item
    return value


def _strict_keys(
    value: Mapping[str, Any], expected: frozenset[str], context: str
) -> None:
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing:
        raise GhidraConfigError(
            f"{context} missing keys: {', '.join(missing)}"
        )
    if unknown:
        raise GhidraConfigError(
            f"{context} has unknown keys: {', '.join(unknown)}"
        )


def _require_dict(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise GhidraConfigError(f"{context} must be a JSON object")
    return value


def _require_string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise GhidraConfigError(f"{context} must be a nonempty string")
    if CONTROL_RE.search(value):
        raise GhidraConfigError(f"{context} contains a control character")
    return value


def _string_array(value: Any, context: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise GhidraConfigError(f"{context} must be a nonempty JSON array")
    result = tuple(
        _require_string(item, f"{context}[{index}]")
        for index, item in enumerate(value)
    )
    if len(set(result)) != len(result):
        raise GhidraConfigError(f"{context} contains duplicate values")
    if any("," in item for item in result):
        raise GhidraConfigError(
            f"{context} values may not contain commas"
        )
    return tuple(sorted(result))


def _relative_path(value: Any, context: str) -> Path:
    text = _require_string(value, context)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts:
        raise GhidraConfigError(
            f"{context} may not escape the repository: {text!r}"
        )
    return path


def _resolved_below(root: Path, path: Path, context: str) -> Path:
    root_resolved = root.resolve()
    resolved = path.resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise GhidraConfigError(
            f"{context} resolves outside the repository"
        ) from exc
    return resolved


def load_config(
    path: Path = DEFAULT_CONFIG_PATH,
    *,
    repository_root: Path | None = None,
) -> GhidraConfig:
    repository = (
        repository_root.resolve()
        if repository_root is not None
        else Path(__file__).resolve().parents[3]
    )
    try:
        payload = path.read_bytes()
    except OSError as exc:
        raise GhidraConfigError(f"cannot read config {path}: {exc}") from exc
    try:
        document = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
        )
    except GhidraConfigError:
        raise
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise GhidraConfigError(f"invalid target JSON: {exc}") from exc
    root = _require_dict(document, "root")
    _strict_keys(root, ROOT_KEYS, "root")
    version = root["schema_version"]
    if isinstance(version, bool) or not isinstance(version, int):
        raise GhidraConfigError("schema_version must be an integer")
    if version != SCHEMA_VERSION:
        raise GhidraConfigError(
            f"unsupported schema_version {version}; expected {SCHEMA_VERSION}"
        )
    raw_targets = _require_dict(root["targets"], "targets")
    if not raw_targets:
        raise GhidraConfigError("targets must not be empty")

    targets: dict[str, GhidraTarget] = {}
    for evidence_id, raw_value in raw_targets.items():
        if ID_RE.fullmatch(evidence_id) is None:
            raise GhidraConfigError(
                f"invalid target ID: {evidence_id!r}"
            )
        context = f"targets.{evidence_id}"
        item = _require_dict(raw_value, context)
        _strict_keys(item, TARGET_KEYS, context)
        relative_path = _relative_path(item["path"], f"{context}.path")
        _resolved_below(
            repository,
            repository / relative_path,
            f"{context}.path",
        )
        digest = _require_string(item["sha256"], f"{context}.sha256")
        if SHA256_RE.fullmatch(digest) is None:
            raise GhidraConfigError(
                f"{context}.sha256 must be a lowercase SHA-256"
            )
        targets[evidence_id] = GhidraTarget(
            evidence_id=evidence_id,
            firmware=_require_string(
                item["firmware"], f"{context}.firmware"
            ),
            path=relative_path,
            sha256=digest,
            roles=_string_array(item["roles"], f"{context}.roles"),
            functions=_string_array(
                item["functions"], f"{context}.functions"
            ),
            string_filters=_string_array(
                item["string_filters"], f"{context}.string_filters"
            ),
        )
    return GhidraConfig(
        repository_root=repository,
        source_path=path.resolve(),
        source_sha256=hashlib.sha256(payload).hexdigest(),
        targets=MappingProxyType(dict(sorted(targets.items()))),
    )


def _bounded_log(value: str) -> str:
    if len(value) <= MAX_TOOL_LOG_CHARS:
        return value
    half = MAX_TOOL_LOG_CHARS // 2
    return (
        value[:half]
        + "\n... [tool log truncated] ...\n"
        + value[-half:]
    )


def _find_analyze_headless() -> Path:
    configured = os.environ.get("GHIDRA_ANALYZE_HEADLESS")
    if configured:
        path = Path(configured)
        if path.is_file():
            return path.resolve()
        raise GhidraExecutionError(
            "GHIDRA_ANALYZE_HEADLESS does not identify a file"
        )
    discovered = shutil.which("analyzeHeadless")
    if discovered:
        return Path(discovered).resolve()
    homebrew_candidates = sorted(
        Path("/opt/homebrew/Cellar/ghidra").glob(
            "*/libexec/support/analyzeHeadless"
        )
    )
    if homebrew_candidates:
        return homebrew_candidates[-1].resolve()
    raise GhidraExecutionError(
        "cannot find analyzeHeadless; set GHIDRA_ANALYZE_HEADLESS"
    )


class HeadlessGhidra:
    def __init__(self, config: GhidraConfig):
        self.config = config
        self.executable = _find_analyze_headless()
        self.script_directory = (
            config.repository_root / "scripts/ghidra"
        ).resolve()
        self.script = self.script_directory / "ExportNpuAbi.java"
        if not self.script.is_file():
            raise GhidraExecutionError(
                f"missing Ghidra script: {self.script}"
            )

    def analyze(
        self, target: GhidraTarget, binary_path: Path
    ) -> dict[str, Any]:
        with tempfile.TemporaryDirectory(prefix="wave-npu-ghidra-") as temp:
            temporary = Path(temp)
            output = temporary / "evidence.json"
            project_name = f"poc1-{target.evidence_id}"
            command = [
                str(self.executable),
                str(temporary),
                project_name,
                "-import",
                str(binary_path),
                "-overwrite",
                "-scriptPath",
                str(self.script_directory),
                "-postScript",
                self.script.name,
                str(output),
                ",".join(target.functions),
                ",".join(target.string_filters),
                "-deleteProject",
            ]
            environment = os.environ.copy()
            java_home = Path(
                "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"
            )
            if "JAVA_HOME" not in environment and java_home.is_dir():
                environment["JAVA_HOME"] = str(java_home)
            try:
                process = subprocess.run(
                    command,
                    cwd=self.config.repository_root,
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    check=False,
                    timeout=HEADLESS_TIMEOUT_SECONDS,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
            except subprocess.TimeoutExpired as exc:
                raise GhidraExecutionError(
                    f"Ghidra timed out for {target.evidence_id}"
                ) from exc
            except OSError as exc:
                raise GhidraExecutionError(
                    f"cannot execute Ghidra for {target.evidence_id}: {exc}"
                ) from exc
            if process.returncode != 0:
                raise GhidraExecutionError(
                    f"Ghidra failed for {target.evidence_id} "
                    f"({process.returncode}):\n"
                    f"{_bounded_log(process.stdout)}"
                )
            try:
                value = json.loads(output.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError) as exc:
                raise GhidraExecutionError(
                    f"invalid Ghidra evidence for {target.evidence_id}: {exc}\n"
                    f"{_bounded_log(process.stdout)}"
                ) from exc
            if not isinstance(value, dict):
                raise GhidraExecutionError(
                    f"Ghidra evidence root is not an object: "
                    f"{target.evidence_id}"
                )
            return value


def _validate_program_evidence(
    target: GhidraTarget, analysis: Mapping[str, Any]
) -> None:
    if analysis.get("schema_version") != 1:
        raise GhidraExecutionError(
            f"{target.evidence_id}: unexpected Ghidra evidence schema"
        )
    program = analysis.get("program")
    if not isinstance(program, dict):
        raise GhidraExecutionError(
            f"{target.evidence_id}: missing program evidence"
        )
    if program.get("sha256") != target.sha256:
        raise GhidraExecutionError(
            f"{target.evidence_id}: Ghidra program SHA-256 mismatch"
        )
    targets = analysis.get("targets")
    if not isinstance(targets, list):
        raise GhidraExecutionError(
            f"{target.evidence_id}: missing target evidence array"
        )
    observed_names = {
        item.get("target")
        for item in targets
        if isinstance(item, dict)
    }
    missing = sorted(set(target.functions) - observed_names)
    if missing:
        raise GhidraExecutionError(
            f"{target.evidence_id}: target records missing: "
            + ", ".join(missing)
        )


def build_evidence(
    config: GhidraConfig, executor: Any | None = None
) -> dict[str, Any]:
    analyzer = executor or HeadlessGhidra(config)
    results: list[dict[str, Any]] = []
    for evidence_id, target in config.targets.items():
        binary_path = _resolved_below(
            config.repository_root,
            config.repository_root / target.path,
            f"target {evidence_id}",
        )
        try:
            payload = binary_path.read_bytes()
        except OSError as exc:
            raise GhidraExecutionError(
                f"cannot read target {evidence_id}: {exc}"
            ) from exc
        digest = hashlib.sha256(payload).hexdigest()
        if digest != target.sha256:
            raise GhidraExecutionError(
                f"{evidence_id}: configured SHA-256 {target.sha256}, "
                f"observed {digest}"
            )
        if not payload.startswith(b"\x7fELF"):
            raise GhidraExecutionError(f"{evidence_id}: target is not ELF")
        analysis = analyzer.analyze(target, binary_path)
        _validate_program_evidence(target, analysis)
        results.append(
            {
                "evidence_id": evidence_id,
                "firmware": target.firmware,
                "path": target.path.as_posix(),
                "sha256": target.sha256,
                "bytes": len(payload),
                "roles": list(target.roles),
                "analysis": analysis,
            }
        )
    return {
        "schema_version": EVIDENCE_SCHEMA_VERSION,
        "config_sha256": config.source_sha256,
        "generator": "waved/scripts/npu/ghidra_abi.py",
        "targets": results,
    }


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


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate focused host-only Wave NPU Ghidra evidence."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    generate = subparsers.add_parser("generate")
    generate.add_argument("--output", type=Path, default=DEFAULT_OUTPUT_PATH)
    subparsers.add_parser("verify-generated")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        config = load_config()
        evidence = build_evidence(config)
        generated = canonical_json(evidence)
        if args.command == "generate":
            _write_atomic(args.output, generated)
            print(args.output)
            return 0
        if args.command == "verify-generated":
            try:
                tracked = DEFAULT_OUTPUT_PATH.read_bytes()
            except OSError as exc:
                print(f"npu-ghidra: {exc}", file=sys.stderr)
                return 1
            if tracked != generated:
                print(
                    "npu-ghidra: tracked Ghidra evidence differs",
                    file=sys.stderr,
                )
                return 1
            print("generated Ghidra evidence is current")
            return 0
        return 2
    except GhidraAbiError as exc:
        print(f"npu-ghidra: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
