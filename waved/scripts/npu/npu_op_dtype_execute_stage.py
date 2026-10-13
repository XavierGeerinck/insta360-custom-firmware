"""Stage and clean only the two qualified PoC 16 execution payloads."""

from __future__ import annotations

import hashlib
import json
import re
import shlex
from dataclasses import replace
from pathlib import Path
from typing import Any

import npu_compile_lab as compile_lab
import npu_erf_execution_lab as execution
import npu_lab
import npu_op_dtype_stage as compiler_stage
from npu_op_dtype_stage import _adb


MODELS = {
    "fp16-conv": {
        "root": "/tmp/wave-npu-exec16a",
        "poc_id": "16C",
        "nbg": "artifacts/npu/runs/rea-poc16a-fp16-conv-20261013-a1/candidate-1.nbg",
        "sha256": "4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d",
        "bytes": 1800,
    },
    "u8-fc": {
        "root": "/tmp/wave-npu-exec16b",
        "poc_id": "16D",
        "nbg": "artifacts/npu/runs/rea-poc16b-u8-fc-20261013-a1/candidate-1.nbg",
        "sha256": "00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e",
        "bytes": 2056,
    },
}
WRAPPER_SHA256 = "c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc"
SYSTEM_HASHES = {
    path: digest
    for path, digest in json.loads(execution.POC7_POLICY.manifest_path.read_text())[
        "required_artifacts"
    ].items()
    if not path.startswith("/tmp/")
}


def _fixed(manifest: execution.ExecutionManifest, model: str) -> dict[str, Any]:
    if model not in MODELS:
        raise execution.ExecutionPreflightError("unknown fixed execution model")
    fixed = MODELS[model]
    root = fixed["root"]
    executable = f"{root}/bin/wave-npu-execute-{model}-v1"
    wrapper = f"{root}/lib/libtim-vx.so"
    nbg = f"{root}/model/wave-{model}.nbg"
    modes = {executable: "-r-xr-xr-x", wrapper: "-r--r--r--", nbg: "-r--r--r--"}
    absent = {"/lib/libCLC.so", "/usr/lib/libCLC.so", "/usr/lib64/libCLC.so",
              f"{root}/lib/libCLC.so"}
    executable_sha = manifest.required_artifacts.get(executable, "")
    expected_hashes = {
        **SYSTEM_HASHES,
        executable: executable_sha,
        wrapper: WRAPPER_SHA256,
        nbg: fixed["sha256"],
    }
    environment = {
        "LD_LIBRARY_PATH": f"{root}/lib:/usr/lib",
        "VIVANTE_SDK_DIR": "/usr",
        "VSI_NN_LOG_LEVEL": "5",
    }
    if (
        manifest.policy.poc_id != fixed["poc_id"]
        or manifest.policy.remote_root != root
        or manifest.remote_executable != executable
        or manifest.policy.remote_executable != executable
        or manifest.policy.remote_tim_vx != wrapper
        or manifest.policy.remote_nbg != nbg
        or manifest.policy.nbg_bytes != fixed["bytes"]
        or manifest.remote_argument != "--execute-once"
        or manifest.serial != "PSP_0000000000000000"
        or manifest.kernel != execution.EXPECTED_KERNEL
        or dict(manifest.npu_parameters) != execution.EXPECTED_PARAMETERS
        or dict(manifest.environment) != environment
        or dict(manifest.policy.staged_modes) != modes
        or set(manifest.absent_artifacts) != absent
        or set(manifest.policy.absent_artifacts) != absent
        or not execution.SHA256_RE.fullmatch(executable_sha)
        or dict(manifest.required_artifacts) != expected_hashes
        or dict(manifest.policy.fixed_artifacts) != expected_hashes
        or not execution.SHA256_RE.fullmatch(manifest.sha256)
        or manifest.candidates
    ):
        raise execution.ExecutionPreflightError("manifest differs from fixed execution staging policy")
    return fixed


def local_files(
    manifest: execution.ExecutionManifest, model: str
) -> dict[str, tuple[Path, str]]:
    fixed = _fixed(manifest, model)
    root = compile_lab.REPOSITORY_ROOT
    return {
        manifest.remote_executable: (
            root / "artifacts/npu/build/poc16/executors" / Path(manifest.remote_executable).name,
            "0555",
        ),
        manifest.policy.remote_tim_vx: (
            root / "artifacts/npu/build/poc16/runtime/libtim-vx.so", "0444",
        ),
        manifest.policy.remote_nbg: (root / fixed["nbg"], "0444"),
    }


