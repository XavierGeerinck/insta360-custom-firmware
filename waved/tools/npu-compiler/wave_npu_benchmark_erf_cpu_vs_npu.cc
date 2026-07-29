// SPDX-License-Identifier: MIT
//
// PoC 13G: compare one fixed Cortex-A55 UINT8 Erf lookup with the already
// qualified Erf NBG on the same Wave device. Its model, tensors, counts,
// paths, and libraries are compile-time constants.

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

constexpr size_t kColdFrames = 1;
constexpr size_t kWarmupFrames = 100;
constexpr size_t kMeasuredFrames = 10000;
constexpr size_t kTotalFrames =
    kColdFrames + kWarmupFrames + kMeasuredFrames;
constexpr size_t kFrameWidth = 481;
constexpr size_t kExpectedNbgBytes = 5840;
constexpr size_t kCanaryBytes = 32;
constexpr uint8_t kCanaryValue = 0xa5;
constexpr uint32_t kInputSeed = 0x5733130fU;
constexpr size_t kCalibrationSamples = 100000;
constexpr uint64_t kFnvOffsetBasis = UINT64_C(0xcbf29ce484222325);
constexpr uint64_t kFnvPrime = UINT64_C(0x100000001b3);
constexpr uint64_t kExpectedInputFnv =
    UINT64_C(0x367c7eaad959d707);
constexpr uint64_t kExpectedOutputFnv =
    UINT64_C(0x742e3a62d58acd0d);
constexpr uint32_t kExpectedFinalState = 0x6ae16fe0U;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveCustomerId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 50;
constexpr uint32_t kTimingSchemaVersion = 1;
constexpr size_t kTimingHeaderBytes = 36;
constexpr size_t kTimingRecordBytes = 64;
constexpr size_t kExpectedTimingBytes =
    kTimingHeaderBytes + kMeasuredFrames * kTimingRecordBytes;

constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc13g/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-poc13g/model/erf-mask.nbg";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-poc13g/lib/libCLC.so";
constexpr char kTimingPath[] =
    "/tmp/wave-npu-poc13g/result/erf-cpu-npu-timings-v1.bin";
constexpr char kPartialTimingPath[] =
    "/tmp/wave-npu-poc13g/result/erf-cpu-npu-timings-v1.partial";

constexpr std::array<uint8_t, 256> kErfLut = {
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02,
    0x02, 0x02, 0x02, 0x02, 0x03, 0x03, 0x03, 0x04,
    0x04, 0x05, 0x05, 0x06, 0x07, 0x07, 0x08, 0x09,
    0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x10, 0x11, 0x13,
    0x14, 0x16, 0x18, 0x1a, 0x1c, 0x1e, 0x20, 0x22,
    0x25, 0x28, 0x2a, 0x2d, 0x30, 0x33, 0x37, 0x3a,
    0x3d, 0x41, 0x45, 0x48, 0x4c, 0x50, 0x54, 0x58,
    0x5d, 0x61, 0x65, 0x6a, 0x6e, 0x72, 0x77, 0x7b,
    0x80, 0x85, 0x89, 0x8e, 0x92, 0x96, 0x9b, 0x9f,
    0xa3, 0xa8, 0xac, 0xb0, 0xb4, 0xb8, 0xbb, 0xbf,
    0xc3, 0xc6, 0xc9, 0xcd, 0xd0, 0xd3, 0xd6, 0xd8,
    0xdb, 0xde, 0xe0, 0xe2, 0xe4, 0xe6, 0xe8, 0xea,
    0xec, 0xed, 0xef, 0xf0, 0xf2, 0xf3, 0xf4, 0xf5,
    0xf6, 0xf7, 0xf8, 0xf9, 0xf9, 0xfa, 0xfb, 0xfb,
    0xfc, 0xfc, 0xfd, 0xfd, 0xfd, 0xfe, 0xfe, 0xfe,
    0xfe, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
};

