// SPDX-License-Identifier: MIT
//
// PoC 13E: execute the sealed UINT8[481] Erf NBG over exactly 1,201
// full-fixture frames and write one fixed, read-only output pack. There is no
// configurable model, input, output, tensor, iteration, retry, recovery, or
// direct-device interface.

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

constexpr size_t kFrameCount = 1201;
constexpr size_t kFrameWidth = 481;
constexpr size_t kStreamHeaderBytes = 16;
constexpr size_t kExpectedStreamBytes =
    kStreamHeaderBytes + kFrameCount * kFrameWidth;
constexpr size_t kExpectedNbgBytes = 5840;
constexpr size_t kCanaryBytes = 32;
constexpr uint8_t kCanaryValue = 0xa5;
constexpr uint64_t kFnvOffsetBasis = UINT64_C(0xcbf29ce484222325);
constexpr uint64_t kFnvPrime = UINT64_C(0x100000001b3);
constexpr uint64_t kExpectedInputStreamFnv =
    UINT64_C(0xe2c6f1eda5f3eaea);
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 30;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc13e/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-poc13e/model/erf-mask.nbg";
constexpr char kInputPath[] =
    "/tmp/wave-npu-poc13e/input/erf-stream-input.bin";
constexpr char kOutputPath[] =
    "/tmp/wave-npu-poc13e/output/erf-stream-output.bin";
constexpr char kPartialOutputPath[] =
    "/tmp/wave-npu-poc13e/output/.erf-stream-output.bin.partial";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-poc13e/lib/libCLC.so";

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "ERF_STREAM status=TIMEOUT mode=fixture-stream exit=124\n";
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
            "wave_npu_run_erf_mask_stream: fixed environment mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: interposition and simulator "
            "variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: isolated CLC path must be "
            "absent\n");
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
            "wave_npu_run_erf_mask_stream: fixed file metadata mismatch: "
            "%s\n",
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
                "wave_npu_run_erf_mask_stream: short read: %s\n",
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
            "wave_npu_run_erf_mask_stream: close/length check failed: "
            "%s\n",
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

bool InputStreamIsFixed(const std::vector<uint8_t>& stream) {
    return stream.size() == kExpectedStreamBytes &&
           std::memcmp(stream.data(), "ERI1", 4) == 0 &&
           ReadLe32(stream.data() + 4) == 1 &&
           ReadLe32(stream.data() + 8) == kFrameCount &&
           ReadLe32(stream.data() + 12) == kFrameWidth &&
           Fnv1a64(stream.data(), stream.size()) ==
               kExpectedInputStreamFnv;
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
    const std::array<uint8_t, kFrameWidth + 2 * kCanaryBytes>& buffer) {
    return std::all_of(
               buffer.begin(),
               buffer.begin() + kCanaryBytes,
               [](uint8_t value) { return value == kCanaryValue; }) &&
           std::all_of(
               buffer.end() - kCanaryBytes,
               buffer.end(),
               [](uint8_t value) { return value == kCanaryValue; });
}

