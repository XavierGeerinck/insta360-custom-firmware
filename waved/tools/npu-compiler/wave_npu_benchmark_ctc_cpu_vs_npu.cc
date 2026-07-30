// SPDX-License-Identifier: MIT
//
// PoC 15A: compare the fixed English CTC source/binary anchor through the
// Wave's stock CPU ONNX Runtime and stock VIPUNIFY/libnnsdk path.

#include "onnxruntime_c_api.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cmath>
#include <csignal>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <fcntl.h>
#include <limits>
#include <memory>
#include <string>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <utility>
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

constexpr size_t kInputElements = 16U * 512U;
constexpr size_t kOutputElements = 16U * 4999U;
constexpr size_t kInputFloatBytes = kInputElements * sizeof(float);
constexpr size_t kInputHalfBytes = kInputElements * sizeof(uint16_t);
constexpr size_t kOutputFloatBytes = kOutputElements * sizeof(float);
constexpr size_t kOutputHalfBytes = kOutputElements * sizeof(uint16_t);
constexpr size_t kExpectedOnnxBytes = 10'258'790;
constexpr size_t kExpectedNbgBytes = 5'263'360;
constexpr size_t kExpectedOrtBytes = 8'299'512;
constexpr size_t kExpectedNnSdkBytes = 288'448;
constexpr size_t kCanaryBytes = 32;
constexpr uint8_t kCanaryValue = 0xa5;
constexpr size_t kFixtureCount = 4;
constexpr size_t kTimeSteps = 16;
constexpr size_t kClasses = 4999;
constexpr size_t kColdPairs = 1;
constexpr size_t kWarmupPairs = 20;
constexpr size_t kMeasuredPairs = 200;
constexpr uint32_t kQualificationSeed = 0x573315a1U;
constexpr uint32_t kBenchmarkSeed = 0x573315b1U;
constexpr uint32_t kExpectedFinalQualificationState = 0x474c5d42U;
constexpr uint32_t kExpectedFinalBenchmarkState = 0x98d70268U;
constexpr double kMeanAbsoluteErrorMax = 0.025;
constexpr double kMaximumAbsoluteErrorMax = 0.25;
constexpr double kCosineSimilarityMin = 0.999;
constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveCustomerId = 0x1000000eU;
constexpr uint32_t kWaveNbgVersion = 0x00020000U;
constexpr unsigned int kWatchdogSeconds = 120;
constexpr uint64_t kFnvOffsetBasis = UINT64_C(0xcbf29ce484222325);
constexpr uint64_t kFnvPrime = UINT64_C(0x100000001b3);
constexpr uint32_t kTimingSchemaVersion = 1;
constexpr size_t kTimingHeaderBytes = 64;
constexpr size_t kQualificationRecordBytes = 88;
constexpr size_t kTimingRecordBytes = 136;
constexpr size_t kExpectedTimingBytes =
    kTimingHeaderBytes +
    kFixtureCount * kQualificationRecordBytes +
    (kColdPairs + kMeasuredPairs) * kTimingRecordBytes;

constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-poc15a/lib:/usr/lib";
constexpr char kExpectedSdkDirectory[] = "/usr";
constexpr char kExpectedLogLevel[] = "2";
constexpr char kOnnxPath[] =
    "/tmp/wave-npu-poc15a/model/ctc.onnx";
constexpr char kNbgPath[] =
    "/tmp/wave-npu-poc15a/model/ctc_online.nb";
constexpr char kOrtLibraryPath[] =
    "/usr/lib/libonnxruntime.so.1.15.1";
constexpr char kNnSdkLibraryPath[] = "/usr/lib/libnnsdk.so";
constexpr char kForbiddenClcPath[] =
    "/tmp/wave-npu-poc15a/lib/libCLC.so";
constexpr char kTimingPath[] =
    "/tmp/wave-npu-poc15a/result/ctc-cpu-npu-timings-v1.bin";
constexpr char kPartialTimingPath[] =
    "/tmp/wave-npu-poc15a/result/ctc-cpu-npu-timings-v1.partial";
constexpr char kInputName[] = "hidden";
constexpr char kOutputName[] = "probs";
constexpr char kOnnxSha256[] =
    "8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219";
constexpr char kNbgSha256[] =
    "3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5";
constexpr char kOrtSha256[] =
    "457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639";
constexpr char kNnSdkSha256[] =
    "36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe";
constexpr char kHeaderSha256[] =
    "cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91";

constexpr int kModelTypeOnnx = 4;
constexpr int kNbgTypeMemory = 1;
constexpr int kInputTypeBinaryRaw = 3;
constexpr int kOutputFormatRaw = 1;
constexpr int kPerfModeRun = 1;
constexpr int kPerfModeOutputGet = 2;
constexpr size_t kAmlConfigBytes = 2272;
constexpr size_t kAmlOutputConfigBytes = 56;
constexpr size_t kMaxTensorDimensions = 6;

struct AmlConfig {
    int32_t type_size;
    int32_t length;
    const char* path;
    const char* data;
    int32_t model_type;
    int32_t nbg_type;
    std::array<uint8_t, kAmlConfigBytes - 32> reserved;
};

struct InputInfo {
    int32_t valid;
    int32_t int16_type;
    int32_t preprocess_debug;
    std::array<float, 3> mean;
    float scale;
    int32_t input_format;
};

struct NnInput {
    int32_t type_size;
    int32_t input_index;
    int32_t size;
    int32_t padding;
    uint8_t* input;
    int32_t input_type;
    InputInfo info;
};

struct AmlOutputConfig {
    int32_t type_size;
    int32_t module_type;
    int32_t perf_mode;
    int32_t format;
    int32_t order;
    std::array<uint8_t, kAmlOutputConfigBytes - 20> reserved;
};

struct NnBufferParams {
    uint32_t num_of_dims;
    std::array<uint32_t, 4> sizes;
    int32_t data_format;
    int32_t quant_format;
    std::array<uint8_t, 8> quant_data;
};

struct OutBuffer {
    uint32_t size;
    std::array<char, 64> name;
    uint8_t* buffer;
    NnBufferParams* params;
    int32_t output_format;
};

struct NnOutput {
    int32_t type_size;
    uint32_t num;
    std::array<OutBuffer, 32> outputs;
};

struct TensorInfoEntry {
    uint32_t dimension_count;
    std::array<uint32_t, kMaxTensorDimensions> dimensions;
    uint32_t data_format;
    uint32_t data_type;
    uint32_t quantization_format;
    int32_t fixed_point_position;
    float tf_scale;
    int32_t tf_zero_point;
    std::array<char, 64> name;
};

struct TensorInfoList {
    uint32_t valid;
    uint32_t count;
    TensorInfoEntry* entries;
};

static_assert(sizeof(AmlConfig) == 2272, "unexpected AmlConfig ABI");
static_assert(offsetof(AmlConfig, data) == 16, "unexpected AmlConfig data");
static_assert(offsetof(AmlConfig, nbg_type) == 28, "unexpected AmlConfig mode");
static_assert(sizeof(InputInfo) == 32, "unexpected InputInfo ABI");
static_assert(sizeof(NnInput) == 64, "unexpected NnInput ABI");
static_assert(offsetof(NnInput, input) == 16, "unexpected NnInput buffer");
static_assert(sizeof(AmlOutputConfig) == 56, "unexpected output config ABI");
static_assert(sizeof(NnBufferParams) == 36, "unexpected output params ABI");
static_assert(sizeof(OutBuffer) == 96, "unexpected output buffer ABI");
static_assert(offsetof(OutBuffer, buffer) == 72, "unexpected output data");
static_assert(sizeof(NnOutput) == 3080, "unexpected NnOutput ABI");
static_assert(sizeof(TensorInfoEntry) == 116, "unexpected tensor-info ABI");
static_assert(sizeof(TensorInfoList) == 16, "unexpected tensor-list ABI");