struct TimingRecord {
    uint32_t sequence;
    uint32_t order;
    uint64_t cpu_wall_ns;
    uint64_t cpu_thread_ns;
    uint64_t npu_copy_in_ns;
    uint64_t npu_run_ns;
    uint64_t npu_copy_out_ns;
    uint64_t npu_total_wall_ns;
    uint64_t npu_thread_ns;
};

struct CpuTiming {
    uint64_t wall_ns;
    uint64_t thread_ns;
};

struct NpuTiming {
    uint64_t copy_in_ns;
    uint64_t run_ns;
    uint64_t copy_out_ns;
    uint64_t total_wall_ns;
    uint64_t thread_ns;
};

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "ERF_CPU_NPU_BENCH status=TIMEOUT mode=benchmark exit=124\n";
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

uint64_t UpdateFnv1a64(
    uint64_t value, const uint8_t* data, size_t bytes) {
    for (size_t index = 0; index < bytes; ++index) {
        value ^= data[index];
        value *= kFnvPrime;
    }
    return value;
}

uint32_t NextXorshift32(uint32_t value) {
    value ^= value << 13U;
    value ^= value >> 17U;
    value ^= value << 5U;
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
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "fixed environment mismatch\n");
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "interposition and simulator variables must be unset\n");
        return false;
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "isolated CLC path must be absent\n");
        return false;
    }
    if (!PathIsAbsent(kTimingPath) ||
        !PathIsAbsent(kPartialTimingPath)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "fixed timing output already exists\n");
        return false;
    }
    return true;
}

bool ReadFixedNbg(std::vector<uint8_t>* nbg) {
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
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "fixed NBG metadata mismatch\n");
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
                stderr,
                "read(%s): %s\n",
                kNbgPath,
                std::strerror(errno));
            close(fd);
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_erf_cpu_vs_npu: short NBG read\n");
            close(fd);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    uint8_t extra = 0;
    if (read(fd, &extra, 1) != 0 || close(fd) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "NBG close/length check failed\n");
        return false;
    }
    return std::memcmp(nbg->data(), "VPMN", 4) == 0 &&
           ReadLe32(nbg->data() + 4) == kWaveNbgVersion &&
           ReadLe32(nbg->data() + 8) == kWaveCustomerId;
}

bool ReadClock(clockid_t clock_id, uint64_t* value) {
    struct timespec timestamp {};
    if (clock_gettime(clock_id, &timestamp) != 0 ||
        timestamp.tv_sec < 0 || timestamp.tv_nsec < 0) {
        return false;
    }
    *value =
        static_cast<uint64_t>(timestamp.tv_sec) * UINT64_C(1000000000) +
        static_cast<uint64_t>(timestamp.tv_nsec);
    return true;
}

bool Duration(uint64_t started, uint64_t finished, uint64_t* elapsed) {
    if (finished <= started) {
        return false;
    }
    *elapsed = finished - started;
    return true;
}

template <size_t BufferBytes>
bool CanariesAreIntact(
    const std::array<uint8_t, BufferBytes>& buffer) {
    static_assert(
        BufferBytes >= 2 * kCanaryBytes,
        "canary-protected buffer is too small");
    return std::all_of(
               buffer.begin(),
               buffer.begin() + kCanaryBytes,
               [](uint8_t value) { return value == kCanaryValue; }) &&
           std::all_of(
               buffer.end() - kCanaryBytes,
               buffer.end(),
               [](uint8_t value) { return value == kCanaryValue; });
}

