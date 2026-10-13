// SPDX-License-Identifier: MIT
//
// PoC 16B: compile one fixed UINT8 FullyConnected control to a
// Wave-targeted NBG. This process never executes the graph and exposes no
// model, operation, shape, coefficient, output-path, or retry interface.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/fullyconnected.h"
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

constexpr size_t kMinimumNbgBytes = 513;
constexpr size_t kMaximumNbgBytes = 64U * 1024U * 1024U;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 15;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-compile16b/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] =
    "/tmp/wave-npu-compile16b/sdk";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-compile16b/lib/libCLC.so";
constexpr char kOutputPath[] =
    "/tmp/wave-npu-compile16b/output/wave-u8-fc-candidate-1.nbg";
constexpr char kPartialPath[] =
    "/tmp/wave-npu-compile16b/output/.wave-u8-fc-candidate-1.nbg.partial";
// Weight shape [16,4] stores each output column contiguously.
constexpr std::array<uint8_t, 64> kWeights{
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};
constexpr std::array<int32_t, 4> kBias{0, 0, 0, 0};

volatile sig_atomic_t g_partial_owned = 0;

void WatchdogHandler(int) {
    if (g_partial_owned != 0) {
        unlink(kPartialPath);
    }
    constexpr char kMessage[] =
        "RESULT status=TIMEOUT mode=compile-only exit=124\n";
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

bool PathIsAbsent(const char* path, const char* label) {
    struct stat path_stat {};
    if (lstat(path, &path_stat) == 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: %s is present: %s\n",
            label,
            path);
        return false;
    }
    if (errno != ENOENT) {
        std::fprintf(stderr, "lstat(%s): %s\n", path, std::strerror(errno));
        return false;
    }
    return true;
}

bool EnvironmentIsFixed() {
    const char* const library_path = std::getenv("LD_LIBRARY_PATH");
    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    const char* const debug_level = std::getenv("VIV_VX_DEBUG_LEVEL");
    const char* const print_target = std::getenv("VIV_VX_ENABLE_PRINT_TARGET");
    if (library_path == nullptr ||
        std::strcmp(library_path, kExpectedLibraryPath) != 0 ||
        sdk_directory == nullptr ||
        std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0 ||
        debug_level == nullptr || std::strcmp(debug_level, "1") != 0 ||
        print_target == nullptr || std::strcmp(print_target, "1") != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: fixed environment "
            "mismatch\n");
        return false;
    }
    if (std::getenv("VSIMULATOR_CONFIG") != nullptr ||
        std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: simulator and loader "
            "interposition variables must be unset\n");
        return false;
    }
    return PathIsAbsent(kForbiddenClcPath, "forbidden CLC");
}

