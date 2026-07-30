"""Deterministic procedural audio corpus for the Erf-mask PoC."""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import tarfile
import urllib.request
import wave
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any, Protocol

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

from .erf_mask_signal import HOP_SAMPLES, SAMPLE_RATE
from .errors import ErrorCode, HybridError

CLEAN_KINDS = ("voiced", "unvoiced")
NOISE_KINDS = (
    "white",
    "pink",
    "narrow-band",
    "hum",
    "impulsive",
    "amplitude-modulated",
)
SNR_VALUES_DB = (-10.0, -5.0, 0.0, 5.0, 10.0)
SLR31_SOURCE_PAGE = "https://openslr.org/31/"
SLR31_LICENSE = "CC BY 4.0"
SLR31_RESAMPLED_PEAK_LIMIT = 0.98
SLR31_ROOT_METADATA = frozenset(
    {
        "BOOKS.TXT",
        "CHAPTERS.TXT",
        "LICENSE.TXT",
        "README.TXT",
        "SPEAKERS.TXT",
    }
)


@dataclass(frozen=True, slots=True)
class Slr31ArchiveSpec:
    subset: str
    filename: str
    url: str
    md5: str
    max_download_bytes: int
    max_extracted_bytes: int


SLR31_ARCHIVES = {
    "train-clean-5": Slr31ArchiveSpec(
        subset="train-clean-5",
        filename="train-clean-5.tar.gz",
        url="https://www.openslr.org/resources/31/train-clean-5.tar.gz",
        md5="5df7d4e78065366204ca6845bb08f490",
        max_download_bytes=400 * 1024 * 1024,
        max_extracted_bytes=2 * 1024 * 1024 * 1024,
    ),
    "dev-clean-2": Slr31ArchiveSpec(
        subset="dev-clean-2",
        filename="dev-clean-2.tar.gz",
        url="https://www.openslr.org/resources/31/dev-clean-2.tar.gz",
        md5="6d7ab67ac6a1d2c993d050e16d61080d",
        max_download_bytes=160 * 1024 * 1024,
        max_extracted_bytes=1024 * 1024 * 1024,
    ),
}

_CONFIG_KEYS = {
    "schema_version",
    "sample_rate",
    "duration_samples",
    "train_seeds",
    "validation_seeds",
    "test_seeds",
    "clean_kinds",
    "noise_kinds",
    "snr_values_db",
}


def _invalid(message: str) -> HybridError:
    return HybridError(ErrorCode.EVIDENCE_INVALID, message)


