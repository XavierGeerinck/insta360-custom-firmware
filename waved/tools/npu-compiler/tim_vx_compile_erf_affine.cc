// SPDX-License-Identifier: MIT
//
// PoC 14B: compile the frozen 481-bin quantized affine-plus-Erf partition.
//
// This binary has exactly six fixed compile-only modes. A1/A2 compile the
// identical frozen model, B1/B2 change weight[17] from 127 to 126, and C1/C2
// change bias[17] from -4773 to -4772. It never runs a graph, uploads an input,
// reads a tensor, loads an NBG, opens galcore directly, or accepts a model path.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/elementwise.h"
#include "tim/vx/ops/erf.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

#include <array>
#include <cerrno>
#include <cmath>
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

extern "C" {
extern const unsigned char _binary_affine_constants_bin_start[];
extern const unsigned char _binary_affine_constants_bin_end[];
}

namespace {

constexpr size_t kBins = 481;
constexpr size_t kHeaderBytes = 68;
constexpr size_t kWeightOffset = kHeaderBytes;
constexpr size_t kWeightScaleOffset = kWeightOffset + kBins;
constexpr size_t kBiasOffset = kWeightScaleOffset + kBins * sizeof(float);
constexpr size_t kAccumulatorScaleOffset =
    kBiasOffset + kBins * sizeof(int32_t);
constexpr size_t kConstantsBytes =
    kAccumulatorScaleOffset + kBins * sizeof(float);
constexpr size_t kDeltaIndex = 17;
constexpr int8_t kBaselineDeltaWeight = 127;
constexpr int32_t kBaselineDeltaBias = -4773;
constexpr size_t kMinimumNbgBytes = 513;
constexpr size_t kMaximumNbgBytes = 64U * 1024U * 1024U;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 15;
constexpr float kFeatureScale = 0.0625F;
constexpr int32_t kFeatureZeroPoint = 128;
constexpr float kErfInputScale = 0.03125F;
constexpr int32_t kErfInputZeroPoint = 128;
constexpr float kErfOutputScale = 0.0078125F;
constexpr int32_t kErfOutputZeroPoint = 128;
constexpr char kExpectedSdkDirectory[] = "/tmp/wave-npu-poc14b/sdk";
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc14b/lib:/usr/lib";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-poc14b/lib/libCLC.so";
constexpr char kConstantsSha256[] =
    "a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4";
constexpr std::array<uint8_t, 32> kContractSha256{{
    0xb5, 0x2d, 0xda, 0xf3, 0xba, 0xed, 0x2a, 0x70,
    0x01, 0xff, 0x10, 0xcb, 0xf9, 0xba, 0x06, 0x00,
    0x94, 0x57, 0x49, 0xa2, 0x33, 0x2f, 0x02, 0xd7,
    0x1d, 0x2a, 0xe0, 0xbe, 0x25, 0xac, 0x98, 0xb8,
}};

constexpr char kA1Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-a1.nbg";
constexpr char kA2Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-a2.nbg";
constexpr char kB1Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-b1.nbg";
constexpr char kB2Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-b2.nbg";
constexpr char kC1Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-c1.nbg";
constexpr char kC2Path[] =
    "/tmp/wave-npu-poc14b/output/erf-affine-c2.nbg";
constexpr char kA1PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-a1.nbg.partial";
constexpr char kA2PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-a2.nbg.partial";
constexpr char kB1PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-b1.nbg.partial";
constexpr char kB2PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-b2.nbg.partial";
constexpr char kC1PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-c1.nbg.partial";
constexpr char kC2PartialPath[] =
    "/tmp/wave-npu-poc14b/output/.erf-affine-c2.nbg.partial";

enum class DeltaKind {
    kNone,
    kWeight,
    kBias,
};

struct Candidate {
    const char* argument;
    const char* label;
    const char* output_path;
    const char* partial_path;
    DeltaKind delta;
};

struct AffineConstants {
    std::array<int8_t, kBins> weights;
    std::vector<float> weight_scales;
    std::array<int32_t, kBins> biases;
    std::vector<float> accumulator_scales;
};

const char* g_timeout_partial_path = nullptr;

void WatchdogHandler(int) {
    if (g_timeout_partial_path != nullptr) {
        unlink(g_timeout_partial_path);
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

float ReadLeFloat(const uint8_t* data) {
    const uint32_t bits = ReadLe32(data);
    float value = 0.0F;
    static_assert(sizeof(value) == sizeof(bits), "unexpected float ABI");
    std::memcpy(&value, &bits, sizeof(value));
    return value;
}

bool PathIsAbsent(const char* path, const char* label) {
    struct stat path_stat {};
    if (lstat(path, &path_stat) == 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: %s is present: %s\n",
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
    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    const char* const library_path = std::getenv("LD_LIBRARY_PATH");
    if (
        sdk_directory == nullptr
        || std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0
        || library_path == nullptr
        || std::strcmp(library_path, kExpectedLibraryPath) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: fixed SDK/library environment required\n");
        return false;
    }
    if (
        std::getenv("VSIMULATOR_CONFIG") != nullptr
        || std::getenv("LD_PRELOAD") != nullptr
        || std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: simulator/interposition variables forbidden\n");
        return false;
    }
    return PathIsAbsent(kForbiddenClcPath, "quarantined CLC");
}

bool WriteAtomicExclusive(
    const char* path,
    const char* partial_path,
    const std::vector<uint8_t>& data) {
    if (
        !PathIsAbsent(path, "output")
        || !PathIsAbsent(partial_path, "partial output")) {
        return false;
    }
    const int descriptor = open(
        partial_path,
        O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
        0600);
    if (descriptor < 0) {
        std::fprintf(
            stderr,
            "open(%s): %s\n",
            partial_path,
            std::strerror(errno));
        return false;
    }
    size_t written = 0;
    while (written < data.size()) {
        const ssize_t count = write(
            descriptor,
            data.data() + written,
            data.size() - written);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(
                stderr,
                "write(%s): %s\n",
                partial_path,
                std::strerror(errno));
            close(descriptor);
            unlink(partial_path);
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr,
                "write(%s): zero-byte write\n",
                partial_path);
            close(descriptor);
            unlink(partial_path);
            return false;
        }
        written += static_cast<size_t>(count);
    }
    if (
        fchmod(descriptor, 0444) != 0
        || fsync(descriptor) != 0
        || close(descriptor) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: output finalization failed\n");
        unlink(partial_path);
        return false;
    }
    if (link(partial_path, path) != 0 || unlink(partial_path) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: atomic output install failed\n");
        unlink(partial_path);
        return false;
    }
    return true;
}

bool ParseConstants(AffineConstants* constants) {
    const auto* const begin = _binary_affine_constants_bin_start;
    const auto* const end = _binary_affine_constants_bin_end;
    if (
        end < begin
        || static_cast<size_t>(end - begin) != kConstantsBytes) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: embedded constants length differs\n");
        return false;
    }
    const uint8_t* const payload = begin;
    if (
        std::memcmp(payload, "AFC1", 4) != 0
        || ReadLe32(payload + 4) != 1
        || ReadLe32(payload + 8) != kBins
        || ReadLe32(payload + 12) != 1
        || ReadLe32(payload + 16) != 2
        || ReadLeFloat(payload + 20) != kFeatureScale
        || ReadLeFloat(payload + 24) != kErfInputScale
        || static_cast<int32_t>(ReadLe32(payload + 28))
            != kFeatureZeroPoint
        || static_cast<int32_t>(ReadLe32(payload + 32))
            != kErfInputZeroPoint
        || std::memcmp(
            payload + 36,
            kContractSha256.data(),
            kContractSha256.size()) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: embedded constants header differs\n");
        return false;
    }

    constants->weight_scales.reserve(kBins);
    constants->accumulator_scales.reserve(kBins);
    for (size_t index = 0; index < kBins; ++index) {
        constants->weights[index] =
            static_cast<int8_t>(payload[kWeightOffset + index]);
        const float weight_scale = ReadLeFloat(
            payload + kWeightScaleOffset + index * sizeof(float));
        const int32_t bias = static_cast<int32_t>(ReadLe32(
            payload + kBiasOffset + index * sizeof(int32_t)));
        const float accumulator_scale = ReadLeFloat(
            payload + kAccumulatorScaleOffset + index * sizeof(float));
        if (
            !std::isfinite(weight_scale)
            || weight_scale <= 0.0F
            || !std::isfinite(accumulator_scale)
            || accumulator_scale <= 0.0F) {
            std::fprintf(
                stderr,
                "wave_npu_poc14b: invalid embedded quantization scale\n");
            return false;
        }
        constants->weight_scales.push_back(weight_scale);
        constants->biases[index] = bias;
        constants->accumulator_scales.push_back(accumulator_scale);
    }
    if (
        constants->weights[kDeltaIndex] != kBaselineDeltaWeight
        || constants->biases[kDeltaIndex] != kBaselineDeltaBias) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: fixed differential coefficient differs\n");
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::printf(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target\":{\"chip_model\":\"0x00009000\","
        "\"customer_id\":\"0x1000000e\","
        "\"nbg_version\":\"0x00020000\"},"
        "\"contract_sha256\":"
        "\"b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8\","
        "\"constants_sha256\":\"%s\","
        "\"candidates\":{\"a1\":\"baseline\",\"a2\":\"baseline\","
        "\"b1\":\"weight[17]=126\",\"b2\":\"weight[17]=126\","
        "\"c1\":\"bias[17]=-4772\",\"c2\":\"bias[17]=-4772\"},"
        "\"operations\":[\"Multiply\",\"Add\",\"Erf\"],"
        "\"input\":{\"dtype\":\"UINT8\",\"shape\":[481],"
        "\"scale\":0.0625,\"zero_point\":128},"
        "\"output\":{\"dtype\":\"UINT8\",\"shape\":[481],"
        "\"scale\":0.0078125,\"zero_point\":128},"
        "\"executes_graph\":false}\n",
        kConstantsSha256);
}

const Candidate* SelectCandidate(const char* argument) {
    static constexpr Candidate kCandidates[] = {
        {"--a1", "a1", kA1Path, kA1PartialPath, DeltaKind::kNone},
        {"--a2", "a2", kA2Path, kA2PartialPath, DeltaKind::kNone},
        {"--b1", "b1", kB1Path, kB1PartialPath, DeltaKind::kWeight},
        {"--b2", "b2", kB2Path, kB2PartialPath, DeltaKind::kWeight},
        {"--c1", "c1", kC1Path, kC1PartialPath, DeltaKind::kBias},
        {"--c2", "c2", kC2Path, kC2PartialPath, DeltaKind::kBias},
    };
    for (const Candidate& candidate : kCandidates) {
        if (std::strcmp(argument, candidate.argument) == 0) {
            return &candidate;
        }
    }
    return nullptr;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    const Candidate* const candidate =
        argc == 2 ? SelectCandidate(argv[1]) : nullptr;
    if (candidate == nullptr) {
        std::fprintf(
            stderr,
            "usage: %s --a1|--a2|--b1|--b2|--c1|--c2|--describe\n",
            argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (
        !PathIsAbsent(candidate->output_path, "output")
        || !PathIsAbsent(candidate->partial_path, "partial output")) {
        return 16;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: parent-death gate failed: %s\n",
            std::strerror(errno));
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    g_timeout_partial_path = candidate->partial_path;
    alarm(kWatchdogSeconds);

    std::fprintf(
        stderr,
        "wave_npu_poc14b: mode=compile-only target=wave candidate=%s "
        "watchdog_seconds=%u constants_sha256=%s\n",
        candidate->label,
        kWatchdogSeconds,
        kConstantsSha256);

    AffineConstants constants {};
    if (!ParseConstants(&constants)) {
        return 19;
    }
    if (candidate->delta == DeltaKind::kWeight) {
        constants.weights[kDeltaIndex] = 126;
    } else if (candidate->delta == DeltaKind::kBias) {
        constants.biases[kDeltaIndex] = -4772;
    }

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(stderr, "wave_npu_poc14b: context create failed\n");
        return 1;
    }
    ChipIdentityEx identity {};
    const int identity_status = gcoHAL_QueryChipIdentityEx(
        nullptr,
        static_cast<uint32_t>(sizeof(identity)),
        &identity);
    if (
        identity_status != 0
        || identity.chip_model != kWaveChipModel
        || identity.customer_id != kWaveProductId) {
        std::fprintf(stderr, "wave_npu_poc14b: identity mismatch\n");
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
        std::fprintf(stderr, "wave_npu_poc14b: graph create failed\n");
        return 2;
    }
    const tim::vx::ShapeType shape{481};
    const tim::vx::Quantization feature_quantization(
        tim::vx::QuantType::ASYMMETRIC,
        kFeatureScale,
        kFeatureZeroPoint);
    const std::vector<int32_t> channel_zero_points(kBins, 0);
    const tim::vx::Quantization weight_quantization(
        tim::vx::QuantType::SYMMETRIC_PER_CHANNEL,
        0,
        constants.weight_scales,
        channel_zero_points);
    const tim::vx::Quantization accumulator_quantization(
        tim::vx::QuantType::SYMMETRIC_PER_CHANNEL,
        0,
        constants.accumulator_scales,
        channel_zero_points);
    const tim::vx::Quantization erf_input_quantization(
        tim::vx::QuantType::ASYMMETRIC,
        kErfInputScale,
        kErfInputZeroPoint);
    const tim::vx::Quantization erf_output_quantization(
        tim::vx::QuantType::ASYMMETRIC,
        kErfOutputScale,
        kErfOutputZeroPoint);
    const tim::vx::TensorSpec input_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::INPUT,
        feature_quantization);
    const tim::vx::TensorSpec weight_spec(
        tim::vx::DataType::INT8,
        shape,
        tim::vx::TensorAttribute::CONSTANT,
        weight_quantization);
    const tim::vx::TensorSpec product_spec(
        tim::vx::DataType::INT32,
        shape,
        tim::vx::TensorAttribute::TRANSIENT,
        accumulator_quantization);
    const tim::vx::TensorSpec bias_spec(
        tim::vx::DataType::INT32,
        shape,
        tim::vx::TensorAttribute::CONSTANT,
        accumulator_quantization);
    const tim::vx::TensorSpec erf_input_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::TRANSIENT,
        erf_input_quantization);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::OUTPUT,
        erf_output_quantization);

    auto input = graph->CreateTensor(input_spec);
    auto weight = graph->CreateTensor(weight_spec, constants.weights.data());
    auto product = graph->CreateTensor(product_spec);
    auto bias = graph->CreateTensor(bias_spec, constants.biases.data());
    auto erf_input = graph->CreateTensor(erf_input_spec);
    auto output = graph->CreateTensor(output_spec);
    if (!input || !weight || !product || !bias || !erf_input || !output) {
        std::fprintf(stderr, "wave_npu_poc14b: tensor create failed\n");
        return 3;
    }

    auto multiply =
        graph->CreateOperation<tim::vx::ops::Multiply>(1.0F);
    auto add = graph->CreateOperation<tim::vx::ops::Add>();
    auto erf = graph->CreateOperation<tim::vx::ops::Erf>();
    if (!multiply || !add || !erf) {
        std::fprintf(stderr, "wave_npu_poc14b: operation create failed\n");
        return 4;
    }
    multiply->BindInputs({input, weight}).BindOutput(product);
    add->BindInputs({product, bias}).BindOutput(erf_input);
    erf->BindInput(erf_input).BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(stderr, "wave_npu_poc14b: size compile failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: rejected NBG size %zu\n",
            nbg_size);
        return 6;
    }
    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(stderr, "wave_npu_poc14b: binary compile failed\n");
        return 7;
    }
    if (
        written_size < kMinimumNbgBytes
        || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: rejected written size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);
    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(stderr, "wave_npu_poc14b: invalid NBG magic\n");
        return 9;
    }
    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product_id = ReadLe32(nbg.data() + 8);
    if (
        version != kWaveNbgVersion
        || product_id != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_poc14b: unexpected NBG version/target\n");
        return 14;
    }
    if (!WriteAtomicExclusive(
            candidate->output_path,
            candidate->partial_path,
            nbg)) {
        return 11;
    }
    alarm(0);
    std::printf(
        "RESULT status=PASS mode=compile-only target=wave "
        "candidate=%s bytes=%zu version=0x%08x product=0x%08x "
        "output=%s\n",
        candidate->label,
        nbg.size(),
        version,
        product_id,
        candidate->output_path);
    std::fflush(stdout);
    return 0;
}