def verify_local_files(
    manifest: execution.ExecutionManifest, model: str
) -> dict[str, tuple[Path, str]]:
    files = local_files(manifest, model)
    for remote, (local, _) in files.items():
        if not local.is_file() or local.is_symlink():
            raise execution.ExecutionPreflightError(f"missing regular local payload: {local}")
        if hashlib.sha256(local.read_bytes()).hexdigest() != manifest.required_artifacts[remote]:
            raise execution.ExecutionPreflightError(f"local payload hash mismatch: {local}")
    if files[manifest.policy.remote_nbg][0].stat().st_size != manifest.policy.nbg_bytes:
        raise execution.ExecutionPreflightError("qualified local NBG size changed")
    return files


def system_manifest(manifest: execution.ExecutionManifest) -> execution.ExecutionManifest:
    """Keep all system/CLC gates while requiring both fixed model paths absent."""
    nbg = manifest.policy.remote_nbg
    candidate = compile_lab.Candidate("absence", "--execute-once", nbg, nbg + ".partial")
    return replace(manifest, required_artifacts=dict(SYSTEM_HASHES), candidates={"absence": candidate})


def _system_snapshot(transport: Any, baseline: execution.ExecutionManifest) -> dict:
    snapshot = compile_lab.collect_snapshot(transport, baseline)
    compile_lab.verify_preflight(baseline, baseline.candidates["absence"], snapshot)
    return snapshot


def _service_processes(snapshot: dict) -> list[str]:
    return sorted(line for line in snapshot.get("processes", [])
                  if re.search(r"\bwaved\b|S60waved|audioserver|pspapp", line))


def _health(manifest: execution.ExecutionManifest, before: dict, after: dict) -> tuple[list, list]:
    differences, alerts = compile_lab.compare_health(manifest, before, after)
    if _service_processes(before) != _service_processes(after):
        differences.append("service process identities changed")
    return differences, alerts


def _root_absent(manifest: execution.ExecutionManifest) -> bool:
    root = shlex.quote(manifest.policy.remote_root)
    return _adb(manifest, "shell", f"if [ -e {root} ] || [ -L {root} ]; then printf PRESENT; else printf ABSENT; fi") == "ABSENT"


def _executor_absent(manifest: execution.ExecutionManifest) -> None:
    # The shared legacy process filter does not list the two new executor names.
    pattern = "[w]ave-npu-execute-(fp16-conv|u8-fc)-v1"
    observed = _adb(manifest, "shell", f"ps 2>/dev/null | grep -E {shlex.quote(pattern)} || true")
    if observed:
        raise execution.ExecutionPreflightError("a fixed PoC 16 executor is already present")


def _inventory(manifest: execution.ExecutionManifest) -> dict[str, str]:
    root = shlex.quote(manifest.policy.remote_root)
    command = (
        f"find {root} -print | sort | while IFS= read -r wave_path; do "
        "metadata=$(ls -ldn \"$wave_path\") || exit 23; "
        "permissions=${metadata%% *}; "
        "if [ -L \"$wave_path\" ]; then kind=L; "
        "elif [ -d \"$wave_path\" ]; then kind=D; "
        "elif [ -f \"$wave_path\" ]; then kind=F; else kind=X; fi; "
        "printf '%s %s %s\\n' \"$kind\" \"$permissions\" \"$wave_path\"; done"
    )
    observed: dict[str, str] = {}
    for line in _adb(manifest, "shell", command).splitlines():
        match = re.fullmatch(r"([DFLX]) ([^\s]+) (.+)", line)
        if match is None or match[3] in observed:
            raise execution.ExecutionPreflightError("malformed or duplicate staged inventory")
        observed[match[3]] = f"{match[1]} {match[2]}"
    return observed


def _verify_inventory(manifest: execution.ExecutionManifest) -> dict[str, str]:
    root = manifest.policy.remote_root
    expected = {path: "D drwxr-xr-x" for path in (root, f"{root}/bin", f"{root}/lib", f"{root}/model")}
    expected.update({path: f"F {mode}" for path, mode in manifest.policy.staged_modes.items()})
    observed = _inventory(manifest)
    if observed != expected:
        raise execution.ExecutionPreflightError("staged root contains unexpected files, directories, modes or links")
    return observed


