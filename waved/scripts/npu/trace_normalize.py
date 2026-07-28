#!/usr/bin/env python3
"""Normalize bounded Wave NPU PoC 3 traces without retaining raw addresses.

The raw stdout/stderr streams remain in the gitignored NPU run directory.
This tool emits only lifecycle markers, path categories, syscall counts, and
the metadata-only GAL request prefix recorded by ``libwave-gal-trace.so``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


SCHEMA_VERSION = 1
MAX_INPUT_BYTES = 1024 * 1024 + 4096
MAX_LINE_BYTES = 4096
MAX_GAL_CALLS = 100_000
GAL_REQUEST = 30_000
MIN_INTERFACE_BYTES = 16
MAX_INTERFACE_BYTES = 64 * 1024

POINTER_RE = re.compile(r"\b0x[0-9a-fA-F]{8,}\b")
PHASE_RE = re.compile(r"^\[npu:(?P<number>[1-8])\]\s+(?P<description>.+)$")
TRACE_PHASE_RE = re.compile(r"\\?\[npu:(?P<number>[1-8])\\?\]")
STRACE_PREFIX_RE = re.compile(
    r"^(?:(?:\[pid\s+\d+\]|\d+)\s+)?"
    r"(?P<timestamp>\d+\.\d+)\s+(?P<body>.*)$"
)
SYSCALL_RE = re.compile(r"^(?P<name>[A-Za-z_][A-Za-z0-9_]*)\(")
RESUMED_RE = re.compile(
    r"^<\.\.\.\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s+resumed>"
)
GAL_PREFIX = "WAVE_GAL_TRACE "
GAL_BEGIN = "WAVE_NPU_TRACE_GAL_BEGIN"
GAL_END = "WAVE_NPU_TRACE_GAL_END"
STRACE_BEGIN = "WAVE_NPU_TRACE_STRACE_BEGIN"
STRACE_END = "WAVE_NPU_TRACE_STRACE_END"
TOKEN_RE = re.compile(r"^[a-z_]+=[A-Za-z0-9_-]+$")
OPENAT_PATH_RE = re.compile(
    r'^openat\([^,]+,\s+"(?P<path>(?:[^"\\]|\\.)*)"'
)
EXECVE_PATH_RE = re.compile(r'^execve\("(?P<path>(?:[^"\\]|\\.)*)"')
RESULT_FAILURE_RE = re.compile(r"\)\s+=\s+-1(?:\s|$)")
SYSCALL_RESULT_RE = re.compile(r"\)\s+=\s+(?P<result>-?[0-9]+|\?)(?:\s|$)")
GAL_WRITE_RE = re.compile(
    r'^write\(2</tmp/wave-npu-lab/gal\.raw>, '
    r'"WAVE_GAL_TRACE v=1 event=(?P<event>enter|exit)(?:\s|\\x20)'
)


# Values 0..32 are linear and unconditional in the pinned public 6.4.18.5
# enumeration. Labels are still adjacent-source evidence; the raw numeric
# command remains authoritative for the exact 6.4.19.5 runtime.
ADJACENT_COMMON_COMMANDS = {
    0: "CHIP_INFO",
    1: "VERSION",
    2: "QUERY_CHIP_IDENTITY",
    3: "QUERY_CHIP_OPTION",
    4: "QUERY_CHIP_FREQUENCY",
    5: "QUERY_VIDEO_MEMORY",
    6: "ALLOCATE_LINEAR_VIDEO_MEMORY",
    7: "WRAP_USER_MEMORY",
    8: "RELEASE_VIDEO_MEMORY",
    9: "LOCK_VIDEO_MEMORY",
    10: "UNLOCK_VIDEO_MEMORY",
    11: "BOTTOM_HALF_UNLOCK_VIDEO_MEMORY",
    12: "MAP_MEMORY",
    13: "UNMAP_MEMORY",
    14: "CACHE",
    15: "ATTACH",
    16: "DETACH",
    17: "EVENT_COMMIT",
    18: "COMMIT",
    19: "SET_TIMEOUT",
    20: "USER_SIGNAL",
    21: "SIGNAL",
    22: "SET_PROFILE_SETTING",
    23: "READ_PROFILER_REGISTER_SETTING",
    24: "READ_ALL_PROFILE_REGISTERS_PART1",
    25: "READ_ALL_PROFILE_REGISTERS_PART2",
    26: "DATABASE",
    27: "CONFIG_POWER_MANAGEMENT",
    28: "DEBUG_DUMP",
    29: "READ_REGISTER",
    30: "WRITE_REGISTER",
    31: "PROFILE_REGISTERS_2D",
    32: "GET_BASE_ADDRESS",
}

ADJACENT_HARDWARE_TYPES = {
    0: "INVALID",
    1: "3D2D",
    2: "3D",
    3: "2D",
    4: "VIP",
    5: "VG",
    6: "NUM_TYPES",
}

ALLOWED_SYSCALLS = frozenset(
    {
        "execve",
        "openat",
        "close",
        "ioctl",
        "mmap",
        "munmap",
        "mprotect",
        "brk",
        "futex",
        "poll",
        "ppoll",
        "select",
        "pselect6",
        "clone",
        "write",
        "exit",
        "exit_group",
    }
)

ENTER_KEYS = frozenset(
    {
        "v",
        "event",
        "seq",
        "request",
        "envelope",
        "command",
        "hardware",
        "core",
        "interface_bytes",
    }
)
EXIT_KEYS = ENTER_KEYS | frozenset({"result", "errno", "status"})


class TraceError(Exception):
    """A raw trace or result envelope violated the PoC 3 contract."""


def _json_object_without_duplicates(
    pairs: list[tuple[str, Any]],
) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise TraceError(f"duplicate JSON key: {key!r}")
        result[key] = value
    return result


def _load_json(path: Path) -> dict[str, Any]:
    try:
        size = path.stat().st_size
    except OSError as exc:
        raise TraceError(f"cannot stat {path}: {exc}") from exc
    if size > MAX_INPUT_BYTES:
        raise TraceError(f"JSON input exceeds {MAX_INPUT_BYTES} bytes: {path}")
    try:
        value = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_json_object_without_duplicates,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise TraceError(f"cannot read JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise TraceError(f"JSON root must be an object: {path}")
    return value


def _read_bounded_text(path: Path) -> str:
    try:
        size = path.stat().st_size
    except OSError as exc:
        raise TraceError(f"cannot stat {path}: {exc}") from exc
    if size > MAX_INPUT_BYTES:
        raise TraceError(f"trace input exceeds {MAX_INPUT_BYTES} bytes: {path}")
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise TraceError(f"cannot read trace input {path}: {exc}") from exc
    for line_number, line in enumerate(text.splitlines(), 1):
        if len(line.encode("utf-8")) > MAX_LINE_BYTES:
            raise TraceError(
                f"{path.name}:{line_number}: line exceeds {MAX_LINE_BYTES} bytes"
            )
    return text


def _bounded_decimal(
    value: str, field: str, minimum: int, maximum: int
) -> int:
    if re.fullmatch(r"-?[0-9]+", value) is None:
        raise TraceError(f"{field} must be decimal, observed {value!r}")
    parsed = int(value)
    if not minimum <= parsed <= maximum:
        raise TraceError(
            f"{field} outside {minimum}..{maximum}: {parsed}"
        )
    return parsed


def parse_gal_line(line: str) -> dict[str, Any]:
    if not line.startswith(GAL_PREFIX):
        raise TraceError("GAL line is missing its fixed prefix")
    if len(line.encode("ascii", errors="strict")) > 512:
        raise TraceError("GAL line exceeds the interposer bound")

    tokens = line[len(GAL_PREFIX) :].split()
    raw: dict[str, str] = {}
    for token in tokens:
        if TOKEN_RE.fullmatch(token) is None:
            raise TraceError(f"malformed GAL token: {token!r}")
        key, value = token.split("=", 1)
        if key in raw:
            raise TraceError(f"duplicate GAL field: {key}")
        raw[key] = value

    event = raw.get("event")
    expected_keys = ENTER_KEYS if event == "enter" else EXIT_KEYS
    if event not in {"enter", "exit"}:
        raise TraceError(f"unknown GAL event: {event!r}")
    if frozenset(raw) != expected_keys:
        missing = sorted(expected_keys - set(raw))
        unknown = sorted(set(raw) - expected_keys)
        raise TraceError(
            f"GAL {event} keys differ; missing={missing}, unknown={unknown}"
        )
    if raw["v"] != "1":
        raise TraceError(f"unsupported GAL trace version: {raw['v']!r}")
    if raw["envelope"] != "ok":
        raise TraceError(
            f"GAL envelope was not readable and valid: {raw['envelope']}"
        )

    record: dict[str, Any] = {
        "event": event,
        "seq": _bounded_decimal(raw["seq"], "seq", 1, MAX_GAL_CALLS),
        "request": _bounded_decimal(raw["request"], "request", 0, 2**32 - 1),
        "command": _bounded_decimal(raw["command"], "command", 0, 2**32 - 1),
        "hardware": _bounded_decimal(
            raw["hardware"], "hardware", 0, 2**32 - 1
        ),
        "core": _bounded_decimal(raw["core"], "core", 0, 2**32 - 1),
        "interface_bytes": _bounded_decimal(
            raw["interface_bytes"],
            "interface_bytes",
            MIN_INTERFACE_BYTES,
            MAX_INTERFACE_BYTES,
        ),
    }
    if record["request"] != GAL_REQUEST:
        raise TraceError(
            f"unexpected request in GAL record: {record['request']}"
        )

    if event == "exit":
        record.update(
            result=_bounded_decimal(
                raw["result"], "result", -(2**31), 2**31 - 1
            ),
            errno=_bounded_decimal(raw["errno"], "errno", 0, 4096),
            status=_bounded_decimal(
                raw["status"], "status", -(2**31), 2**31 - 1
            ),
        )
    return record


def _decode_strace_string(value: str) -> str:
    """Decode only strace's bounded ASCII escapes, never arbitrary Python ones."""

    output: list[str] = []
    index = 0
    while index < len(value):
        character = value[index]
        if character != "\\":
            if ord(character) < 0x20 or ord(character) == 0x7F:
                raise TraceError("control byte in strace path")
            output.append(character)
            index += 1
            continue

        if index + 1 >= len(value):
            raise TraceError("trailing escape in strace path")
        escaped = value[index + 1]
        if escaped in {'"', "\\", "'"}:
            output.append(escaped)
            index += 2
        elif escaped == "x" and index + 3 < len(value):
            digits = value[index + 2 : index + 4]
            if re.fullmatch(r"[0-9a-fA-F]{2}", digits) is None:
                raise TraceError("malformed hexadecimal strace escape")
            byte = int(digits, 16)
            if byte < 0x20 or byte == 0x7F:
                raise TraceError("control escape in strace path")
            output.append(chr(byte))
            index += 4
        else:
            raise TraceError(f"unsupported strace path escape: \\{escaped}")
    return "".join(output)


