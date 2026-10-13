"""Explicit package lifecycle commands for the Wave native backend."""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any

from .errors import ErrorCode, HybridError
from .native_backend import native_bridge


def _invoke(operation: str, *arguments: Any) -> int:
    try:
        getattr(native_bridge(), operation)(*arguments)
    except HybridError:
        raise
    except Exception as exc:
        raise HybridError(
            ErrorCode.MODEL_PACKAGE_INVALID,
            f"native {operation} failed: {exc}",
        ) from exc
    return 0


def _import(args: argparse.Namespace) -> int:
    return _invoke("import_package", args.model, args.package_dir)


def _build(args: argparse.Namespace) -> int:
    return _invoke("build_package", args.package_dir)


def _compile(args: argparse.Namespace) -> int:
    return _invoke("compile_package", args.package_dir, args.run_id)


def _qualify(args: argparse.Namespace) -> int:
    return _invoke("qualify_package", args.package_dir, args.inputs, args.run_id)


def add_native_commands(subparsers: Any) -> None:
    imported = subparsers.add_parser(
        "import-native", help="import one ONNX native package"
    )
    imported.add_argument("model", type=Path)
    imported.add_argument("--package-dir", type=Path, required=True)
    imported.set_defaults(handler=_import)

    built = subparsers.add_parser("build-native", help="build a native package offline")
    built.add_argument("package_dir", type=Path)
    built.set_defaults(handler=_build)

    compiled = subparsers.add_parser("compile-native", help="compile one native NBG")
    compiled.add_argument("package_dir", type=Path)
    compiled.add_argument("--run-id", required=True)
    compiled.set_defaults(handler=_compile)

    qualified = subparsers.add_parser("qualify-native", help="qualify one native NBG")
    qualified.add_argument("package_dir", type=Path)
    qualified.add_argument("--inputs", type=Path, required=True)
    qualified.add_argument("--run-id", required=True)
    qualified.set_defaults(handler=_qualify)
