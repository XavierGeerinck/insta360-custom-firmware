from __future__ import annotations

import json
import tarfile
from dataclasses import replace
from hashlib import md5
from io import BytesIO
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

from wave_onnx_hybrid.erf_mask_corpus import (
    CLEAN_KINDS,
    NOISE_KINDS,
    SLR31_ARCHIVES,
    SLR31_LICENSE,
    SLR31_RESAMPLED_PEAK_LIMIT,
    SLR31_SOURCE_PAGE,
    SNR_VALUES_DB,
    ProceduralCorpusConfig,
    Slr31ArchiveSpec,
    build_slr31_manifest,
    deterministic_wav_bytes,
    download_slr31_archive,
    extract_slr31_archive,
    generate_clean_source,
    generate_noise_source,
    generate_procedural_split,
    load_and_resample_slr31_item,
    load_procedural_config,
    mix_at_snr,
    select_slr31_items,
    verify_slr31_archive,
)
from wave_onnx_hybrid.erf_mask_signal import SAMPLE_RATE
from wave_onnx_hybrid.errors import ErrorCode, HybridError


def _small_config() -> ProceduralCorpusConfig:
    return ProceduralCorpusConfig(
        duration_samples=4_800,
        train_seeds=(13001,),
        validation_seeds=(23001,),
        test_seeds=(33001,),
    )


def test_default_corpus_contract_is_exact_and_splits_are_disjoint() -> None:
    config = ProceduralCorpusConfig()

    assert config.sample_rate == SAMPLE_RATE
    assert config.duration_samples == SAMPLE_RATE * 4
    assert config.snr_values_db == SNR_VALUES_DB
    assert config.clean_kinds == CLEAN_KINDS
    assert config.noise_kinds == NOISE_KINDS
    all_seeds = config.train_seeds + config.validation_seeds + config.test_seeds
    assert len(set(all_seeds)) == len(all_seeds)


def test_config_rejects_overlapping_split_seed() -> None:
    with pytest.raises(HybridError) as raised:
        ProceduralCorpusConfig(
            train_seeds=(1, 2),
            validation_seeds=(3,),
            test_seeds=(2, 4),
        )
    assert raised.value.code is ErrorCode.EVIDENCE_INVALID


def test_strict_config_loader_rejects_duplicate_unknown_and_missing_keys(
    tmp_path,
) -> None:
    config = _small_config().to_document()
    path = tmp_path / "config.json"

    path.write_text(
        '{"schema_version":1,"schema_version":1}',
        encoding="utf-8",
    )
    with pytest.raises(HybridError) as duplicate:
        load_procedural_config(path)
    assert duplicate.value.code is ErrorCode.EVIDENCE_INVALID

    unknown = dict(config)
    unknown["surprise"] = True
    path.write_text(json.dumps(unknown), encoding="utf-8")
    with pytest.raises(HybridError) as extra:
        load_procedural_config(path)
    assert extra.value.code is ErrorCode.EVIDENCE_INVALID

    missing = dict(config)
    missing.pop("sample_rate")
    path.write_text(json.dumps(missing), encoding="utf-8")
    with pytest.raises(HybridError) as absent:
        load_procedural_config(path)
    assert absent.value.code is ErrorCode.EVIDENCE_INVALID


@pytest.mark.parametrize("kind", CLEAN_KINDS)
def test_clean_sources_are_deterministic_finite_and_bounded(kind: str) -> None:
    left = generate_clean_source(kind, 9_600, seed=0x1301)
    np.random.seed(42)
    right = generate_clean_source(kind, 9_600, seed=0x1301)

    np.testing.assert_array_equal(left, right)
    assert left.shape == (9_600,)
    assert left.dtype == np.float32
    assert np.isfinite(left).all()
    assert np.max(np.abs(left)) <= 0.8
    assert np.sqrt(np.mean(np.square(left, dtype=np.float64))) > 0.01


@pytest.mark.parametrize("kind", NOISE_KINDS)
def test_noise_sources_are_deterministic_finite_and_bounded(kind: str) -> None:
    left = generate_noise_source(kind, 9_600, seed=0x1302)
    np.random.seed(13)
    right = generate_noise_source(kind, 9_600, seed=0x1302)

    np.testing.assert_array_equal(left, right)
    assert left.shape == (9_600,)
    assert left.dtype == np.float32
    assert np.isfinite(left).all()
    assert np.max(np.abs(left)) <= 0.8
    assert np.sqrt(np.mean(np.square(left, dtype=np.float64))) > 0.01