using ModuleCreateFn = void* (*)(AmlConfig*);
using ModuleInputSetFn = int32_t (*)(void*, NnInput*);
using ModuleOutputGetFn = NnOutput* (*)(void*, AmlOutputConfig);
using ModuleDestroyFn = int32_t (*)(void*);
using GetTensorInfoFn = TensorInfoList* (*)(const char*);
using FreeTensorInfoFn = int32_t (*)(TensorInfoList*);
using OrtGetApiBaseFn = const OrtApiBase* (*)();

struct TimingRecord {
    uint32_t sequence;
    uint32_t order;
    uint64_t cpu_wall_ns;
    uint64_t cpu_thread_ns;
    uint64_t convert_in_wall_ns;
    uint64_t convert_in_thread_ns;
    uint64_t input_set_wall_ns;
    uint64_t input_set_thread_ns;
    uint64_t run_wall_ns;
    uint64_t run_thread_ns;
    uint64_t output_get_wall_ns;
    uint64_t output_get_thread_ns;
    uint64_t convert_out_wall_ns;
    uint64_t convert_out_thread_ns;
    uint64_t native_total_wall_ns;
    uint64_t native_total_thread_ns;
    uint64_t bridge_total_wall_ns;
    uint64_t bridge_total_thread_ns;
};

static_assert(sizeof(TimingRecord) == kTimingRecordBytes, "timing ABI drift");

struct QualificationRecord {
    uint32_t fixture_id;
    uint32_t top1_matches;
    uint32_t finite_count;
    uint32_t reserved;
    uint64_t cpu_float_fnv;
    uint64_t cpu_half_fnv;
    uint64_t npu_half_fnv;
    uint64_t npu_repeat_fnv;
    double sum_absolute_error;
    double maximum_absolute_error;
    double cosine_dot_product;
    double cosine_cpu_squared;
    double cosine_npu_squared;
};

static_assert(
    sizeof(QualificationRecord) == kQualificationRecordBytes,
    "qualification ABI drift");

template <typename T>
class GuardedArray {
  public:
    explicit GuardedArray(size_t count)
        : padding_(kCanaryBytes / sizeof(T)),
          count_(count),
          storage_(count + 2U * padding_) {
        static_assert(kCanaryBytes % sizeof(T) == 0, "canary alignment");
        Reset();
    }

    void Reset() {
        std::memset(
            reinterpret_cast<uint8_t*>(storage_.data()),
            kCanaryValue,
            storage_.size() * sizeof(T));
    }

    T* data() {
        return storage_.data() + padding_;
    }

    const T* data() const {
        return storage_.data() + padding_;
    }

    size_t count() const {
        return count_;
    }

    size_t bytes() const {
        return count_ * sizeof(T);
    }

    bool CanariesIntact() const {
        const auto* raw_bytes =
            reinterpret_cast<const uint8_t*>(storage_.data());
        const size_t payload_offset = padding_ * sizeof(T);
        const size_t suffix_offset = payload_offset + bytes();
        return std::all_of(
                   raw_bytes,
                   raw_bytes + payload_offset,
                   [](uint8_t value) { return value == kCanaryValue; }) &&
               std::all_of(
                   raw_bytes + suffix_offset,
                   raw_bytes + storage_.size() * sizeof(T),
                   [](uint8_t value) { return value == kCanaryValue; });
    }

  private:
    size_t padding_;
    size_t count_;
    std::vector<T> storage_;
};

struct CpuRuntime {
    void* library = nullptr;
    const OrtApi* api = nullptr;
    OrtEnv* env = nullptr;
    OrtSessionOptions* options = nullptr;
    OrtSession* session = nullptr;
    OrtMemoryInfo* memory_info = nullptr;
    OrtValue* input_value = nullptr;
    OrtValue* output_value = nullptr;
};

struct NpuRuntime {
    void* library = nullptr;
    ModuleCreateFn create = nullptr;
    ModuleInputSetFn input_set = nullptr;
    ModuleOutputGetFn output_get = nullptr;
    ModuleDestroyFn destroy = nullptr;
    GetTensorInfoFn get_input_info = nullptr;
    GetTensorInfoFn get_output_info = nullptr;
    FreeTensorInfoFn free_tensor_info = nullptr;
    void* context = nullptr;
    std::vector<uint8_t> model;
};

struct ClockPair {
    uint64_t wall_ns = 0;
    uint64_t thread_ns = 0;
};

struct NumericalMetrics {
    uint32_t top1_matches = 0;
    uint32_t finite_count = 0;
    double mean_absolute_error = 0.0;
    double maximum_absolute_error = 0.0;
    double cosine_similarity = 0.0;
    double sum_absolute_error = 0.0;
    double cosine_dot_product = 0.0;
    double cosine_cpu_squared = 0.0;
    double cosine_npu_squared = 0.0;
};

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "CTC_CPU_NPU_BENCH status=TIMEOUT mode=benchmark exit=124\n";
    const ssize_t written =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)written;
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
    const char* const log_level = std::getenv("VSI_NN_LOG_LEVEL");
    if (library_path == nullptr ||
        std::strcmp(library_path, kExpectedLibraryPath) != 0 ||
        sdk_directory == nullptr ||
        std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0 ||
        log_level == nullptr ||
        std::strcmp(log_level, kExpectedLogLevel) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "fixed environment mismatch\n");
        return false;
    }
    constexpr std::array<const char*, 8> kForbiddenVariables = {
        "LD_PRELOAD",
        "LD_AUDIT",
        "VSIMULATOR_CONFIG",
        "OMP_NUM_THREADS",
        "OMP_WAIT_POLICY",
        "KMP_AFFINITY",
        "ORT_NUM_THREADS",
        "ORT_TENSORRT_ENGINE_CACHE_ENABLE",
    };
    for (const char* variable : kForbiddenVariables) {
        if (std::getenv(variable) != nullptr) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_ctc_cpu_vs_npu: "
                "forbidden environment variable is set: %s\n",
                variable);
            return false;
        }
    }
    if (!PathIsAbsent(kForbiddenClcPath)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "isolated CLC path must be absent\n");
        return false;
    }
    if (!PathIsAbsent(kTimingPath) ||
        !PathIsAbsent(kPartialTimingPath)) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "fixed timing output already exists\n");
        return false;
    }
    return true;
}

bool ValidateRegularFile(
    const char* path,
    size_t expected_bytes,
    bool require_mode_0444) {
    const int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) {
        std::fprintf(stderr, "open(%s): %s\n", path, std::strerror(errno));
        return false;
    }
    struct stat file_stat {};
    const bool metadata_ok =
        fstat(fd, &file_stat) == 0 &&
        S_ISREG(file_stat.st_mode) &&
        file_stat.st_size == static_cast<off_t>(expected_bytes) &&
        (require_mode_0444
             ? (file_stat.st_mode & 0777) == 0444
             : ((file_stat.st_mode & 0444) == 0444 &&
                (file_stat.st_mode & 0022) == 0));
    const bool closed = close(fd) == 0;
    if (!metadata_ok || !closed) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "fixed file metadata mismatch: %s\n",
            path);
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
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
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
                stderr, "read(%s): %s\n", kNbgPath, std::strerror(errno));
            close(fd);
            return false;
        }
        if (count == 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_ctc_cpu_vs_npu: short NBG read\n");
            close(fd);
            return false;
        }
        offset += static_cast<size_t>(count);
    }
    uint8_t extra = 0;
    if (read(fd, &extra, 1) != 0 || close(fd) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "NBG close/length check failed\n");
        return false;
    }
    if (std::memcmp(nbg->data(), "VPMN", 4) != 0 ||
        ReadLe32(nbg->data() + 4) != kWaveNbgVersion ||
        ReadLe32(nbg->data() + 8) != kWaveCustomerId) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: NBG header mismatch\n");
        return false;
    }
    return true;
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

bool StartClocks(ClockPair* started) {
    return ReadClock(CLOCK_MONOTONIC_RAW, &started->wall_ns) &&
           ReadClock(CLOCK_THREAD_CPUTIME_ID, &started->thread_ns);
}