def _path_observation(path: str) -> tuple[str, str] | None:
    if path == "/dev/galcore":
        return ("device", "/dev/galcore")
    if path == "/dev/galcore1":
        return ("device-alternate", "/dev/galcore1")
    if path == "/model/asr_de/de_online/ctc_online.nb":
        return ("model", "ctc_online.nb")
    if path.startswith("/sys/module/galcore/parameters/"):
        return ("npu-parameter", Path(path).name)

    basename = Path(path).name
    if ".so" in basename and (
        path.startswith("/usr/lib/")
        or path.startswith("/lib/")
        or path.startswith("/lib64/")
        or path.startswith("/data/waved/")
    ):
        return ("shared-library", basename)
    return None


def _parse_stdout_phases(stdout: str) -> list[dict[str, Any]]:
    phases: list[dict[str, Any]] = []
    for line in stdout.splitlines():
        match = PHASE_RE.fullmatch(line)
        if match is None:
            continue
        phases.append(
            {
                "phase": int(match.group("number")),
                "description": match.group("description"),
            }
        )
    numbers = [item["phase"] for item in phases]
    if numbers != list(range(1, 9)):
        raise TraceError(
            f"stdout lifecycle must be exactly phases 1..8, observed {numbers}"
        )
    return phases


def _pair_gal_records(
    records: Sequence[Mapping[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, int]], list[int]]:
    pending: dict[int, Mapping[str, Any]] = {}
    calls_by_sequence: dict[int, dict[str, Any]] = {}
    results_by_sequence: dict[int, dict[str, int]] = {}
    for record in records:
        sequence = int(record["seq"])
        if record["event"] == "enter":
            if sequence in pending or sequence in calls_by_sequence:
                raise TraceError(f"duplicate GAL enter sequence: {sequence}")
            pending[sequence] = record
            continue

        enter = pending.pop(sequence, None)
        if enter is None:
            raise TraceError(f"GAL exit without enter: {sequence}")
        for key in (
            "request",
            "command",
            "hardware",
            "core",
            "interface_bytes",
        ):
            if enter[key] != record[key]:
                raise TraceError(
                    f"GAL sequence {sequence} changed {key}: "
                    f"{enter[key]} -> {record[key]}"
                )
        calls_by_sequence[sequence] = {
            "request": record["request"],
            "command": record["command"],
            "command_name_adjacent": ADJACENT_COMMON_COMMANDS.get(
                int(record["command"])
            ),
            "hardware": record["hardware"],
            "hardware_name_adjacent": ADJACENT_HARDWARE_TYPES.get(
                int(record["hardware"])
            ),
            "core": record["core"],
            "interface_bytes": record["interface_bytes"],
        }
        results_by_sequence[sequence] = {
            "result": int(record["result"]),
            "errno": int(record["errno"]),
            "status": int(record["status"]),
        }

    if pending:
        pending_sequences = sorted(pending)
        all_sequences = sorted(set(calls_by_sequence) | set(pending))
        if pending_sequences != [all_sequences[-1]]:
            raise TraceError(
                "only the final GAL call may lack an exit record, observed "
                f"{pending_sequences}"
            )
        sequence = pending_sequences[0]
        enter = pending[sequence]
        calls_by_sequence[sequence] = {
            "request": enter["request"],
            "command": enter["command"],
            "command_name_adjacent": ADJACENT_COMMON_COMMANDS.get(
                int(enter["command"])
            ),
            "hardware": enter["hardware"],
            "hardware_name_adjacent": ADJACENT_HARDWARE_TYPES.get(
                int(enter["hardware"])
            ),
            "core": enter["core"],
            "interface_bytes": enter["interface_bytes"],
        }

    sequences = sorted(calls_by_sequence)
    if sequences != list(range(1, len(sequences) + 1)):
        raise TraceError(
            f"GAL sequences are not contiguous from 1: {sequences[:16]}"
        )

    calls: list[dict[str, Any]] = []
    for ordinal, sequence in enumerate(sequences, 1):
        call = {"ordinal": ordinal, **calls_by_sequence[sequence]}
        calls.append(call)
    results = [
        {"ordinal": sequence, **results_by_sequence[sequence]}
        for sequence in sorted(results_by_sequence)
    ]
    return calls, results, sorted(pending)