def _strict_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise _invalid(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _strict_int(value: Any, *, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise _invalid(f"{name} must be an integer")
    return value


def _seed_tuple(value: Any, *, name: str) -> tuple[int, ...]:
    if not isinstance(value, list) or not value:
        raise _invalid(f"{name} must be a nonempty array")
    seeds = tuple(_strict_int(item, name=f"{name} item") for item in value)
    if any(seed < 0 or seed >= 2**63 for seed in seeds):
        raise _invalid(f"{name} contains an out-of-range seed")
    if len(set(seeds)) != len(seeds):
        raise _invalid(f"{name} contains a duplicate seed")
    return seeds


@dataclass(frozen=True, slots=True)
class ProceduralCorpusConfig:
    schema_version: int = 1
    sample_rate: int = SAMPLE_RATE
    duration_samples: int = SAMPLE_RATE * 4
    train_seeds: tuple[int, ...] = tuple(range(13_001, 13_009))
    validation_seeds: tuple[int, ...] = tuple(range(23_001, 23_005))
    test_seeds: tuple[int, ...] = tuple(range(33_001, 33_005))
    clean_kinds: tuple[str, ...] = CLEAN_KINDS
    noise_kinds: tuple[str, ...] = NOISE_KINDS
    snr_values_db: tuple[float, ...] = SNR_VALUES_DB

    def __post_init__(self) -> None:
        if self.schema_version != 1:
            raise _invalid("procedural corpus schema_version must be 1")
        if self.sample_rate != SAMPLE_RATE:
            raise _invalid(f"sample_rate must be {SAMPLE_RATE}")
        if (
            isinstance(self.duration_samples, bool)
            or not isinstance(self.duration_samples, int)
            or self.duration_samples <= 0
            or self.duration_samples % HOP_SAMPLES != 0
        ):
            raise _invalid(
                f"duration_samples must be a positive multiple of {HOP_SAMPLES}"
            )
        if self.clean_kinds != CLEAN_KINDS:
            raise _invalid("clean_kinds does not match the fixed contract")
        if self.noise_kinds != NOISE_KINDS:
            raise _invalid("noise_kinds does not match the fixed contract")
        if self.snr_values_db != SNR_VALUES_DB:
            raise _invalid("snr_values_db does not match the fixed contract")
        all_seeds = self.train_seeds + self.validation_seeds + self.test_seeds
        if (
            not self.train_seeds
            or not self.validation_seeds
            or not self.test_seeds
            or len(set(all_seeds)) != len(all_seeds)
        ):
            raise _invalid("train, validation, and test seeds must be disjoint")
        if any(
            isinstance(seed, bool)
            or not isinstance(seed, int)
            or seed < 0
            or seed >= 2**63
            for seed in all_seeds
        ):
            raise _invalid("split seeds must be nonnegative signed-64-bit integers")

    def to_document(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "sample_rate": self.sample_rate,
            "duration_samples": self.duration_samples,
            "train_seeds": list(self.train_seeds),
            "validation_seeds": list(self.validation_seeds),
            "test_seeds": list(self.test_seeds),
            "clean_kinds": list(self.clean_kinds),
            "noise_kinds": list(self.noise_kinds),
            "snr_values_db": list(self.snr_values_db),
        }


def load_procedural_config(path: Path | str) -> ProceduralCorpusConfig:
    try:
        document = json.loads(
            Path(path).read_text(encoding="utf-8"),
            object_pairs_hook=_strict_pairs,
        )
    except HybridError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise _invalid(f"cannot load procedural corpus config: {exc}") from exc
    if not isinstance(document, dict) or set(document) != _CONFIG_KEYS:
        observed = set(document) if isinstance(document, dict) else set()
        raise _invalid(
            "procedural corpus config keys differ: "
            f"missing={sorted(_CONFIG_KEYS - observed)} "
            f"unknown={sorted(observed - _CONFIG_KEYS)}"
        )
    if document["clean_kinds"] != list(CLEAN_KINDS):
        raise _invalid("clean_kinds does not match the fixed contract")
    if document["noise_kinds"] != list(NOISE_KINDS):
        raise _invalid("noise_kinds does not match the fixed contract")
    if document["snr_values_db"] != list(SNR_VALUES_DB):
        raise _invalid("snr_values_db does not match the fixed contract")
    return ProceduralCorpusConfig(
        schema_version=_strict_int(
            document["schema_version"],
            name="schema_version",
        ),
        sample_rate=_strict_int(document["sample_rate"], name="sample_rate"),
        duration_samples=_strict_int(
            document["duration_samples"],
            name="duration_samples",
        ),
        train_seeds=_seed_tuple(document["train_seeds"], name="train_seeds"),
        validation_seeds=_seed_tuple(
            document["validation_seeds"],
            name="validation_seeds",
        ),
        test_seeds=_seed_tuple(document["test_seeds"], name="test_seeds"),
    )


def _rng(seed: int) -> np.random.Generator:
    if isinstance(seed, bool) or not isinstance(seed, int) or seed < 0:
        raise _invalid("seed must be a nonnegative integer")
    return np.random.Generator(np.random.PCG64(seed))


def _derive_seed(*parts: object) -> int:
    identity = "|".join(str(part) for part in parts).encode()
    return int.from_bytes(hashlib.sha256(identity).digest()[:8], "big") >> 1


def _normalize_source(signal: np.ndarray) -> np.ndarray:
    values = np.asarray(signal, dtype=np.float64)
    values = values - np.mean(values)
    rms = float(np.sqrt(np.mean(np.square(values))))
    if not np.isfinite(rms) or rms <= 1e-12:
        raise _invalid("generated source has no finite energy")
    values *= 0.18 / rms
    peak = float(np.max(np.abs(values)))
    if peak > 0.8:
        values *= 0.8 / peak
    return np.ascontiguousarray(values, dtype=np.float32)


def generate_clean_source(kind: str, samples: int, *, seed: int) -> np.ndarray:
    if kind not in CLEAN_KINDS:
        raise _invalid(f"unknown clean source kind: {kind}")
    if isinstance(samples, bool) or not isinstance(samples, int) or samples <= 0:
        raise _invalid("clean source sample count must be positive")
    generator = _rng(seed)
    time = np.arange(samples, dtype=np.float64) / SAMPLE_RATE
    syllable_rate = generator.uniform(2.2, 4.8)
    envelope = 0.15 + 0.85 * np.square(
        np.sin(np.pi * syllable_rate * time + generator.uniform(0.0, np.pi))
    )

    if kind == "voiced":
        base = generator.uniform(90.0, 260.0)
        modulation_rate = generator.uniform(0.7, 2.0)
        instantaneous = base * (
            1.0
            + 0.08
            * np.sin(
                2.0 * np.pi * modulation_rate * time
                + generator.uniform(0.0, 2.0 * np.pi)
            )
        )
        phase = 2.0 * np.pi * np.cumsum(instantaneous) / SAMPLE_RATE
        signal = np.zeros(samples, dtype=np.float64)
        spectral_tilt = generator.uniform(0.85, 1.2)
        for harmonic in range(1, 13):
            signal += (
                np.sin(harmonic * phase + generator.uniform(0.0, 2.0 * np.pi))
                / harmonic**spectral_tilt
            )
    else:
        white = generator.standard_normal(samples)
        spectrum = np.fft.rfft(white)
        frequencies = np.fft.rfftfreq(samples, 1.0 / SAMPLE_RATE)
        lower = generator.uniform(500.0, 1_500.0)
        upper = generator.uniform(5_000.0, 9_000.0)
        taper = np.clip((frequencies - lower) / 400.0, 0.0, 1.0)
        taper *= np.clip((upper - frequencies) / 800.0, 0.0, 1.0)
        signal = np.fft.irfft(spectrum * taper, n=samples)
    return _normalize_source(signal * envelope)


def generate_noise_source(kind: str, samples: int, *, seed: int) -> np.ndarray:
    if kind not in NOISE_KINDS:
        raise _invalid(f"unknown noise source kind: {kind}")
    if isinstance(samples, bool) or not isinstance(samples, int) or samples <= 0:
        raise _invalid("noise source sample count must be positive")
    generator = _rng(seed)
    time = np.arange(samples, dtype=np.float64) / SAMPLE_RATE

    if kind == "white":
        signal = generator.standard_normal(samples)
    elif kind == "pink":
        spectrum = np.fft.rfft(generator.standard_normal(samples))
        frequencies = np.fft.rfftfreq(samples, 1.0 / SAMPLE_RATE)
        shaping = np.zeros_like(frequencies)
        shaping[1:] = 1.0 / np.sqrt(frequencies[1:])
        signal = np.fft.irfft(spectrum * shaping, n=samples)
    elif kind == "narrow-band":
        signal = np.zeros(samples, dtype=np.float64)
        for _ in range(4):
            frequency = generator.uniform(350.0, 7_500.0)
            signal += np.sin(
                2.0 * np.pi * frequency * time + generator.uniform(0.0, 2.0 * np.pi)
            )
        signal += 0.05 * generator.standard_normal(samples)
    elif kind == "hum":
        phase = generator.uniform(0.0, 2.0 * np.pi)
        signal = sum(
            np.sin(2.0 * np.pi * 50.0 * harmonic * time + phase * harmonic) / harmonic
            for harmonic in range(1, 6)
        )
        signal += 0.03 * generator.standard_normal(samples)
    elif kind == "impulsive":
        impulses = np.zeros(samples, dtype=np.float64)
        count = max(2, samples // 2_400)
        positions = generator.choice(samples, size=count, replace=False)
        impulses[positions] = generator.uniform(-1.0, 1.0, size=count)
        decay = np.exp(-np.arange(240, dtype=np.float64) / 35.0)
        signal = np.convolve(impulses, decay, mode="full")[:samples]
        signal += 0.02 * generator.standard_normal(samples)
    else:
        carrier = generator.standard_normal(samples)
        rate = generator.uniform(0.8, 5.0)
        modulation = 0.08 + 0.92 * np.square(
            np.sin(np.pi * rate * time + generator.uniform(0.0, 2.0 * np.pi))
        )
        signal = carrier * modulation
    return _normalize_source(np.asarray(signal, dtype=np.float64))


def _readonly_f32(values: np.ndarray) -> np.ndarray:
    result = np.ascontiguousarray(values, dtype=np.float32)
    result.flags.writeable = False
    return result


@dataclass(frozen=True, slots=True)
class MixtureComponents:
    clean: np.ndarray
    noise: np.ndarray
    mixture: np.ndarray

    def __post_init__(self) -> None:
        object.__setattr__(self, "clean", _readonly_f32(self.clean))
        object.__setattr__(self, "noise", _readonly_f32(self.noise))
        object.__setattr__(self, "mixture", _readonly_f32(self.mixture))


def mix_at_snr(
    clean: np.ndarray,
    noise: np.ndarray,
    snr_db: float,
) -> MixtureComponents:
    clean_values = np.asarray(clean, dtype=np.float64)
    noise_values = np.asarray(noise, dtype=np.float64)
    if (
        clean_values.ndim != 1
        or noise_values.shape != clean_values.shape
        or clean_values.size == 0
        or not np.isfinite(clean_values).all()
        or not np.isfinite(noise_values).all()
        or not np.isfinite(snr_db)
    ):
        raise _invalid("invalid clean/noise mixture inputs")
    clean_rms = float(np.sqrt(np.mean(np.square(clean_values))))
    noise_rms = float(np.sqrt(np.mean(np.square(noise_values))))
    if clean_rms <= 1e-12 or noise_rms <= 1e-12:
        raise _invalid("clean and noise inputs must both have energy")
    target_noise_rms = clean_rms / (10.0 ** (float(snr_db) / 20.0))
    scaled_noise = noise_values * (target_noise_rms / noise_rms)
    mixture = clean_values + scaled_noise
    peak = float(np.max(np.abs(mixture)))
    if peak > 0.98:
        gain = 0.98 / peak
        clean_values = clean_values * gain
        scaled_noise = scaled_noise * gain
        mixture = mixture * gain
    return MixtureComponents(clean_values, scaled_noise, mixture)


@dataclass(frozen=True, slots=True)
class CorpusItem:
    identity: str
    split: str
    category: str
    seed: int
    clean_kind: str | None
    noise_kind: str | None
    snr_db: float | None
    clean: np.ndarray = field(repr=False)
    noise: np.ndarray = field(repr=False)
    mixture: np.ndarray = field(repr=False)
    payload_sha256: str = field(init=False)

    def __post_init__(self) -> None:
        clean = _readonly_f32(self.clean)
        noise = _readonly_f32(self.noise)
        mixture = _readonly_f32(self.mixture)
        digest = hashlib.sha256()
        for values in (clean, noise, mixture):
            digest.update(values.astype("<f4", copy=False).tobytes())
        object.__setattr__(self, "clean", clean)
        object.__setattr__(self, "noise", noise)
        object.__setattr__(self, "mixture", mixture)
        object.__setattr__(self, "payload_sha256", digest.hexdigest())


def _item(
    *,
    identity: str,
    split: str,
    category: str,
    seed: int,
    clean_kind: str | None,
    noise_kind: str | None,
    snr_db: float | None,
    clean: np.ndarray,
    noise: np.ndarray,
    mixture: np.ndarray,
) -> CorpusItem:
    return CorpusItem(
        identity=identity,
        split=split,
        category=category,
        seed=seed,
        clean_kind=clean_kind,
        noise_kind=noise_kind,
        snr_db=snr_db,
        clean=clean,
        noise=noise,
        mixture=mixture,
    )


def generate_procedural_split(
    config: ProceduralCorpusConfig,
    split: str,
) -> list[CorpusItem]:
    seeds_by_split = {
        "train": config.train_seeds,
        "validation": config.validation_seeds,
        "test": config.test_seeds,
    }
    if split not in seeds_by_split:
        raise _invalid(f"unknown procedural split: {split}")
    items: list[CorpusItem] = []
    zeros = np.zeros(config.duration_samples, dtype=np.float32)

    for base_seed in seeds_by_split[split]:
        clean_sources = {
            kind: generate_clean_source(
                kind,
                config.duration_samples,
                seed=_derive_seed(split, base_seed, "clean", kind),
            )
            for kind in config.clean_kinds
        }
        noise_sources = {
            kind: generate_noise_source(
                kind,
                config.duration_samples,
                seed=_derive_seed(split, base_seed, "noise", kind),
            )
            for kind in config.noise_kinds
        }
        for clean_kind, clean in clean_sources.items():
            for noise_kind, noise in noise_sources.items():
                for snr_db in config.snr_values_db:
                    identity = (
                        f"{split}/{base_seed}/mixture/"
                        f"{clean_kind}/{noise_kind}/{snr_db:+.1f}"
                    )
                    components = mix_at_snr(clean, noise, snr_db)
                    items.append(
                        _item(
                            identity=identity,
                            split=split,
                            category="mixture",
                            seed=_derive_seed(identity),
                            clean_kind=clean_kind,
                            noise_kind=noise_kind,
                            snr_db=snr_db,
                            clean=components.clean,
                            noise=components.noise,
                            mixture=components.mixture,
                        )
                    )
        for clean_kind, clean in clean_sources.items():
            identity = f"{split}/{base_seed}/speech-only/{clean_kind}"
            items.append(
                _item(
                    identity=identity,
                    split=split,
                    category="speech-only",
                    seed=_derive_seed(identity),
                    clean_kind=clean_kind,
                    noise_kind=None,
                    snr_db=None,
                    clean=clean,
                    noise=zeros,
                    mixture=clean,
                )
            )
        for noise_kind, noise in noise_sources.items():
            identity = f"{split}/{base_seed}/noise-only/{noise_kind}"
            items.append(
                _item(
                    identity=identity,
                    split=split,
                    category="noise-only",
                    seed=_derive_seed(identity),
                    clean_kind=None,
                    noise_kind=noise_kind,
                    snr_db=None,
                    clean=zeros,
                    noise=noise,
                    mixture=noise,
                )
            )
        identity = f"{split}/{base_seed}/silence"
        items.append(
            _item(
                identity=identity,
                split=split,
                category="silence",
                seed=_derive_seed(identity),
                clean_kind=None,
                noise_kind=None,
                snr_db=None,
                clean=zeros,
                noise=zeros,
                mixture=zeros,
            )
        )
    return items


def deterministic_wav_bytes(samples: np.ndarray) -> bytes:
    values = np.asarray(samples)
    if (
        values.ndim != 1
        or values.size == 0
        or not np.issubdtype(values.dtype, np.floating)
        or not np.isfinite(values).all()
    ):
        raise _invalid("WAV samples must be a finite floating-point vector")
    pcm = np.rint(np.clip(values, -1.0, 1.0) * 32767.0).astype("<i2")
    output = io.BytesIO()
    with wave.open(output, "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(2)
        writer.setframerate(SAMPLE_RATE)
        writer.setnframes(pcm.size)
        writer.writeframes(pcm.tobytes())
    return output.getvalue()


class _Response(Protocol):
    def __enter__(self) -> _Response: ...

    def __exit__(self, *args: object) -> object: ...

    def read(self, size: int) -> bytes: ...


class _Opener(Protocol):
    def __call__(self, url: str, *, timeout: int) -> _Response: ...


def _validate_slr31_spec(spec: Slr31ArchiveSpec) -> None:
    expected_url = f"https://www.openslr.org/resources/31/{spec.filename}"
    if (
        spec.subset not in SLR31_ARCHIVES
        or spec.filename != f"{spec.subset}.tar.gz"
        or spec.url != expected_url
        or len(spec.md5) != 32
        or any(character not in "0123456789abcdef" for character in spec.md5)
        or spec.max_download_bytes <= 0
        or spec.max_extracted_bytes <= 0
    ):
        raise _invalid("SLR31 archive specification is outside the allowlist")


def _lexists(path: Path) -> bool:
    return os.path.lexists(path)


def _hash_file(path: Path) -> dict[str, Any]:
    md5_digest = hashlib.md5(usedforsecurity=False)
    sha256_digest = hashlib.sha256()
    total = 0
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                total += len(chunk)
                md5_digest.update(chunk)
                sha256_digest.update(chunk)
    except OSError as exc:
        raise _invalid(f"cannot hash archive {path}: {exc}") from exc
    return {
        "bytes": total,
        "md5": md5_digest.hexdigest(),
        "sha256": sha256_digest.hexdigest(),
    }


def download_slr31_archive(
    spec: Slr31ArchiveSpec,
    destination: Path | str,
    *,
    opener: _Opener = urllib.request.urlopen,
) -> dict[str, Any]:
    _validate_slr31_spec(spec)
    output = Path(destination)
    if output.name != spec.filename:
        raise _invalid("SLR31 download destination filename does not match")
    output.parent.mkdir(parents=True, exist_ok=True)
    partial = output.with_name(f".{output.name}.partial")
    if _lexists(output) or _lexists(partial):
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"SLR31 output or partial already exists: {output}",
        )

    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(partial, flags, 0o600)
    md5_digest = hashlib.md5(usedforsecurity=False)
    sha256_digest = hashlib.sha256()
    total = 0
    try:
        with os.fdopen(descriptor, "wb") as handle:
            with opener(spec.url, timeout=30) as response:
                while chunk := response.read(1024 * 1024):
                    total += len(chunk)
                    if total > spec.max_download_bytes:
                        raise _invalid(f"{spec.filename} exceeds download byte limit")
                    handle.write(chunk)
                    md5_digest.update(chunk)
                    sha256_digest.update(chunk)
            handle.flush()
            os.fsync(handle.fileno())
        if md5_digest.hexdigest() != spec.md5:
            raise _invalid(f"{spec.filename} publisher MD5 mismatch")
        os.replace(partial, output)
        output.chmod(0o444)
    except BaseException:
        partial.unlink(missing_ok=True)
        raise
    return {
        "bytes": total,
        "md5": md5_digest.hexdigest(),
        "sha256": sha256_digest.hexdigest(),
    }


def verify_slr31_archive(
    spec: Slr31ArchiveSpec,
    archive_path: Path | str,
) -> dict[str, Any]:
    _validate_slr31_spec(spec)
    path = Path(archive_path)
    if path.is_symlink() or not path.is_file():
        raise _invalid(f"SLR31 archive is not a regular file: {path}")
    evidence = _hash_file(path)
    if evidence["bytes"] > spec.max_download_bytes:
        raise _invalid(f"{spec.filename} exceeds download byte limit")
    if evidence["md5"] != spec.md5:
        raise _invalid(f"{spec.filename} publisher MD5 mismatch")
    return evidence


def _safe_member_path(member: tarfile.TarInfo, spec: Slr31ArchiveSpec) -> Path:
    if "\\" in member.name:
        raise _invalid(f"archive member contains backslash: {member.name}")
    pure = PurePosixPath(member.name)
    if (
        pure.is_absolute()
        or not pure.parts
        or any(part in {"", ".", ".."} for part in pure.parts)
    ):
        raise _invalid(f"unsafe archive member path: {member.name}")
    required = ("LibriSpeech", spec.subset)
    is_root_directory = pure.parts == ("LibriSpeech",) and member.isdir()
    is_subset_directory = pure.parts == required and member.isdir()
    is_root_metadata = (
        len(pure.parts) == 2
        and pure.parts[0] == "LibriSpeech"
        and pure.parts[1] in SLR31_ROOT_METADATA
        and member.isfile()
    )
    is_subset_member = pure.parts[:2] == required
    if not (
        is_root_directory or is_subset_directory or is_root_metadata or is_subset_member
    ):
        raise _invalid(f"archive member has unexpected subset root: {member.name}")
    if not (member.isdir() or member.isfile()):
        raise _invalid(f"archive member is not a regular file/directory: {member.name}")
    return Path(*pure.parts)


def extract_slr31_archive(
    spec: Slr31ArchiveSpec,
    archive_path: Path | str,
    destination: Path | str,
) -> dict[str, Any]:
    verify_slr31_archive(spec, archive_path)
    archive_file = Path(archive_path)
    output = Path(destination)
    partial = output.with_name(f".{output.name}.partial")
    if _lexists(output) or _lexists(partial):
        raise HybridError(
            ErrorCode.OUTPUT_EXISTS,
            f"SLR31 extraction output or partial already exists: {output}",
        )
    output.parent.mkdir(parents=True, exist_ok=True)

    try:
        with tarfile.open(archive_file, mode="r:gz") as archive:
            members = archive.getmembers()
            seen: set[Path] = set()
            validated: list[tuple[tarfile.TarInfo, Path]] = []
            total = 0
            for member in members:
                relative = _safe_member_path(member, spec)
                if relative in seen:
                    raise _invalid(f"duplicate archive member: {member.name}")
                seen.add(relative)
                total += member.size
                if total > spec.max_extracted_bytes:
                    raise _invalid(f"{spec.filename} exceeds extracted byte limit")
                validated.append((member, relative))

            partial.mkdir(mode=0o700)
            file_count = 0
            for member, relative in validated:
                target = partial / relative
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                source = archive.extractfile(member)
                if source is None:
                    raise _invalid(f"cannot read archive member: {member.name}")
                flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
                if hasattr(os, "O_NOFOLLOW"):
                    flags |= os.O_NOFOLLOW
                descriptor = os.open(target, flags, 0o444)
                written = 0
                with source, os.fdopen(descriptor, "wb") as handle:
                    while chunk := source.read(1024 * 1024):
                        written += len(chunk)
                        if written > member.size:
                            raise _invalid(
                                f"archive member exceeds declared size: {member.name}"
                            )
                        handle.write(chunk)
                    handle.flush()
                    os.fsync(handle.fileno())
                if written != member.size:
                    raise _invalid(f"archive member size mismatch: {member.name}")
                file_count += 1
        os.rename(partial, output)
    except BaseException:
        if partial.is_dir() and not partial.is_symlink():
            shutil.rmtree(partial)
        else:
            partial.unlink(missing_ok=True)
        raise
    return {
        "file_count": file_count,
        "extracted_bytes": total,
    }


@dataclass(frozen=True, slots=True)
class Slr31Item:
    split: str
    subset: str
    speaker_id: int
    chapter_id: int
    relative_path: str
    path: Path = field(repr=False, compare=False)
    source_samples: int
    sha256: str


def _scan_slr31_subset(root: Path, subset: str) -> list[Slr31Item]:
    subset_root = root / "LibriSpeech" / subset
    if subset_root.is_symlink() or not subset_root.is_dir():
        raise _invalid(f"missing regular SLR31 subset root: {subset_root}")
    items: list[Slr31Item] = []
    for path in sorted(subset_root.rglob("*.flac")):
        if path.is_symlink() or not path.is_file():
            raise _invalid(f"SLR31 item is not a regular file: {path}")
        relative = path.relative_to(subset_root)
        if len(relative.parts) != 3:
            raise _invalid(f"unexpected SLR31 item layout: {relative}")
        try:
            speaker = int(relative.parts[0])
            chapter = int(relative.parts[1])
        except ValueError as exc:
            raise _invalid(f"nonnumeric SLR31 speaker/chapter: {relative}") from exc
        try:
            info = sf.info(path)
        except (OSError, RuntimeError) as exc:
            raise _invalid(f"cannot inspect SLR31 FLAC {relative}: {exc}") from exc
        if info.samplerate != 16_000 or info.channels != 1:
            raise _invalid(f"unexpected SLR31 audio contract: {relative}")
        if not (2 * info.samplerate <= info.frames <= 20 * info.samplerate):
            continue
        items.append(
            Slr31Item(
                split="unassigned",
                subset=subset,
                speaker_id=speaker,
                chapter_id=chapter,
                relative_path=relative.as_posix(),
                path=path,
                source_samples=info.frames,
                sha256=_hash_file(path)["sha256"],
            )
        )
    if not items:
        raise _invalid(f"SLR31 subset has no qualifying FLAC files: {subset}")
    return items


def select_slr31_items(
    train_root: Path | str,
    dev_root: Path | str,
) -> dict[str, list[Slr31Item]]:
    train_items = _scan_slr31_subset(Path(train_root), "train-clean-5")
    dev_items = _scan_slr31_subset(Path(dev_root), "dev-clean-2")
    train_speakers = {item.speaker_id for item in train_items}
    dev_speakers = sorted({item.speaker_id for item in dev_items})
    if train_speakers & set(dev_speakers):
        raise _invalid("SLR31 train and dev speaker IDs overlap")
    boundary = max(1, len(dev_speakers) // 2)
    validation_speakers = set(dev_speakers[:boundary])
    test_speakers = set(dev_speakers[boundary:])

    def assign(item: Slr31Item, split: str) -> Slr31Item:
        return Slr31Item(
            split=split,
            subset=item.subset,
            speaker_id=item.speaker_id,
            chapter_id=item.chapter_id,
            relative_path=item.relative_path,
            path=item.path,
            source_samples=item.source_samples,
            sha256=item.sha256,
        )

    return {
        "train": [assign(item, "train") for item in train_items],
        "validation": [
            assign(item, "validation")
            for item in dev_items
            if item.speaker_id in validation_speakers
        ],
        "test": [
            assign(item, "test")
            for item in dev_items
            if item.speaker_id in test_speakers
        ],
    }


def load_and_resample_slr31_item(item: Slr31Item) -> np.ndarray:
    if item.path.is_symlink() or not item.path.is_file():
        raise _invalid(f"SLR31 item is not a regular file: {item.path}")
    if _hash_file(item.path)["sha256"] != item.sha256:
        raise _invalid(f"SLR31 item hash changed: {item.relative_path}")
    try:
        audio, sample_rate = sf.read(
            item.path,
            dtype="float64",
            always_2d=False,
        )
    except (OSError, RuntimeError) as exc:
        raise _invalid(f"cannot decode SLR31 item: {exc}") from exc
    if (
        sample_rate != 16_000
        or audio.ndim != 1
        or audio.shape != (item.source_samples,)
        or not np.isfinite(audio).all()
    ):
        raise _invalid(f"decoded SLR31 item contract mismatch: {item.relative_path}")
    resampled = resample_poly(
        audio,
        up=3,
        down=1,
        window=("kaiser", 5.0),
        padtype="constant",
    )
    if (
        resampled.shape != (item.source_samples * 3,)
        or not np.isfinite(resampled).all()
    ):
        raise _invalid(f"resampled SLR31 item contract mismatch: {item.relative_path}")
    peak = float(np.max(np.abs(resampled)))
    if peak > SLR31_RESAMPLED_PEAK_LIMIT:
        resampled *= SLR31_RESAMPLED_PEAK_LIMIT / peak
    return np.ascontiguousarray(resampled, dtype=np.float32)


def build_slr31_manifest(
    selection: dict[str, list[Slr31Item]],
    archive_evidence: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    if set(selection) != {"train", "validation", "test"}:
        raise _invalid("SLR31 selection keys differ")
    if set(archive_evidence) != set(SLR31_ARCHIVES):
        raise _invalid("SLR31 archive evidence keys differ")
    archives: dict[str, Any] = {}
    for subset, spec in sorted(SLR31_ARCHIVES.items()):
        evidence = archive_evidence[subset]
        if (
            set(evidence) != {"bytes", "md5", "sha256"}
            or evidence["md5"] != spec.md5
            or not isinstance(evidence["bytes"], int)
            or evidence["bytes"] <= 0
            or not isinstance(evidence["sha256"], str)
            or len(evidence["sha256"]) != 64
        ):
            raise _invalid(f"invalid SLR31 archive evidence: {subset}")
        archives[subset] = {
            "filename": spec.filename,
            "url": spec.url,
            **evidence,
        }
    selection_document = {
        split: [
            {
                "subset": item.subset,
                "speaker_id": item.speaker_id,
                "chapter_id": item.chapter_id,
                "relative_path": item.relative_path,
                "source_samples": item.source_samples,
                "sha256": item.sha256,
            }
            for item in items
        ]
        for split, items in sorted(selection.items())
    }
    return {
        "schema_version": 1,
        "source_page": SLR31_SOURCE_PAGE,
        "license": SLR31_LICENSE,
        "archives": archives,
        "selection": selection_document,
        "resampler": {
            "source_rate": 16_000,
            "target_rate": SAMPLE_RATE,
            "method": "scipy.signal.resample_poly",
            "up": 3,
            "down": 1,
            "window": ["kaiser", 5.0],
            "padtype": "constant",
            "post_resample_peak_policy": {
                "method": "whole-utterance-gain-if-needed",
                "limit": SLR31_RESAMPLED_PEAK_LIMIT,
            },
        },
    }