@pytest.mark.parametrize("snr_db", SNR_VALUES_DB)
def test_mix_has_requested_snr_without_clipping(snr_db: float) -> None:
    clean = generate_clean_source("voiced", 48_000, seed=0x1303)
    noise = generate_noise_source("pink", 48_000, seed=0x1304)

    mixture = mix_at_snr(clean, noise, snr_db)

    clean_rms = np.sqrt(np.mean(np.square(mixture.clean, dtype=np.float64)))
    noise_rms = np.sqrt(np.mean(np.square(mixture.noise, dtype=np.float64)))
    observed_snr = 20.0 * np.log10(clean_rms / noise_rms)
    assert abs(observed_snr - snr_db) < 0.05
    np.testing.assert_allclose(
        mixture.mixture,
        mixture.clean + mixture.noise,
        atol=1e-7,
        rtol=0,
    )
    assert np.max(np.abs(mixture.mixture)) <= 0.98
    assert np.isfinite(mixture.mixture).all()


def test_procedural_split_contains_all_required_item_categories() -> None:
    items = generate_procedural_split(_small_config(), "train")

    mixture_items = [item for item in items if item.category == "mixture"]
    assert len(mixture_items) == len(CLEAN_KINDS) * len(NOISE_KINDS) * len(
        SNR_VALUES_DB
    )
    assert {item.clean_kind for item in mixture_items} == set(CLEAN_KINDS)
    assert {item.noise_kind for item in mixture_items} == set(NOISE_KINDS)
    assert {item.snr_db for item in mixture_items} == set(SNR_VALUES_DB)
    assert any(item.category == "speech-only" for item in items)
    assert any(item.category == "noise-only" for item in items)
    assert any(item.category == "silence" for item in items)
    assert len({item.identity for item in items}) == len(items)


def test_procedural_generation_is_order_and_global_rng_independent() -> None:
    config = _small_config()
    expected = generate_procedural_split(config, "validation")
    np.random.seed(0xFFFF)
    observed = generate_procedural_split(config, "validation")

    assert [item.identity for item in observed] == [item.identity for item in expected]
    for left, right in zip(expected, observed, strict=True):
        assert left.payload_sha256 == right.payload_sha256
        np.testing.assert_array_equal(left.mixture, right.mixture)


def test_item_arrays_are_immutable() -> None:
    item = generate_procedural_split(_small_config(), "test")[0]
    assert not item.clean.flags.writeable
    assert not item.noise.flags.writeable
    assert not item.mixture.flags.writeable
    with pytest.raises(ValueError):
        item.mixture[0] = 1.0


def test_deterministic_wav_bytes_are_stable_and_sensitive() -> None:
    signal = generate_clean_source("unvoiced", 4_800, seed=0x1305)
    left = deterministic_wav_bytes(signal)
    right = deterministic_wav_bytes(signal.copy())
    changed = signal.copy()
    changed[100] += np.float32(0.01)

    assert left == right
    assert left.startswith(b"RIFF")
    assert b"WAVE" in left[:16]
    assert deterministic_wav_bytes(changed) != left


def test_invalid_config_split_and_signal_are_rejected() -> None:
    with pytest.raises(HybridError) as split:
        generate_procedural_split(_small_config(), "holdout")
    assert split.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as length:
        generate_clean_source("voiced", 0, seed=1)
    assert length.value.code is ErrorCode.EVIDENCE_INVALID

    with pytest.raises(HybridError) as category:
        generate_noise_source("ocean", 480, seed=1)
    assert category.value.code is ErrorCode.EVIDENCE_INVALID


def test_config_document_round_trips_exactly(tmp_path) -> None:
    expected = _small_config()
    path = tmp_path / "config.json"
    path.write_text(
        json.dumps(expected.to_document(), sort_keys=True),
        encoding="utf-8",
    )

    observed = load_procedural_config(path)

    assert observed == expected
    assert replace(observed, duration_samples=9_600).duration_samples == 9_600


def test_slr31_archive_contract_is_exact() -> None:
    assert SLR31_SOURCE_PAGE == "https://openslr.org/31/"
    assert SLR31_LICENSE == "CC BY 4.0"
    assert set(SLR31_ARCHIVES) == {"train-clean-5", "dev-clean-2"}
    assert SLR31_ARCHIVES["train-clean-5"].filename == "train-clean-5.tar.gz"
    assert SLR31_ARCHIVES["train-clean-5"].md5 == "5df7d4e78065366204ca6845bb08f490"
    assert SLR31_ARCHIVES["dev-clean-2"].filename == "dev-clean-2.tar.gz"
    assert SLR31_ARCHIVES["dev-clean-2"].md5 == "6d7ab67ac6a1d2c993d050e16d61080d"
    for spec in SLR31_ARCHIVES.values():
        assert spec.url == (f"https://www.openslr.org/resources/31/{spec.filename}")