bool RunCpu(
    const std::array<uint8_t, kFrameWidth>& input,
    uint8_t* cpu_output,
    CpuTiming* timing) {
    uint64_t wall_started = 0;
    uint64_t thread_started = 0;
    uint64_t thread_finished = 0;
    uint64_t wall_finished = 0;
    if (!ReadClock(CLOCK_MONOTONIC_RAW, &wall_started) ||
        !ReadClock(CLOCK_THREAD_CPUTIME_ID, &thread_started)) {
        return false;
    }
    for (size_t index = 0; index < input.size(); ++index) {
        cpu_output[index] = kErfLut[input[index]];
    }
    return ReadClock(CLOCK_THREAD_CPUTIME_ID, &thread_finished) &&
           ReadClock(CLOCK_MONOTONIC_RAW, &wall_finished) &&
           Duration(wall_started, wall_finished, &timing->wall_ns) &&
           Duration(
               thread_started, thread_finished, &timing->thread_ns);
}

bool RunNpu(
    const std::array<uint8_t, kFrameWidth>& input_bytes,
    const std::shared_ptr<tim::vx::Tensor>& input,
    const std::shared_ptr<tim::vx::Tensor>& output,
    const std::shared_ptr<tim::vx::Graph>& graph,
    uint8_t* output_bytes,
    NpuTiming* timing) {
    uint64_t total_started = 0;
    uint64_t thread_started = 0;
    uint64_t copy_in_started = 0;
    uint64_t copy_in_finished = 0;
    uint64_t run_started = 0;
    uint64_t run_finished = 0;
    uint64_t copy_out_started = 0;
    uint64_t copy_out_finished = 0;
    uint64_t thread_finished = 0;
    uint64_t total_finished = 0;
    if (!ReadClock(CLOCK_MONOTONIC_RAW, &total_started) ||
        !ReadClock(CLOCK_THREAD_CPUTIME_ID, &thread_started) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &copy_in_started) ||
        !input->CopyDataToTensor(
            input_bytes.data(), input_bytes.size()) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &copy_in_finished) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &run_started) ||
        !graph->Run() ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &run_finished) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &copy_out_started) ||
        !output->CopyDataFromTensor(output_bytes) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &copy_out_finished) ||
        !ReadClock(CLOCK_THREAD_CPUTIME_ID, &thread_finished) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &total_finished)) {
        return false;
    }
    return Duration(
               copy_in_started,
               copy_in_finished,
               &timing->copy_in_ns) &&
           Duration(run_started, run_finished, &timing->run_ns) &&
           Duration(
               copy_out_started,
               copy_out_finished,
               &timing->copy_out_ns) &&
           Duration(
               total_started,
               total_finished,
               &timing->total_wall_ns) &&
           Duration(
               thread_started,
               thread_finished,
               &timing->thread_ns);
}

void AppendLe32(std::vector<uint8_t>* output, uint32_t value) {
    for (unsigned int shift = 0; shift < 32; shift += 8) {
        output->push_back(static_cast<uint8_t>(value >> shift));
    }
}

void AppendLe64(std::vector<uint8_t>* output, uint64_t value) {
    for (unsigned int shift = 0; shift < 64; shift += 8) {
        output->push_back(static_cast<uint8_t>(value >> shift));
    }
}

std::vector<uint8_t> SerializeTimings(
    uint64_t calibration_total,
    uint64_t calibration_minimum,
    const std::vector<TimingRecord>& records) {
    std::vector<uint8_t> payload;
    payload.reserve(kExpectedTimingBytes);
    payload.insert(payload.end(), {'E', 'C', 'N', '1'});
    AppendLe32(&payload, kTimingSchemaVersion);
    AppendLe32(&payload, static_cast<uint32_t>(records.size()));
    AppendLe32(&payload, static_cast<uint32_t>(kTimingRecordBytes));
    AppendLe32(&payload, static_cast<uint32_t>(kCalibrationSamples));
    AppendLe64(&payload, calibration_total);
    AppendLe64(&payload, calibration_minimum);
    for (const TimingRecord& record : records) {
        AppendLe32(&payload, record.sequence);
        AppendLe32(&payload, record.order);
        AppendLe64(&payload, record.cpu_wall_ns);
        AppendLe64(&payload, record.cpu_thread_ns);
        AppendLe64(&payload, record.npu_copy_in_ns);
        AppendLe64(&payload, record.npu_run_ns);
        AppendLe64(&payload, record.npu_copy_out_ns);
        AppendLe64(&payload, record.npu_total_wall_ns);
        AppendLe64(&payload, record.npu_thread_ns);
    }
    return payload;
}

