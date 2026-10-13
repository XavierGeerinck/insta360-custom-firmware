// SPDX-License-Identifier: MIT
//
// PoC 16A: execute the validated Wave FP16 Conv NBG once with one fixed
// alternating-sign input. There is no configurable model, tensor, operation, iteration,
// output-file, retry, or recovery interface.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/nbg.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

#include <array>
#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <time.h>
#include <sys/prctl.h>
#include <sys/stat.h>
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

constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr size_t kExpectedNbgBytes = 1800;
// The fixed host execution manifest verifies this digest before launch.
constexpr char kExpectedModelSha256[] =
    "4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d";
constexpr size_t kInputBytes = 32;
constexpr size_t kOutputBytes = 32;
constexpr size_t kGuardBytes = 16;
constexpr uint8_t kCanaryByte = 0xa5U;
constexpr uint8_t kGuardByte = 0x5aU;
constexpr unsigned int kWatchdogSeconds = 8;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-exec16a/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-exec16a/model/wave-fp16-conv.nbg";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-exec16a/lib/libCLC.so";

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "EXECUTION status=TIMEOUT mode=execute-once exit=124\n";
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
            "wave_npu_execute_fp16_conv: fixed environment mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: interposition and simulator "
            "variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: isolated CLC path must be "
            "absent\n");
        return false;
    }
    return true;
}

