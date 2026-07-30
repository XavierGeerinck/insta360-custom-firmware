// SPDX-License-Identifier: MIT
//
// Fixed read-only OpenVX capability query for the Insta360 Wave.

#include "VX/vx.h"
#include "VX/vx_khr_nn.h"

#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sys/prctl.h>
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

struct HardwareCapsBaseBatch {
    uint32_t eco_id;
    uint32_t customer_id;
    uint32_t evis1;
    uint32_t evis2;
    uint32_t core_count;
};

struct HardwareCapsExtBatch {
    HardwareCapsBaseBatch base;
    uint32_t subgroup_size;
    uint32_t support_va40;
    uint32_t support_stream_processor;
};

struct HardwareCapsExt2Batch {
    HardwareCapsExtBatch base;
    uint32_t stream_processor_exec_count;
    uint32_t stream_processor_vector_size;
};

static_assert(sizeof(ChipIdentityEx) == 0x20, "unexpected identity ABI");
static_assert(sizeof(HardwareCapsBaseBatch) == 0x14, "unexpected caps base ABI");
static_assert(sizeof(HardwareCapsExtBatch) == 0x20, "unexpected caps ext ABI");
static_assert(sizeof(HardwareCapsExt2Batch) == 0x28, "unexpected caps ext2 ABI");

extern "C" int gcoHAL_QueryChipIdentityEx(
    void* hal,
    uint32_t identity_size,
    ChipIdentityEx* identity);

namespace {

constexpr uint32_t kWaveChipModel = 0x00009000U;
constexpr uint32_t kWaveCustomerId = 0x1000000eU;
constexpr unsigned int kWatchdogSeconds = 8;

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "RESULT status=TIMEOUT mode=query-only exit=124\n";
    const ssize_t message_bytes =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)message_bytes;
    _exit(124);
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"query-only\","
        "\"queries\":[\"gcoHAL_QueryChipIdentityEx\","
        "\"vxQueryHardwareCaps:0x20\","
        "\"vxQueryHardwareCaps:0x28\"],"
        "\"creates_graph\":false,\"compiles_graph\":false,"
        "\"executes_graph\":false,\"writes_files\":false}");
}

bool InstallContainment() {
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_query_caps: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return false;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr, "wave_npu_query_caps: parent exited before launch\n");
        return false;
    }
    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_query_caps: watchdog setup failed: %s\n",
            std::strerror(errno));
        return false;
    }
    alarm(kWatchdogSeconds);
    return true;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--query") != 0) {
        std::fprintf(stderr, "usage: %s --query\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (std::getenv("VSIMULATOR_CONFIG") != nullptr ||
        std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_query_caps: simulator and loader interposition "
            "variables must be unset\n");
        return 12;
    }
    if (!InstallContainment()) {
        return 17;
    }

    vx_context context = vxCreateContext();
    if (context == nullptr ||
        vxGetStatus(reinterpret_cast<vx_reference>(context)) != VX_SUCCESS) {
        std::fprintf(stderr, "wave_npu_query_caps: context create failed\n");
        if (context != nullptr) {
            vxReleaseContext(&context);
        }
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
            "wave_npu_query_caps: identity query failed: %d\n",
            identity_status);
        vxReleaseContext(&context);
        return 2;
    }
    if (identity.chip_model != kWaveChipModel ||
        identity.customer_id != kWaveCustomerId) {
        std::fprintf(stderr, "wave_npu_query_caps: identity mismatch\n");
        vxReleaseContext(&context);
        return 3;
    }

    HardwareCapsExtBatch extended {};
    HardwareCapsExt2Batch extended2 {};
    const vx_status extended_status = vxQueryHardwareCaps(
        context,
        reinterpret_cast<const vx_hardware_caps_params_t*>(&extended),
        static_cast<vx_size>(sizeof(extended)));
    const vx_status extended2_status = vxQueryHardwareCaps(
        context,
        reinterpret_cast<const vx_hardware_caps_params_t*>(&extended2),
        static_cast<vx_size>(sizeof(extended2)));
    if (extended_status != VX_SUCCESS || extended2_status != VX_SUCCESS) {
        std::fprintf(
            stderr,
            "wave_npu_query_caps: capability query failed: ext=%d ext2=%d\n",
            extended_status,
            extended2_status);
        vxReleaseContext(&context);
        return 4;
    }
    if (std::memcmp(&extended, &extended2, sizeof(extended)) != 0) {
        std::fprintf(
            stderr, "wave_npu_query_caps: capability prefix mismatch\n");
        vxReleaseContext(&context);
        return 5;
    }
    if (extended.base.customer_id != identity.customer_id ||
        extended.base.eco_id != identity.chip_eco_id) {
        std::fprintf(
            stderr, "wave_npu_query_caps: capability identity mismatch\n");
        vxReleaseContext(&context);
        return 6;
    }

    const vx_status release_status = vxReleaseContext(&context);
    if (release_status != VX_SUCCESS || context != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_query_caps: context release failed: %d\n",
            release_status);
        return 7;
    }
    alarm(0);

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
    std::printf(
        "CAPS status=PASS mode=query-only ext_bytes=%zu ext2_bytes=%zu "
        "eco_id=0x%08x customer_id=0x%08x evis1=%u evis2=%u "
        "core_count=%u subgroup_size=%u support_va40=%u "
        "support_stream_processor=%u stream_processor_exec_count=%u "
        "stream_processor_vector_size=%u\n",
        sizeof(extended),
        sizeof(extended2),
        extended.base.eco_id,
        extended.base.customer_id,
        extended.base.evis1,
        extended.base.evis2,
        extended.base.core_count,
        extended.subgroup_size,
        extended.support_va40,
        extended.support_stream_processor,
        extended2.stream_processor_exec_count,
        extended2.stream_processor_vector_size);
    std::fflush(stdout);
    return 0;
}
