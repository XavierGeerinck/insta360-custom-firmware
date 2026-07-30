// SPDX-License-Identifier: MIT
//
// PoC 13D: execute one sealed UINT8[481] Erf NBG against exactly six
// qualification vectors. There is no configurable model, vector, tensor,
// operation, iteration, output-file, retry, recovery, or direct-device
// interface.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/nbg.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <vector>

struct ChipIdentityEx {
    uint32_t chip_model;
    uint32_t chip_revision;
    uint32_t chip_product_id;
    uint32_t customer_id;
    uint32_t chip_eco_id;
    uint32_t reserved;
    uint64_t chip_flags;
};

static_assert(sizeof(ChipIdentityEx) == 0x20, "unexpected identity ABI");

extern "C" int gcoHAL_QueryChipIdentityEx(
    void* hal,
    uint32_t identity_size,
    ChipIdentityEx* identity);

namespace {

constexpr size_t kVectorCount = 6;
constexpr size_t kVectorWidth = 481;
constexpr size_t kVectorHeaderBytes = 16;
constexpr size_t kExpectedVectorPackBytes =
    kVectorHeaderBytes + kVectorCount * kVectorWidth;
constexpr size_t kExpectedNbgBytes = 5840;
constexpr size_t kCanaryBytes = 32;
constexpr uint8_t kCanaryValue = 0xa5;
constexpr uint64_t kFnvOffsetBasis = UINT64_C(0xcbf29ce484222325);
constexpr uint64_t kFnvPrime = UINT64_C(0x100000001b3);
constexpr uint64_t kExpectedVectorPackFnv =
    UINT64_C(0x8f90178b58ca8afb);
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 20;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc13d/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-poc13d/model/erf-mask.nbg";
constexpr char kVectorPackPath[] =
    "/tmp/wave-npu-poc13d/input/qualification-vectors.bin";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-poc13d/lib/libCLC.so";
constexpr std::array<const char*, kVectorCount> kVectorIds{
    "zero-point",
    "minimum",
    "maximum",
    "repeated-ramp",
    "seeded-random",
    "learned-fixture",
};

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "ERF_MASK status=TIMEOUT mode=qualify-six exit=124\n";
    const ssize_t message_bytes =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)message_bytes;
    _exit(124);
}

uint32_t ReadLe32(const uint8_t* data) {
    return static_cast<uint32_t>(data[0]) |
           (static_cast<uint32_t>(data[1]) << 8U) |
           (static_cast<uint32_t>(data[2]) << 16U) |
           (static_cast<uint32_t>(data[3]) << 24U);
}

uint64_t Fnv1a64(const uint8_t* data, size_t bytes) {
    uint64_t value = kFnvOffsetBasis;
    for (size_t index = 0; index < bytes; ++index) {
        value ^= data[index];
        value *= kFnvPrime;
    }
    return value;
}

bool PathIsAbsent(const char* path) {
    struct stat path_stat {};
    if (lstat(path, &path_stat) == 0) {
        return false;
    }
    return errno == ENOENT;
}

bool EnvironmentIsFixed() {
    const char* const library_path = std::getenv("LD_LIBRARY_PATH");
    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    if (library_path == nullptr ||
        std::strcmp(library_path, kExpectedLibraryPath) != 0 ||
        sdk_directory == nullptr ||
        std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: fixed environment "
            "mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: interposition and "
            "simulator variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: isolated CLC path "
            "must be absent\n");
        return false;
    }
    return true;
}

bool ReadFixedFile(
    const char* path,
    size_t expected_bytes,
    std::vector<uint8_t>* contents) {
    const int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) {
        std::fprintf(stderr, "open(%s): %s\n", path, std::strerror(errno));
        return false;
    }
    struct stat file_stat {};
    if (fstat(fd, &file_stat) != 0 ||
        !S_ISREG(file_stat.st_mode) ||
        (file_stat.st_mode & 0777) != 0444 ||
        file_stat.st_size != static_cast<off_t>(expected_bytes)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: fixed file metadata "
            "mismatch: %s\n",
            path);
        close(fd);
        return false;
    }

    contents->assign(expected_bytes, 0);
    size_t offset = 0;
    while (offset < contents->size()) {
        const ssize_t count = read(
            fd, contents->data() + offset, contents->size() - offset);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(
                stderr, "read(%s): %s\n", path, std::strerror(errno));
            close(fd);
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: short fixed-file "
                "read: %s\n",
                path);
            close(fd);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    uint8_t extra = 0;
    if (read(fd, &extra, 1) != 0 || close(fd) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: fixed-file "
            "close/length check failed: %s\n",
            path);
        return false;
    }
    return true;
}

