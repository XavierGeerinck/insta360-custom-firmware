"""Persistent host-only continuity anchor; no device or recovery interface."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import uuid
from pathlib import Path

import npu_compile_lab as compile_lab

SYSTEM_ABSENT = ("/lib/libCLC.so", "/usr/lib/libCLC.so", "/usr/lib64/libCLC.so")
PHASE_ID_RE = re.compile(r"[a-z0-9][a-z0-9_-]{1,63}\Z")


class PhaseError(compile_lab.PreflightError):
    """The encompassing native phase is missing, invalid or persistently halted."""


def _bytes(value):
    return (
        json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + "\n"
    ).encode()


def _sha(raw):
    return hashlib.sha256(raw).hexdigest()


def _read(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        raise PhaseError("phase evidence is missing, linked or oversized")
    return path.read_bytes()


def _json(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise PhaseError("duplicate phase evidence key")
            result[key] = value
        return result

    def nonfinite(value):
        raise PhaseError("nonfinite phase evidence")

    try:
        return json.loads(raw, object_pairs_hook=pairs, parse_constant=nonfinite)
    except (ValueError, UnicodeError) as exc:
        raise PhaseError("invalid phase evidence JSON") from exc


def _save(path, record, exclusive=False):
    record["state_sha256"] = _sha(
        _bytes({k: v for k, v in record.items() if k != "state_sha256"})
    )
    if exclusive:
        with path.open("xb") as handle:
            handle.write(_bytes(record))
        return
    temporary = path.parent / (".native-phase-" + uuid.uuid4().hex)
    try:
        with temporary.open("xb") as handle:
            handle.write(_bytes(record))
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _unit(manifest):
    return {
        "serial": manifest.serial,
        "kernel": manifest.kernel,
        "npu_parameters": dict(manifest.npu_parameters),
    }


def _view(snapshot, manifest, system_hashes):
    if not isinstance(snapshot, dict):
        raise PhaseError("health snapshot is missing or invalid")
    if snapshot.get("errors") or snapshot.get("galcore_holders"):
        raise PhaseError("health snapshot errors or active galcore holders")
    unit = _unit(manifest)
    if (
        snapshot.get("serials") != [unit["serial"]]
        or snapshot.get("kernel") != unit["kernel"]
        or snapshot.get("npu_parameters") != unit["npu_parameters"]
    ):
        raise PhaseError("selected unit/kernel/NPU state changed")
    if not isinstance(
        snapshot.get("boot_id"), str
    ) or not compile_lab.BOOT_ID_RE.fullmatch(snapshot["boot_id"]):
        raise PhaseError("health boot identity missing or invalid")
    if re.search(r"\b199,\s*0\b", snapshot.get("galcore_device") or "") is None:
        raise PhaseError("health galcore identity differs")
    hashes = {p: snapshot.get("artifact_hashes", {}).get(p) for p in system_hashes}
    if hashes != system_hashes:
        raise PhaseError("SYSTEM baseline artifact identity changed")
    absent = {
        p: snapshot.get("absent_artifact_states", {}).get(p) for p in SYSTEM_ABSENT
    }
    if any(
        not isinstance(s, dict) or s.get("status") != "absent" for s in absent.values()
    ):
        raise PhaseError("SYSTEM compiler absence state differs")
    diagnostics = snapshot.get("dmesg_tail")
    if (
        not isinstance(diagnostics, list)
        or not diagnostics
        or not all(isinstance(s, str) for s in diagnostics)
    ):
        raise PhaseError("kernel diagnostic history missing or invalid")
    if any(
        re.search(r"\bwave-npu-onnx-(?:compile|execute)-v1\b", s)
        for s in snapshot.get("processes", [])
    ):
        raise PhaseError("a lab native process is still present")
    return copy.deepcopy(
        {
            **{
                k: snapshot.get(k)
                for k in (
                    "serials",
                    "kernel",
                    "boot_id",
                    "galcore_device",
                    "npu_parameters",
                    "processes",
                )
            },
            "errors": [],
            "galcore_holders": [],
            "artifact_hashes": hashes,
            "absent_artifact_states": absent,
            "dmesg_tail": diagnostics,
        }
    )


def seed_phase(
    path: Path,
    reviewed_snapshot_path: Path,
    expected_snapshot_sha256: str,
    phase_id: str,
    authorization_note: str,
    system_manifest,
    system_hashes: dict,
) -> dict:
    """Explicit reviewed seed only; never replace or resume an existing phase."""
    path, source = Path(path), Path(reviewed_snapshot_path)
    if path.name != "native-phase.json" or path.exists() or path.is_symlink():
        raise PhaseError("phase path must be new; automatic reseed is forbidden")
    if (
        not isinstance(phase_id, str)
        or not PHASE_ID_RE.fullmatch(phase_id)
        or not isinstance(authorization_note, str)
        or not authorization_note.strip()
    ):
        raise PhaseError("explicit phase ID and authorization note required")
    raw = _read(source)
    if (
        not re.fullmatch(r"[0-9a-f]{64}", expected_snapshot_sha256)
        or _sha(raw) != expected_snapshot_sha256
    ):
        raise PhaseError("reviewed seed snapshot SHA differs")
    document = _json(raw)
    if not isinstance(document, dict):
        raise PhaseError("reviewed snapshot must be an object")
    if "snapshot" in document:
        if (
            document.get("status") != "PASS"
            or document.get("health_differences")
            or document.get("kernel_alerts")
        ):
            raise PhaseError("reviewed snapshot has a failed encompassing interval")
        document = document["snapshot"]
    anchor = _view(document, system_manifest, system_hashes)
    seed_name = "native-phase.seed." + phase_id + ".json"
    with (path.parent / seed_name).open("xb") as handle:
        handle.write(raw)
    record = {
        "schema_version": 1,
        "phase_id": phase_id,
        "authorization_note": authorization_note,
        "seed_file": seed_name,
        "reviewed_snapshot_sha256": expected_snapshot_sha256,
        "unit": _unit(system_manifest),
        "system_hashes": dict(system_hashes),
        "state": "READY",
        "accepted_anchor": anchor,
        "accepted_anchor_sha256": _sha(_bytes(anchor)),
        "owner": None,
        "halt_reason": None,
        "events": [],
    }
    _save(path, record, exclusive=True)
    return record


class NativePhase:
    def __init__(self, path: Path, system_manifest, system_hashes: dict):
        self.path, self.manifest = Path(path), system_manifest
        self.system_hashes, self.owner = dict(system_hashes), uuid.uuid4().hex
        self.record, self.pending_cleanup = {}, None
        try:
            self.record = self._load()
            if self.record["state"] != "READY":
                raise PhaseError(
                    "phase is HALTED or an earlier native operation is incomplete"
                )
        except Exception as exc:
            self.halt(str(exc))
            raise PhaseError(str(exc)) from exc

    def _load(self):
        record = _json(_read(self.path))
        if (
            not isinstance(record, dict)
            or type(record.get("schema_version")) is not int
            or record.get("schema_version") != 1
        ):
            raise PhaseError("phase schema invalid")
        expected = {
            "schema_version",
            "phase_id",
            "authorization_note",
            "seed_file",
            "reviewed_snapshot_sha256",
            "unit",
            "system_hashes",
            "state",
            "accepted_anchor",
            "accepted_anchor_sha256",
            "owner",
            "halt_reason",
            "events",
            "state_sha256",
        }
        if (
            set(record) != expected
            or not isinstance(record["events"], list)
            or not all(isinstance(e, dict) for e in record["events"])
        ):
            raise PhaseError("phase receipt fields invalid")
        if record["state"] not in {"READY", "ACTIVE", "HALTED"} or (
            record["state"] != "ACTIVE" and record["owner"] is not None
        ):
            raise PhaseError("phase state/owner invalid")
        if record["state"] == "ACTIVE" and (
            not isinstance(record["owner"], str)
            or not re.fullmatch(r"[0-9a-f]{32}", record["owner"])
        ):
            raise PhaseError("active phase owner invalid")
        if record.get("state_sha256") != _sha(
            _bytes({k: v for k, v in record.items() if k != "state_sha256"})
        ):
            raise PhaseError("phase receipt identity changed")
        if (
            record.get("unit") != _unit(self.manifest)
            or record.get("system_hashes") != self.system_hashes
        ):
            raise PhaseError("phase selected-unit/SYSTEM binding differs")
        phase_id = record.get("phase_id")
        if (
            not isinstance(phase_id, str)
            or not PHASE_ID_RE.fullmatch(phase_id)
            or record.get("seed_file") != "native-phase.seed." + phase_id + ".json"
        ):
            raise PhaseError("phase seed path/ID invalid")
        if _sha(_read(self.path.parent / record["seed_file"])) != record.get(
            "reviewed_snapshot_sha256"
        ):
            raise PhaseError("reviewed phase seed changed")
        anchor = _view(record.get("accepted_anchor"), self.manifest, self.system_hashes)
        if _sha(_bytes(anchor)) != record.get("accepted_anchor_sha256"):
            raise PhaseError("accepted anchor identity changed")
        return record

    def halt(self, reason):
        self.record.update(state="HALTED", halt_reason=str(reason), owner=None)
        if not isinstance(self.record.get("events"), list):
            self.record["events"] = []
        self.record["events"].append({"action": "HALT", "reason": str(reason)})
        _save(self.path, self.record)

    def check(self, snapshot, label):
        try:
            current = self._load()
            if current["state"] not in {"READY", "ACTIVE"} or (
                current["state"] == "ACTIVE" and current.get("owner") != self.owner
            ):
                raise PhaseError(
                    "phase is halted or owned by another/incomplete operation"
                )
            self.record = current
            after = _view(snapshot, self.manifest, self.system_hashes)
            differences, alerts = compile_lab.compare_health(
                self.manifest, current["accepted_anchor"], after
            )
            if differences or alerts:
                raise PhaseError(
                    "encompassing continuity failed: " + "; ".join(differences + alerts)
                )
            self.record.update(state="ACTIVE", owner=self.owner)
            self.record["events"].append(
                {
                    "action": "CHECK",
                    "label": label,
                    "snapshot_sha256": _sha(_bytes(after)),
                }
            )
            _save(self.path, self.record)
        except Exception as exc:
            self.halt(str(exc))
            raise PhaseError(str(exc)) from exc

    def callback(self, error_type, label):
        def validate(snapshot):
            try:
                self.check(snapshot, label)
            except PhaseError as exc:
                raise error_type(str(exc)) from exc

        return validate

    def advance(self, cleanup):
        try:
            if (
                not isinstance(cleanup, dict)
                or cleanup.get("status") != "CLEAN"
                or cleanup.get("health_differences") != []
                or cleanup.get("kernel_alerts") != []
            ):
                raise PhaseError("only verified CLEAN cleanup may advance the anchor")
            self.check(cleanup.get("after"), "clean-cleanup")
            anchor = _view(cleanup["after"], self.manifest, self.system_hashes)
            self.record.update(
                state="READY",
                owner=None,
                accepted_anchor=anchor,
                accepted_anchor_sha256=_sha(_bytes(anchor)),
            )
            self.record["events"].append(
                {
                    "action": "ADVANCE",
                    "cleanup_sha256": _sha(_bytes(cleanup)),
                    "anchor_sha256": self.record["accepted_anchor_sha256"],
                }
            )
            _save(self.path, self.record)
        except Exception as exc:
            self.halt(str(exc))
            raise PhaseError(str(exc)) from exc