bool WriteFixedOutput(const std::vector<uint8_t>& output) {
    if (!PathIsAbsent(kOutputPath) ||
        !PathIsAbsent(kPartialOutputPath)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: fixed output already exists\n");
        return false;
    }
    const int fd = open(
        kPartialOutputPath,
        O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
        0444);
    if (fd < 0) {
        std::fprintf(
            stderr,
            "open(%s): %s\n",
            kPartialOutputPath,
            std::strerror(errno));
        return false;
    }
    size_t offset = 0;
    while (offset < output.size()) {
        const ssize_t count = write(
            fd, output.data() + offset, output.size() - offset);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(
                stderr,
                "write(%s): %s\n",
                kPartialOutputPath,
                std::strerror(errno));
            close(fd);
            unlink(kPartialOutputPath);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    if (fsync(fd) != 0 || close(fd) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: output sync/close failed\n");
        unlink(kPartialOutputPath);
        return false;
    }
    if (link(kPartialOutputPath, kOutputPath) != 0 ||
        unlink(kPartialOutputPath) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: output publish failed\n");
        unlink(kPartialOutputPath);
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"fixture-stream\","
        "\"model\":\"/tmp/wave-npu-poc13e/model/erf-mask.nbg\","
        "\"model_bytes\":5840,\"model_sha256\":"
        "\"96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94\","
        "\"input\":\"/tmp/wave-npu-poc13e/input/"
        "erf-stream-input.bin\",\"input_bytes\":577697,"
        "\"input_sha256\":"
        "\"a409686c2482b576288b6f46ecf77d3b99b019c9c436e0f042cb1ac39dacc3e9\","
        "\"input_fnv1a64\":\"0xe2c6f1eda5f3eaea\","
        "\"output\":\"/tmp/wave-npu-poc13e/output/"
        "erf-stream-output.bin\",\"operation\":\"NBG\","
        "\"input_tensor\":{\"type\":\"UINT8\",\"shape\":[481],"
        "\"scale\":0.03125,\"zero_point\":128},"
        "\"output_tensor\":{\"type\":\"UINT8\",\"shape\":[481],"
        "\"scale\":0.0078125,\"zero_point\":128},\"frames\":1201,"
        "\"compile_count\":1,\"run_count\":1201,"
        "\"output_canary_bytes\":32,\"executes_graph\":true}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--fixture-stream") != 0) {
        std::fprintf(stderr, "usage: %s --fixture-stream\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: parent-death gate failed\n");
        return 17;
    }
    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    alarm(kWatchdogSeconds);

    std::vector<uint8_t> nbg;
    std::vector<uint8_t> input_stream;
    if (!ReadFixedFile(kNbgPath, kExpectedNbgBytes, &nbg) ||
        !NbgIsFixed(nbg)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: fixed NBG rejected\n");
        return 13;
    }
    if (!ReadFixedFile(
            kInputPath, kExpectedStreamBytes, &input_stream) ||
        !InputStreamIsFixed(input_stream)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: fixed input stream rejected\n");
        return 14;
    }
    if (!PathIsAbsent(kOutputPath) ||
        !PathIsAbsent(kPartialOutputPath)) {
        std::fprintf(
            stderr,
            "wave_npu_run_erf_mask_stream: output precondition failed\n");
        return 16;
    }

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr, "wave_npu_run_erf_mask_stream: context failed\n");
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
            "wave_npu_run_erf_mask_stream: identity mismatch\n");
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
            stderr, "wave_npu_run_erf_mask_stream: graph failed\n");
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
            stderr, "wave_npu_run_erf_mask_stream: tensors failed\n");
        return 3;
    }
    auto nbg_operation = graph->CreateOperation<tim::vx::ops::NBG>(
        reinterpret_cast<const char*>(nbg.data()), 1, 1);
    if (!nbg_operation) {
        std::fprintf(
            stderr, "wave_npu_run_erf_mask_stream: NBG op failed\n");
        return 4;
    }
    nbg_operation->BindInput(input).BindOutput(output);

    std::vector<uint8_t> output_stream(kExpectedStreamBytes, 0);
    std::memcpy(output_stream.data(), "ERO1", 4);
    std::memcpy(
        output_stream.data() + 4,
        input_stream.data() + 4,
        kStreamHeaderBytes - 4);
    bool compiled = false;
    uint64_t total_run_us = 0;
    uint64_t minimum_run_us = UINT64_MAX;
    uint64_t maximum_run_us = 0;
    for (size_t frame_index = 0;
         frame_index < kFrameCount;
         ++frame_index) {
        const uint8_t* const input_bytes =
            input_stream.data() + kStreamHeaderBytes +
            frame_index * kFrameWidth;
        if (!input->CopyDataToTensor(input_bytes, kFrameWidth)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_stream: input copy failed at %zu\n",
                frame_index);
            return 5;
        }
        if (!compiled) {
            if (!graph->Compile()) {
                std::fprintf(
                    stderr,
                    "wave_npu_run_erf_mask_stream: compile failed\n");
                return 6;
            }
            compiled = true;
        }
        struct timespec started {};
        struct timespec finished {};
        if (clock_gettime(CLOCK_MONOTONIC, &started) != 0 ||
            !graph->Run() ||
            clock_gettime(CLOCK_MONOTONIC, &finished) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_stream: run failed at %zu\n",
                frame_index);
            return 7;
        }
        const uint64_t elapsed =
            ElapsedMicroseconds(started, finished);
        total_run_us += elapsed;
        minimum_run_us = std::min(minimum_run_us, elapsed);
        maximum_run_us = std::max(maximum_run_us, elapsed);

        std::array<uint8_t, kFrameWidth + 2 * kCanaryBytes> observed {};
        observed.fill(kCanaryValue);
        uint8_t* const output_bytes = observed.data() + kCanaryBytes;
        if (!output->CopyDataFromTensor(output_bytes)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_stream: output copy failed at "
                "%zu\n",
                frame_index);
            return 8;
        }
        if (!CanariesAreIntact(observed)) {
            std::fprintf(
                stderr,
                "wave_npu_run_erf_mask_stream: canary changed at %zu\n",
                frame_index);
            return 9;
        }
        std::memcpy(
            output_stream.data() + kStreamHeaderBytes +
                frame_index * kFrameWidth,
            output_bytes,
            kFrameWidth);
    }

    nbg_operation.reset();
    output.reset();
    input.reset();
    graph.reset();
    context.reset();
    if (!WriteFixedOutput(output_stream)) {
        return 11;
    }
    alarm(0);

    std::printf(
        "ERF_STREAM status=PASS mode=fixture-stream frames=1201 "
        "bytes_per_frame=481 compile_count=1 run_count=1201 "
        "canaries=PASS total_run_us=%llu min_run_us=%llu "
        "max_run_us=%llu output_bytes=577697 "
        "output_fnv1a64=0x%016llx output=%s\n",
        static_cast<unsigned long long>(total_run_us),
        static_cast<unsigned long long>(minimum_run_us),
        static_cast<unsigned long long>(maximum_run_us),
        static_cast<unsigned long long>(
            Fnv1a64(output_stream.data(), output_stream.size())),
        kOutputPath);
    return 0;
}
