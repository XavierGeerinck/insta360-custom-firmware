"""Stage and remove only the fixed, hash-bound PoC 16 compiler payloads."""

from __future__ import annotations

import hashlib
import json
import shlex
import subprocess
import uuid
from dataclasses import replace
from pathlib import Path

import npu_compile_lab as lab
import npu_lab


def local_files(manifest: lab.CompileManifest) -> dict[str, tuple[Path, str]]:
    root = manifest.policy.remote_root
    if manifest.policy.poc_id not in ("16A", "16B"):
        raise lab.PreflightError("staging accepts only fixed PoC 16 policies")
    return {
        manifest.remote_executable: (
            lab.REPOSITORY_ROOT / "artifacts/npu/build/poc16/probes" / Path(manifest.remote_executable).name,
            "0555",
        ),
        f"{root}/lib/libtim-vx.so": (
            lab.REPOSITORY_ROOT / "artifacts/npu/build/poc16/runtime/libtim-vx.so", "0444",
        ),
        f"{root}/sdk/include/CL/cl_viv_vx_ext.h": (
            lab.REPOSITORY_ROOT / "waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h", "0444",
        ),
    }


def verify_local_files(manifest: lab.CompileManifest) -> dict[str, tuple[Path, str]]:
    files = local_files(manifest)
    for remote, (local, _) in files.items():
        if not local.is_file() or local.is_symlink():
            raise lab.PreflightError(f"missing regular local payload: {local}")
        digest = hashlib.sha256(local.read_bytes()).hexdigest()
        if manifest.required_artifacts.get(remote) != digest:
            raise lab.PreflightError(f"local payload hash mismatch: {local}")
    return files


