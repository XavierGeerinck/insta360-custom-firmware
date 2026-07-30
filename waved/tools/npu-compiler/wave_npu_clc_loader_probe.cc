// SPDX-License-Identifier: MIT
//
// Fixed-path relocation-only probe for the Vivante CLC compiler library.
//
// The probe loads one allowlisted library with RTLD_NOW|RTLD_LOCAL and
// resolves three fixed symbols. It never calls those symbols, constructs an
// OpenVX object, opens a device node, or accepts a library/symbol path.

#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <sys/prctl.h>
#include <unistd.h>

namespace {

constexpr unsigned int kWatchdogSeconds = 8;
constexpr char kCompilerPath[] =
    "/tmp/wave-npu-compile/lib/libCLC.so";
constexpr char kExpectedLibraryPath[] =
    "/tmp/wave-npu-compile/lib:/usr/lib";

void WatchdogHandler(int) {
    constexpr char kMessage[] =
        "CLC status=TIMEOUT mode=relocation-only exit=124\n";
    const ssize_t message_bytes =
        write(STDERR_FILENO, kMessage, sizeof(kMessage) - 1);
    (void)message_bytes;
    _exit(124);
}

bool EnvironmentIsFixed() {
    const char* const library_path = std::getenv("LD_LIBRARY_PATH");
    if (library_path == nullptr ||
        std::strcmp(library_path, kExpectedLibraryPath) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: LD_LIBRARY_PATH must equal %s\n",
            kExpectedLibraryPath);
        return false;
    }
    if (std::getenv("LD_PRELOAD") != nullptr ||
        std::getenv("LD_AUDIT") != nullptr ||
        std::getenv("VSIMULATOR_CONFIG") != nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: interposition and simulator "
            "variables must be unset\n");
        return false;
    }
    return true;
}

void PrintDefinition() {
    std::puts(
        "{\"schema_version\":1,\"mode\":\"relocation-only\","
        "\"library\":\"/tmp/wave-npu-compile/lib/libCLC.so\","
        "\"flags\":[\"RTLD_NOW\",\"RTLD_LOCAL\"],"
        "\"symbols\":[\"gcLoadKernelCompiler\",\"gcCompileKernel\","
        "\"gcUnloadKernelCompiler\"],\"calls_symbols\":false,"
        "\"creates_context\":false,\"creates_graph\":false,"
        "\"executes_graph\":false}");
}

}  // namespace

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--describe") == 0) {
        PrintDefinition();
        return 0;
    }
    if (argc != 2 || std::strcmp(argv[1], "--probe") != 0) {
        std::fprintf(stderr, "usage: %s --probe\n", argv[0]);
        std::fprintf(stderr, "       %s --describe\n", argv[0]);
        return 10;
    }
    if (!EnvironmentIsFixed()) {
        return 12;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: PR_SET_PDEATHSIG failed: %s\n",
            std::strerror(errno));
        return 17;
    }
    if (getppid() == 1) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: parent exited before launch\n");
        return 17;
    }

    struct sigaction watchdog_action {};
    watchdog_action.sa_handler = WatchdogHandler;
    sigemptyset(&watchdog_action.sa_mask);
    if (sigaction(SIGALRM, &watchdog_action, nullptr) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: watchdog setup failed: %s\n",
            std::strerror(errno));
        return 18;
    }
    alarm(kWatchdogSeconds);

    dlerror();
    void* const handle = dlopen(kCompilerPath, RTLD_NOW | RTLD_LOCAL);
    if (handle == nullptr) {
        const char* const message = dlerror();
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: fixed library load failed: %s\n",
            message != nullptr ? message : "unknown loader error");
        return 1;
    }

    dlerror();
    void* const load_symbol = dlsym(handle, "gcLoadKernelCompiler");
    const char* load_error = dlerror();
    void* const compile_symbol = dlsym(handle, "gcCompileKernel");
    const char* compile_error = dlerror();
    void* const unload_symbol = dlsym(handle, "gcUnloadKernelCompiler");
    const char* unload_error = dlerror();
    if (load_error != nullptr || compile_error != nullptr ||
        unload_error != nullptr || load_symbol == nullptr ||
        compile_symbol == nullptr || unload_symbol == nullptr) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: fixed symbol resolution failed\n");
        dlclose(handle);
        return 2;
    }
    if (load_symbol == compile_symbol || load_symbol == unload_symbol ||
        compile_symbol == unload_symbol) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: compiler symbols alias\n");
        dlclose(handle);
        return 3;
    }
    if (dlclose(handle) != 0) {
        std::fprintf(
            stderr,
            "wave_npu_clc_loader_probe: fixed library close failed\n");
        return 4;
    }
    alarm(0);

    std::puts(
        "CLC status=PASS mode=relocation-only "
        "library=/tmp/wave-npu-compile/lib/libCLC.so "
        "exports=3 calls=0");
    return 0;
}
