// SPDX-License-Identifier: MIT
//
// Fixed compile-only probe for the PoC 12B Wave NBG constant differential.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/erf.h"
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
constexpr char kExpectedSdkDirectory[] = "/tmp/wave-npu-poc12b/sdk";
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc12b/lib:/usr/lib";
constexpr char kA1Path[] =
    "/tmp/wave-npu-poc12b/output/erf-constant-a1.nbg";
constexpr char kA2Path[] =
    "/tmp/wave-npu-poc12b/output/erf-constant-a2.nbg";
constexpr char kBPath[] =
    "/tmp/wave-npu-poc12b/output/erf-constant-b.nbg";
constexpr char kA1PartialPath[] =
    "/tmp/wave-npu-poc12b/output/.erf-constant-a1.nbg.partial";
constexpr char kA2PartialPath[] =
    "/tmp/wave-npu-poc12b/output/.erf-constant-a2.nbg.partial";
constexpr char kBPartialPath[] =
    "/tmp/wave-npu-poc12b/output/.erf-constant-b.nbg.partial";
constexpr std::array<uint8_t, 4> kConstantA{{0, 1, 2, 3}};
constexpr std::array<uint8_t, 4> kConstantB{{0, 1, 2, 4}};

struct Candidate {
    const char* argument;
    const char* label;
    const char* output_path;
    const char* partial_path;
    const std::array<uint8_t, 4>* constant;
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

bool PathIsAbsent(const char* path) {
    struct stat path_stat {};
    if (lstat(path, &path_stat) == 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: output already exists: %s\n",
            path);
        return false;
    }
    if (errno != ENOENT) {
        std::fprintf(stderr, "lstat(%s): %s\n", path, std::strerror(errno));
        return false;
    }
    return true;
}

