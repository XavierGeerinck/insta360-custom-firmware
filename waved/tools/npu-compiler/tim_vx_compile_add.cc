// SPDX-License-Identifier: MIT
//
// Fixed compiler probe for the first Wave custom-graph gate.
//
// This process constructs one fixed UINT8 Add graph and asks TIM-VX to
// compile it to an NBG. It never invokes graph execution, directly opens a
// device node, or accepts a model/operation definition from the command line.

#include "tim/vx/context.h"
#include "tim/vx/graph.h"
#include "tim/vx/ops.h"
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
#include <string>
#include <sys/stat.h>
#ifdef WAVE_DEVICE_TARGET
#include <sys/prctl.h>
#endif
#include <unistd.h>
#include <vector>

#ifndef WAVE_DEVICE_TARGET
// Exported by the pinned x86 simulator's libOpenVX.so. Binary inspection
// shows that the argument is an index into libEmulator.so::vsimConfigs, not
// a product ID. The call only sets the simulator target selector and disables
// simulator execution; this program still contains no graph-execution call.
extern "C" int vivSetVSimulatorTarget(int target_index);
extern "C" void gcoOS_SetVSimulatorTarget(
    uint32_t target_index,
    uint32_t skipped_execution);
#endif

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
constexpr int kWaveSimulatorProfileIndex = 157;
constexpr uint32_t kWaveProductId = 0x1000000eU;
constexpr uint32_t kDefaultProductId = 0x10000020U;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;

#ifdef WAVE_DEVICE_TARGET
constexpr unsigned int kWatchdogSeconds = 15;
constexpr char kExpectedSdkDirectory[] = "/tmp/wave-npu-compile/sdk";
constexpr char kCandidate1Path[] =
    "/tmp/wave-npu-compile/output/wave-add-candidate-1.nbg";
constexpr char kCandidate2Path[] =
    "/tmp/wave-npu-compile/output/wave-add-candidate-2.nbg";
constexpr char kCandidate1PartialPath[] =
    "/tmp/wave-npu-compile/output/.wave-add-candidate-1.nbg.partial";
constexpr char kCandidate2PartialPath[] =
    "/tmp/wave-npu-compile/output/.wave-add-candidate-2.nbg.partial";

const char* g_timeout_output_path = nullptr;
const char* g_timeout_partial_path = nullptr;

void WatchdogHandler(int) {
    if (g_timeout_output_path != nullptr) {
        unlink(g_timeout_output_path);
    }
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
#endif

#ifndef WAVE_DEVICE_TARGET
enum class TargetMode {
    kDefault,
    kWave,
};
#endif

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
            stderr, "wave_npu_compile_add: output already exists: %s\n", path);
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
        unlink(path);
        return false;
    }
    return true;
}

void PrintDefinition() {
#ifdef WAVE_DEVICE_TARGET
    std::puts(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target\":{\"chip_model\":\"0x00009000\","
        "\"customer_id\":\"0x1000000e\","
        "\"nbg_version\":\"0x00020000\"},"
        "\"candidates\":[1,2],\"operation\":\"Add\","
        "\"data_type\":\"UINT8\","
        "\"quantization\":{\"type\":\"ASYMMETRIC\",\"scale\":1.0,"
        "\"zero_point\":0},\"inputs\":[{\"shape\":[2]},{\"shape\":[2]}],"
        "\"outputs\":[{\"shape\":[2]}],\"executes_graph\":false}");
#else
    std::puts(
        "{\"schema_version\":1,\"mode\":\"compile-only\","
        "\"target_modes\":{\"default\":{\"product_id\":\"0x10000020\"},"
        "\"wave\":{\"profile_index\":157,"
        "\"product_id\":\"0x1000000e\"}},"
        "\"operation\":\"Add\",\"data_type\":\"UINT8\","
        "\"quantization\":{\"type\":\"ASYMMETRIC\",\"scale\":1.0,"
        "\"zero_point\":0},\"inputs\":[{\"shape\":[2]},{\"shape\":[2]}],"
        "\"outputs\":[{\"shape\":[2]}],\"executes_graph\":false}");
#endif
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
#ifdef WAVE_DEVICE_TARGET
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
    const char* const target_name = "wave";

    const char* const sdk_directory = std::getenv("VIVANTE_SDK_DIR");
    if (sdk_directory == nullptr ||
        std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: VIVANTE_SDK_DIR must equal %s\n",
            kExpectedSdkDirectory);
        return 12;
    }
    if (std::getenv("VSIMULATOR_CONFIG") != nullptr ||
        std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: simulator and loader interposition "
            "variables must be unset\n");
        return 12;
    }
    if (!PathIsAbsent(output_path) || !PathIsAbsent(partial_path)) {
        return 16;
    }

    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr, "wave_npu_compile_add: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    g_timeout_output_path = output_path;
    g_timeout_partial_path = partial_path;
    alarm(kWatchdogSeconds);

    std::fprintf(
        stderr,
        "wave_npu_compile_add: mode=compile-only target=wave "
        "candidate=%s watchdog_seconds=%u\n",
        candidate_name,
        kWatchdogSeconds);