bool NbgIsFixed(const std::vector<uint8_t>& nbg) {
    return nbg.size() == kExpectedNbgBytes &&
           std::memcmp(nbg.data(), "VPMN", 4) == 0 &&
           ReadLe32(nbg.data() + 4) == kWaveNbgVersion &&
           ReadLe32(nbg.data() + 8) == kWaveProductId;
}

bool VectorPackIsFixed(const std::vector<uint8_t>& pack) {
    return pack.size() == kExpectedVectorPackBytes &&
           std::memcmp(pack.data(), "ERF1", 4) == 0 &&
           ReadLe32(pack.data() + 4) == 1 &&
           ReadLe32(pack.data() + 8) == kVectorCount &&
           ReadLe32(pack.data() + 12) == kVectorWidth &&
           Fnv1a64(pack.data(), pack.size()) == kExpectedVectorPackFnv;
}

uint64_t ElapsedMicroseconds(
    const struct timespec& started,
    const struct timespec& finished) {
    const int64_t seconds =
        static_cast<int64_t>(finished.tv_sec) -
        static_cast<int64_t>(started.tv_sec);
    const int64_t nanoseconds =
        static_cast<int64_t>(finished.tv_nsec) -
        static_cast<int64_t>(started.tv_nsec);
    return static_cast<uint64_t>(
        seconds * INT64_C(1000000) + nanoseconds / INT64_C(1000));
}

bool CanariesAreIntact(
    const std::array<uint8_t, kVectorWidth + 2 * kCanaryBytes>& buffer) {
    return std::all_of(
               buffer.begin(),
               buffer.begin() + kCanaryBytes,
               [](uint8_t value) { return value == kCanaryValue; }) &&
           std::all_of(
               buffer.end() - kCanaryBytes,
               buffer.end(),
               [](uint8_t value) { return value == kCanaryValue; });
}

