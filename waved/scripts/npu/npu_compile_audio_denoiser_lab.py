#!/usr/bin/env python3
"""Fixed PoC 8A entry point for the audio-denoiser compiler probe."""

from __future__ import annotations

import npu_compile_lab


if __name__ == "__main__":
    raise SystemExit(
        npu_compile_lab.main(
            policy=npu_compile_lab.AUDIO_DENOISER_POLICY
        )
    )
