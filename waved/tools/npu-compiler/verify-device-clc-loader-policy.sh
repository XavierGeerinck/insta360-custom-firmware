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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc5f/wave-npu-clc-loader-probe-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_clc_loader_probe.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 5F-A paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 5F-A binary or source is missing" >&2
    exit 3
fi

if [[ "$(rg -o -- 'dlopen[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one dlopen call" >&2
    exit 4
fi
if [[ "$(rg -o -- 'dlsym[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "3" ]]; then
    echo "policy: source must contain exactly three dlsym calls" >&2
    exit 4
fi
if [[ "$(rg -o -- 'dlclose[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "3" ]]; then
    echo "policy: source must contain exactly three guarded dlclose calls" >&2
    exit 4
fi
for fixed_value in \
    /tmp/wave-npu-compile/lib/libCLC.so \
    gcLoadKernelCompiler \
    gcCompileKernel \
    gcUnloadKernelCompiler; do
    if ! rg -Fq -- "$fixed_value" "$SOURCE"; then
        echo "policy: fixed loader value missing: $fixed_value" >&2
        exit 4
    fi
done
if rg -n -- \
    'vx(Create|Process|Schedule)|CreateGraph|CompileToBinary|(^|[^[:alnum:]_])Run[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|(^|[^[:alnum:]_])open[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains a graph, execution, or device primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc5f/wave-npu-clc-loader-probe-v1"
        clc="waved/third_party/vip9000-sdk/6.4.18.5/drivers/libCLC.so"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(aarch64-linux-gnu-nm -D --undefined-only "$binary")"
        clc_dynamic="$(aarch64-linux-gnu-readelf -d "$clc")"
        clc_exports="$(aarch64-linux-gnu-nm -D --defined-only "$clc")"
        clc_imports="$(aarch64-linux-gnu-nm -D --undefined-only "$clc")"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-compile/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Eq "[[:space:]]dlopen(@|$)" <<<"$imports"
        grep -Eq "[[:space:]]dlsym(@|$)" <<<"$imports"
        grep -Eq "[[:space:]]dlclose(@|$)" <<<"$imports"
        if grep -Eq \
            "libtim-vx|libOpenVX|libovxlib|vx(Process|Schedule)Graph|[[:space:]]ioctl(@|$)" \
            <<<"$dynamic"$'\''\n'\''"$imports"; then
            echo "policy: loader probe links a graph/runtime primitive" >&2
            exit 6
        fi

        for dependency in \
            libVSC.so libGAL.so libm.so.6 libpthread.so.0 libdl.so.2 \
            librt.so.1 libc.so.6; do
            grep -Fq "Shared library: [$dependency]" <<<"$clc_dynamic"
        done
        for symbol in \
            gcLoadKernelCompiler gcCompileKernel gcUnloadKernelCompiler; do
            grep -Eq "[[:space:]]$symbol$" <<<"$clc_exports"
        done

        imported="$(mktemp)"
        exported="$(mktemp)"
        awk "{print \$NF}" <<<"$clc_imports" |
            sed "s/@.*//" |
            sort -u >"$imported"
        for library in \
            waved/third_party/device-sdk/lib/libVSC.so \
            waved/third_party/device-sdk/lib/libGAL.so; do
            aarch64-linux-gnu-nm -D --defined-only "$library"
        done |
            awk "{print \$NF}" |
            sed "s/@.*//" |
            sort -u >"$exported"
        if comm -23 "$imported" "$exported" | grep -Eq "^(gc|gco|vsc)"; then
            echo "policy: CLC has unresolved vendor imports" >&2
            exit 7
        fi
    '

echo "device CLC loader policy: PASS"
