#!/bin/sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPOSITORY_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc3"
OUTPUT_FILE="$OUTPUT_DIRECTORY/libwave-gal-trace.so"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main-centos@sha256:59a28167cffea49e71e1951bcaf9105d6a31dcf130c95197e2faff8fbf032508"

mkdir -p "$OUTPUT_DIRECTORY"

docker run --rm \
    --platform linux/amd64 \
    --user "$(id -u):$(id -g)" \
    --volume "$REPOSITORY_ROOT:/work:ro" \
    --volume "$OUTPUT_DIRECTORY:/out" \
    --workdir /work \
    "$IMAGE" \
    aarch64-linux-gnu-gcc \
    -std=gnu11 \
    -shared \
    -fPIC \
    -O2 \
    -g0 \
    -fno-ident \
    -fvisibility=hidden \
    -D_FORTIFY_SOURCE=2 \
    -Wall \
    -Wextra \
    -Werror \
    -Wconversion \
    -Wformat=2 \
    -Wshadow \
    -Wstrict-prototypes \
    -Wl,-z,relro \
    -Wl,-z,now \
    -Wl,-s \
    -Wl,--build-id=sha1 \
    -o /out/libwave-gal-trace.so \
    /work/waved/tools/npu-trace/gal_trace.c

chmod 0644 "$OUTPUT_FILE"
sha256sum "$OUTPUT_FILE"
