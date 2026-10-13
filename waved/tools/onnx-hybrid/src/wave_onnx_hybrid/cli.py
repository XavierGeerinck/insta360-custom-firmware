"""Command-line boundary for ONNX audit and CPU fallback evidence."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import numpy as np

from .audit import (
    AuditResult,
    audit_model_path,
    canonical_json,
    load_policy,
)
from .errors import ErrorCode, HybridError
from .fixtures import (
    deterministic_npz_bytes,
    generate_fixtures,
    verify_fixtures,
)
from .native_cli import add_native_commands
from .runtime import ExecutionMode, execute_cpu_model

REPOSITORY_ROOT = Path(__file__).resolve().parents[5]
DEFAULT_POLICY = REPOSITORY_ROOT / "docs/npu/onnx/hybrid-policy.json"


def _write_exclusive(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        descriptor = os.open(
            path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL,
            0o644,
        )
    except FileExistsError as exc:
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {path}",
        ) from exc
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _load_npz(path: Path) -> dict[str, np.ndarray]:
    try:
        with np.load(path, allow_pickle=False) as archive:
            return {
                name: np.array(archive[name], copy=True, order="C")
                for name in archive.files
            }
    except (OSError, ValueError) as exc:
        raise HybridError(
            ErrorCode.ONNX_INPUT_INVALID,
            f"cannot load input archive {path}: {exc}",
        ) from exc


def _load_and_audit(args: argparse.Namespace) -> AuditResult:
    policy = load_policy(
        args.policy,
        repository_root=args.repository_root,
    )
    return audit_model_path(args.model, policy=policy)


def _command_audit(args: argparse.Namespace) -> int:
    result = _load_and_audit(args)
    _write_exclusive(args.output, canonical_json(result.document))
    print(f"wrote {args.output}")
    return 0


def _command_run(args: argparse.Namespace) -> int:
    if args.backend == "wave-native":
        return _command_native_run(args)
    if args.package is not None or args.run_id is not None:
        raise HybridError(
            ErrorCode.POLICY_INVALID, "package and run ID require wave-native backend"
        )
    if args.outputs.exists():
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {args.outputs}",
        )
    if args.evidence.exists():
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"output already exists: {args.evidence}",
        )
    audit = _load_and_audit(args)
    inputs = _load_npz(args.inputs)
    mode = ExecutionMode(args.mode)
    result = execute_cpu_model(args.model, audit, inputs, mode=mode)
    output_payload = deterministic_npz_bytes(result.outputs)
    evidence_document: dict[str, Any] = {
        "schema_version": 1,
        "audit": audit.document,
        "execution": result.evidence,
    }
    _write_exclusive(args.outputs, output_payload)
    try:
        _write_exclusive(args.evidence, canonical_json(evidence_document))
    except BaseException:
        args.outputs.unlink(missing_ok=True)
        raise
    print(f"wrote {args.outputs}")
    print(f"wrote {args.evidence}")
    return 0


def _command_native_run(args: argparse.Namespace) -> int:
    from .native_backend import NativeSession

    if (
        args.policy.resolve() != DEFAULT_POLICY.resolve()
        or args.repository_root.resolve() != REPOSITORY_ROOT.resolve()
    ):
        raise HybridError(
            ErrorCode.POLICY_INVALID,
            "wave-native uses its fixed qualification policy",
        )
    if args.package is None or args.run_id is None:
        raise HybridError(
            ErrorCode.POLICY_INVALID, "native run requires package and run ID"
        )
    if args.mode not in (ExecutionMode.STRICT.value, ExecutionMode.HYBRID.value):
        raise HybridError(
            ErrorCode.POLICY_INVALID, "native run requires strict or hybrid mode"
        )
    if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", args.run_id) is None:
        raise HybridError(ErrorCode.POLICY_INVALID, "invalid native run ID")
    paths = [args.model, args.inputs, args.outputs, args.evidence]
    resolved = [path.resolve() for path in paths]
    if len(set(resolved)) != len(resolved):
        raise HybridError(
            ErrorCode.POLICY_INVALID, "model, input and output paths must differ"
        )
    package_root = args.package.resolve()
    if any(
        path == package_root or package_root in path.parents for path in resolved[2:]
    ):
        raise HybridError(
            ErrorCode.POLICY_INVALID, "outputs must be outside native package"
        )
    for output in (args.outputs, args.evidence):
        if output.exists() or output.is_symlink():
            raise HybridError(
                ErrorCode.OUTPUT_EXISTS, f"output already exists: {output}"
            )
    inputs = _load_npz(args.inputs)
    session = NativeSession(
        args.model, args.package, run_id=args.run_id, mode=ExecutionMode(args.mode)
    )
    outputs = session.run(inputs)
    evidence = {
        "schema_version": 1,
        "audit": session.audit.document,
        "execution": session.evidence,
    }
    _write_exclusive(args.outputs, deterministic_npz_bytes(outputs))
    try:
        _write_exclusive(args.evidence, canonical_json(evidence))
    except BaseException:
        args.outputs.unlink(missing_ok=True)
        raise
    print(f"wrote {args.outputs}")
    print(f"wrote {args.evidence}")
    return 0


def _command_generate_fixtures(args: argparse.Namespace) -> int:
    manifest = generate_fixtures(args.output_directory)
    print(
        f"wrote {len(manifest['models'])} models and "
        f"{len(manifest['tensor_sets'])} tensor sets to "
        f"{args.output_directory}"
    )
    return 0


def _command_verify_fixtures(args: argparse.Namespace) -> int:
    manifest = verify_fixtures(args.fixture_directory)
    print(
        f"verified {len(manifest['models'])} models and "
        f"{len(manifest['tensor_sets'])} tensor sets"
    )
    return 0


def _add_policy_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Fail-closed Wave ONNX audit and CPU fallback PoC"
    )
    parser.set_defaults(handler=None)
    subparsers = parser.add_subparsers(dest="command")

    audit = subparsers.add_parser(
        "audit", help="audit placement without creating a runtime session"
    )
    audit.add_argument("model", type=Path)
    audit.add_argument("--output", type=Path, required=True)
    _add_policy_arguments(audit)
    audit.set_defaults(handler=_command_audit)

    run = subparsers.add_parser(
        "run", help="execute CPU or fail-closed hybrid inference"
    )
    run.add_argument("model", type=Path)
    run.add_argument("--inputs", type=Path, required=True)
    run.add_argument(
        "--mode",
        choices=(
            ExecutionMode.CPU.value,
            ExecutionMode.HYBRID.value,
            ExecutionMode.STRICT.value,
        ),
        required=True,
    )
    run.add_argument("--outputs", type=Path, required=True)
    run.add_argument("--evidence", type=Path, required=True)
    run.add_argument("--backend", choices=("default", "wave-native"), default="default")
    run.add_argument("--package", type=Path)
    run.add_argument("--run-id")
    _add_policy_arguments(run)
    run.set_defaults(handler=_command_run)

    generate = subparsers.add_parser(
        "generate-fixtures", help="create deterministic fixed ONNX controls"
    )
    generate.add_argument("output_directory", type=Path)
    generate.set_defaults(handler=_command_generate_fixtures)

    verify = subparsers.add_parser(
        "verify-fixtures",
        help="regenerate and compare every fixture byte",
    )
    verify.add_argument("fixture_directory", type=Path)
    verify.set_defaults(handler=_command_verify_fixtures)
    add_native_commands(subparsers)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    if args.handler is None:
        parser.print_help()
        return 2
    try:
        return int(args.handler(args))
    except HybridError as exc:
        sys.stderr.write(
            json.dumps(
                {"error": exc.to_document()},
                ensure_ascii=False,
                sort_keys=True,
            )
            + "\n"
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
