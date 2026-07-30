// SPDX-License-Identifier: MIT
//
// PoC 8D: execute one fixed FLOAT32 Conv2d audio denoiser once through
// TIM-VX. No graph, tensor, coefficient, iteration, output, retry, or
// recovery parameter is configurable.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/conv2d.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cmath>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <unistd.h>

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

constexpr size_t kFrameSamples = 16;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr unsigned int kWatchdogSeconds = 8;
constexpr float kMaximumError = 1.0e-6F;
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-audio8d/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-audio8d/lib/libCLC.so";
constexpr std::array<float, 3> kWeights{0.25F, 0.50F, 0.25F};
constexpr std::array<float, 1> kBias{0.0F};
constexpr std::array<float, kFrameSamples> kInput{
    1.0F, -1.0F, 1.0F, -1.0F,
    1.0F, -1.0F, 1.0F, -1.0F,
    1.0F, -1.0F, 1.0F, -1.0F,
    1.0F, -1.0F, 1.0F, -1.0F};
constexpr std::array<float, kFrameSamples> kExpected{
    0.25F, 0.0F, 0.0F, 0.0F,
    0.0F, 0.0F, 0.0F, 0.0F,
    0.0F, 0.0F, 0.0F, 0.0F,
    0.0F, 0.0F, 0.0F, -0.25F};

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "AUDIO status=TIMEOUT mode=conv-direct exit=124\n";
    const ssize_t message_bytes =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)message_bytes;
    _exit(124);
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
            "wave_npu_run_conv_audio_denoiser: fixed environment "
            "mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: interposition and "
            "simulator variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: isolated CLC path must "
            "be absent\n");
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"conv-direct\","
        "\"operation\":\"Conv2d\",\"data_type\":\"FLOAT32\","
        "\"input\":{\"shape\":[16,1,1,1],"
        "\"values\":[1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1]},"
        "\"constant_weights\":{\"shape\":[3,1,1,1],"
        "\"values\":[0.25,0.5,0.25]},"
        "\"constant_bias\":{\"shape\":[1],\"values\":[0]},"
        "\"output\":{\"shape\":[16,1,1,1],"
        "\"expected\":[0.25,0,0,0,0,0,0,0,0,0,0,0,0,0,0,-0.25]},"
        "\"padding\":\"SAME\",\"stride\":[1,1],"
        "\"dilation\":[0,0],\"iterations\":1,"
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
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: parent-death gate "
            "failed\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: watchdog setup failed\n");
        return 18;
    }
    alarm(kWatchdogSeconds);

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: context create failed\n");
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
            "wave_npu_run_conv_audio_denoiser: identity mismatch\n");
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
            "wave_npu_run_conv_audio_denoiser: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType input_shape{16, 1, 1, 1};
    const tim::vx::ShapeType weight_shape{3, 1, 1, 1};
    const tim::vx::ShapeType bias_shape{1};
    const tim::vx::ShapeType output_shape{16, 1, 1, 1};
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::FLOAT32,
        input_shape,
        tim::vx::TensorAttribute::INPUT);
    const tim::vx::TensorSpec weight_spec(
        tim::vx::DataType::FLOAT32,
        weight_shape,
        tim::vx::TensorAttribute::CONSTANT);
    const tim::vx::TensorSpec bias_spec(
        tim::vx::DataType::FLOAT32,
        bias_shape,
        tim::vx::TensorAttribute::CONSTANT);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::FLOAT32,
        output_shape,
        tim::vx::TensorAttribute::OUTPUT);
    auto input = graph->CreateTensor(input_spec);
    auto weight = graph->CreateTensor(weight_spec, kWeights.data());
    auto bias = graph->CreateTensor(bias_spec, kBias.data());
    auto output = graph->CreateTensor(output_spec);
    if (!input || !weight || !bias || !output) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: tensor create failed\n");
        return 3;
    }

    const std::array<uint32_t, 2> stride{1, 1};
    const std::array<uint32_t, 2> dilation{0, 0};
    auto conv = graph->CreateOperation<tim::vx::ops::Conv2d>(
        tim::vx::PadType::SAME, stride, dilation);
    if (!conv) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: Conv2d create failed\n");
        return 4;
    }
    conv->BindInput(input)
        .BindInput(weight)
        .BindInput(bias)
        .BindOutput(output);

    if (!input->CopyDataToTensor(kInput.data(), sizeof(kInput))) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: input copy failed\n");
        return 5;
    }
    if (!graph->Compile()) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: graph compile failed\n");
        return 6;
    }
    if (!graph->Run()) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: graph run failed\n");
        return 7;
    }

    std::array<float, kFrameSamples> observed {};
    observed.fill(123.0F);
    if (!output->CopyDataFromTensor(observed.data())) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: output copy failed\n");
        return 8;
    }

    double input_energy = 0.0;
    double output_energy = 0.0;
    float max_error = 0.0F;
    for (size_t index = 0; index < kFrameSamples; ++index) {
        input_energy +=
            static_cast<double>(kInput[index]) * kInput[index];
        output_energy +=
            static_cast<double>(observed[index]) * observed[index];
        max_error = std::max(
            max_error, std::fabs(observed[index] - kExpected[index]));
    }
    if (max_error > kMaximumError ||
        std::fabs(input_energy - 16.0) > 1.0e-9 ||
        std::fabs(output_energy - 0.125) > 1.0e-9) {
        std::fprintf(
            stderr,
            "wave_npu_run_conv_audio_denoiser: output mismatch "
            "input_energy=%.9f output_energy=%.9f max_error=%.9f\n",
            input_energy,
            output_energy,
            static_cast<double>(max_error));
        return 9;
    }

    conv.reset();
    output.reset();
    bias.reset();
    weight.reset();
    input.reset();
    graph.reset();
    context.reset();
    alarm(0);

    std::printf(
        "AUDIO status=PASS mode=conv-direct iterations=1 "
        "input_energy=%.6f output_energy=%.6f max_error=%.6f\n",
        input_energy,
        output_energy,
        static_cast<double>(max_error));
    return 0;
}