#else
    if (argc != 3 ||
        (std::strcmp(argv[1], "--default") != 0 &&
         std::strcmp(argv[1], "--wave") != 0)) {
        std::fprintf(
            stderr, "usage: %s --default <new-output.nbg>\n", argv[0]);
        std::fprintf(
            stderr, "       %s --wave <new-output.nbg>\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }

    if (std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: VSIMULATOR_CONFIG must be unset; "
            "use a fixed target mode\n");
        return 12;
    }

    const TargetMode target_mode =
        std::strcmp(argv[1], "--wave") == 0
            ? TargetMode::kWave
            : TargetMode::kDefault;
    const char* const target_name =
        target_mode == TargetMode::kWave ? "wave" : "default";
    const char* const output_path = argv[2];
    const std::string partial_path_storage =
        std::string(output_path) + ".partial";
    const char* const partial_path = partial_path_storage.c_str();
    if (!PathIsAbsent(output_path) || !PathIsAbsent(partial_path)) {
        return 16;
    }

    if (target_mode == TargetMode::kWave) {
        const int status =
            vivSetVSimulatorTarget(kWaveSimulatorProfileIndex);
        if (status != 0) {
            std::fprintf(
                stderr,
                "wave_npu_compile_add: target selection failed: %d\n",
                status);
            return 13;
        }
        // vxCreateContext calls this same libGAL routine, but only after
        // earlier initialization has already queried the default identity.
        // Set the fixed selector before any context/HAL object can exist.
        gcoOS_SetVSimulatorTarget(
            static_cast<uint32_t>(kWaveSimulatorProfileIndex), 1U);
    }

    std::fprintf(
        stderr,
        "wave_npu_compile_add: mode=compile-only target=%s "
        "profile_index=%d\n",
        target_name,
        target_mode == TargetMode::kWave
            ? kWaveSimulatorProfileIndex
            : -1);
#endif

    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(stderr, "wave_npu_compile_add: context create failed\n");
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
            "wave_npu_compile_add: chip identity query failed: %d\n",
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

#ifdef WAVE_DEVICE_TARGET
    constexpr uint32_t kExpectedIdentityCustomerId = kWaveProductId;
#else
    const uint32_t kExpectedIdentityCustomerId =
        target_mode == TargetMode::kWave
            ? kWaveProductId
            : kDefaultProductId;
#endif
    if (identity.chip_model != kWaveChipModel ||
        identity.customer_id != kExpectedIdentityCustomerId) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: identity mismatch: "
            "expected_model=0x%08x actual_model=0x%08x "
            "expected_customer_id=0x%08x actual_customer_id=0x%08x\n",
            kWaveChipModel,
            identity.chip_model,
            kExpectedIdentityCustomerId,
            identity.customer_id);
        return 15;
    }

    auto graph = context->CreateGraph();
    if (!graph) {
        std::fprintf(stderr, "wave_npu_compile_add: graph create failed\n");
        return 2;
    }

    const tim::vx::ShapeType shape{2};
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

    auto input_a = graph->CreateTensor(input_spec);
    auto input_b = graph->CreateTensor(input_spec);
    auto output = graph->CreateTensor(output_spec);
    if (!input_a || !input_b || !output) {
        std::fprintf(stderr, "wave_npu_compile_add: tensor create failed\n");
        return 3;
    }

    auto add = graph->CreateOperation<tim::vx::ops::Add>();
    if (!add) {
        std::fprintf(stderr, "wave_npu_compile_add: Add create failed\n");
        return 4;
    }
    add->BindInput(input_a).BindInput(input_b).BindOutput(output);

    size_t nbg_size = 0;
    if (!graph->CompileToBinary(nullptr, &nbg_size)) {
        std::fprintf(
            stderr, "wave_npu_compile_add: size-only compile failed\n");
        return 5;
    }
    if (nbg_size < kMinimumNbgBytes || nbg_size > kMaximumNbgBytes) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: rejected NBG size %zu\n",
            nbg_size);
        return 6;
    }

    std::vector<uint8_t> nbg(nbg_size);
    size_t written_size = nbg.size();
    if (!graph->CompileToBinary(nbg.data(), &written_size)) {
        std::fprintf(stderr, "wave_npu_compile_add: binary compile failed\n");
        return 7;
    }
    if (written_size < kMinimumNbgBytes || written_size > nbg.size()) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: rejected written size %zu\n",
            written_size);
        return 8;
    }
    nbg.resize(written_size);

    if (std::memcmp(nbg.data(), "VPMN", 4) != 0) {
        std::fprintf(stderr, "wave_npu_compile_add: invalid NBG magic\n");
        return 9;
    }

    const uint32_t version = ReadLe32(nbg.data() + 4);
    const uint32_t product = ReadLe32(nbg.data() + 8);
#ifdef WAVE_DEVICE_TARGET
    constexpr uint32_t expected_product = kWaveProductId;
    if (version != kWaveNbgVersion) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: NBG version mismatch: "
            "expected=0x%08x actual=0x%08x\n",
            kWaveNbgVersion,
            version);
        return 19;
    }
#else
    const uint32_t expected_product =
        target_mode == TargetMode::kWave
            ? kWaveProductId
            : kDefaultProductId;
#endif
    if (product != expected_product) {
        std::fprintf(
            stderr,
            "wave_npu_compile_add: target mismatch: expected=0x%08x "
            "actual=0x%08x\n",
            expected_product,
            product);
        return 14;
    }
    if (!WriteAtomicExclusive(output_path, partial_path, nbg)) {
        return 11;
    }

#ifdef WAVE_DEVICE_TARGET
    std::printf(
        "RESULT status=PASS mode=compile-only target=%s candidate=%s "
        "bytes=%zu version=0x%08x product=0x%08x output=%s\n",
        target_name,
        candidate_name,
        nbg.size(),
        version,
        product,
        output_path);
#else
    std::printf(
        "RESULT status=PASS mode=compile-only target=%s bytes=%zu "
        "version=0x%08x product=0x%08x\n",
        target_name,
        nbg.size(),
        version,
        product);
#endif
    std::fflush(stdout);
    return 0;
}