def _extract_framed_sections(
    stdout: str, stderr: str
) -> tuple[str, str, str]:
    marker_set = {GAL_BEGIN, GAL_END, STRACE_BEGIN, STRACE_END}
    candidates = [
        (name, text)
        for name, text in (("stdout", stdout), ("stderr", stderr))
        if any(marker in text for marker in marker_set)
    ]
    if len(candidates) != 1:
        raise TraceError(
            "expected exactly one captured stream with trace frames, "
            f"observed {[name for name, _ in candidates]}"
        )

    channel, text = candidates[0]
    lines = text.splitlines()
    indices: dict[str, int] = {}
    for marker in (GAL_BEGIN, GAL_END, STRACE_BEGIN, STRACE_END):
        matches = [index for index, line in enumerate(lines) if line == marker]
        if len(matches) != 1:
            raise TraceError(
                f"expected one {marker} marker, observed {len(matches)}"
            )
        indices[marker] = matches[0]

    ordered = [
        indices[GAL_BEGIN],
        indices[GAL_END],
        indices[STRACE_BEGIN],
        indices[STRACE_END],
    ]
    if ordered != sorted(ordered):
        raise TraceError(f"trace frames are out of order: {ordered}")

    gal = "\n".join(lines[ordered[0] + 1 : ordered[1]])
    strace = "\n".join(lines[ordered[2] + 1 : ordered[3]])
    if not gal or not strace:
        raise TraceError("GAL and strace frames must both be nonempty")
    return gal, strace, channel