bool FinishClocks(
    const ClockPair& started,
    uint64_t* wall_elapsed,
    uint64_t* thread_elapsed) {
    uint64_t thread_finished = 0;
    uint64_t wall_finished = 0;
    if (!ReadClock(CLOCK_THREAD_CPUTIME_ID, &thread_finished) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &wall_finished) ||
        thread_finished <= started.thread_ns ||
        wall_finished <= started.wall_ns) {
        return false;
    }
    *thread_elapsed = thread_finished - started.thread_ns;
    *wall_elapsed = wall_finished - started.wall_ns;
    return true;
}

template <typename FunctionPointer>
bool Resolve(
    void* library,
    const char* name,
    FunctionPointer* output) {
    dlerror();
    void* const symbol = dlsym(library, name);
    const char* const error = dlerror();
    if (error != nullptr || symbol == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "dlsym(%s) failed: %s\n",
            name,
            error == nullptr ? "NULL symbol" : error);
        return false;
    }
    *output = reinterpret_cast<FunctionPointer>(symbol);
    return true;
}

bool OrtCall(
    const OrtApi* api,
    OrtStatus* status,
    const char* operation) {
    if (status == nullptr) {
        return true;
    }
    std::fprintf(
        stderr,
        "wave_npu_benchmark_ctc_cpu_vs_npu: ORT %s failed: %s\n",
        operation,
        api->GetErrorMessage(status));
    api->ReleaseStatus(status);
    return false;
}

void CleanupCpu(CpuRuntime* runtime) {
    if (runtime->api != nullptr) {
        if (runtime->output_value != nullptr) {
            runtime->api->ReleaseValue(runtime->output_value);
        }
        if (runtime->input_value != nullptr) {
            runtime->api->ReleaseValue(runtime->input_value);
        }
        if (runtime->memory_info != nullptr) {
            runtime->api->ReleaseMemoryInfo(runtime->memory_info);
        }
        if (runtime->session != nullptr) {
            runtime->api->ReleaseSession(runtime->session);
        }
        if (runtime->options != nullptr) {
            runtime->api->ReleaseSessionOptions(runtime->options);
        }
        if (runtime->env != nullptr) {
            runtime->api->ReleaseEnv(runtime->env);
        }
    }
    runtime->output_value = nullptr;
    runtime->input_value = nullptr;
    runtime->memory_info = nullptr;
    runtime->session = nullptr;
    runtime->options = nullptr;
    runtime->env = nullptr;
    runtime->api = nullptr;
    if (runtime->library != nullptr) {
        if (dlclose(runtime->library) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_ctc_cpu_vs_npu: "
                "ORT dlclose failed: %s\n",
                dlerror());
        }
        runtime->library = nullptr;
    }
}

bool InitializeCpu(
    CpuRuntime* runtime,
    GuardedArray<float>* input,
    GuardedArray<float>* output) {
    if (!ValidateRegularFile(
            kOrtLibraryPath, kExpectedOrtBytes, false)) {
        return false;
    }
    runtime->library =
        dlopen(kOrtLibraryPath, RTLD_NOW | RTLD_LOCAL);
    if (runtime->library == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: dlopen(%s): %s\n",
            kOrtLibraryPath,
            dlerror());
        return false;
    }
    OrtGetApiBaseFn get_api_base = nullptr;
    if (!Resolve(runtime->library, "OrtGetApiBase", &get_api_base)) {
        return false;
    }
    const OrtApiBase* const api_base = get_api_base();
    if (api_base == nullptr ||
        api_base->GetVersionString == nullptr ||
        api_base->GetApi == nullptr ||
        std::strcmp(api_base->GetVersionString(), "1.15.1") != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "ORT API base/version mismatch\n");
        return false;
    }
    runtime->api = api_base->GetApi(ORT_API_VERSION);
    if (runtime->api == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "ORT API version 15 unavailable\n");
        return false;
    }

    char** providers = nullptr;
    int provider_count = 0;
    if (!OrtCall(
            runtime->api,
            runtime->api->GetAvailableProviders(
                &providers, &provider_count),
            "GetAvailableProviders")) {
        return false;
    }
    const bool cpu_only =
        provider_count == 1 &&
        providers != nullptr &&
        providers[0] != nullptr &&
        std::strcmp(providers[0], "CPUExecutionProvider") == 0;
    const bool providers_released = OrtCall(
        runtime->api,
        runtime->api->ReleaseAvailableProviders(
            providers, provider_count),
        "ReleaseAvailableProviders");
    if (!cpu_only || !providers_released) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "ORT provider set is not CPU-only\n");
        return false;
    }

    if (!OrtCall(
            runtime->api,
            runtime->api->CreateEnv(
                ORT_LOGGING_LEVEL_WARNING,
                "wave-poc15a",
                &runtime->env),
            "CreateEnv") ||
        !OrtCall(
            runtime->api,
            runtime->api->CreateSessionOptions(&runtime->options),
            "CreateSessionOptions") ||
        !OrtCall(
            runtime->api,
            runtime->api->SetSessionGraphOptimizationLevel(
                runtime->options, ORT_ENABLE_ALL),
            "SetSessionGraphOptimizationLevel") ||
        !OrtCall(
            runtime->api,
            runtime->api->CreateSession(
                runtime->env,
                kOnnxPath,
                runtime->options,
                &runtime->session),
            "CreateSession")) {
        return false;
    }
    size_t input_count = 0;
    size_t output_count = 0;
    if (!OrtCall(
            runtime->api,
            runtime->api->SessionGetInputCount(
                runtime->session, &input_count),
            "SessionGetInputCount") ||
        !OrtCall(
            runtime->api,
            runtime->api->SessionGetOutputCount(
                runtime->session, &output_count),
            "SessionGetOutputCount") ||
        input_count != 1 || output_count != 1) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "ORT tensor count mismatch\n");
        return false;
    }
    if (!OrtCall(
            runtime->api,
            runtime->api->CreateCpuMemoryInfo(
                OrtArenaAllocator,
                OrtMemTypeDefault,
                &runtime->memory_info),
            "CreateCpuMemoryInfo")) {
        return false;
    }
    constexpr std::array<int64_t, 3> kInputShape = {1, 16, 512};
    constexpr std::array<int64_t, 3> kOutputShape = {1, 16, 4999};
    if (!OrtCall(
            runtime->api,
            runtime->api->CreateTensorWithDataAsOrtValue(
                runtime->memory_info,
                input->data(),
                input->bytes(),
                kInputShape.data(),
                kInputShape.size(),
                ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                &runtime->input_value),
            "CreateTensorWithDataAsOrtValue(input)") ||
        !OrtCall(
            runtime->api,
            runtime->api->CreateTensorWithDataAsOrtValue(
                runtime->memory_info,
                output->data(),
                output->bytes(),
                kOutputShape.data(),
                kOutputShape.size(),
                ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
                &runtime->output_value),
            "CreateTensorWithDataAsOrtValue(output)")) {
        return false;
    }
    int input_is_tensor = 0;
    int output_is_tensor = 0;
    void* input_data = nullptr;
    void* output_data = nullptr;
    if (!OrtCall(
            runtime->api,
            runtime->api->IsTensor(
                runtime->input_value, &input_is_tensor),
            "IsTensor(input)") ||
        !OrtCall(
            runtime->api,
            runtime->api->IsTensor(
                runtime->output_value, &output_is_tensor),
            "IsTensor(output)") ||
        !OrtCall(
            runtime->api,
            runtime->api->GetTensorMutableData(
                runtime->input_value, &input_data),
            "GetTensorMutableData(input)") ||
        !OrtCall(
            runtime->api,
            runtime->api->GetTensorMutableData(
                runtime->output_value, &output_data),
            "GetTensorMutableData(output)") ||
        input_is_tensor != 1 || output_is_tensor != 1 ||
        input_data != input->data() || output_data != output->data()) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "ORT external tensor binding mismatch\n");
        return false;
    }
    return true;
}

bool TensorNameEquals(
    const std::array<char, 64>& observed,
    const char* expected) {
    const size_t expected_bytes = std::strlen(expected);
    return expected_bytes < observed.size() &&
           std::memcmp(observed.data(), expected, expected_bytes) == 0 &&
           observed[expected_bytes] == '\0';
}