class _FakeResponse:
    def __init__(self, payload: bytes, *, chunk_size: int = 7) -> None:
        self._payload = payload
        self._offset = 0
        self._chunk_size = chunk_size

    def __enter__(self) -> _FakeResponse:
        return self

    def __exit__(self, *_args) -> None:
        return None

    def read(self, size: int) -> bytes:
        if self._offset >= len(self._payload):
            return b""
        count = min(size, self._chunk_size)
        result = self._payload[self._offset : self._offset + count]
        self._offset += len(result)
        return result


def _tiny_archive_spec(payload: bytes, *, max_bytes: int = 1024) -> Slr31ArchiveSpec:
    return Slr31ArchiveSpec(
        subset="train-clean-5",
        filename="train-clean-5.tar.gz",
        url="https://www.openslr.org/resources/31/train-clean-5.tar.gz",
        md5=md5(payload).hexdigest(),
        max_download_bytes=max_bytes,
        max_extracted_bytes=4096,
    )


def test_download_streams_verifies_and_refuses_overwrite(tmp_path) -> None:
    payload = b"publisher archive bytes"
    spec = _tiny_archive_spec(payload)
    destination = tmp_path / spec.filename

    result = download_slr31_archive(
        spec,
        destination,
        opener=lambda *_args, **_kwargs: _FakeResponse(payload),
    )

    assert destination.read_bytes() == payload
    assert result["bytes"] == len(payload)
    assert result["md5"] == spec.md5
    assert len(result["sha256"]) == 64
    assert not (tmp_path / f".{spec.filename}.partial").exists()
    with pytest.raises(HybridError) as overwrite:
        download_slr31_archive(
            spec,
            destination,
            opener=lambda *_args, **_kwargs: _FakeResponse(payload),
        )
    assert overwrite.value.code is ErrorCode.OUTPUT_EXISTS


def test_download_rejects_checksum_and_byte_limit_and_cleans_partial(
    tmp_path,
) -> None:
    payload = b"longer-than-limit"
    destination = tmp_path / "train-clean-5.tar.gz"
    wrong = replace(_tiny_archive_spec(payload), md5="0" * 32)

    with pytest.raises(HybridError) as checksum:
        download_slr31_archive(
            wrong,
            destination,
            opener=lambda *_args, **_kwargs: _FakeResponse(payload),
        )
    assert checksum.value.code is ErrorCode.EVIDENCE_INVALID
    assert not destination.exists()
    assert not (tmp_path / ".train-clean-5.tar.gz.partial").exists()

    limited = _tiny_archive_spec(payload, max_bytes=4)
    with pytest.raises(HybridError) as size:
        download_slr31_archive(
            limited,
            destination,
            opener=lambda *_args, **_kwargs: _FakeResponse(payload),
        )
    assert size.value.code is ErrorCode.EVIDENCE_INVALID
    assert not destination.exists()


def test_download_rejects_unapproved_url_and_symlink_destination(tmp_path) -> None:
    payload = b"x"
    wrong_url = replace(_tiny_archive_spec(payload), url="https://example.com/x")
    with pytest.raises(HybridError) as url:
        download_slr31_archive(
            wrong_url,
            tmp_path / "x.tar.gz",
            opener=lambda *_args, **_kwargs: _FakeResponse(payload),
        )
    assert url.value.code is ErrorCode.EVIDENCE_INVALID

    target = tmp_path / "target"
    target.write_bytes(b"")
    link = tmp_path / "train-clean-5.tar.gz"
    link.symlink_to(target)
    with pytest.raises(HybridError) as symlink:
        download_slr31_archive(
            _tiny_archive_spec(payload),
            link,
            opener=lambda *_args, **_kwargs: _FakeResponse(payload),
        )
    assert symlink.value.code is ErrorCode.OUTPUT_EXISTS


def test_verify_archive_recomputes_both_hashes(tmp_path) -> None:
    payload = b"verified"
    spec = _tiny_archive_spec(payload)
    path = tmp_path / spec.filename
    path.write_bytes(payload)

    result = verify_slr31_archive(spec, path)

    assert result["md5"] == spec.md5
    assert len(result["sha256"]) == 64
    path.write_bytes(b"tampered")
    with pytest.raises(HybridError) as tampered:
        verify_slr31_archive(spec, path)
    assert tampered.value.code is ErrorCode.EVIDENCE_INVALID