def _parse_gal_section(
    gal_text: str,
) -> tuple[
    list[dict[str, Any]],
    list[dict[str, int]],
    list[int],
    list[tuple[int, str]],
]:
    records: list[dict[str, Any]] = []
    for line_number, line in enumerate(gal_text.splitlines(), 1):
        if not line:
            continue
        if not line.startswith(GAL_PREFIX):
            raise TraceError(
                f"GAL frame:{line_number}: unrecognized diagnostic line"
            )
        try:
            records.append(parse_gal_line(line))
        except TraceError as exc:
            raise TraceError(f"GAL frame:{line_number}: {exc}") from exc
        if len(records) > MAX_GAL_CALLS * 2:
            raise TraceError("GAL record count exceeds bound")
    if not records:
        raise TraceError("trace contains no metadata-only GAL records")
    calls, results, incomplete = _pair_gal_records(records)
    event_order = [
        (int(record["seq"]), str(record["event"])) for record in records
    ]
    return calls, results, incomplete, event_order


def _parse_strace_section(
    strace_text: str,
) -> tuple[
    dict[str, int],
    list[int],
    list[dict[str, str]],
    dict[str, int],
    dict[str, int],
    list[tuple[str, int]],
    list[dict[str, Any]],
]:
    syscall_counts: Counter[str] = Counter()
    traced_phases: list[int] = []
    path_observations: list[dict[str, str]] = []
    path_seen: set[tuple[str, str]] = set()
    ignored_events: Counter[str] = Counter()
    ioctl_results: Counter[str] = Counter()
    gal_phase_events: list[tuple[str, int]] = []
    driver_events: list[dict[str, Any]] = []
    current_phase: int | None = None

    for line_number, line in enumerate(strace_text.splitlines(), 1):
        if not line:
            continue
        prefix = STRACE_PREFIX_RE.fullmatch(line)
        if prefix is None:
            if line.startswith("--- SIG") and line.endswith("---"):
                ignored_events["signal"] += 1
                continue
            if line.startswith("+++ ") and line.endswith(" +++"):
                ignored_events["process-status"] += 1
                continue
            if line.startswith("strace: Process ") and line.endswith(" attached"):
                ignored_events["attach"] += 1
                continue
            raise TraceError(
                f"strace frame:{line_number}: unrecognized trace line"
            )

        body = prefix.group("body")
        syscall_match = SYSCALL_RE.match(body)
        if syscall_match is None:
            resumed = RESUMED_RE.match(body)
            if resumed is not None:
                if resumed.group("name") not in ALLOWED_SYSCALLS:
                    raise TraceError(
                        f"strace frame:{line_number}: resumed unknown syscall "
                        f"{resumed.group('name')}"
                    )
                if resumed.group("name") == "ioctl":
                    result_match = re.search(
                        r"=\s+(?P<result>-?[0-9]+|\?)(?:\s|$)", body
                    )
                    if result_match is None:
                        ioctl_results["unresolved"] += 1
                    elif result_match.group("result") == "0":
                        ioctl_results["success"] += 1
                    else:
                        ioctl_results["failure"] += 1
                ignored_events["resumed"] += 1
                continue
            raise TraceError(
                f"strace frame:{line_number}: malformed syscall line"
            )

        syscall = syscall_match.group("name")
        if syscall not in ALLOWED_SYSCALLS:
            raise TraceError(
                f"strace frame:{line_number}: syscall outside fixed allowlist: "
                f"{syscall}"
            )
        syscall_counts[syscall] += 1

        if syscall == "ioctl" and "<unfinished ...>" not in body:
            result_match = SYSCALL_RESULT_RE.search(body)
            if result_match is None or result_match.group("result") == "?":
                ioctl_results["unresolved"] += 1
            elif result_match.group("result") == "0":
                ioctl_results["success"] += 1
            else:
                ioctl_results["failure"] += 1

        if syscall == "write":
            phase_match = TRACE_PHASE_RE.search(body)
            if phase_match is not None:
                current_phase = int(phase_match.group("number"))
                traced_phases.append(current_phase)
            gal_write = GAL_WRITE_RE.match(body)
            if gal_write is not None:
                if current_phase is None:
                    raise TraceError(
                        "GAL metadata write occurred before lifecycle phase 1"
                    )
                gal_phase_events.append(
                    (gal_write.group("event"), current_phase)
                )

        path_match = None
        if syscall == "openat":
            path_match = OPENAT_PATH_RE.match(body)
        elif syscall == "execve":
            path_match = EXECVE_PATH_RE.match(body)
        if path_match is not None and RESULT_FAILURE_RE.search(body) is None:
            path = _decode_strace_string(path_match.group("path"))
            observation = _path_observation(path)
            if observation is not None and observation not in path_seen:
                path_seen.add(observation)
                path_observations.append(
                    {"kind": observation[0], "name": observation[1]}
                )
            if path in {"/dev/galcore", "/dev/galcore1"}:
                driver_events.append(
                    {
                        "operation": "open",
                        "path": path,
                        "phase": current_phase,
                        "result": "success",
                    }
                )
        elif path_match is not None:
            path = _decode_strace_string(path_match.group("path"))
            if path == "/dev/galcore1":
                observation = ("device-alternate-missing", path)
                if observation not in path_seen:
                    path_seen.add(observation)
                    path_observations.append(
                        {"kind": observation[0], "name": observation[1]}
                    )
                driver_events.append(
                    {
                        "operation": "open",
                        "path": path,
                        "phase": current_phase,
                        "result": "missing",
                    }
                )

        if syscall == "close" and "</dev/galcore" in body:
            driver_events.append(
                {
                    "operation": "close",
                    "path": "/dev/galcore",
                    "phase": current_phase,
                    "result": "success",
                }
            )

    if traced_phases != list(range(1, 9)):
        raise TraceError(
            "strace write ordering must expose phases 1..8 exactly, "
            f"observed {traced_phases}"
        )

    return (
        dict(sorted(syscall_counts.items())),
        traced_phases,
        path_observations,
        dict(sorted(ignored_events.items())),
        dict(sorted(ioctl_results.items())),
        gal_phase_events,
        driver_events,
    )