bool ValidateTensorMetadata(
    NpuRuntime* runtime,
    GetTensorInfoFn getter,
    const char* kind,
    const char* expected_name,
    const std::array<uint32_t, 3>& expected_dimensions) {
    TensorInfoList* const list =
        getter(reinterpret_cast<const char*>(runtime->model.data()));
    const uintptr_t list_address = reinterpret_cast<uintptr_t>(list);
    if (list == nullptr ||
        list_address < 4096U ||
        list_address % alignof(TensorInfoList) != 0U) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "%s tensor metadata pointer invalid\n",
            kind);
        return false;
    }
    bool valid =
        list->valid != 0U &&
        list->count == 1U &&
        list->entries != nullptr &&
        reinterpret_cast<uintptr_t>(list->entries) >= 4096U &&
        reinterpret_cast<uintptr_t>(list->entries) %
                alignof(TensorInfoEntry) ==
            0U;
    if (valid) {
        const TensorInfoEntry& entry = list->entries[0];
        valid =
            entry.dimension_count == expected_dimensions.size() &&
            std::equal(
                expected_dimensions.begin(),
                expected_dimensions.end(),
                entry.dimensions.begin()) &&
            entry.data_format == 1U &&
            TensorNameEquals(entry.name, expected_name);
    }
    const int32_t free_status = runtime->free_tensor_info(list);
    if (!valid || free_status != 0) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "%s tensor metadata mismatch/free=%d\n",
            kind,
            free_status);
        return false;
    }
    return true;
}

bool CleanupNpu(NpuRuntime* runtime) {
    bool passed = true;
    if (runtime->context != nullptr) {
        const int32_t status = runtime->destroy(runtime->context);
        runtime->context = nullptr;
        if (status != 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_ctc_cpu_vs_npu: "
                "aml_module_destroy returned %d\n",
                status);
            passed = false;
        }
    }
    if (runtime->library != nullptr) {
        if (dlclose(runtime->library) != 0) {
            std::fprintf(
                stderr,
                "wave_npu_benchmark_ctc_cpu_vs_npu: "
                "libnnsdk dlclose failed: %s\n",
                dlerror());
            passed = false;
        }
        runtime->library = nullptr;
    }
    return passed;
}

bool InitializeNpu(NpuRuntime* runtime) {
    if (!ReadFixedNbg(&runtime->model)) {
        return false;
    }
    if (!ValidateRegularFile(
            kNnSdkLibraryPath, kExpectedNnSdkBytes, false)) {
        return false;
    }
    runtime->library =
        dlopen(kNnSdkLibraryPath, RTLD_NOW | RTLD_LOCAL);
    if (runtime->library == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: dlopen(%s): %s\n",
            kNnSdkLibraryPath,
            dlerror());
        return false;
    }
    if (!Resolve(runtime->library, "aml_module_create", &runtime->create) ||
        !Resolve(
            runtime->library,
            "aml_module_input_set",
            &runtime->input_set) ||
        !Resolve(
            runtime->library,
            "aml_module_output_get",
            &runtime->output_get) ||
        !Resolve(
            runtime->library,
            "aml_module_destroy",
            &runtime->destroy) ||
        !Resolve(
            runtime->library,
            "aml_util_getInputTensorInfo",
            &runtime->get_input_info) ||
        !Resolve(
            runtime->library,
            "aml_util_getOutputTensorInfo",
            &runtime->get_output_info) ||
        !Resolve(
            runtime->library,
            "aml_util_freeTensorInfo",
            &runtime->free_tensor_info)) {
        return false;
    }
    constexpr std::array<uint32_t, 3> kInputDimensions = {
        512, 16, 1};
    constexpr std::array<uint32_t, 3> kOutputDimensions = {
        4999, 16, 1};
    if (!ValidateTensorMetadata(
            runtime,
            runtime->get_input_info,
            "input",
            "input/output[0]",
            kInputDimensions) ||
        !ValidateTensorMetadata(
            runtime,
            runtime->get_output_info,
            "output",
            "uid_1_out_0",
            kOutputDimensions)) {
        return false;
    }

    AmlConfig config {};
    config.type_size = static_cast<int32_t>(sizeof(config));
    config.length = static_cast<int32_t>(runtime->model.size());
    config.path = nullptr;
    config.data =
        reinterpret_cast<const char*>(runtime->model.data());
    config.model_type = kModelTypeOnnx;
    config.nbg_type = kNbgTypeMemory;
    runtime->context = runtime->create(&config);
    if (runtime->context == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_benchmark_ctc_cpu_vs_npu: "
            "aml_module_create returned NULL\n");
        return false;
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
            "wave_npu_benchmark_ctc_cpu_vs_npu: identity mismatch\n");
        return false;
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
    return true;
}

uint16_t FloatToHalfBits(float value) {
    static_assert(sizeof(float) == sizeof(uint32_t), "binary32 ABI");
    uint32_t bits = 0;
    std::memcpy(&bits, &value, sizeof(bits));
    const uint16_t sign =
        static_cast<uint16_t>((bits >> 16U) & 0x8000U);
    const uint32_t exponent = (bits >> 23U) & 0xffU;
    const uint32_t fraction = bits & 0x7fffffU;
    if (exponent == 0xffU) {
        if (fraction == 0U) {
            return static_cast<uint16_t>(sign | 0x7c00U);
        }
        uint16_t payload =
            static_cast<uint16_t>(fraction >> 13U);
        payload = static_cast<uint16_t>(payload | 0x0200U);
        return static_cast<uint16_t>(sign | 0x7c00U | payload);
    }

    const int32_t half_exponent =
        static_cast<int32_t>(exponent) - 127 + 15;
    if (half_exponent >= 31) {
        return static_cast<uint16_t>(sign | 0x7c00U);
    }
    if (half_exponent <= 0) {
        if (half_exponent < -10) {
            return sign;
        }
        const uint32_t significand = fraction | 0x800000U;
        const uint32_t shift =
            static_cast<uint32_t>(14 - half_exponent);
        uint32_t rounded = significand >> shift;
        const uint32_t remainder =
            significand & ((UINT32_C(1) << shift) - 1U);
        const uint32_t halfway = UINT32_C(1) << (shift - 1U);
        if (remainder > halfway ||
            (remainder == halfway && (rounded & 1U) != 0U)) {
            ++rounded;
        }
        return static_cast<uint16_t>(sign | rounded);
    }

    uint32_t rounded_fraction = fraction >> 13U;
    const uint32_t remainder = fraction & 0x1fffU;
    if (remainder > 0x1000U ||
        (remainder == 0x1000U &&
         (rounded_fraction & 1U) != 0U)) {
        ++rounded_fraction;
    }
    uint32_t rounded_exponent =
        static_cast<uint32_t>(half_exponent);
    if (rounded_fraction == 0x400U) {
        rounded_fraction = 0;
        ++rounded_exponent;
        if (rounded_exponent >= 31U) {
            return static_cast<uint16_t>(sign | 0x7c00U);
        }
    }
    return static_cast<uint16_t>(
        sign | (rounded_exponent << 10U) | rounded_fraction);
}

float HalfBitsToFloat(uint16_t bits) {
    const uint32_t sign =
        static_cast<uint32_t>(bits & 0x8000U) << 16U;
    uint32_t exponent = (bits >> 10U) & 0x1fU;
    uint32_t fraction = bits & 0x03ffU;
    uint32_t float_bits = 0;
    if (exponent == 0U) {
        if (fraction == 0U) {
            float_bits = sign;
        } else {
            int32_t unbiased_exponent = -14;
            while ((fraction & 0x0400U) == 0U) {
                fraction <<= 1U;
                --unbiased_exponent;
            }
            fraction &= 0x03ffU;
            exponent =
                static_cast<uint32_t>(unbiased_exponent + 127);
            float_bits =
                sign | (exponent << 23U) | (fraction << 13U);
        }
    } else if (exponent == 0x1fU) {
        float_bits = sign | 0x7f800000U | (fraction << 13U);
    } else {
        exponent = exponent - 15U + 127U;
        float_bits =
            sign | (exponent << 23U) | (fraction << 13U);
    }
    float value = 0.0F;
    std::memcpy(&value, &float_bits, sizeof(value));
    return value;
}

