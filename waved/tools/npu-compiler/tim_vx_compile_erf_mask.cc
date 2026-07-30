// SPDX-License-Identifier: MIT
//
// Fixed compile-only producer for the PoC 13B learned Erf-mask partition.
//
// This process can construct only one UINT8[481] Erf graph. It never loads or
// executes an NBG, transfers tensor data, or directly opens a device node,
// or accepts an arbitrary graph, tensor, path, library, or retry argument.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/erf.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
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
constexpr char kExpectedSdkDirectory[] = "/tmp/wave-npu-poc13b/sdk";
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc13b/lib:/usr/lib";
constexpr char kA1Path[] =
    "/tmp/wave-npu-poc13b/output/erf-mask-a1.nbg";
constexpr char kA2Path[] =
    "/tmp/wave-npu-poc13b/output/erf-mask-a2.nbg";
constexpr char kA1PartialPath[] =
    "/tmp/wave-npu-poc13b/output/.erf-mask-a1.nbg.partial";
constexpr char kA2PartialPath[] =
    "/tmp/wave-npu-poc13b/output/.erf-mask-a2.nbg.partial";

struct Candidate {
    const char* argument;
    const char* id;
    const char* output_path;
    const char* partial_path;
};

const char* g_timeout_partial_path = nullptr;

void WatchdogHandler(int) {
    if (g_timeout_partial_path != nullptr) {
        unlink(g_timeout_partial_path);
    }
    constexpr char kMessage[] =
        "RESULT status=TIMEOUT mode=compile-only exit=124\n";
    const ssize_t count =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)count;
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
            stderr, "wave_npu_poc13b: output already exists: %s\n", path);
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
            write(descriptor, data.data() + written, data.size() - written);
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
                stderr, "write(%s): zero-byte write\n", partial_path);
            close(descriptor);
            unlink(partial_path);
            return false;
        }
        written += static_cast<size_t>(count);
    }

    if (fchmod(descriptor, 0444) != 0 || fsync(descriptor) != 0) {
        std::fprintf(
            stderr,
            "finalize(%s): %s\n",
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

const Candidate* SelectCandidate(const char* argument) {
    static constexpr Candidate kCandidates[] = {
        {"--a1", "a1", kA1Path, kA1PartialPath},
        {"--a2", "a2", kA2Path, kA2PartialPath},
    };
    for (const Candidate& candidate : kCandidates) {
        if (std::strcmp(argument, candidate.argument) == 0) {
            return &candidate;
        }
    }
    return nullptr;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target\":{\"chip_model\":\"0x00009000\","
        "\"customer_id\":\"0x1000000e\","
        "\"nbg_version\":\"0x00020000\"},"
        "\"candidates\":[\"a1\",\"a2\"],\"operation\":\"Erf\","
        "\"input\":{\"attribute\":\"INPUT\",\"data_type\":\"UINT8\","
        "\"shape\":[481],\"quantization\":{\"type\":\"ASYMMETRIC\","
        "\"scale\":0.03125,\"zero_point\":128}},"
        "\"output\":{\"attribute\":\"OUTPUT\",\"data_type\":\"UINT8\","
        "\"shape\":[481],\"quantization\":{\"type\":\"ASYMMETRIC\","
        "\"scale\":0.0078125,\"zero_point\":128}},"
        "\"compile_to_binary_calls\":2,\"executes_graph\":false}");
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
        std::fprintf(stderr, "usage: %s --a1|--a2|--describe\n", argv[0]);
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
            "wave_npu_poc13b: fixed SDK/library environment required\n");
        return 12;
    }
    if (
        std::getenv("VSIMULATOR_CONFIG") != nullptr
        || std::getenv("LD_PRELOAD") != nullptr
        || std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_poc13b: simulator/interposition variables forbidden\n");
        return 12;
    }
    if (
        !PathIsAbsent(candidate->output_path)
        || !PathIsAbsent(candidate->partial_path)) {
        return 16;
    }

    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(stderr, "wave_npu_poc13b: parent-death guard failed\n");
        return 17;
    }
    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(stderr, "wave_npu_poc13b: watchdog setup failed\n");
        return 18;
    }
    g_timeout_partial_path = candidate->partial_path;
    alarm(kWatchdogSeconds);

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(stderr, "wave_npu_poc13b: context create failed\n");
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
            "wave_npu_poc13b: chip identity query failed: %d\n",
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
        std::fprintf(stderr, "wave_npu_poc13b: live target mismatch\n");
        return 15;
    }

    auto graph = context->CreateGraph();
    if (!graph) {
        std::fprintf(stderr, "wave_npu_poc13b: graph create failed\n");
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
        std::fprintf(stderr, "wave_npu_poc13b: tensor create failed\n");
        return 3;
    }
    auto erf = graph->CreateOperation<tim::vx::ops::Erf>();
    if (!erf) {
        std::fprintf(stderr, "wave_npu_poc13b: Erf create failed\n");
        return 4;
    }
    erf->BindInput(input).BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(stderr, "wave_npu_poc13b: size compile failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr, "wave_npu_poc13b: rejected NBG size %zu\n", nbg_size);
        return 6;
    }
    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(stderr, "wave_npu_poc13b: binary compile failed\n");
        return 7;
    }
    if (written_size < kMinimumNbgBytes || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_poc13b: rejected written size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);
    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(stderr, "wave_npu_poc13b: invalid NBG magic\n");
        return 9;
    }
    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product = ReadLe32(nbg.data() + 8);
    if (version != kWaveNbgVersion || product != kWaveProductId) {
        std::fprintf(stderr, "wave_npu_poc13b: NBG target mismatch\n");
        return 14;
    }
    if (!WriteAtomicExclusive(
            candidate->output_path, candidate->partial_path, nbg)) {
        return 11;
    }
    alarm(0);

    std::printf(
        "RESULT status=PASS mode=compile-only target=wave "
        "candidate=%s bytes=%zu version=0x%08x product=0x%08x output=%s\n",
        candidate->id,
        nbg.size(),
        version,
        product,
        candidate->output_path);
    std::fflush(stdout);
    return 0;
}
