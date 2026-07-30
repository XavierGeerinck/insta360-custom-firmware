// SPDX-License-Identifier: MIT
//
// First fixed execution probe for a generated Wave-targeted NBG.
//
// The probe loads exactly one hash-pinned-by-harness NBG, runs one all-zero
// U8 tensor through it, validates four zero output bytes, and exits. It has no
// configurable model, tensor, operation, iteration, or output-file interface.

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
constexpr size_t kExpectedNbgBytes = 5840;
constexpr unsigned int kWatchdogSeconds = 8;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-exec/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-exec/model/wave-erf-openvx.nbg";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-exec/lib/libCLC.so";

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
            "wave_npu_execute_erf_zero: fixed environment mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: interposition and simulator "
            "variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: isolated CLC path must be absent\n");
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
            "wave_npu_execute_erf_zero: fixed NBG metadata mismatch\n");
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
                stderr, "wave_npu_execute_erf_zero: short NBG read\n");
            close(fd);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    char extra = 0;
    if (read(fd, &extra, 1) != 0 || close(fd) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: NBG close/length check failed\n");
        return false;
    }

    const auto* const bytes =
        reinterpret_cast<const uint8_t*>(nbg->data());
    if (std::memcmp(bytes, "VPMN", 4) != 0 ||
        ReadLe32(bytes + 4) != kWaveNbgVersion ||
        ReadLe32(bytes + 8) != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: fixed NBG target mismatch\n");
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"execute-once\","
        "\"model\":\"/tmp/wave-npu-exec/model/wave-erf-openvx.nbg\","
        "\"model_bytes\":5840,\"model_sha256\":"
        "\"6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee\","
        "\"operation\":\"NBG\",\"inputs\":[{\"type\":\"UINT8\","
        "\"shape\":[4],\"data\":[0,0,0,0]}],"
        "\"outputs\":[{\"type\":\"UINT8\",\"shape\":[4],"
        "\"expected\":[0,0,0,0]}],\"iterations\":1,"
        "\"executes_graph\":true}");
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
            "wave_npu_execute_erf_zero: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: watchdog setup failed: %s\n",
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
            stderr, "wave_npu_execute_erf_zero: context create failed\n");
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
            stderr, "wave_npu_execute_erf_zero: identity mismatch\n");
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
            stderr, "wave_npu_execute_erf_zero: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType shape{4};
    const tim::vx::Quantization quantization(
        tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::INPUT,
        quantization);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::OUTPUT,
        quantization);
    auto input = graph->CreateTensor(input_spec);
    auto output = graph->CreateTensor(output_spec);
    if (!input || !output) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: tensor create failed\n");
        return 3;
    }

    auto nbg_operation =
        graph->CreateOperation<tim::vx::ops::NBG>(nbg.data(), 1, 1);
    if (!nbg_operation) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: NBG operation failed\n");
        return 4;
    }
    nbg_operation->BindInput(input).BindOutput(output);

    constexpr std::array<uint8_t, 4> kInput{0, 0, 0, 0};
    constexpr std::array<uint8_t, 4> kExpected{0, 0, 0, 0};
    if (!input->CopyDataToTensor(kInput.data(), kInput.size())) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: input copy failed\n");
        return 5;
    }
    if (!graph->Compile()) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: graph compile failed\n");
        return 6;
    }
    if (!graph->Run()) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: graph run failed\n");
        return 7;
    }

    std::array<uint8_t, 4> observed{0xa5, 0xa5, 0xa5, 0xa5};
    if (!output->CopyDataFromTensor(observed.data())) {
        std::fprintf(
            stderr, "wave_npu_execute_erf_zero: output copy failed\n");
        return 8;
    }
    if (observed != kExpected) {
        std::fprintf(
            stderr,
            "wave_npu_execute_erf_zero: output mismatch "
            "observed=%u,%u,%u,%u\n",
            observed[0],
            observed[1],
            observed[2],
            observed[3]);
        return 9;
    }

    nbg_operation.reset();
    output.reset();
    input.reset();
    graph.reset();
    context.reset();
    alarm(0);

    std::puts(
        "EXECUTION status=PASS mode=execute-once iterations=1 "
        "input=0,0,0,0 output=0,0,0,0 bytes=4");
    return 0;
}
