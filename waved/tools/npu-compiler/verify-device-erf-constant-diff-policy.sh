#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
    echo "usage: $0 <device-binary> <source>" >&2
    exit 2
fi

BINARY="$1"
SOURCE="$2"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc12b-erf-constant-diff/wave-npu-compile-erf-constant-diff-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/tim_vx_compile_erf_constant_diff.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 12B paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 12B binary or source is missing" >&2
    exit 3
fi

count_source() {
    local expression="$1"
    grep -E -o -- "$expression" "$SOURCE" | wc -l | tr -d ' '
}

if grep -E -n -- '(^|[^[:alnum:]_])Run[[:space:]]*\(' "$SOURCE"; then
    echo "policy: graph execution call found" >&2
    exit 4
fi
if grep -E -n -- \
    'CopyDataToTensor[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|system[[:space:]]*\(|(^|[^[:alnum:]_])(fork|vfork|execv|execve|execl|execlp|execvp)[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: forbidden runtime primitive found" >&2
    exit 4
fi
if [[ "$(count_source 'CompileToBinary[[:space:]]*\(')" != "2" ]]; then
    echo "policy: expected exactly two CompileToBinary call sites" >&2
    exit 4
fi
if [[ "$(count_source 'CreateOperation<tim::vx::ops::Erf>[[:space:]]*\(')" != "1" ]]; then
    echo "policy: expected exactly one Erf operation" >&2
    exit 4
fi
if grep -E -n -- \
    'CreateOperation<tim::vx::ops::(Add|Conv|Custom|Prelu)' \
    "$SOURCE"; then
    echo "policy: alternate operation found" >&2
    exit 4
fi
if [[ "$(count_source 'TensorAttribute::CONSTANT')" != "1" ]]; then
    echo "policy: expected exactly one constant tensor declaration" >&2
    exit 4
fi
if [[ "$(count_source 'BindInput[[:space:]]*\(')" != "1" ||
      "$(count_source 'BindOutput[[:space:]]*\(')" != "1" ]]; then
    echo "policy: expected exactly one input/output binding" >&2
    exit 4
fi
if ! grep -Fq -- 'constexpr std::array<uint8_t, 4> kConstantA{{0, 1, 2, 3}};' "$SOURCE" ||
   ! grep -Fq -- 'constexpr std::array<uint8_t, 4> kConstantB{{0, 1, 2, 4}};' "$SOURCE"; then
    echo "policy: fixed A/A/B source constants changed" >&2
    exit 4
fi
if ! grep -Fq -- 'const tim::vx::ShapeType shape{4};' "$SOURCE" ||
   ! grep -Fq -- 'constexpr unsigned int kWatchdogSeconds = 15;' "$SOURCE" ||
   ! grep -Fq -- 'constexpr size_t kMaximumNbgBytes = 64U * 1024U * 1024U;' "$SOURCE"; then
    echo "policy: fixed graph/watchdog/size bound changed" >&2
    exit 4
fi
for mode in --a1 --a2 --b --describe; do
    if ! grep -Fq -- "$mode" "$SOURCE"; then
        echo "policy: fixed mode missing: $mode" >&2
        exit 4
    fi
done
for fixed_path in \
    /tmp/wave-npu-poc12b/output/erf-constant-a1.nbg \
    /tmp/wave-npu-poc12b/output/erf-constant-a2.nbg \
    /tmp/wave-npu-poc12b/output/erf-constant-b.nbg \
    /tmp/wave-npu-poc12b/output/.erf-constant-a1.nbg.partial \
    /tmp/wave-npu-poc12b/output/.erf-constant-a2.nbg.partial \
    /tmp/wave-npu-poc12b/output/.erf-constant-b.nbg.partial; do
    if ! grep -Fq -- "$fixed_path" "$SOURCE"; then
        echo "policy: fixed output path missing: $fixed_path" >&2
        exit 4
    fi
done
unexpected_temp_paths="$(
    grep -E -o -- '"/tmp/[^"]*"' "$SOURCE" |
        grep -F -v -- '"/tmp/wave-npu-poc12b' ||
        true
)"
if [[ -n "$unexpected_temp_paths" ]]; then
    echo "policy: unapproved temporary path found" >&2
    printf '%s\n' "$unexpected_temp_paths" >&2
    exit 4