bool RunCpu(
    CpuRuntime* runtime,
    const GuardedArray<float>& input,
    GuardedArray<float>* output,
    TimingRecord* timing,
    const char** failure) {
    ClockPair started {};
    if (!StartClocks(&started)) {
        *failure = "cpu-clock-start";
        return false;
    }
    const OrtValue* inputs[] = {runtime->input_value};
    OrtValue* outputs[] = {runtime->output_value};
    const char* input_names[] = {kInputName};
    const char* output_names[] = {kOutputName};
    OrtStatus* const status = runtime->api->Run(
        runtime->session,
        nullptr,
        input_names,
        inputs,
        1,
        output_names,
        1,
        outputs);
    if (status != nullptr) {
        OrtCall(runtime->api, status, "Run");
        *failure = "cpu-run";
        return false;
    }
    if (!FinishClocks(
            started,
            &timing->cpu_wall_ns,
            &timing->cpu_thread_ns)) {
        *failure = "cpu-clock-finish";
        return false;
    }
    if (outputs[0] != runtime->output_value ||
        !input.CanariesIntact() ||
        !output->CanariesIntact()) {
        *failure = "cpu-output-binding-or-canary";
        return false;
    }
    return true;
}

AmlOutputConfig MakeOutputConfig(int32_t perf_mode) {
    AmlOutputConfig config {};
    config.type_size = static_cast<int32_t>(sizeof(config));
    config.module_type = 0;
    config.perf_mode = perf_mode;
    config.format = kOutputFormatRaw;
    config.order = 0;
    return config;
}

bool RunNpu(
    NpuRuntime* runtime,
    const GuardedArray<float>& input,
    GuardedArray<uint16_t>* input_half,
    GuardedArray<uint16_t>* output_half,
    GuardedArray<float>* output,
    TimingRecord* timing,
    const char** failure) {
    input_half->Reset();
    output_half->Reset();
    output->Reset();

    ClockPair bridge_started {};
    ClockPair conversion_started {};
    ClockPair native_started {};
    ClockPair input_set_started {};
    ClockPair run_started {};
    ClockPair output_get_started {};
    if (!StartClocks(&bridge_started) ||
        !StartClocks(&conversion_started)) {
        *failure = "npu-convert-in-clock-start";
        return false;
    }
    for (size_t index = 0; index < kInputElements; ++index) {
        input_half->data()[index] =
            FloatToHalfBits(input.data()[index]);
    }
    if (!FinishClocks(
            conversion_started,
            &timing->convert_in_wall_ns,
            &timing->convert_in_thread_ns)) {
        *failure = "npu-convert-in-clock-finish";
        return false;
    }
    if (!StartClocks(&native_started) ||
        !StartClocks(&input_set_started)) {
        *failure = "npu-input-set-clock-start";
        return false;
    }

    NnInput npu_input {};
    npu_input.type_size = 0;
    npu_input.input_index = 0;
    npu_input.size = static_cast<int32_t>(input_half->bytes());
    npu_input.padding = 0;
    npu_input.input =
        reinterpret_cast<uint8_t*>(input_half->data());
    npu_input.input_type = kInputTypeBinaryRaw;
    if (runtime->input_set(runtime->context, &npu_input) != 0) {
        *failure = "npu-input-set";
        return false;
    }
    if (!FinishClocks(
            input_set_started,
            &timing->input_set_wall_ns,
            &timing->input_set_thread_ns)) {
        *failure = "npu-input-set-clock-finish";
        return false;
    }

    if (!StartClocks(&run_started)) {
        *failure = "npu-run-clock-start";
        return false;
    }
    const AmlOutputConfig run_config =
        MakeOutputConfig(kPerfModeRun);
    (void)runtime->output_get(runtime->context, run_config);
    if (!FinishClocks(
            run_started,
            &timing->run_wall_ns,
            &timing->run_thread_ns)) {
        *failure = "npu-run-clock-finish";
        return false;
    }

    if (!StartClocks(&output_get_started)) {
        *failure = "npu-output-get-clock-start";
        return false;
    }
    const AmlOutputConfig output_config =
        MakeOutputConfig(kPerfModeOutputGet);
    NnOutput* const npu_output =
        runtime->output_get(runtime->context, output_config);
    const uintptr_t output_address =
        reinterpret_cast<uintptr_t>(npu_output);
    if (npu_output == nullptr ||
        output_address < 4096U ||
        output_address % alignof(NnOutput) != 0U ||
        npu_output->num != 1U) {
        *failure = "npu-output-descriptor";
        return false;
    }
    const OutBuffer& first = npu_output->outputs[0];
    const uintptr_t buffer_address =
        reinterpret_cast<uintptr_t>(first.buffer);
    if (first.size != kOutputHalfBytes ||
        first.buffer == nullptr ||
        buffer_address < 4096U ||
        buffer_address % alignof(uint16_t) != 0U) {
        *failure = "npu-output-buffer";
        return false;
    }
    if (first.params != nullptr) {
        const uintptr_t params_address =
            reinterpret_cast<uintptr_t>(first.params);
        if (params_address < 4096U ||
            params_address % alignof(NnBufferParams) != 0U) {
            *failure = "npu-output-metadata-pointer";
            return false;
        }
        const NnBufferParams& params = *first.params;
        if (params.num_of_dims != 3U ||
            params.sizes[0] != 4999U ||
            params.sizes[1] != 16U ||
            params.sizes[2] != 1U ||
            params.data_format != 1) {
            *failure = "npu-output-metadata";
            return false;
        }
    }
    std::memcpy(
        output_half->data(), first.buffer, kOutputHalfBytes);
    if (!FinishClocks(
            output_get_started,
            &timing->output_get_wall_ns,
            &timing->output_get_thread_ns)) {
        *failure = "npu-output-get-clock-finish";
        return false;
    }
    if (!FinishClocks(
            native_started,
            &timing->native_total_wall_ns,
            &timing->native_total_thread_ns)) {
        *failure = "npu-native-clock-finish";
        return false;
    }

    if (!StartClocks(&conversion_started)) {
        *failure = "npu-convert-out-clock-start";
        return false;
    }
    for (size_t index = 0; index < kOutputElements; ++index) {
        output->data()[index] =
            HalfBitsToFloat(output_half->data()[index]);
    }
    if (!FinishClocks(
            conversion_started,
            &timing->convert_out_wall_ns,
            &timing->convert_out_thread_ns) ||
        !FinishClocks(
            bridge_started,
            &timing->bridge_total_wall_ns,
            &timing->bridge_total_thread_ns)) {
        *failure = "npu-convert-out-or-bridge-clock-finish";
        return false;
    }
    if (!input.CanariesIntact() ||
        !input_half->CanariesIntact() ||
        !output_half->CanariesIntact() ||
        !output->CanariesIntact()) {
        *failure = "npu-canary";
        return false;
    }
    return true;
}

void FillRandomInput(
    GuardedArray<float>* input,
    uint32_t* state) {
    for (size_t index = 0; index < kInputElements; ++index) {
        *state = NextXorshift32(*state);
        const int32_t quantized =
            static_cast<int32_t>(*state & 0x1ffU) - 256;
        input->data()[index] =
            static_cast<float>(quantized) / 256.0F;
    }
}

const char* FillQualificationFixture(
    size_t fixture,
    GuardedArray<float>* input,
    uint32_t* qualification_state) {
    input->Reset();
    if (fixture == 0U) {
        std::fill_n(input->data(), kInputElements, 0.0F);
        return "zero";
    }
    if (fixture == 1U) {
        std::fill_n(input->data(), kInputElements, 0.0F);
        input->data()[kInputElements / 2U] = 0.5F;
        return "impulse";
    }
    if (fixture == 2U) {
        constexpr std::array<float, 5> kStripe = {
            -0.5F, -0.25F, 0.0F, 0.25F, 0.5F};
        for (size_t index = 0; index < kInputElements; ++index) {
            input->data()[index] = kStripe[index % kStripe.size()];
        }
        return "stripe";
    }
    FillRandomInput(input, qualification_state);
    return "xorshift";
}