def _tar_bytes(
    members: list[tuple[str, bytes, str]],
) -> bytes:
    output = BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, payload, member_type in members:
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            if member_type == "file":
                archive.addfile(info, BytesIO(payload))
            elif member_type == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = "target"
                info.size = 0
                archive.addfile(info)
            elif member_type == "fifo":
                info.type = tarfile.FIFOTYPE
                info.size = 0
                archive.addfile(info)
            else:
                raise AssertionError(member_type)
    return output.getvalue()


def test_safe_extract_accepts_regular_files_and_directories(tmp_path) -> None:
    payload = _tar_bytes(
        [
            ("LibriSpeech/LICENSE.TXT", b"license", "file"),
            ("LibriSpeech/README.TXT", b"readme", "file"),
            ("LibriSpeech/train-clean-5/1/2/a.flac", b"a", "file"),
            ("LibriSpeech/train-clean-5/1/2/b.txt", b"b", "file"),
        ]
    )
    spec = _tiny_archive_spec(payload)
    archive = tmp_path / spec.filename
    archive.write_bytes(payload)
    destination = tmp_path / "extracted"

    result = extract_slr31_archive(spec, archive, destination)

    assert result["file_count"] == 4
    assert result["extracted_bytes"] == 15
    assert (destination / "LibriSpeech/LICENSE.TXT").read_bytes() == b"license"
    assert (destination / "LibriSpeech/train-clean-5/1/2/a.flac").read_bytes() == b"a"
    assert not any(path.name.endswith(".partial") for path in tmp_path.iterdir())


def test_safe_extract_rejects_unlisted_root_metadata(tmp_path) -> None:
    payload = _tar_bytes([("LibriSpeech/UNEXPECTED.TXT", b"x", "file")])
    spec = _tiny_archive_spec(payload)
    archive = tmp_path / spec.filename
    archive.write_bytes(payload)

    with pytest.raises(HybridError) as unexpected:
        extract_slr31_archive(spec, archive, tmp_path / "out")
    assert unexpected.value.code is ErrorCode.EVIDENCE_INVALID
    assert not (tmp_path / "out").exists()


@pytest.mark.parametrize(
    "name,member_type",
    [
        ("/absolute.flac", "file"),
        ("../escape.flac", "file"),
        ("safe/../../escape.flac", "file"),
        ("safe/link", "symlink"),
        ("safe/fifo", "fifo"),
    ],
)
def test_safe_extract_rejects_unsafe_member(
    tmp_path,
    name: str,
    member_type: str,
) -> None:
    payload = _tar_bytes([(name, b"x", member_type)])
    spec = _tiny_archive_spec(payload)
    archive = tmp_path / spec.filename
    archive.write_bytes(payload)

    with pytest.raises(HybridError) as unsafe:
        extract_slr31_archive(spec, archive, tmp_path / "out")
    assert unsafe.value.code is ErrorCode.EVIDENCE_INVALID
    assert not (tmp_path / "out").exists()


def test_safe_extract_rejects_duplicate_and_size_limit(tmp_path) -> None:
    duplicate_payload = _tar_bytes([("same", b"a", "file"), ("same", b"b", "file")])
    duplicate_spec = _tiny_archive_spec(duplicate_payload)
    duplicate_path = tmp_path / "duplicate.tar.gz"
    duplicate_path.write_bytes(duplicate_payload)
    with pytest.raises(HybridError) as duplicate:
        extract_slr31_archive(
            duplicate_spec,
            duplicate_path,
            tmp_path / "duplicate-out",
        )
    assert duplicate.value.code is ErrorCode.EVIDENCE_INVALID

    large_payload = _tar_bytes([("large", b"12345", "file")])
    large_spec = replace(_tiny_archive_spec(large_payload), max_extracted_bytes=4)
    large_path = tmp_path / "large.tar.gz"
    large_path.write_bytes(large_payload)
    with pytest.raises(HybridError) as size:
        extract_slr31_archive(large_spec, large_path, tmp_path / "large-out")
    assert size.value.code is ErrorCode.EVIDENCE_INVALID