def _correlate_gal_phases(
    calls: Sequence[Mapping[str, Any]],
    gal_event_order: Sequence[tuple[int, str]],
    gal_phase_events: Sequence[tuple[str, int]],
) -> list[dict[str, Any]]:
    if len(gal_event_order) != len(gal_phase_events):
        raise TraceError(
            "GAL metadata/strace phase-event count mismatch: "
            f"{len(gal_event_order)} != {len(gal_phase_events)}"
        )

    phase_by_sequence: dict[int, int] = {}
    for (sequence, gal_event), (trace_event, phase) in zip(
        gal_event_order, gal_phase_events, strict=True
    ):
        if gal_event != trace_event:
            raise TraceError(
                f"GAL event order mismatch at sequence {sequence}: "
                f"{gal_event} != {trace_event}"
            )
        existing = phase_by_sequence.get(sequence)
        if existing is None:
            phase_by_sequence[sequence] = phase
        elif existing != phase:
            raise TraceError(
                f"GAL sequence {sequence} crossed lifecycle phases "
                f"{existing} -> {phase}"
            )

    correlated: list[dict[str, Any]] = []
    for call in calls:
        sequence = int(call["ordinal"])
        phase = phase_by_sequence.get(sequence)
        if phase is None:
            raise TraceError(f"GAL sequence {sequence} has no phase evidence")
        correlated.append({**call, "phase": phase})
    return correlated