void PrintHex(const uint8_t* data, size_t bytes) {
    for (size_t index = 0; index < bytes; ++index) {
        std::printf("%02x", static_cast<unsigned int>(data[index]));
    }
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"qualify-six\","
        "\"model\":\"/tmp/wave-npu-poc13d/model/erf-mask.nbg\","
        "\"model_bytes\":5840,\"model_sha256\":"
        "\"96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94\","
        "\"vectors\":\"/tmp/wave-npu-poc13d/input/"
        "qualification-vectors.bin\",\"vector_pack_bytes\":2902,"
        "\"vector_pack_sha256\":"
        "\"3374b928722c7d504e9ae8d8e1e99c1dc7b3542ee08cbede60b2f8bb8f93a232\","
        "\"vector_pack_fnv1a64\":\"0x8f90178b58ca8afb\","
        "\"operation\":\"NBG\",\"input\":{\"type\":\"UINT8\","
        "\"shape\":[481],\"scale\":0.03125,\"zero_point\":128},"
        "\"output\":{\"type\":\"UINT8\",\"shape\":[481],"
        "\"scale\":0.0078125,\"zero_point\":128},\"vectors\":6,"
        "\"compile_count\":1,\"run_count\":6,\"output_canary_bytes\":32,"
        "\"executes_graph\":true}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--qualify-six") != 0) {
        std::fprintf(stderr, "usage: %s --qualify-six\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: parent-death gate "
            "failed\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: watchdog setup failed: "
            "%s\n",
            std::strerror(errno));
        return 18;
    }
    alarm(kWatchdogSeconds);

    std::vector<uint8_t> nbg;
    std::vector<uint8_t> vector_pack;
    if (!ReadFixedFile(kNbgPath, kExpectedNbgBytes, &nbg) ||
        !NbgIsFixed(nbg)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: fixed NBG rejected\n");
        return 13;
    }
    if (!ReadFixedFile(
            kVectorPackPath,
            kExpectedVectorPackBytes,
            &vector_pack) ||
        !VectorPackIsFixed(vector_pack)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: fixed vector pack "
            "rejected\n");
        return 14;
    }

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: context create "
            "failed\n");
        return 1;
    }

    ChipIdentityEx identity {};
    const int identity_status = gcoHAL_QueryChipIdentityEx(
        nullptr,
        static_cast<uint32_t>(sizeof(identity)),
        &identity);
    if (identity_status != 0 ||
        identity.chip_model != kWaveChipModel ||
        identity.customer_id != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: identity mismatch\n");
        return 15;
    }
    std::printf(
        "IDENTITY chip_model=0x%08x chip_revision=0x%08x "
        "chip_product_id=0x%08x chip_eco_id=0x%08x "
        "customer_id=0x%08x reserved=0x%08x flags=0x%016llx\n",
        identity.chip_model,
        identity.chip_revision,
        identity.chip_product_id,
        identity.chip_eco_id,
        identity.customer_id,
        identity.reserved,
        static_cast<unsigned long long>(identity.chip_flags));
    std::fflush(stdout);

    auto graph = context->CreateGraph();
    if (!graph) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType shape{481};
    const tim::vx::Quantization input_quantization(
        tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128);
    const tim::vx::Quantization output_quantization(
        tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128);
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::INPUT,
        input_quantization);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::OUTPUT,
        output_quantization);
    auto input = graph->CreateTensor(input_spec);
    auto output = graph->CreateTensor(output_spec);
    if (!input || !output) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: tensor create failed\n");
        return 3;
    }

    auto nbg_operation = graph->CreateOperation<tim::vx::ops::NBG>(
        reinterpret_cast<const char*>(nbg.data()), 1, 1);
    if (!nbg_operation) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_qualification: NBG operation "
            "failed\n");
        return 4;
    }
    nbg_operation->BindInput(input).BindOutput(output);

    bool compiled = false;
    for (size_t vector_index = 0;
         vector_index < kVectorCount;
         ++vector_index) {
        const uint8_t* const input_bytes =
            vector_pack.data() + kVectorHeaderBytes +
            vector_index * kVectorWidth;
        if (!input->CopyDataToTensor(input_bytes, kVectorWidth)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: input copy failed "
                "at vector %zu\n",
                vector_index);
            return 5;
        }
        if (!compiled) {
            if (!graph->Compile()) {
                std::fprintf(
                    stderr,
                    "wave_npu_run_erf_mask_qualification: graph compile "
                    "failed\n");
                return 6;
            }
            compiled = true;
        }

        struct timespec started {};
        struct timespec finished {};
        if (clock_gettime(CLOCK_MONOTONIC, &started) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: start clock "
                "failed\n");
            return 19;
        }
        if (!graph->Run()) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: graph run failed "
                "at vector %zu\n",
                vector_index);
            return 7;
        }
        if (clock_gettime(CLOCK_MONOTONIC, &finished) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: finish clock "
                "failed\n");
            return 19;
        }

        std::array<uint8_t, kVectorWidth + 2 * kCanaryBytes> observed {};
        observed.fill(kCanaryValue);
        uint8_t* const output_bytes = observed.data() + kCanaryBytes;
        if (!output->CopyDataFromTensor(output_bytes)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: output copy failed "
                "at vector %zu\n",
                vector_index);
            return 8;
        }
        if (!CanariesAreIntact(observed)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_qualification: output canary "
                "changed at vector %zu\n",
                vector_index);
            return 9;
        }

        std::printf(
            "VECTOR index=%zu id=%s duration_us=%llu "
            "input_fnv1a64=0x%016llx output_fnv1a64=0x%016llx "
            "output_hex=",
            vector_index,
            kVectorIds[vector_index],
            static_cast<unsigned long long>(
                ElapsedMicroseconds(started, finished)),
            static_cast<unsigned long long>(
                Fnv1a64(input_bytes, kVectorWidth)),
            static_cast<unsigned long long>(
                Fnv1a64(output_bytes, kVectorWidth)));
        PrintHex(output_bytes, kVectorWidth);
        std::putchar('\n');
        std::fflush(stdout);
    }

    nbg_operation.reset();
    output.reset();
    input.reset();
    graph.reset();
    context.reset();
    alarm(0);

    std::puts(
        "ERF_MASK status=PASS mode=qualify-six vectors=6 "
        "bytes_per_vector=481 compile_count=1 run_count=6 "
        "canaries=PASS");
    return 0;
}
