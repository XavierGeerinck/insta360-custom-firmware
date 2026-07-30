#!/usr/bin/env python3
"""Fixed PoC 14B affine-plus-Erf compile-only entry point."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(npu_compile_lab.main(policy=npu_compile_lab.ERF_AFFINE_POLICY))