bool WriteAtomicExclusive(
    const char* path,
    const char* partial_path,
    const std::vector<uint8_t>& data) {
    if (!PathIsAbsent(path, "output") ||
        !PathIsAbsent(partial_path, "partial output")) {
        return false;
    }
    const int fd = open(
        partial_path,
        O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
        0600);
    if (fd < 0) {
        std::fprintf(
            stderr, "open(%s): %s\n", partial_path, std::strerror(errno));
        return false;
    }
    g_partial_owned = 1;
    size_t written = 0;
    while (written < data.size()) {
        const ssize_t count =
            write(fd, data.data() + written, data.size() - written);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(
                stderr,
                "write(%s): %s\n",
                partial_path,
                std::strerror(errno));
            close(fd);
            unlink(partial_path);
            g_partial_owned = 0;
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr, "write(%s): zero-byte write\n", partial_path);
            close(fd);
            unlink(partial_path);
            g_partial_owned = 0;
            return false;
        }
        written += static_cast<size_t>(count);
    }
    const bool mode_ok = fchmod(fd, 0444) == 0;
    const bool sync_ok = fsync(fd) == 0;
    const bool close_ok = close(fd) == 0;
    if (!mode_ok || !sync_ok || !close_ok) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: finalize failed\n");
        unlink(partial_path);
        g_partial_owned = 0;
        return false;
    }
    if (link(partial_path, path) != 0 || unlink(partial_path) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: install failed\n");
        unlink(partial_path);
        g_partial_owned = 0;
        return false;
    }
    g_partial_owned = 0;
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target\":{\"chip_model\":\"0x00009000\","
        "\"customer_id\":\"0x1000000e\","
        "\"nbg_version\":\"0x00020000\"},"
        "\"candidates\":[1],\"operation\":\"FullyConnected\","
        "\"backend_required\":\"OPENVX\",\"model\":\"sparse-u8-fc-control\","
        "\"data_type\":\"UINT8\","
        "\"input\":{\"shape\":[16,1],\"dtype\":\"UINT8\"},"
        "\"constant_weights\":{\"shape\":[16,4],\"dtype\":\"UINT8\","
        "\"select_input_indices\":[0,5,10,15]},"
        "\"constant_bias\":{\"shape\":[4],\"dtype\":\"INT32\","
        "\"values\":[0,0,0,0],\"scale\":1,\"zero_point\":0},"
        "\"output\":{\"shape\":[4,1],\"dtype\":\"UINT8\"},"
        "\"quantization\":{\"type\":\"ASYMMETRIC\",\"scale\":1,"
        "\"zero_point\":0},\"axis\":0,\"units\":4,"
        "\"executes_graph\":false}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--candidate-1") != 0) {
        std::fprintf(stderr, "usage: %s --candidate-1\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (!PathIsAbsent(kOutputPath, "output") ||
        !PathIsAbsent(kPartialPath, "partial output")) {
        return 16;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: parent-death gate "
            "failed\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: watchdog setup "
            "failed\n");
        return 18;
    }
    alarm(kWatchdogSeconds);

    std::fprintf(
        stderr,
        "wave_npu_compile_u8_fc: mode=compile-only "
        "target=wave candidate=candidate-1 watchdog_seconds=%u\n",
        kWatchdogSeconds);

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: context create "
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
            "wave_npu_compile_u8_fc: identity mismatch\n");
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
            "wave_npu_compile_u8_fc: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType input_shape{16, 1};
    const tim::vx::ShapeType weight_shape{16, 4};
    const tim::vx::ShapeType bias_shape{4};
    const tim::vx::ShapeType output_shape{4, 1};
    const tim::vx::Quantization quantization(
        tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::UINT8,
        input_shape,
        tim::vx::TensorAttribute::INPUT,
        quantization);
    const tim::vx::TensorSpec weight_spec(
        tim::vx::DataType::UINT8,
        weight_shape,
        tim::vx::TensorAttribute::CONSTANT,
        quantization);
    const tim::vx::TensorSpec bias_spec(
        tim::vx::DataType::INT32,
        bias_shape,
        tim::vx::TensorAttribute::CONSTANT,
        quantization);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::UINT8,
        output_shape,
        tim::vx::TensorAttribute::OUTPUT,
        quantization);
    auto input = graph->CreateTensor(input_spec);
    auto weight = graph->CreateTensor(weight_spec, kWeights.data());
    auto bias = graph->CreateTensor(bias_spec, kBias.data());
    auto output = graph->CreateTensor(output_spec);
    if (!input || !weight || !bias || !output) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: tensor create "
            "failed\n");
        return 3;
    }

    auto fully_connected =
        graph->CreateOperation<tim::vx::ops::FullyConnected>(0, 4);
    if (!fully_connected) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: FullyConnected create failed\n");
        return 4;
    }
    fully_connected->BindInput(input)
        .BindInput(weight)
        .BindInput(bias)
        .BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: size-only compile "
            "failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: rejected NBG size "
            "%zu\n",
            nbg_size);
        return 6;
    }
    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: binary compile "
            "failed\n");
        return 7;
    }
    if (written_size < kMinimumNbgBytes || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: rejected written "
            "size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);
    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: invalid NBG magic\n");
        return 9;
    }
    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product = ReadLe32(nbg.data() + 8);
    if (version != kWaveNbgVersion || product != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_compile_u8_fc: NBG target "
            "mismatch\n");
        return 14;
    }
    if (!WriteAtomicExclusive(kOutputPath, kPartialPath, nbg)) {
        return 11;
    }
    std::printf(
        "RESULT status=PASS mode=compile-only target=wave "
        "candidate=candidate-1 bytes=%zu version=0x%08x "
        "product=0x%08x output=%s\n",
        nbg.size(),
        version,
        product,
        kOutputPath);
    std::fflush(stdout);
    return 0;
}