fi

READELF="${READELF:-aarch64-linux-gnu-readelf}"
NM="${NM:-aarch64-linux-gnu-nm}"
CXXFILT="${CXXFILT:-c++filt}"
if ! command -v "$CXXFILT" >/dev/null 2>&1; then
    echo "policy: required demangler is unavailable: $CXXFILT" >&2
    exit 5
fi

if command -v "$READELF" >/dev/null 2>&1 &&
   command -v "$NM" >/dev/null 2>&1; then
    header="$("$READELF" -h "$BINARY")"
    dynamic="$("$READELF" -d "$BINARY")"
    program_headers="$("$READELF" -W -l "$BINARY")"
    imports="$("$NM" -D --undefined-only "$BINARY" | "$CXXFILT")"

    grep -Fq 'Class:                             ELF64' <<<"$header"
    grep -Fq 'Machine:                           AArch64' <<<"$header"
    grep -Fq 'Type:                              DYN' <<<"$header"
    grep -Fq \
        'Library runpath: [/tmp/wave-npu-poc12b/lib:/usr/lib]' \
        <<<"$dynamic"
    grep -Fq 'BIND_NOW' <<<"$dynamic"
    grep -Fq 'GNU_RELRO' <<<"$program_headers"
    needed="$(
        sed -n 's/.*Shared library: \[\(.*\)\]/\1/p' \
            <<<"$dynamic" |
            sort
    )"
elif command -v objdump >/dev/null 2>&1 &&
     command -v file >/dev/null 2>&1; then
    header="$(file "$BINARY")"
    dynamic="$(objdump -p "$BINARY")"
    program_headers="$dynamic"
    imports="$(objdump -T "$BINARY" | "$CXXFILT")"

    grep -Fq 'ELF 64-bit LSB pie executable, ARM aarch64' <<<"$header"
    grep -Fq 'RUNPATH      /tmp/wave-npu-poc12b/lib:/usr/lib' \
        <<<"$dynamic"
    grep -Fq 'FLAGS        0x0000000000000008' <<<"$dynamic"
    grep -Fq 'FLAGS_1      0x0000000008000001' <<<"$dynamic"
    grep -Fq 'RELRO off' <<<"$program_headers"
    stack_flags="$(
        awk '$1 == "STACK" {getline; print; exit}' \
            <<<"$program_headers"
    )"
    if [[ -z "$stack_flags" ]] ||
       grep -E -q 'flags .*x' <<<"$stack_flags"; then
        echo "policy: executable stack found" >&2
        exit 6
    fi
    needed="$(
        awk '$1 == "NEEDED" {print $2}' <<<"$dynamic" | sort
    )"
else
    echo "policy: no supported ELF inspection toolchain is available" >&2
    exit 5
fi

grep -Fq 'gcoHAL_QueryChipIdentityEx' <<<"$imports"
grep -Fq 'tim::vx::ops::Erf::Erf(tim::vx::Graph*)' <<<"$imports"

expected_needed="$(
    printf '%s\n' \
        ld-linux-aarch64.so.1 \
        libc.so.6 \
        libGAL.so \
        libgcc_s.so.1 \
        libpthread.so.0 \
        libstdc++.so.6 \
        libtim-vx.so |
        sort
)"
if [[ "$needed" != "$expected_needed" ]]; then
    echo "policy: DT_NEEDED set differs from the fixed allowlist" >&2
    printf 'observed:\n%s\n' "$needed" >&2
    exit 6
fi
if grep -E -q \
    'tim::vx::Graph::Run|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)|[[:space:]](system|fork|vfork|execv|execve|execl)(@|$)' \
    <<<"$imports"; then
    echo "policy: executable imports a prohibited primitive" >&2
    exit 6
fi
if grep -a -Fq 'libCLC.so' "$BINARY"; then
    echo "policy: executable references quarantined libCLC.so" >&2
    exit 6
fi

echo "device Erf constant-differential policy: PASS"
