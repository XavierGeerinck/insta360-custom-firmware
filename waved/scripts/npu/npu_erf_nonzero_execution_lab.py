#!/usr/bin/env python3
"""Fixed PoC 7 entry point for nonzero Erf NBG characterization."""

from __future__ import annotations

import npu_erf_execution_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_erf_execution_lab.main(
            policy=npu_erf_execution_lab.POC7_POLICY
        )
    )
