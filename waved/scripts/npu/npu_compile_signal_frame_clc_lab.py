#!/usr/bin/env python3
"""Fixed PoC 5F-B Wave EVIS SignalFrame compiler probe with CLC."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_compile_lab.main(
            policy=npu_compile_lab.SIGNAL_FRAME_CLC_POLICY
        )
    )