def _sha256_json(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=True, sort_keys=True, separators=(",", ":")
    ).encode("ascii")
    return hashlib.sha256(encoded).hexdigest()


def _require_mapping(value: Any, context: str) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise TraceError(f"{context} must be an object")
    return value


def normalize_run_directory(run_directory: Path) -> dict[str, Any]:
    envelope_path = run_directory / "envelope.json"
    stdout_path = run_directory / "stdout.txt"
    stderr_path = run_directory / "stderr.txt"
    envelope = _load_json(envelope_path)
    stdout = _read_bounded_text(stdout_path)
    stderr = _read_bounded_text(stderr_path)

    if envelope.get("experiment_id") != "stock-ctc-trace":
        raise TraceError(
            "normalizer accepts only the stock-ctc-trace experiment"
        )
    execution = _require_mapping(envelope.get("execution"), "execution")
    if execution.get("classification") != "success":
        raise TraceError(
            "experiment did not succeed: "
            f"{execution.get('classification')!r}"
        )
    semantic = _require_mapping(
        envelope.get("semantic_result"), "semantic_result"
    )
    health = _require_mapping(envelope.get("health"), "health")

    phases = _parse_stdout_phases(stdout)
    gal_text, strace_text, framed_channel = _extract_framed_sections(
        stdout, stderr
    )
    (
        gal_calls,
        gal_results,
        incomplete_gal,
        gal_event_order,
    ) = _parse_gal_section(gal_text)
    (
        syscall_counts,
        traced_phases,
        path_observations,
        ignored_events,
        ioctl_results,
        gal_phase_events,
        driver_events,
    ) = _parse_strace_section(strace_text)
    gal_calls = _correlate_gal_phases(
        gal_calls, gal_event_order, gal_phase_events
    )

    failed_gal = [
        result
        for result in gal_results
        if result["result"] != 0
        or result["errno"] != 0
        or result["status"] != 0
    ]
    if failed_gal:
        raise TraceError(
            f"{len(failed_gal)} GAL calls reported a nonzero result/status"
        )
    ioctl_count = syscall_counts.get("ioctl", 0)
    if ioctl_count != len(gal_calls):
        raise TraceError(
            f"strace/GAL ioctl count mismatch: {ioctl_count} != "
            f"{len(gal_calls)}"
        )
    if ioctl_results.get("success", 0) != ioctl_count:
        raise TraceError(
            "not every traced ioctl completed with syscall result zero: "
            f"{ioctl_results}"
        )
    if ioctl_results.get("failure", 0) or ioctl_results.get("unresolved", 0):
        raise TraceError(f"failed or unresolved ioctl result: {ioctl_results}")

    command_counts = Counter(
        str(call["command"]) for call in gal_calls
    )
    phase_command_counts: dict[str, dict[str, int]] = {}
    for phase in range(1, 9):
        counts = Counter(
            str(call["command"])
            for call in gal_calls
            if call["phase"] == phase
        )
        phase_command_counts[str(phase)] = dict(sorted(counts.items()))
    report: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "experiment_id": "stock-ctc-trace",
        "source": {
            "definition_sha256": envelope.get("definition_sha256"),
            "required_artifacts": envelope.get("required_artifacts"),
            "raw_artifacts": envelope.get("artifacts"),
        },
        "control_result": {
            key: semantic.get(key)
            for key in (
                "status",
                "iterations",
                "output_bytes",
                "output_sha256",
                "cleanup",
            )
        },
        "health": {
            key: health.get(key)
            for key in (
                "adb_responsive",
                "post_snapshot_complete",
                "npu_parameters_unchanged",
                "required_artifacts_unchanged",
                "galcore_present",
                "timeout_requires_manual_holder_review",
            )
        },
        "lifecycle": {
            "phases": phases,
            "traced_phase_order": traced_phases,
        },
        "paths": path_observations,
        "driver_events": driver_events,
        "syscall_counts": syscall_counts,
        "gal": {
            "request": GAL_REQUEST,
            "calls": gal_calls,
            "call_count": len(gal_calls),
            "command_counts": dict(sorted(command_counts.items())),
            "phase_command_counts": phase_command_counts,
            "sequence_sha256": _sha256_json(gal_calls),
            "completion": {
                "metadata_and_status_count": len(gal_results),
                "entry_only_process_exit_ordinals": incomplete_gal,
                "all_strace_ioctl_results_zero": True,
                "strace_ioctl_results": ioctl_results,
            },
            "adjacent_label_source": (
                "eCV5546 GAL 6.4.18.5 at "
                "946d2c841fd82b916bdd2168033906bfec89db61"
            ),
        },
        "ignored_strace_events": ignored_events,
        "trace_frame_channel": framed_channel,
    }

    serialized = json.dumps(report, ensure_ascii=True, sort_keys=True)
    pointer = POINTER_RE.search(serialized)
    if pointer is not None:
        raise TraceError(
            f"normalized report retained a pointer-shaped value: {pointer.group(0)}"
        )
    return report


