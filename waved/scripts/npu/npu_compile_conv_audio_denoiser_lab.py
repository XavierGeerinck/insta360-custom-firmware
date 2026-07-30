#!/usr/bin/env python3
"""Fixed PoC 8C entry point for the convolutional denoiser compiler."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_compile_lab.main(
            policy=npu_compile_lab.CONV_AUDIO_DENOISER_POLICY
        )
    )
