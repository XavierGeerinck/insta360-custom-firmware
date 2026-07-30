#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
    echo "usage: $0 <device-binary> <source>" >&2
    exit 2
fi

BINARY="$1"
SOURCE="$2"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

case "$BINARY" in
    "$REPOSITORY_ROOT"/artifacts/npu/build/poc5b/*) ;;
    *)
        echo "policy: binary must be in the fixed PoC 5B build directory" >&2
        exit 3
        ;;
esac

if rg -n -- '(^|[^[:alnum:]_])Run[[:space:]]*\(' "$SOURCE"; then
    echo "policy: source contains a Run call" >&2
    exit 4
fi
COMPILE_CALLS="$(
    rg -o -- 'CompileToBinary[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' '
)"
if [[ "$COMPILE_CALLS" != "2" ]]; then
    echo "policy: source must contain exactly two CompileToBinary calls" >&2
    exit 4
fi
if rg -n -- 'ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(' "$SOURCE"; then
    echo "policy: source contains a forbidden direct-runtime primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc5b/wave-npu-compile-add"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libtim-vx.so]" <<<"$dynamic"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-compile/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"

        if grep -Eq \
            "tim::vx::Graph::Run|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an execution primitive" >&2
            exit 6
        fi
    '

echo "device compiler policy: PASS"