def stage(manifest: execution.ExecutionManifest, model: str) -> dict:
    files = verify_local_files(manifest, model)
    baseline = system_manifest(manifest)
    transport = npu_lab.AdbTransport()
    before = _system_snapshot(transport, baseline)
    _executor_absent(manifest)
    if not _root_absent(manifest):
        raise execution.ExecutionPreflightError("execution staging root already exists")
    directory = compile_lab.REPOSITORY_ROOT / "artifacts/npu/build/poc16"
    directory.mkdir(parents=True, exist_ok=True)
    record = directory / f"stage-exec-{model}.json"
    result_record = directory / f"stage-exec-{model}.result.json"
    if result_record.exists() or result_record.is_symlink():
        raise execution.ExecutionPreflightError("execution stage result evidence already exists")
    compile_lab._write_json_exclusive(record, {
        "status": "BEFORE_STAGE", "model": model, "manifest_sha256": manifest.sha256,
        "before": before, "payloads": {remote: {"local": str(local), "mode": mode,
            "sha256": manifest.required_artifacts[remote]} for remote, (local, mode) in files.items()},
    })
    root = shlex.quote(manifest.policy.remote_root)
    _adb(manifest, "shell", f"umask 022; mkdir {root} && mkdir {root}/bin {root}/lib {root}/model")
    for remote, (local, mode) in files.items():
        incoming = remote + ".incoming"
        _adb(manifest, "push", str(local), incoming)
        q_incoming, q_remote = shlex.quote(incoming), shlex.quote(remote)
        observed = _adb(manifest, "shell", f"test ! -L {q_incoming} && test -f {q_incoming} && sha256sum {q_incoming}").split()
        if not observed or observed[0] != manifest.required_artifacts[remote]:
            raise execution.ExecutionPreflightError(f"pushed payload hash mismatch: {remote}")
        _adb(manifest, "shell", f"chmod {mode} {q_incoming} && ln {q_incoming} {q_remote} && rm {q_incoming}")
    inventory = _verify_inventory(manifest)
    after = execution.collect_snapshot(transport, manifest)
    execution.verify_preflight(manifest, after)
    _executor_absent(manifest)
    system_after = compile_lab.collect_snapshot(transport, baseline)
    differences, alerts = _health(baseline, before, system_after)
    # The NBG is now present; only its absence candidate is omitted from the
    # already-verified system observation. System hashes and CLC gates remain.
    if differences or alerts:
        raise execution.ExecutionPreflightError(f"staging changed device health: {differences}; {alerts}")
    result = {"status": "STAGED", "model": model, "root": manifest.policy.remote_root,
              "manifest_sha256": manifest.sha256, "inventory": inventory,
              "after": after, "system_after": system_after,
              "health_differences": differences, "kernel_alerts": alerts}
    compile_lab._write_json_exclusive(result_record, result)
    return {"status": "STAGED", "root": manifest.policy.remote_root,
            "manifest_sha256": manifest.sha256, "record": str(result_record)}


def _read_regular_json(path: Path) -> dict:
    if path.is_symlink() or not path.is_file():
        raise execution.ExecutionPreflightError(f"missing regular evidence file: {path}")
    try:
        value = json.loads(path.read_text(), object_pairs_hook=execution._without_duplicate_keys)
    except (OSError, ValueError, execution.ExecutionManifestError) as error:
        raise execution.ExecutionPreflightError(f"invalid evidence file: {path}") from error
    if not isinstance(value, dict):
        raise execution.ExecutionPreflightError("cleanup evidence must be an object")
    return value