bool WriteAtomicExclusive(
    const char* path,
    const char* partial_path,
    const std::vector<uint8_t>& data) {
    if (!PathIsAbsent(path) || !PathIsAbsent(partial_path)) {
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
        const ssize_t count =
            write(
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
    if (fchmod(descriptor, 0444) != 0) {
        std::fprintf(
            stderr,
            "fchmod(%s): %s\n",
            partial_path,
            std::strerror(errno));
        close(descriptor);
        unlink(partial_path);
        return false;
    }
    if (fsync(descriptor) != 0) {
        std::fprintf(
            stderr,
            "fsync(%s): %s\n",
            partial_path,
            std::strerror(errno));
        close(descriptor);
        unlink(partial_path);
        return false;
    }
    if (close(descriptor) != 0) {
        std::fprintf(
            stderr,
            "close(%s): %s\n",
            partial_path,
            std::strerror(errno));
        unlink(partial_path);
        return false;
    }
    if (link(partial_path, path) != 0) {
        std::fprintf(
            stderr,
            "link(%s, %s): %s\n",
            partial_path,
            path,
            std::strerror(errno));
        unlink(partial_path);
        return false;
    }
    if (unlink(partial_path) != 0) {
        std::fprintf(
            stderr,
            "unlink(%s): %s\n",
            partial_path,
            std::strerror(errno));
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target\":{\"chip_model\":\"0x00009000\","
        "\"customer_id\":\"0x1000000e\","
        "\"nbg_version\":\"0x00020000\"},"
        "\"candidates\":{\"A1\":[0,1,2,3],\"A2\":[0,1,2,3],"
        "\"B\":[0,1,2,4]},\"operation\":\"Erf\","
        "\"input_attribute\":\"CONSTANT\",\"data_type\":\"UINT8\","
        "\"shape\":[4],\"quantization\":{\"type\":\"ASYMMETRIC\","
        "\"scale\":1.0,\"zero_point\":0},\"output_count\":1,"
        "\"executes_graph\":false}");
}

const Candidate* SelectCandidate(const char* argument) {
    static constexpr Candidate kCandidates[] = {
        {"--a1", "A1", kA1Path, kA1PartialPath, &kConstantA},
        {"--a2", "A2", kA2Path, kA2PartialPath, &kConstantA},
        {"--b", "B", kBPath, kBPartialPath, &kConstantB},
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
    const Candidate* candidate =
        argc == 2 ? SelectCandidate(argv[1]) : nullptr;
    if (candidate == nullptr) {
        std::fprintf(stderr, "usage: %s --a1|--a2|--b|--describe\n", argv[0]);
        return 10;
    }

    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    const char* const library_path = std::getenv("LD_LIBRARY_PATH");
    if (
        sdk_directory == nullptr
        || std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0
        || library_path == nullptr
        || std::strcmp(library_path, kExpectedLibraryPath) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: fixed SDK/library environment required\n");
        return 12;
    }
    if (
        std::getenv("VSIMULATOR_CONFIG") != nullptr
        || std::getenv("LD_PRELOAD") != nullptr
        || std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: simulator/interposition variables forbidden\n");
        return 12;
    }
    if (
        !PathIsAbsent(candidate->output_path)
        || !PathIsAbsent(candidate->partial_path)) {
        return 16;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    g_timeout_partial_path = candidate->partial_path;
    alarm(kWatchdogSeconds);

    std::fprintf(
        stderr,
        "wave_npu_poc12b: mode=compile-only candidate=%s "
        "watchdog_seconds=%u\n",
        candidate->label,
        kWatchdogSeconds);

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(stderr, "wave_npu_poc12b: context create failed\n");
        return 1;
    }

    ChipIdentityEx identity {};
    const int identity_status = gcoHAL_QueryChipIdentityEx(
        nullptr,
        static_cast<uint32_t>(sizeof(identity)),
        &identity);
    if (identity_status != 0) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: chip identity query failed: %d\n",
            identity_status);
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
    if (
        identity.chip_model != kWaveChipModel
        || identity.customer_id != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: unexpected chip identity\n");
        return 15;
    }

    auto graph = context->CreateGraph();
    if (!graph) {
        std::fprintf(stderr, "wave_npu_poc12b: graph create failed\n");
        return 2;
    }
    const tim::vx::ShapeType shape{4};
    const tim::vx::Quantization quantization(
        tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);
    const tim::vx::TensorSpec constant_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::CONSTANT,
        quantization);
    const tim::vx::TensorSpec output_spec(
        tim::vx::DataType::UINT8,
        shape,
        tim::vx::TensorAttribute::OUTPUT,
        quantization);
    auto constant =
        graph->CreateTensor(constant_spec, candidate->constant->data());
    auto output = graph->CreateTensor(output_spec);
    if (!constant || !output) {
        std::fprintf(stderr, "wave_npu_poc12b: tensor create failed\n");
        return 3;
    }
    auto erf = graph->CreateOperation<tim::vx::ops::Erf>();
    if (!erf) {
        std::fprintf(stderr, "wave_npu_poc12b: Erf create failed\n");
        return 4;
    }
    erf->BindInput(constant).BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(stderr, "wave_npu_poc12b: size compile failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: rejected NBG size %zu\n",
            nbg_size);
        return 6;
    }
    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(stderr, "wave_npu_poc12b: binary compile failed\n");
        return 7;
    }
    if (
        written_size < kMinimumNbgBytes
        || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: rejected written size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);
    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(stderr, "wave_npu_poc12b: invalid NBG magic\n");
        return 9;
    }
    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product = ReadLe32(nbg.data() + 8);
    if (
        version != kWaveNbgVersion
        || product != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_poc12b: unexpected NBG version/target\n");
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
        "RESULT status=PASS mode=compile-only candidate=%s "
        "bytes=%zu version=0x%08x product=0x%08x output=%s\n",
        candidate->label,
        nbg.size(),
        version,
        product,
        candidate->output_path);
    std::fflush(stdout);
    return 0;
}