def _write_test_flac(
    root: Path,
    subset: str,
    speaker: int,
    utterance: int,
    *,
    seconds: float = 3.0,
) -> Path:
    path = (
        root
        / "LibriSpeech"
        / subset
        / str(speaker)
        / "1"
        / f"{speaker}-1-{utterance:04d}.flac"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    time = np.arange(round(16_000 * seconds), dtype=np.float64)
    audio = 0.1 * np.sin(2.0 * np.pi * (180.0 + speaker) * time / 16_000.0)
    sf.write(path, audio, 16_000, subtype="PCM_16")
    return path


def test_selection_is_speaker_disjoint_sorted_and_duration_bounded(
    tmp_path,
) -> None:
    train = tmp_path / "train"
    dev = tmp_path / "dev"
    for speaker in (1, 2):
        _write_test_flac(train, "train-clean-5", speaker, 1)
    for speaker in (3, 4, 5, 6):
        _write_test_flac(dev, "dev-clean-2", speaker, 2)
    _write_test_flac(dev, "dev-clean-2", 7, 3, seconds=1.0)
    _write_test_flac(dev, "dev-clean-2", 8, 4, seconds=21.0)

    selection = select_slr31_items(train, dev)

    assert [item.speaker_id for item in selection["train"]] == [1, 2]
    assert [item.speaker_id for item in selection["validation"]] == [3, 4]
    assert [item.speaker_id for item in selection["test"]] == [5, 6]
    assert not (
        {item.speaker_id for item in selection["train"]}
        & {item.speaker_id for item in selection["validation"]}
    )
    assert not (
        {item.speaker_id for item in selection["validation"]}
        & {item.speaker_id for item in selection["test"]}
    )


def test_selection_rejects_speaker_overlap_and_symlink(tmp_path) -> None:
    train = tmp_path / "train"
    dev = tmp_path / "dev"
    source = _write_test_flac(train, "train-clean-5", 10, 1)
    _write_test_flac(dev, "dev-clean-2", 10, 2)
    with pytest.raises(HybridError) as overlap:
        select_slr31_items(train, dev)
    assert overlap.value.code is ErrorCode.EVIDENCE_INVALID

    dev = tmp_path / "dev-safe"
    real = _write_test_flac(dev, "dev-clean-2", 11, 1)
    link = real.with_name("11-1-0002.flac")
    link.symlink_to(source)
    with pytest.raises(HybridError) as symlink:
        select_slr31_items(train, dev)
    assert symlink.value.code is ErrorCode.EVIDENCE_INVALID


def test_resampling_is_deterministic_exact_length_and_finite(tmp_path) -> None:
    train = tmp_path / "train"
    dev = tmp_path / "dev"
    _write_test_flac(train, "train-clean-5", 1, 1)
    _write_test_flac(dev, "dev-clean-2", 2, 1)
    selection = select_slr31_items(train, dev)
    item = selection["train"][0]

    left = load_and_resample_slr31_item(item)
    right = load_and_resample_slr31_item(item)

    np.testing.assert_array_equal(left, right)
    assert left.dtype == np.float32
    assert left.shape == (item.source_samples * 3,)
    assert np.isfinite(left).all()
    assert np.max(np.abs(left)) <= SLR31_RESAMPLED_PEAK_LIMIT


def test_resampling_applies_deterministic_whole_utterance_peak_gain(
    tmp_path,
) -> None:
    train = tmp_path / "train"
    dev = tmp_path / "dev"
    source = _write_test_flac(train, "train-clean-5", 1, 1)
    _write_test_flac(dev, "dev-clean-2", 2, 1)
    samples = np.zeros(48_000, dtype=np.float64)
    samples[24_000] = 0.999
    sf.write(source, samples, 16_000, subtype="PCM_16")
    selection = select_slr31_items(train, dev)

    observed = load_and_resample_slr31_item(selection["train"][0])

    assert np.max(np.abs(observed)) == pytest.approx(
        SLR31_RESAMPLED_PEAK_LIMIT,
        abs=1e-7,
    )


def test_slr31_manifest_covers_license_archives_and_selected_files(
    tmp_path,
) -> None:
    train = tmp_path / "train"
    dev = tmp_path / "dev"
    _write_test_flac(train, "train-clean-5", 1, 1)
    _write_test_flac(dev, "dev-clean-2", 2, 1)
    selection = select_slr31_items(train, dev)
    archive_evidence = {
        subset: {
            "bytes": 1,
            "md5": spec.md5,
            "sha256": "a" * 64,
        }
        for subset, spec in SLR31_ARCHIVES.items()
    }

    manifest = build_slr31_manifest(selection, archive_evidence)

    assert manifest["source_page"] == SLR31_SOURCE_PAGE
    assert manifest["license"] == SLR31_LICENSE
    assert set(manifest["archives"]) == set(SLR31_ARCHIVES)
    assert manifest["selection"]["train"][0]["speaker_id"] == 1
    assert manifest["selection"]["validation"][0]["speaker_id"] == 2
    assert manifest["resampler"]["post_resample_peak_policy"] == {
        "method": "whole-utterance-gain-if-needed",
        "limit": SLR31_RESAMPLED_PEAK_LIMIT,
    }