bool WriteAll(int fd, const uint8_t* data, size_t bytes) {
    size_t offset = 0;
    while (offset < bytes) {
        const ssize_t count = write(fd, data + offset, bytes - offset);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            return false;
        }
        if (count == 0) {
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    return true;
}

bool PublishTimings(const std::vector<uint8_t>& payload) {
    if (payload.size() != kExpectedTimingBytes) {
        return false;
    }
    const int fd = open(
        kPartialTimingPath,
        O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
        0444);
    if (fd < 0) {
        return false;
    }
    const bool written = WriteAll(fd, payload.data(), payload.size());
    const bool synced = written && fsync(fd) == 0;
    const bool protected_mode = synced && fchmod(fd, 0444) == 0;
    const bool closed = close(fd) == 0;
    if (!written || !synced || !protected_mode || !closed) {
        return false;
    }
    if (rename(kPartialTimingPath, kTimingPath) != 0) {
        return false;
    }
    const int directory_fd = open(
        "/tmp/wave-npu-poc13g/result",
        O_RDONLY | O_CLOEXEC | O_DIRECTORY);
    if (directory_fd < 0) {
        return false;
    }
    const bool directory_synced = fsync(directory_fd) == 0;
    const bool directory_closed = close(directory_fd) == 0;
    return directory_synced && directory_closed;
}

bool CalibrateWallTimer(uint64_t* total, uint64_t* minimum) {
    *total = 0;
    *minimum = UINT64_MAX;
    for (size_t sample = 0; sample < kCalibrationSamples; ++sample) {
        uint64_t started = 0;
        uint64_t finished = 0;
        if (!ReadClock(CLOCK_MONOTONIC_RAW, &started) ||
            !ReadClock(CLOCK_MONOTONIC_RAW, &finished) ||
            finished < started) {
            return false;
        }
        const uint64_t elapsed = finished - started;
        *total += elapsed;
        if (elapsed > 0) {
            *minimum = std::min(*minimum, elapsed);
        }
    }
    return *total > 0 && *minimum != UINT64_MAX;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"benchmark\","
        "\"model\":\"/tmp/wave-npu-poc13g/model/erf-mask.nbg\","
        "\"model_bytes\":5840,\"model_sha256\":"
        "\"96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94\","
        "\"cpu\":{\"implementation\":\"embedded-u8-lut\","
        "\"lut_sha256\":"
        "\"c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296\"},"
        "\"input\":{\"type\":\"UINT8\",\"shape\":[481],"
        "\"generator\":\"xorshift32\",\"seed\":\"0x5733130f\"},"
        "\"cold_frames\":1,\"warmup_frames\":100,"
        "\"measured_frames\":10000,\"total_frames\":10101,"
        "\"compile_count\":1,\"cpu_count\":10101,"
        "\"npu_run_count\":10101,\"alternating_order\":true,"
        "\"wall_clock\":\"CLOCK_MONOTONIC_RAW\","
        "\"thread_clock\":\"CLOCK_THREAD_CPUTIME_ID\","
        "\"timing_schema\":\"ECN1-v1\",\"timing_bytes\":640036,"
        "\"output_canary_bytes\":32,\"executes_graph\":true}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--benchmark") != 0) {
        std::fprintf(stderr, "usage: %s --benchmark\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "parent-death gate failed\n");
        return 17;
    }
    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    alarm(kWatchdogSeconds);

    uint64_t calibration_total = 0;
    uint64_t calibration_minimum = 0;
    if (!CalibrateWallTimer(
            &calibration_total, &calibration_minimum)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "wall timer calibration failed\n");
        return 19;
    }

    std::vector<uint8_t> nbg;
    if (!ReadFixedNbg(&nbg)) {
        return 13;
    }
    auto context = tim::vx::Context::Create();
    if (!context) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: context failed\n");
        return 1;
    }
    ChipIdentityEx identity {};
    const int identity_status = gcoHAL_QueryChipIdentityEx(
        nullptr,
        static_cast<uint32_t>(sizeof(identity)),
        &identity);
    if (identity_status != 0 ||
        identity.chip_model != kWaveChipModel ||
        identity.customer_id != kWaveCustomerId) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: identity mismatch\n");
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
            "wave_npu_benchmark_erf_cpu_vs_npu: graph failed\n");
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
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: tensors failed\n");
        return 3;
    }
    auto nbg_operation = graph->CreateOperation<tim::vx::ops::NBG>(
        reinterpret_cast<const char*>(nbg.data()), 1, 1);
    if (!nbg_operation) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: NBG op failed\n");
        return 4;
    }
    nbg_operation->BindInput(input).BindOutput(output);
    if (!graph->Compile()) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: compile failed\n");
        return 6;
    }

    std::array<uint8_t, kFrameWidth> input_bytes {};
    std::array<
        uint8_t,
        kFrameWidth + 2 * kCanaryBytes>
        cpu_observed {};
    std::array<
        uint8_t,
        kFrameWidth + 2 * kCanaryBytes>
        npu_observed {};
    std::vector<TimingRecord> records;
    records.reserve(kMeasuredFrames);
    uint32_t random_state = kInputSeed;
    uint64_t input_fnv = kFnvOffsetBasis;
    uint64_t cpu_output_fnv = kFnvOffsetBasis;
    uint64_t npu_output_fnv = kFnvOffsetBasis;
    uint64_t process_started = 0;
    if (!ReadClock(CLOCK_PROCESS_CPUTIME_ID, &process_started)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "process timer start failed\n");
        return 20;
    }

    for (size_t frame_index = 0;
         frame_index < kTotalFrames;
         ++frame_index) {
        for (uint8_t& value : input_bytes) {
            random_state = NextXorshift32(random_state);
            value = static_cast<uint8_t>(random_state & 0xffU);
        }
        input_fnv = UpdateFnv1a64(
            input_fnv, input_bytes.data(), input_bytes.size());
        cpu_observed.fill(kCanaryValue);
        npu_observed.fill(kCanaryValue);
        uint8_t* const cpu_output =
            cpu_observed.data() + kCanaryBytes;
        uint8_t* const npu_output =
            npu_observed.data() + kCanaryBytes;
        CpuTiming cpu_timing {};
        NpuTiming npu_timing {};
        const bool measured =
            frame_index >= kColdFrames + kWarmupFrames;
        const uint32_t sequence = measured
            ? static_cast<uint32_t>(
                  frame_index - kColdFrames - kWarmupFrames)
            : 0;
        const uint32_t order = measured
            ? sequence & 1U
            : static_cast<uint32_t>(frame_index & 1U);
        bool paths_passed = false;
        if (order == 0) {
            paths_passed =
                RunCpu(input_bytes, cpu_output, &cpu_timing) &&
                RunNpu(
                    input_bytes,
                    input,
                    output,
                    graph,
                    npu_output,
                    &npu_timing);
        } else {
            paths_passed =
                RunNpu(
                    input_bytes,
                    input,
                    output,
                    graph,
                    npu_output,
                    &npu_timing) &&
                RunCpu(input_bytes, cpu_output, &cpu_timing);
        }
        if (!paths_passed) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_erf_cpu_vs_npu: "
                "timed path failed at %zu\n",
                frame_index);
            return 7;
        }
        if (!CanariesAreIntact(cpu_observed) ||
            !CanariesAreIntact(npu_observed)) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_erf_cpu_vs_npu: "
                "canary changed at %zu\n",
                frame_index);
            return 8;
        }
        if (std::memcmp(cpu_output, npu_output, kFrameWidth) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_erf_cpu_vs_npu: "
                "CPU/NPU mismatch at %zu\n",
                frame_index);
            return 9;
        }
        cpu_output_fnv = UpdateFnv1a64(
            cpu_output_fnv, cpu_output, kFrameWidth);
        npu_output_fnv = UpdateFnv1a64(
            npu_output_fnv, npu_output, kFrameWidth);

        if (frame_index == 0) {
            std::printf(
                "COLD cpu_wall_ns=%llu npu_copy_in_ns=%llu "
                "npu_run_ns=%llu npu_copy_out_ns=%llu "
                "npu_total_ns=%llu\n",
                static_cast<unsigned long long>(cpu_timing.wall_ns),
                static_cast<unsigned long long>(
                    npu_timing.copy_in_ns),
                static_cast<unsigned long long>(npu_timing.run_ns),
                static_cast<unsigned long long>(
                    npu_timing.copy_out_ns),
                static_cast<unsigned long long>(
                    npu_timing.total_wall_ns));
            std::fflush(stdout);
        }
        if (measured) {
            records.push_back(
                TimingRecord{
                    sequence,
                    order,
                    cpu_timing.wall_ns,
                    cpu_timing.thread_ns,
                    npu_timing.copy_in_ns,
                    npu_timing.run_ns,
                    npu_timing.copy_out_ns,
                    npu_timing.total_wall_ns,
                    npu_timing.thread_ns,
                });
        }
    }
    uint64_t process_finished = 0;
    uint64_t process_elapsed = 0;
    if (!ReadClock(CLOCK_PROCESS_CPUTIME_ID, &process_finished) ||
        !Duration(
            process_started, process_finished, &process_elapsed)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "process timer finish failed\n");
        return 20;
    }
    if (records.size() != kMeasuredFrames ||
        input_fnv != kExpectedInputFnv ||
        cpu_output_fnv != kExpectedOutputFnv ||
        npu_output_fnv != kExpectedOutputFnv ||
        random_state != kExpectedFinalState) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "fixed count or digest mismatch\n");
        return 21;
    }

    const std::vector<uint8_t> timing_payload = SerializeTimings(
        calibration_total, calibration_minimum, records);
    if (timing_payload.size() != kExpectedTimingBytes) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "timing serialization failed\n");
        return 22;
    }
    const uint64_t timing_fnv = UpdateFnv1a64(
        kFnvOffsetBasis, timing_payload.data(), timing_payload.size());
    if (!PublishTimings(timing_payload)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_erf_cpu_vs_npu: "
            "timing publication failed: %s\n",
            std::strerror(errno));
        return 23;
    }

    nbg_operation.reset();
    output.reset();
    input.reset();
    graph.reset();
    context.reset();
    alarm(0);
    std::printf(
        "PROCESS_CPU frames=10101 total_ns=%llu\n",
        static_cast<unsigned long long>(process_elapsed));
    std::printf(
        "ERF_CPU_NPU_BENCH status=PASS mode=benchmark "
        "cold_frames=1 warmup_frames=100 measured_frames=10000 "
        "cpu_count=10101 npu_run_count=10101 compile_count=1 "
        "canaries=PASS timing_bytes=640036 "
        "timing_fnv1a64=0x%016llx "
        "input_fnv1a64=0x%016llx "
        "cpu_output_fnv1a64=0x%016llx "
        "npu_output_fnv1a64=0x%016llx "
        "final_state=0x%08x\n",
        static_cast<unsigned long long>(timing_fnv),
        static_cast<unsigned long long>(input_fnv),
        static_cast<unsigned long long>(cpu_output_fnv),
        static_cast<unsigned long long>(npu_output_fnv),
        random_state);
    return 0;
}
