#!/usr/bin/env bash
# Run cargo inside a native arm64 Debian container (OrbStack/Docker).
# macOS can't load the sqlx-macros dylib built by the Xcode 27 linker, so
# waved-meeting and the waved binary are built and tested here instead.
#
# Usage: scripts/linux-test.sh <cargo args...>
#   scripts/linux-test.sh test -p waved-meeting --lib
#   scripts/linux-test.sh check -p waved --features ui,capture-alsa
set -euo pipefail
WAVED_DIR="${WAVED_DIR:-$(cd "$(dirname "$0")/.." && pwd)}"
IMAGE="${WAVED_LINUX_IMAGE:-rust:1-bookworm}"
docker run --rm \
  -v "$WAVED_DIR":/work -w /work \
  -v waved-cargo-registry:/usr/local/cargo/registry \
  -v waved-cargo-git:/usr/local/cargo/git \
  -e CARGO_TARGET_DIR=/work/target-linux \
  -e RUSTC_WRAPPER= \
  "$IMAGE" bash -c '
    set -euo pipefail
    if ! dpkg -s libclang-dev libasound2-dev >/dev/null 2>&1; then
      apt-get update -qq && apt-get install -y -qq libclang-dev libasound2-dev pkg-config >/dev/null
    fi
    cargo "$@"' _ "$@"
