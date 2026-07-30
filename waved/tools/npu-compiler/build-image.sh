#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="wave-npu-compiler-toolchain:ubuntu22.04-amd64"

docker build \
    --platform=linux/amd64 \
    --tag "$IMAGE" \
    "$SCRIPT_DIR"

docker image inspect \
    --format 'image_id={{.Id}} repo_digests={{json .RepoDigests}}' \
    "$IMAGE"
