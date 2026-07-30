#!/usr/bin/env python3
"""Fixed PoC 13B entry point for the Wave Erf-mask compiler capture."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_compile_lab.main(policy=npu_compile_lab.ERF_MASK_POLICY)
    )
