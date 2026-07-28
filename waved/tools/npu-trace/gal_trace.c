#define _GNU_SOURCE

#include <errno.h>
#include <inttypes.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <unistd.h>

/*
 * Metadata-only observation shim for the Wave's GAL userspace boundary.
 *
 * This intentionally does not include a vendor header. The two prefixes below
 * are the complete version-adjacent evidence used by this probe, copied into
 * neutral fixed-width types. No command-specific union is read.
 */

#define WAVE_GAL_INTERFACE_IOCTL 30000UL
#define WAVE_GAL_INTERFACE_MIN_BYTES 16U
#define WAVE_GAL_INTERFACE_MAX_BYTES (64U * 1024U)
#define WAVE_GAL_TRACE_BUFFER_BYTES 512U

typedef struct {
    uint64_t input_buffer;
    uint64_t input_buffer_size;
    uint64_t output_buffer;
    uint64_t output_buffer_size;
} wave_driver_args;

typedef struct {
    uint32_t command;
    uint32_t hardware_type;
    uint32_t core_index;
    int32_t status;
} wave_hal_prefix;

_Static_assert(sizeof(wave_driver_args) == 32U,
               "the observed GAL driver envelope must remain 32 bytes");
_Static_assert(sizeof(wave_hal_prefix) == 16U,
               "the observed HAL metadata prefix must remain 16 bytes");
_Static_assert(offsetof(wave_hal_prefix, status) == 12U,
               "the observed HAL status offset must remain 12");

static uint64_t next_sequence = 1U;
static __thread bool inside_ioctl_hook;

static bool safe_read_self(const uint64_t remote_address, void *destination,
                           const size_t length)
{
    if (remote_address == 0U || destination == NULL || length == 0U) {
        return false;
    }

    const struct iovec local = {
        .iov_base = destination,
        .iov_len = length,
    };
    const struct iovec remote = {
        .iov_base = (void *)(uintptr_t)remote_address,
        .iov_len = length,
    };

    const long copied =
        syscall(SYS_process_vm_readv, getpid(), &local, 1UL, &remote, 1UL, 0UL);
    return copied == (long)length;
}

static void emit_line(const char *const line, const int length)
{
    if (line == NULL || length <= 0 ||
        (size_t)length >= WAVE_GAL_TRACE_BUFFER_BYTES) {
        return;
    }

    const int saved_errno = errno;
    (void)syscall(SYS_write, STDERR_FILENO, line, (size_t)length);
    errno = saved_errno;
}

static bool envelope_is_valid(const wave_driver_args *const args)
{
    if (args == NULL || args->input_buffer == 0U || args->output_buffer == 0U) {
        return false;
    }

    if (args->input_buffer_size != args->output_buffer_size) {
        return false;
    }

    return args->input_buffer_size >= WAVE_GAL_INTERFACE_MIN_BYTES &&
           args->input_buffer_size <= WAVE_GAL_INTERFACE_MAX_BYTES;
}

static void emit_unreadable(const char *const event, const uint64_t sequence,
                            const bool include_result, const int result,
                            const int call_errno)
{
    char line[WAVE_GAL_TRACE_BUFFER_BYTES];
    int length;

    if (include_result) {
        length = snprintf(line, sizeof(line),
                          "WAVE_GAL_TRACE v=1 event=%s seq=%" PRIu64
                          " request=%lu envelope=unreadable"
                          " result=%d errno=%d\n",
                          event, sequence, WAVE_GAL_INTERFACE_IOCTL, result,
                          call_errno);
    } else {
        length = snprintf(line, sizeof(line),
                          "WAVE_GAL_TRACE v=1 event=%s seq=%" PRIu64
                          " request=%lu envelope=unreadable\n",
                          event, sequence, WAVE_GAL_INTERFACE_IOCTL);
    }
    emit_line(line, length);
}

static void emit_invalid(const char *const event, const uint64_t sequence,
                         const wave_driver_args *const args,
                         const bool include_result, const int result,
                         const int call_errno)
{
    char line[WAVE_GAL_TRACE_BUFFER_BYTES];
    int length;

    if (include_result) {
        length = snprintf(
            line, sizeof(line),
            "WAVE_GAL_TRACE v=1 event=%s seq=%" PRIu64
            " request=%lu envelope=invalid input_bytes=%" PRIu64
            " output_bytes=%" PRIu64 " result=%d errno=%d\n",
            event, sequence, WAVE_GAL_INTERFACE_IOCTL, args->input_buffer_size,
            args->output_buffer_size, result, call_errno);
    } else {
        length = snprintf(
            line, sizeof(line),
            "WAVE_GAL_TRACE v=1 event=%s seq=%" PRIu64
            " request=%lu envelope=invalid input_bytes=%" PRIu64
            " output_bytes=%" PRIu64 "\n",
            event, sequence, WAVE_GAL_INTERFACE_IOCTL, args->input_buffer_size,
            args->output_buffer_size);
    }
    emit_line(line, length);
}