def _adb(manifest: lab.CompileManifest, *arguments: str) -> str:
    sentinel = None
    if len(arguments) == 2 and arguments[0] == "shell":
        sentinel = f"__WAVE_COMPILE_EXIT_{uuid.uuid4().hex}__"
        arguments = ("shell", arguments[1] + "\nwave_stage_rc=$?\n"
                     + f"printf '\\n{sentinel}:%s\\n' \"$wave_stage_rc\"\n")
    try:
        result = subprocess.run(
            ["adb", "-s", manifest.serial, *arguments], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=20,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise lab.PreflightError(f"staging transport failed: {error}") from error
    if result.returncode:
        raise lab.PreflightError(f"staging ADB error: {result.stderr.strip()[:1024]}")
    stdout = result.stdout.replace("\r", "")
    if sentinel is not None:
        try:
            stdout, remote_exit = lab.parse_remote_exit(stdout, sentinel)
            lab.verify_transport_status(result.returncode, remote_exit, "staging shell")
        except lab.ResultError as error:
            raise lab.PreflightError(str(error)) from error
    return stdout.strip()


def system_manifest(manifest: lab.CompileManifest) -> lab.CompileManifest:
    return replace(manifest, required_artifacts={
        path: digest for path, digest in manifest.required_artifacts.items()
        if not path.startswith("/tmp/")
    })


def align_cleanup_diagnostics(transport, previous: dict, current: dict) -> None:
    """Retain the original marker when a busy driver exceeds the short window."""
    _, aligned = lab._new_dmesg_lines(previous.get("dmesg_tail", []), current.get("dmesg_tail", []))
    if aligned or previous.get("dmesg_tail") == current.get("dmesg_tail"):
        return
    errors = current["errors"]
    text = lab._shell_value(
        transport, "dmesg 2>/dev/null | tail -n 16000",
        "expanded cleanup diagnostics", errors,
    )
    if text is not None:
        current["dmesg_tail"] = lab._bounded_lines(text, 16000)


def stage(manifest: lab.CompileManifest) -> dict:
    files = verify_local_files(manifest)
    transport = npu_lab.AdbTransport()
    baseline = system_manifest(manifest)
    before = lab.collect_snapshot(transport, baseline)
    lab.verify_preflight(baseline, manifest.candidates["candidate-1"], before)
    root = manifest.policy.remote_root
    quoted_root = shlex.quote(root)
    if _adb(manifest, "shell", f"if [ -e {quoted_root} ] || [ -L {quoted_root} ]; then printf PRESENT; else printf ABSENT; fi") != "ABSENT":
        raise lab.PreflightError(f"staged root already exists: {root}")
    directory = lab.REPOSITORY_ROOT / "artifacts/npu/build/poc16"
    record = directory / f"stage-{manifest.policy.poc_id.lower()}.json"
    lab._write_json_exclusive(record, {"manifest_sha256": manifest.sha256, "before": before})
    _adb(manifest, "shell", f"umask 022; mkdir -p {quoted_root}/bin {quoted_root}/lib {quoted_root}/sdk/include/CL {quoted_root}/output")
    for remote, (local, mode) in files.items():
        incoming = remote + ".incoming"
        _adb(manifest, "push", str(local), incoming)
        q_incoming, q_remote = shlex.quote(incoming), shlex.quote(remote)
        observed = _adb(manifest, "shell", f"sha256sum {q_incoming}").split()[0]
        if observed != manifest.required_artifacts[remote]:
            raise lab.PreflightError(f"pushed payload hash mismatch: {remote}")
        _adb(manifest, "shell", f"chmod {mode} {q_incoming} && ln {q_incoming} {q_remote} && rm {q_incoming}")
        metadata = _adb(manifest, "shell", f"test ! -L {q_remote} && ls -ldn {q_remote}").split()[0]
        if metadata != ("-r-xr-xr-x" if mode == "0555" else "-r--r--r--"):
            raise lab.PreflightError(f"staged payload mode mismatch: {remote}")
    after = lab.collect_snapshot(transport, manifest)
    lab.verify_preflight(manifest, manifest.candidates["candidate-1"], after)
    differences, alerts = lab.compare_health(
        baseline, before, lab.collect_snapshot(transport, baseline),
    )
    if differences or alerts:
        raise lab.PreflightError(f"staging changed device health: {differences}")
    return {"status": "STAGED", "root": root, "manifest_sha256": manifest.sha256}


def cleanup(manifest: lab.CompileManifest, run_id: str) -> dict:
    if lab.RUN_ID_RE.fullmatch(run_id) is None:
        raise lab.PreflightError("invalid run ID")
    run_directory = lab.DEFAULT_RUN_ROOT / run_id
    envelope = json.loads((run_directory / "run.json").read_text())
    if envelope.get("manifest_sha256") != manifest.sha256:
        raise lab.PreflightError("cleanup run does not match manifest")
    if envelope.get("health_differences") or envelope.get("transport", {}).get("timed_out"):
        raise lab.PreflightError("cleanup refused after an uncertain or unhealthy run")
    if envelope.get("transport", {}).get("remote_exit") not in (0, 5):
        raise lab.PreflightError("cleanup requires a normal completion or clean size-query rejection")
    postflight = json.loads((run_directory / "postflight.json").read_text())
    transport = npu_lab.AdbTransport()
    before = lab.collect_snapshot(transport, manifest)
    align_cleanup_diagnostics(transport, postflight, before)
    differences, alerts = lab.compare_health(manifest, postflight, before)
    if differences or alerts:
        raise lab.PreflightError(f"device changed since compile: {differences}")
    files = {remote: manifest.required_artifacts[remote] for remote in local_files(manifest)}
    for remote in (manifest.candidates["candidate-1"].output, manifest.candidates["candidate-1"].partial_output):
        captured = postflight["output_states"][remote]
        if captured["status"] == "present":
            files[remote] = captured["sha256"]
        elif captured["status"] != "absent":
            raise lab.PreflightError("cleanup output state is unknown")
        elif before["output_states"][remote]["status"] != "absent":
            raise lab.PreflightError("unexpected output appeared after compile")
    root = manifest.policy.remote_root
    observed_files = _adb(manifest, "shell", f"find {shlex.quote(root)} \\( -type f -o -type l \\) | sort").splitlines()
    if set(observed_files) != set(files):
        raise lab.PreflightError("staged root contains unexpected files")
    for remote, digest in files.items():
        quoted = shlex.quote(remote)
        observed = _adb(manifest, "shell", f"test ! -L {quoted} && sha256sum {quoted}").split()[0]
        if observed != digest:
            raise lab.PreflightError(f"cleanup artifact changed: {remote}")
    baseline = system_manifest(manifest)
    system_before = lab.collect_snapshot(transport, baseline)
    _adb(manifest, "shell", "rm " + " ".join(shlex.quote(remote) for remote in files))
    directories = [f"{root}/sdk/include/CL", f"{root}/sdk/include", f"{root}/sdk", f"{root}/bin", f"{root}/lib", f"{root}/output", root]
    _adb(manifest, "shell", "rmdir " + " ".join(shlex.quote(path) for path in directories))
    after = lab.collect_snapshot(transport, baseline)
    differences, alerts = lab.compare_health(baseline, system_before, after)
    lab.verify_preflight(baseline, manifest.candidates["candidate-1"], after)
    result = {"status": "CLEAN" if not differences and not alerts else "FAIL", "health_differences": differences, "after": after}
    lab._write_json_exclusive(run_directory / "cleanup.json", result)
    if differences or alerts:
        raise lab.PreflightError("cleanup health gate failed")
    return {"status": "CLEAN", "root": root}