bool ReadFixedNbg(std::vector<char>* nbg) {
    const int fd = open(kNbgPath, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) {
        std::fprintf(
            stderr, "open(%s): %s\n", kNbgPath, std::strerror(errno));
        return false;
    }
    struct stat model_stat {};
    if (fstat(fd, &model_stat) != 0 ||
        !S_ISREG(model_stat.st_mode) ||
        (model_stat.st_mode & 0777) != 0444 ||
        model_stat.st_size != static_cast<off_t>(kExpectedNbgBytes)) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: fixed NBG metadata mismatch\n");
        close(fd);
        return false;
    }

    nbg->assign(kExpectedNbgBytes, 0);
    size_t offset = 0;
    while (offset < nbg->size()) {
        const ssize_t count =
            read(fd, nbg->data() + offset, nbg->size() - offset);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(
                stderr, "read(%s): %s\n", kNbgPath, std::strerror(errno));
            close(fd);
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr,
                "wave_npu_execute_fp16_conv: short NBG read\n");
            close(fd);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    char extra = 0;
    const ssize_t extra_bytes = read(fd, &extra, 1);
    const int close_status = close(fd);
    if (extra_bytes != 0 || close_status != 0) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: NBG close/length check "
            "failed\n");
        return false;
    }

    const auto* const bytes =
        reinterpret_cast<const uint8_t*>(nbg->data());
    if (std::memcmp(bytes, "VPMN", 4) != 0 ||
        ReadLe32(bytes + 4) != kWaveNbgVersion ||
        ReadLe32(bytes + 8) != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: fixed NBG target mismatch\n");
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::printf(
        "{\"schema_version\":1,\"mode\":\"execute-once\","
        "\"model\":\"%s\",\"model_bytes\":%zu,\"model_sha256\":\"%s\","
        "\"operation\":\"NBG\",\"model_operation\":\"fp16-conv\","
        "\"inputs\":[{\"type\":\"FLOAT16\",\"shape\":[16,1,1,1],"
        "\"half_bits\":[15360,48128,15360,48128,15360,48128,15360,48128,"
        "15360,48128,15360,48128,15360,48128,15360,48128]}],"
        "\"outputs\":[{\"type\":\"FLOAT16\",\"shape\":[16,1,1,1],"
        "\"expected_half_bits\":[13312,0,0,0,0,0,0,0,0,0,0,0,0,0,0,46080]}],"
        "\"iterations\":1,\"executes_graph\":true}\n",
        kNbgPath,
        kExpectedNbgBytes,
        kExpectedModelSha256);
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--execute-once") != 0) {
        std::fprintf(stderr, "usage: %s --execute-once\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    alarm(kWatchdogSeconds);

    std::vector<char> nbg;
    if (!ReadFixedNbg(&nbg)) {
        return 13;
    }

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: context create failed\n");
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
            "wave_npu_execute_fp16_conv: identity mismatch\n");
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
            "wave_npu_execute_fp16_conv: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType input_shape{16, 1, 1, 1};
    const tim::vx::ShapeType output_shape{16, 1, 1, 1};
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::FLOAT16,
        input_shape,
        tim::vx::TensorAttribute::INPUT);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::FLOAT16,
        output_shape,
        tim::vx::TensorAttribute::OUTPUT);
    auto input = graph->CreateTensor(input_spec);
    auto output = graph->CreateTensor(output_spec);
    if (!input || !output) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: tensor create failed\n");
        return 3;
    }

    auto nbg_operation =
        graph->CreateOperation<tim::vx::ops::NBG>(nbg.data(), 1, 1);
    if (!nbg_operation) {
        std::fprintf(
            stderr,
            "wave_npu_execute_fp16_conv: NBG operation failed\n");
        return 4;
    }
    nbg_operation->BindInput(input).BindOutput(output);

    constexpr std::array<uint16_t, 16> kInput{
        0x3c00U, 0xbc00U, 0x3c00U, 0xbc00U,
        0x3c00U, 0xbc00U, 0x3c00U, 0xbc00U,
        0x3c00U, 0xbc00U, 0x3c00U, 0xbc00U,
        0x3c00U, 0xbc00U, 0x3c00U, 0xbc00U};
    constexpr std::array<uint16_t, 16> kExpected{
        0x3400U, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0xb400U};
    static_assert(sizeof(kInput) == kInputBytes, "fixed input byte count");
    static_assert(sizeof(kExpected) == kOutputBytes, "fixed output byte count");
    if (!input->CopyDataToTensor(kInput.data(), kInputBytes)) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: input copy failed\n");
        return 5;
    }
    if (!graph->Compile()) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: graph compile failed\n");
        return 6;
    }
    std::array<uint8_t, kOutputBytes> device_canary{};
    device_canary.fill(kCanaryByte);
    if (!output->CopyDataToTensor(device_canary.data(), kOutputBytes)) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: output canary copy failed\n");
        return 21;
    }
    struct timespec before {};
    struct timespec after {};
    if (clock_gettime(CLOCK_MONOTONIC, &before) != 0) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: timing setup failed\n");
        return 20;
    }
    if (!graph->Run()) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: graph run failed\n");
        return 7;
    }
    if (clock_gettime(CLOCK_MONOTONIC, &after) != 0) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: timing read failed\n");
        return 20;
    }
    const int64_t elapsed_ns =
        static_cast<int64_t>(after.tv_sec - before.tv_sec) * 1000000000LL +
        static_cast<int64_t>(after.tv_nsec - before.tv_nsec);
    if (elapsed_ns < 0) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: invalid graph timing\n");
        return 20;
    }
    const int64_t graph_us = elapsed_ns / 1000LL;

    alignas(uint16_t)
        std::array<uint8_t, kOutputBytes + 2 * kGuardBytes> guarded_output{};
    guarded_output.fill(kGuardByte);
    std::memset(guarded_output.data() + kGuardBytes, kCanaryByte, kOutputBytes);
    if (!output->CopyDataFromTensor(guarded_output.data() + kGuardBytes)) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: output copy failed\n");
        return 8;
    }
    for (size_t index = 0; index < kGuardBytes; ++index) {
        if (guarded_output[index] != kGuardByte ||
            guarded_output[kGuardBytes + kOutputBytes + index] != kGuardByte) {
            std::fprintf(stderr, "wave_npu_execute_fp16_conv: output guard changed\n");
            return 19;
        }
    }
    std::array<uint8_t, kOutputBytes> observed{};
    std::memcpy(observed.data(), guarded_output.data() + kGuardBytes, kOutputBytes);
    if (std::memcmp(observed.data(), kExpected.data(), kOutputBytes) != 0) {
        std::fprintf(stderr, "wave_npu_execute_fp16_conv: output mismatch output_hex=");
        for (const uint8_t byte : observed) {
            std::fprintf(stderr, "%02x", static_cast<unsigned int>(byte));
        }
        std::fputc('\n', stderr);
        return 9;
    }

    nbg_operation.reset();
    output.reset();
    input.reset();
    graph.reset();
    context.reset();

    std::printf(
        "EXECUTION status=PASS mode=execute-once target=wave "
        "operation=fp16-conv iterations=1 input_bytes=%zu output_bytes=%zu "
        "graph_us=%lld output_hex=",
        kInputBytes,
        kOutputBytes,
        static_cast<long long>(graph_us));
    for (const uint8_t byte : observed) {
        std::printf("%02x", static_cast<unsigned int>(byte));
    }
    std::puts(" cleanup=0");
    std::fflush(stdout);
    return 0;
}