def cleanup(manifest: execution.ExecutionManifest, model: str, run_id: str) -> dict:
    _fixed(manifest, model)
    if execution.RUN_ID_RE.fullmatch(run_id) is None:
        raise execution.ExecutionPreflightError("invalid execution cleanup run ID")
    run_directory = execution.DEFAULT_RUN_ROOT / run_id
    cleanup_record = run_directory / "cleanup.json"
    if cleanup_record.exists() or cleanup_record.is_symlink():
        raise execution.ExecutionPreflightError("cleanup evidence already exists")
    envelope = _read_regular_json(run_directory / "run.json")
    transport_result = envelope.get("transport", {})
    if not isinstance(transport_result, dict):
        raise execution.ExecutionPreflightError("cleanup transport evidence must be an object")
    if (
        envelope.get("manifest_sha256") != manifest.sha256
        or envelope.get("experiment") != manifest.policy.experiment
        or envelope.get("run_id") != run_id
        or envelope.get("status") != "PASS"
        or type(envelope.get("execution_attempts")) is not int
        or envelope.get("execution_attempts") != 1
        or envelope.get("errors") != []
        or envelope.get("health_differences") != []
        or envelope.get("kernel_alerts") != []
        or transport_result.get("timed_out") is not False
        or transport_result.get("remote_exit") != 0
        or transport_result.get("adb_returncode") != 0
    ):
        raise execution.ExecutionPreflightError("cleanup requires one manifest-bound, healthy PASS execution")
    result = envelope.get("result") or {}
    if (not isinstance(result, dict)
        or result.get("iterations") != 1
        or result.get("bytes") != len(manifest.policy.output_values)
        or result.get("output") != list(manifest.policy.output_values)):
        raise execution.ExecutionPreflightError("cleanup execution result does not match the fixed oracle")
    postflight = _read_regular_json(run_directory / "postflight.json")
    execution.verify_preflight(manifest, postflight)
    transport = npu_lab.AdbTransport()
    before = execution.collect_snapshot(transport, manifest)
    execution.verify_preflight(manifest, before)
    original_diagnostics = list(before.get("dmesg_tail", []))
    compiler_stage.align_cleanup_diagnostics(transport, postflight, before)
    if before.get("dmesg_tail") != original_diagnostics:
        compile_lab._write_json_exclusive(run_directory / "cleanup-before.json", before)
    differences, alerts = execution.compare_health(manifest, postflight, before)
    if _service_processes(postflight) != _service_processes(before):
        differences.append("service process identities changed since execution")
    if differences or alerts:
        raise execution.ExecutionPreflightError(f"device changed since execution: {differences}; {alerts}")
    _executor_absent(manifest)
    inventory = _verify_inventory(manifest)
    files = tuple(manifest.policy.staged_modes)
    for remote in files:
        quoted = shlex.quote(remote)
        observed = _adb(manifest, "shell", f"test ! -L {quoted} && test -f {quoted} && sha256sum {quoted}").split()
        if not observed or observed[0] != manifest.required_artifacts[remote]:
            raise execution.ExecutionPreflightError(f"cleanup payload hash changed: {remote}")
    baseline = system_manifest(manifest)
    system_before = compile_lab.collect_snapshot(transport, baseline)
    # Full preflight above has already validated these system facts; retain
    # exactly the same hash set before removing only the three fixed files.
    current_system = {**before, "artifact_hashes": {
        path: before["artifact_hashes"][path] for path in SYSTEM_HASHES
    }}
    compiler_stage.align_cleanup_diagnostics(transport, current_system, system_before)
    differences, alerts = _health(baseline, current_system, system_before)
    if differences or alerts:
        raise execution.ExecutionPreflightError(f"cleanup system gate changed: {differences}; {alerts}")
    _adb(manifest, "shell", "rm " + " ".join(shlex.quote(path) for path in files))
    root = manifest.policy.remote_root
    directories = [f"{root}/bin", f"{root}/lib", f"{root}/model", root]
    _adb(manifest, "shell", "rmdir " + " ".join(shlex.quote(path) for path in directories))
    after = _system_snapshot(transport, baseline)
    if not _root_absent(manifest):
        raise execution.ExecutionPreflightError("execution root remains after cleanup")
    _executor_absent(manifest)
    compiler_stage.align_cleanup_diagnostics(transport, system_before, after)
    differences, alerts = _health(baseline, system_before, after)
    final = {"status": "CLEAN" if not differences and not alerts else "FAIL",
             "model": model, "root": root, "manifest_sha256": manifest.sha256,
             "run_id": run_id, "inventory_before": inventory, "before": before,
             "system_before": system_before, "after": after,
             "health_differences": differences, "kernel_alerts": alerts}
    compile_lab._write_json_exclusive(cleanup_record, final)
    if differences or alerts:
        raise execution.ExecutionPreflightError("cleanup health gate failed")
    return {"status": "CLEAN", "root": root, "record": str(cleanup_record)}
