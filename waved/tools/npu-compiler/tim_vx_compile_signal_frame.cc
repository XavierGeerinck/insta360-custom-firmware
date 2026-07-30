// SPDX-License-Identifier: MIT
//
// Fixed compiler probe for the Wave EVIS SignalFrame gate.
//
// This process constructs one fixed FLOAT16 SignalFrame graph and asks TIM-VX
// to compile it to an NBG. It never invokes graph execution, directly opens a
// device node, or accepts a model/operation definition from the command line.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops/signal_frame.h"
#include "tim/vx/tensor.h"
#include "tim/vx/types.h"

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

// Recovered from the exact libGAL wrapper and its internal copy routine:
// gcoHAL_QueryChipIdentityEx(Hal, sizeof(ChipIdentityEx), &identity).
extern "C" int gcoHAL_QueryChipIdentityEx(
    void* hal,
    uint32_t identity_size,
    ChipIdentityEx* identity);

namespace {

constexpr size_t kMinimumNbgBytes = 12;
constexpr size_t kMaximumNbgBytes = 64U * 1024U * 1024U;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 15;
constexpr char kExpectedSdkDirectory[] = "/tmp/wave-npu-compile/sdk";
constexpr char kCandidate1Path[] =
    "/tmp/wave-npu-compile/output/wave-signal-frame-candidate-1.nbg";
constexpr char kCandidate2Path[] =
    "/tmp/wave-npu-compile/output/wave-signal-frame-candidate-2.nbg";
constexpr char kCandidate1PartialPath[] =
    "/tmp/wave-npu-compile/output/.wave-signal-frame-candidate-1.nbg.partial";
constexpr char kCandidate2PartialPath[] =
    "/tmp/wave-npu-compile/output/.wave-signal-frame-candidate-2.nbg.partial";

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
            "wave_npu_compile_signal_frame: output already exists: %s\n",
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

    const int fd = open(
        partial_path,
        O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
        0600);
    if (fd < 0) {
        std::fprintf(
            stderr, "open(%s): %s\n", partial_path, std::strerror(errno));
        return false;
    }

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
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr, "write(%s): zero-byte write\n", partial_path);
            close(fd);
            unlink(partial_path);
            return false;
        }
        written += static_cast<size_t>(count);
    }

    if (fchmod(fd, 0444) != 0) {
        std::fprintf(
            stderr, "fchmod(%s): %s\n", partial_path, std::strerror(errno));
        close(fd);
        unlink(partial_path);
        return false;
    }
    if (fsync(fd) != 0) {
        std::fprintf(
            stderr, "fsync(%s): %s\n", partial_path, std::strerror(errno));
        close(fd);
        unlink(partial_path);
        return false;
    }
    if (close(fd) != 0) {
        std::fprintf(
            stderr, "close(%s): %s\n", partial_path, std::strerror(errno));
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
            stderr, "unlink(%s): %s\n", partial_path, std::strerror(errno));
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
        "\"candidates\":[1,2],\"operation\":\"SignalFrame\","
        "\"backend_intent\":\"EVIS\","
        "\"parameters\":{\"window_length\":8,\"step\":4,"
        "\"pad_end\":0,\"axis\":0},"
        "\"data_type\":\"FLOAT16\","
        "\"inputs\":[{\"shape\":[32]}],"
        "\"outputs\":[{\"shape\":[8,7]}],\"executes_graph\":false}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 ||
        (std::strcmp(argv[1], "--candidate-1") != 0 &&
         std::strcmp(argv[1], "--candidate-2") != 0)) {
        std::fprintf(stderr, "usage: %s --candidate-1\n", argv[0]);
        std::fprintf(stderr, "       %s --candidate-2\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }

    const bool is_candidate_1 =
        std::strcmp(argv[1], "--candidate-1") == 0;
    const char* const candidate_name =
        is_candidate_1 ? "candidate-1" : "candidate-2";
    const char* const output_path =
        is_candidate_1 ? kCandidate1Path : kCandidate2Path;
    const char* const partial_path =
        is_candidate_1 ? kCandidate1PartialPath : kCandidate2PartialPath;

    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    if (sdk_directory == nullptr ||
        std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: VIVANTE_SDK_DIR must equal %s\n",
            kExpectedSdkDirectory);
        return 12;
    }
    if (std::getenv("VSIMULATOR_CONFIG") != nullptr ||
        std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: simulator and loader "
            "interposition variables must be unset\n");
        return 12;
    }
    if (!PathIsAbsent(output_path) || !PathIsAbsent(partial_path)) {
        return 16;
    }

    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    g_timeout_partial_path = partial_path;
    alarm(kWatchdogSeconds);

    std::fprintf(
        stderr,
        "wave_npu_compile_signal_frame: mode=compile-only target=wave "
        "candidate=%s watchdog_seconds=%u\n",
        candidate_name,
        kWatchdogSeconds);

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr, "wave_npu_compile_signal_frame: context create failed\n");
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
            "wave_npu_compile_signal_frame: chip identity query failed: %d\n",
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

    if (identity.chip_model != kWaveChipModel ||
        identity.customer_id != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: identity mismatch: "
            "expected_model=0x%08x actual_model=0x%08x "
            "expected_customer_id=0x%08x actual_customer_id=0x%08x\n",
            kWaveChipModel,
            identity.chip_model,
            kWaveProductId,
            identity.customer_id);
        return 15;
    }

    auto graph = context->CreateGraph();
    if (!graph) {
        std::fprintf(
            stderr, "wave_npu_compile_signal_frame: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType input_shape{32};
    const tim::vx::ShapeType output_shape{8, 7};
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
            stderr, "wave_npu_compile_signal_frame: tensor create failed\n");
        return 3;
    }

    auto signal_frame =
        graph->CreateOperation<tim::vx::ops::SignalFrame>(8, 4, 0, 0);
    if (!signal_frame) {
        std::fprintf(
            stderr, "wave_npu_compile_signal_frame: operation create failed\n");
        return 4;
    }
    signal_frame->BindInput(input).BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: size-only compile failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: rejected NBG size %zu\n",
            nbg_size);
        return 6;
    }

    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(
            stderr, "wave_npu_compile_signal_frame: binary compile failed\n");
        return 7;
    }
    if (written_size < kMinimumNbgBytes || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: rejected written size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);

    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(
            stderr, "wave_npu_compile_signal_frame: invalid NBG magic\n");
        return 9;
    }

    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product = ReadLe32(nbg.data() + 8);
    if (version != kWaveNbgVersion) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: NBG version mismatch: "
            "expected=0x%08x actual=0x%08x\n",
            kWaveNbgVersion,
            version);
        return 19;
    }
    if (product != kWaveProductId) {
        std::fprintf(
            stderr,
            "wave_npu_compile_signal_frame: target mismatch: "
            "expected=0x%08x actual=0x%08x\n",
            kWaveProductId,
            product);
        return 14;
    }
    if (!WriteAtomicExclusive(output_path, partial_path, nbg)) {
        return 11;
    }
    alarm(0);

    std::printf(
        "RESULT status=PASS mode=compile-only target=wave "
        "candidate=%s bytes=%zu version=0x%08x product=0x%08x output=%s\n",
        candidate_name,
        nbg.size(),
        version,
        product,
        output_path);
    std::fflush(stdout);
    return 0;
}