bool CompareOutputs(
    const GuardedArray<float>& cpu_output,
    const GuardedArray<float>& npu_output,
    GuardedArray<uint16_t>* cpu_half,
    NumericalMetrics* metrics) {
    cpu_half->Reset();
    long double sum_absolute_error = 0.0L;
    long double dot_product = 0.0L;
    long double cpu_squared = 0.0L;
    long double npu_squared = 0.0L;
    double maximum_absolute_error = 0.0;
    uint32_t finite_count = 0;
    for (size_t index = 0; index < kOutputElements; ++index) {
        const uint16_t rounded_bits =
            FloatToHalfBits(cpu_output.data()[index]);
        cpu_half->data()[index] = rounded_bits;
        const float rounded_cpu = HalfBitsToFloat(rounded_bits);
        const float npu = npu_output.data()[index];
        if (!std::isfinite(cpu_output.data()[index]) ||
            !std::isfinite(rounded_cpu) ||
            !std::isfinite(npu)) {
            continue;
        }
        ++finite_count;
        const double difference =
            std::fabs(
                static_cast<double>(rounded_cpu) -
                static_cast<double>(npu));
        sum_absolute_error += difference;
        maximum_absolute_error =
            std::max(maximum_absolute_error, difference);
        dot_product +=
            static_cast<long double>(rounded_cpu) *
            static_cast<long double>(npu);
        cpu_squared +=
            static_cast<long double>(rounded_cpu) *
            static_cast<long double>(rounded_cpu);
        npu_squared +=
            static_cast<long double>(npu) *
            static_cast<long double>(npu);
    }
    uint32_t top1_matches = 0;
    if (finite_count == kOutputElements) {
        for (size_t step = 0; step < kTimeSteps; ++step) {
            const size_t first = step * kClasses;
            size_t cpu_top1 = 0;
            size_t npu_top1 = 0;
            float cpu_max =
                HalfBitsToFloat(cpu_half->data()[first]);
            float npu_max = npu_output.data()[first];
            for (size_t class_index = 1;
                 class_index < kClasses;
                 ++class_index) {
                const float cpu_value = HalfBitsToFloat(
                    cpu_half->data()[first + class_index]);
                const float npu_value =
                    npu_output.data()[first + class_index];
                if (cpu_value > cpu_max) {
                    cpu_max = cpu_value;
                    cpu_top1 = class_index;
                }
                if (npu_value > npu_max) {
                    npu_max = npu_value;
                    npu_top1 = class_index;
                }
            }
            if (cpu_top1 == npu_top1) {
                ++top1_matches;
            }
        }
    }
    const long double cosine_denominator =
        std::sqrt(cpu_squared) * std::sqrt(npu_squared);
    const double cosine_similarity =
        cosine_denominator > 0.0L
            ? static_cast<double>(dot_product / cosine_denominator)
            : 0.0;
    metrics->top1_matches = top1_matches;
    metrics->finite_count = finite_count;
    metrics->sum_absolute_error =
        static_cast<double>(sum_absolute_error);
    metrics->mean_absolute_error =
        finite_count == 0
            ? std::numeric_limits<double>::infinity()
            : static_cast<double>(
                  sum_absolute_error /
                  static_cast<long double>(finite_count));
    metrics->maximum_absolute_error = maximum_absolute_error;
    metrics->cosine_dot_product =
        static_cast<double>(dot_product);
    metrics->cosine_cpu_squared =
        static_cast<double>(cpu_squared);
    metrics->cosine_npu_squared =
        static_cast<double>(npu_squared);
    metrics->cosine_similarity = cosine_similarity;
    return cpu_output.CanariesIntact() &&
           npu_output.CanariesIntact() &&
           cpu_half->CanariesIntact() &&
           finite_count == kOutputElements &&
           top1_matches == kTimeSteps &&
           std::isfinite(metrics->mean_absolute_error) &&
           std::isfinite(metrics->maximum_absolute_error) &&
           std::isfinite(metrics->cosine_similarity) &&
           metrics->mean_absolute_error <=
               kMeanAbsoluteErrorMax &&
           metrics->maximum_absolute_error <=
               kMaximumAbsoluteErrorMax &&
           metrics->cosine_similarity >=
               kCosineSimilarityMin;
}

bool QualifyFixture(
    uint32_t fixture_id,
    const char* fixture_name,
    CpuRuntime* cpu,
    NpuRuntime* npu,
    GuardedArray<float>* input,
    GuardedArray<float>* cpu_output,
    GuardedArray<uint16_t>* cpu_half,
    GuardedArray<uint16_t>* npu_input_half,
    GuardedArray<uint16_t>* npu_output_half,
    GuardedArray<float>* npu_output,
    GuardedArray<uint16_t>* repeat_output_half,
    GuardedArray<float>* repeat_output,
    QualificationRecord* record,
    const char** failure) {
    TimingRecord ignored_cpu_timing {};
    TimingRecord ignored_npu_timing {};
    TimingRecord ignored_repeat_timing {};
    cpu_output->Reset();
    if (!RunCpu(
            cpu,
            *input,
            cpu_output,
            &ignored_cpu_timing,
            failure) ||
        !RunNpu(
            npu,
            *input,
            npu_input_half,
            npu_output_half,
            npu_output,
            &ignored_npu_timing,
            failure) ||
        !RunNpu(
            npu,
            *input,
            npu_input_half,
            repeat_output_half,
            repeat_output,
            &ignored_repeat_timing,
            failure)) {
        return false;
    }
    NumericalMetrics metrics {};
    if (!CompareOutputs(
            *cpu_output, *npu_output, cpu_half, &metrics)) {
        *failure = "qualification-numerical-gate";
        return false;
    }
    if (std::memcmp(
            npu_output_half->data(),
            repeat_output_half->data(),
            kOutputHalfBytes) != 0 ||
        !repeat_output_half->CanariesIntact() ||
        !repeat_output->CanariesIntact()) {
        *failure = "qualification-repeatability";
        return false;
    }

    record->fixture_id = fixture_id;
    record->top1_matches = metrics.top1_matches;
    record->finite_count = metrics.finite_count;
    record->reserved = 0;
    record->cpu_float_fnv = UpdateFnv1a64(
        kFnvOffsetBasis,
        reinterpret_cast<const uint8_t*>(cpu_output->data()),
        kOutputFloatBytes);
    record->cpu_half_fnv = UpdateFnv1a64(
        kFnvOffsetBasis,
        reinterpret_cast<const uint8_t*>(cpu_half->data()),
        kOutputHalfBytes);
    record->npu_half_fnv = UpdateFnv1a64(
        kFnvOffsetBasis,
        reinterpret_cast<const uint8_t*>(npu_output_half->data()),
        kOutputHalfBytes);
    record->npu_repeat_fnv = UpdateFnv1a64(
        kFnvOffsetBasis,
        reinterpret_cast<const uint8_t*>(repeat_output_half->data()),
        kOutputHalfBytes);
    record->sum_absolute_error = metrics.sum_absolute_error;
    record->maximum_absolute_error =
        metrics.maximum_absolute_error;
    record->cosine_dot_product = metrics.cosine_dot_product;
    record->cosine_cpu_squared = metrics.cosine_cpu_squared;
    record->cosine_npu_squared = metrics.cosine_npu_squared;

    std::printf(
        "CTC_QUAL fixture=%s fixture_id=%u status=PASS "
        "top1=%u finite=%u mae=%.9g max_abs=%.9g cosine=%.12g "
        "repeat=PASS\n",
        fixture_name,
        fixture_id,
        metrics.top1_matches,
        metrics.finite_count,
        metrics.mean_absolute_error,
        metrics.maximum_absolute_error,
        metrics.cosine_similarity);
    std::fflush(stdout);
    return true;
}

