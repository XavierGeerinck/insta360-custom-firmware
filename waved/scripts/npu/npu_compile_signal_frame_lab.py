#!/usr/bin/env python3
"""Fixed PoC 5E entry point for the Wave EVIS SignalFrame compiler probe."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_compile_lab.main(policy=npu_compile_lab.SIGNAL_FRAME_POLICY)
    )