static void emit_enter(const uint64_t sequence,
                       const wave_driver_args *const args,
                       const wave_hal_prefix *const prefix)
{
    char line[WAVE_GAL_TRACE_BUFFER_BYTES];
    const int length = snprintf(
        line, sizeof(line),
        "WAVE_GAL_TRACE v=1 event=enter seq=%" PRIu64
        " request=%lu envelope=ok command=%" PRIu32
        " hardware=%" PRIu32 " core=%" PRIu32
        " interface_bytes=%" PRIu64 "\n",
        sequence, WAVE_GAL_INTERFACE_IOCTL, prefix->command,
        prefix->hardware_type, prefix->core_index, args->input_buffer_size);
    emit_line(line, length);
}

static void emit_exit(const uint64_t sequence,
                      const wave_driver_args *const args,
                      const wave_hal_prefix *const prefix, const int result,
                      const int call_errno)
{
    char line[WAVE_GAL_TRACE_BUFFER_BYTES];
    const int length = snprintf(
        line, sizeof(line),
        "WAVE_GAL_TRACE v=1 event=exit seq=%" PRIu64
        " request=%lu envelope=ok command=%" PRIu32
        " hardware=%" PRIu32 " core=%" PRIu32
        " interface_bytes=%" PRIu64 " result=%d errno=%d status=%" PRId32
        "\n",
        sequence, WAVE_GAL_INTERFACE_IOCTL, prefix->command,
        prefix->hardware_type, prefix->core_index, args->output_buffer_size,
        result, call_errno, prefix->status);
    emit_line(line, length);
}

__attribute__((visibility("default"))) int ioctl(const int file_descriptor,
                                                 const unsigned long request,
                                                 ...)
{
    va_list arguments;
    va_start(arguments, request);
    void *const argument = va_arg(arguments, void *);
    va_end(arguments);

    if (inside_ioctl_hook) {
        return (int)syscall(SYS_ioctl, file_descriptor, request, argument);
    }

    inside_ioctl_hook = true;

    if (request != WAVE_GAL_INTERFACE_IOCTL) {
        const int passthrough_result =
            (int)syscall(SYS_ioctl, file_descriptor, request, argument);
        inside_ioctl_hook = false;
        return passthrough_result;
    }

    const uint64_t sequence = __sync_fetch_and_add(&next_sequence, 1U);
    const int caller_errno = errno;
    wave_driver_args args = {0};
    wave_hal_prefix input_prefix = {0};
    const bool args_read =
        safe_read_self((uint64_t)(uintptr_t)argument, &args, sizeof(args));
    const bool valid = args_read && envelope_is_valid(&args);
    const bool input_read =
        valid && safe_read_self(args.input_buffer, &input_prefix,
                                sizeof(input_prefix));

    errno = caller_errno;
    if (!args_read || (valid && !input_read)) {
        emit_unreadable("enter", sequence, false, 0, 0);
    } else if (!valid) {
        emit_invalid("enter", sequence, &args, false, 0, 0);
    } else {
        emit_enter(sequence, &args, &input_prefix);
    }

    errno = caller_errno;
    const int result =
        (int)syscall(SYS_ioctl, file_descriptor, request, argument);
    const int call_errno = errno;
    const int reported_errno = result < 0 ? call_errno : 0;

    wave_hal_prefix output_prefix = {0};
    const bool output_read =
        valid && safe_read_self(args.output_buffer, &output_prefix,
                                sizeof(output_prefix));

    errno = call_errno;
    if (!args_read || (valid && !output_read)) {
        emit_unreadable("exit", sequence, true, result, reported_errno);
    } else if (!valid) {
        emit_invalid("exit", sequence, &args, true, result, reported_errno);
    } else {
        emit_exit(sequence, &args, &output_prefix, result, reported_errno);
    }

    inside_ioctl_hook = false;
    errno = call_errno;
    return result;
}
