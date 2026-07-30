#!/usr/bin/env python3
"""Fixed PoC 8D entry point for one Conv2d denoiser execution."""

from __future__ import annotations

import npu_audio_denoiser_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_audio_denoiser_lab.main(
            policy=npu_audio_denoiser_lab.POC8D_POLICY
        )
    )