def compare_reports(
    left: Mapping[str, Any], right: Mapping[str, Any]
) -> dict[str, Any]:
    checks = {
        "control_result_equal": left.get("control_result")
        == right.get("control_result"),
        "lifecycle_equal": left.get("lifecycle") == right.get("lifecycle"),
        "path_observations_equal": left.get("paths") == right.get("paths"),
        "driver_events_equal": left.get("driver_events")
        == right.get("driver_events"),
        "gal_sequence_equal": _require_mapping(left.get("gal"), "left.gal").get(
            "calls"
        )
        == _require_mapping(right.get("gal"), "right.gal").get("calls"),
        "gal_sequence_sha256_equal": _require_mapping(
            left.get("gal"), "left.gal"
        ).get("sequence_sha256")
        == _require_mapping(right.get("gal"), "right.gal").get(
            "sequence_sha256"
        ),
        "gal_completion_equal": _require_mapping(
            left.get("gal"), "left.gal"
        ).get("completion")
        == _require_mapping(right.get("gal"), "right.gal").get("completion"),
        "syscall_counts_equal": left.get("syscall_counts")
        == right.get("syscall_counts"),
        "health_equal": left.get("health") == right.get("health"),
    }
    required = (
        "control_result_equal",
        "lifecycle_equal",
        "path_observations_equal",
        "driver_events_equal",
        "gal_sequence_equal",
        "gal_sequence_sha256_equal",
        "health_equal",
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "equivalent": all(checks[key] for key in required),
        "checks": checks,
        "required_checks": list(required),
        "note": (
            "syscall count and final-thread GAL completion equality are "
            "reported but are not required repeatability gates because "
            "thread scheduling around process exit can vary them"
        ),
    }


def _write_json(path: Path | None, value: Mapping[str, Any]) -> None:
    rendered = (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    )
    if path is None:
        sys.stdout.write(rendered)
        return
    if path.exists():
        raise TraceError(f"refusing to overwrite output: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rendered, encoding="utf-8", newline="\n")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    normalize = subparsers.add_parser(
        "normalize", help="normalize one successful PoC 3 run directory"
    )
    normalize.add_argument("--run-directory", required=True, type=Path)
    normalize.add_argument("--output", type=Path)

    compare = subparsers.add_parser(
        "compare", help="compare two previously normalized reports"
    )
    compare.add_argument("--left", required=True, type=Path)
    compare.add_argument("--right", required=True, type=Path)
    compare.add_argument("--output", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        if arguments.command == "normalize":
            result = normalize_run_directory(arguments.run_directory)
        else:
            left = _load_json(arguments.left)
            right = _load_json(arguments.right)
            result = compare_reports(left, right)
        _write_json(arguments.output, result)
    except TraceError as exc:
        print(f"trace-normalize: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