bool RunPair(
    uint32_t sequence,
    uint32_t order,
    CpuRuntime* cpu,
    NpuRuntime* npu,
    GuardedArray<float>* input,
    GuardedArray<float>* cpu_output,
    GuardedArray<uint16_t>* cpu_half,
    GuardedArray<uint16_t>* npu_input_half,
    GuardedArray<uint16_t>* npu_output_half,
    GuardedArray<float>* npu_output,
    TimingRecord* record,
    const char** failure) {
    *record = TimingRecord {};
    record->sequence = sequence;
    record->order = order;
    cpu_output->Reset();
    bool ran = false;
    if (order == 0U) {
        ran =
            RunCpu(cpu, *input, cpu_output, record, failure) &&
            RunNpu(
                npu,
                *input,
                npu_input_half,
                npu_output_half,
                npu_output,
                record,
                failure);
    } else if (order == 1U) {
        ran =
            RunNpu(
                npu,
                *input,
                npu_input_half,
                npu_output_half,
                npu_output,
                record,
                failure) &&
            RunCpu(cpu, *input, cpu_output, record, failure);
    } else {
        *failure = "pair-order";
        return false;
    }
    if (!ran) {
        return false;
    }
    NumericalMetrics metrics {};
    if (!CompareOutputs(
            *cpu_output, *npu_output, cpu_half, &metrics)) {
        *failure = "pair-numerical-gate";
        return false;
    }
    return true;
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

void AppendDouble(std::vector<uint8_t>* output, double value) {
    static_assert(sizeof(double) == sizeof(uint64_t), "binary64 ABI");
    uint64_t bits = 0;
    std::memcpy(&bits, &value, sizeof(bits));
    AppendLe64(output, bits);
}

std::vector<uint8_t> SerializeTimings(
    const std::array<QualificationRecord, kFixtureCount>& qualifications,
    const TimingRecord& cold,
    const std::vector<TimingRecord>& measured) {
    std::vector<uint8_t> payload;
    payload.reserve(kExpectedTimingBytes);
    payload.insert(payload.end(), {'C', 'C', 'N', '1'});
    AppendLe32(&payload, kTimingSchemaVersion);
    AppendLe32(&payload, static_cast<uint32_t>(kFixtureCount));
    AppendLe32(&payload, static_cast<uint32_t>(kMeasuredPairs));
    AppendLe32(&payload, static_cast<uint32_t>(kInputElements));
    AppendLe32(&payload, static_cast<uint32_t>(kOutputElements));
    AppendLe32(&payload, static_cast<uint32_t>(kInputFloatBytes));
    AppendLe32(&payload, static_cast<uint32_t>(kInputHalfBytes));
    AppendLe32(&payload, static_cast<uint32_t>(kOutputFloatBytes));
    AppendLe32(&payload, static_cast<uint32_t>(kOutputHalfBytes));
    AppendLe32(&payload, static_cast<uint32_t>(kColdPairs));
    AppendLe32(&payload, static_cast<uint32_t>(kWarmupPairs));
    AppendLe32(&payload, kQualificationSeed);
    AppendLe32(&payload, kBenchmarkSeed);
    AppendLe32(&payload, static_cast<uint32_t>(kTimingRecordBytes));
    AppendLe32(
        &payload, static_cast<uint32_t>(kQualificationRecordBytes));
    for (const QualificationRecord& record : qualifications) {
        AppendLe32(&payload, record.fixture_id);
        AppendLe32(&payload, record.top1_matches);
        AppendLe32(&payload, record.finite_count);
        AppendLe32(&payload, record.reserved);
        AppendLe64(&payload, record.cpu_float_fnv);
        AppendLe64(&payload, record.cpu_half_fnv);
        AppendLe64(&payload, record.npu_half_fnv);
        AppendLe64(&payload, record.npu_repeat_fnv);
        AppendDouble(&payload, record.sum_absolute_error);
        AppendDouble(&payload, record.maximum_absolute_error);
        AppendDouble(&payload, record.cosine_dot_product);
        AppendDouble(&payload, record.cosine_cpu_squared);
        AppendDouble(&payload, record.cosine_npu_squared);
    }
    const auto append_timing = [&payload](const TimingRecord& record) {
        AppendLe32(&payload, record.sequence);
        AppendLe32(&payload, record.order);
        AppendLe64(&payload, record.cpu_wall_ns);
        AppendLe64(&payload, record.cpu_thread_ns);
        AppendLe64(&payload, record.convert_in_wall_ns);
        AppendLe64(&payload, record.convert_in_thread_ns);
        AppendLe64(&payload, record.input_set_wall_ns);
        AppendLe64(&payload, record.input_set_thread_ns);
        AppendLe64(&payload, record.run_wall_ns);
        AppendLe64(&payload, record.run_thread_ns);
        AppendLe64(&payload, record.output_get_wall_ns);
        AppendLe64(&payload, record.output_get_thread_ns);
        AppendLe64(&payload, record.convert_out_wall_ns);
        AppendLe64(&payload, record.convert_out_thread_ns);
        AppendLe64(&payload, record.native_total_wall_ns);
        AppendLe64(&payload, record.native_total_thread_ns);
        AppendLe64(&payload, record.bridge_total_wall_ns);
        AppendLe64(&payload, record.bridge_total_thread_ns);
    };
    append_timing(cold);
    for (const TimingRecord& record : measured) {
        append_timing(record);
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
        "/tmp/wave-npu-poc15a/result",
        O_RDONLY | O_CLOEXEC | O_DIRECTORY);
    if (directory_fd < 0) {
        return false;
    }
    const bool directory_synced = fsync(directory_fd) == 0;
    const bool directory_closed = close(directory_fd) == 0;
    return directory_synced && directory_closed;
}

void PrintDefinition() {
    std::printf(
        "{\"schema_version\":1,\"mode\":\"benchmark\","
        "\"onnx\":{\"path\":\"%s\",\"bytes\":%zu,\"sha256\":\"%s\","
        "\"input\":\"FLOAT32[1,16,512]\","
        "\"output\":\"FLOAT32[1,16,4999]\"},"
        "\"nbg\":{\"path\":\"%s\",\"bytes\":%zu,\"sha256\":\"%s\","
        "\"input\":\"FP16[512,16,1]\","
        "\"output\":\"FP16[4999,16,1]\"},"
        "\"ort\":{\"path\":\"%s\",\"bytes\":%zu,\"sha256\":\"%s\","
        "\"version\":\"1.15.1\","
        "\"provider\":\"CPUExecutionProvider\"},"
        "\"nnsdk\":{\"path\":\"%s\",\"bytes\":%zu,\"sha256\":\"%s\"},"
        "\"header_sha256\":\"%s\","
        "\"fixtures\":4,\"qualification_seed\":\"0x%08x\","
        "\"benchmark_seed\":\"0x%08x\",\"cold_pairs\":1,"
        "\"warmup_pairs\":20,\"measured_pairs\":200,"
        "\"alternating_order\":true,"
        "\"timing_schema\":\"CCN1-v1\","
        "\"qualification_record_bytes\":%zu,"
        "\"timing_record_bytes\":%zu,\"timing_bytes\":%zu,"
        "\"executes_graph\":true}\n",
        kOnnxPath,
        kExpectedOnnxBytes,
        kOnnxSha256,
        kNbgPath,
        kExpectedNbgBytes,
        kNbgSha256,
        kOrtLibraryPath,
        kExpectedOrtBytes,
        kOrtSha256,
        kNnSdkLibraryPath,
        kExpectedNnSdkBytes,
        kNnSdkSha256,
        kHeaderSha256,
        kQualificationSeed,
        kBenchmarkSeed,
        kQualificationRecordBytes,
        kTimingRecordBytes,
        kExpectedTimingBytes);
}

int Fail(int exit_code, const char* stage) {
    std::fprintf(
        stderr,
        "CTC_CPU_NPU_BENCH status=FAIL mode=benchmark "
        "stage=%s exit=%d\n",
        stage,
        exit_code);
    return exit_code;
}

int Benchmark() {
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() == 1) {
        return Fail(10, "parent-death");
    }
    if (std::signal(SIGALRM, WatchdogHandler) == SIG_ERR) {
        return Fail(10, "watchdog-signal");
    }
    alarm(kWatchdogSeconds);
    if (!EnvironmentIsFixed()) {
        return Fail(10, "environment");
    }
    if (!ValidateRegularFile(
            kOnnxPath, kExpectedOnnxBytes, true)) {
        return Fail(11, "onnx-metadata");
    }

    uint64_t process_started = 0;
    uint64_t wall_started = 0;
    if (!ReadClock(CLOCK_PROCESS_CPUTIME_ID, &process_started) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &wall_started)) {
        return Fail(10, "process-clock-start");
    }

    GuardedArray<float> input(kInputElements);
    GuardedArray<float> cpu_output(kOutputElements);
    GuardedArray<uint16_t> cpu_half(kOutputElements);
    GuardedArray<uint16_t> npu_input_half(kInputElements);
    GuardedArray<uint16_t> npu_output_half(kOutputElements);
    GuardedArray<float> npu_output(kOutputElements);
    GuardedArray<uint16_t> repeat_output_half(kOutputElements);
    GuardedArray<float> repeat_output(kOutputElements);
    CpuRuntime cpu {};
    NpuRuntime npu {};

    if (!InitializeCpu(&cpu, &input, &cpu_output)) {
        CleanupCpu(&cpu);
        return Fail(13, "cpu-runtime-init");
    }
    if (!InitializeNpu(&npu)) {
        CleanupNpu(&npu);
        CleanupCpu(&cpu);
        return Fail(15, "npu-runtime-init");
    }

    const char* failure = "unknown";
    uint32_t benchmark_state = kBenchmarkSeed;
    FillRandomInput(&input, &benchmark_state);
    TimingRecord cold {};
    if (!RunPair(
            0,
            0,
            &cpu,
            &npu,
            &input,
            &cpu_output,
            &cpu_half,
            &npu_input_half,
            &npu_output_half,
            &npu_output,
            &cold,
            &failure)) {
        CleanupNpu(&npu);
        CleanupCpu(&cpu);
        return Fail(19, failure);
    }

    std::array<QualificationRecord, kFixtureCount> qualifications {};
    uint32_t qualification_state = kQualificationSeed;
    for (size_t fixture = 0; fixture < kFixtureCount; ++fixture) {
        const char* const fixture_name = FillQualificationFixture(
            fixture, &input, &qualification_state);
        if (!QualifyFixture(
                static_cast<uint32_t>(fixture),
                fixture_name,
                &cpu,
                &npu,
                &input,
                &cpu_output,
                &cpu_half,
                &npu_input_half,
                &npu_output_half,
                &npu_output,
                &repeat_output_half,
                &repeat_output,
                &qualifications[fixture],
                &failure)) {
            CleanupNpu(&npu);
            CleanupCpu(&cpu);
            return Fail(19, failure);
        }
    }
    if (qualification_state != kExpectedFinalQualificationState) {
        CleanupNpu(&npu);
        CleanupCpu(&cpu);
        return Fail(19, "qualification-generator-state");
    }

    for (size_t pair = 0; pair < kWarmupPairs; ++pair) {
        FillRandomInput(&input, &benchmark_state);
        TimingRecord ignored {};
        if (!RunPair(
                static_cast<uint32_t>(pair),
                static_cast<uint32_t>(pair & 1U),
                &cpu,
                &npu,
                &input,
                &cpu_output,
                &cpu_half,
                &npu_input_half,
                &npu_output_half,
                &npu_output,
                &ignored,
                &failure)) {
            CleanupNpu(&npu);
            CleanupCpu(&cpu);
            return Fail(19, failure);
        }
    }

    std::vector<TimingRecord> measured;
    measured.reserve(kMeasuredPairs);
    uint64_t measured_input_fnv = kFnvOffsetBasis;
    uint64_t cpu_output_fnv = kFnvOffsetBasis;
    uint64_t npu_output_fnv = kFnvOffsetBasis;
    for (size_t pair = 0; pair < kMeasuredPairs; ++pair) {
        FillRandomInput(&input, &benchmark_state);
        measured_input_fnv = UpdateFnv1a64(
            measured_input_fnv,
            reinterpret_cast<const uint8_t*>(input.data()),
            kInputFloatBytes);
        TimingRecord record {};
        if (!RunPair(
                static_cast<uint32_t>(pair),
                static_cast<uint32_t>(pair & 1U),
                &cpu,
                &npu,
                &input,
                &cpu_output,
                &cpu_half,
                &npu_input_half,
                &npu_output_half,
                &npu_output,
                &record,
                &failure)) {
            CleanupNpu(&npu);
            CleanupCpu(&cpu);
            return Fail(19, failure);
        }
        cpu_output_fnv = UpdateFnv1a64(
            cpu_output_fnv,
            reinterpret_cast<const uint8_t*>(cpu_output.data()),
            kOutputFloatBytes);
        npu_output_fnv = UpdateFnv1a64(
            npu_output_fnv,
            reinterpret_cast<const uint8_t*>(
                npu_output_half.data()),
            kOutputHalfBytes);
        measured.push_back(record);
    }
    if (benchmark_state != kExpectedFinalBenchmarkState) {
        CleanupNpu(&npu);
        CleanupCpu(&cpu);
        return Fail(19, "benchmark-generator-state");
    }

    const std::vector<uint8_t> timing_payload =
        SerializeTimings(qualifications, cold, measured);
    if (timing_payload.size() != kExpectedTimingBytes) {
        CleanupNpu(&npu);
        CleanupCpu(&cpu);
        return Fail(23, "timing-serialization");
    }
    const uint64_t timing_fnv = UpdateFnv1a64(
        kFnvOffsetBasis,
        timing_payload.data(),
        timing_payload.size());

    const bool npu_cleanup_passed = CleanupNpu(&npu);
    CleanupCpu(&cpu);
    if (!npu_cleanup_passed) {
        return Fail(20, "cleanup");
    }
    uint64_t process_finished = 0;
    uint64_t wall_finished = 0;
    if (!ReadClock(CLOCK_PROCESS_CPUTIME_ID, &process_finished) ||
        !ReadClock(CLOCK_MONOTONIC_RAW, &wall_finished) ||
        process_finished <= process_started ||
        wall_finished <= wall_started) {
        return Fail(23, "process-clock-finish");
    }
    if (!PublishTimings(timing_payload)) {
        return Fail(23, "timing-publication");
    }

    alarm(0);
    std::printf(
        "CTC_CPU_NPU_BENCH status=PASS mode=benchmark "
        "cold_pairs=1 qualification_fixtures=4 warmup_pairs=20 "
        "measured_pairs=200 cpu_run_count=225 npu_run_count=229 "
        "module_create_count=1 session_create_count=1 "
        "canaries=PASS semantics=PASS timing_bytes=%zu "
        "timing_fnv1a64=0x%016llx "
        "measured_input_fnv1a64=0x%016llx "
        "cpu_output_fnv1a64=0x%016llx "
        "npu_output_fnv1a64=0x%016llx "
        "final_qualification_state=0x%08x "
        "final_benchmark_state=0x%08x "
        "process_cpu_ns=%llu process_wall_ns=%llu\n",
        timing_payload.size(),
        static_cast<unsigned long long>(timing_fnv),
        static_cast<unsigned long long>(measured_input_fnv),
        static_cast<unsigned long long>(cpu_output_fnv),
        static_cast<unsigned long long>(npu_output_fnv),
        qualification_state,
        benchmark_state,
        static_cast<unsigned long long>(
            process_finished - process_started),
        static_cast<unsigned long long>(wall_finished - wall_started));
    std::fflush(stdout);
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc == 2 && std::strcmp(argv[1], "--benchmark") == 0) {
        return Benchmark();
    }
    std::fprintf(
        stderr,
        "usage: %s --describe|--benchmark\n",
        argc > 0 ? argv[0] : "wave-npu-benchmark-ctc-cpu-vs-npu");
    return 2;
}
