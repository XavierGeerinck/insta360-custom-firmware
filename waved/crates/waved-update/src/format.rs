//! The waved update package (`.wup`). All integers are little-endian.
//!
//! ```text
//! 0        8   magic "WAVEDUP1"
//! 8        4   header length H (1..=64 KiB)
//! 12       H   header: the manifest as UTF-8 JSON
//! 12+H     64  ed25519 signature over bytes 0..12+H
//! 76+H     ..  each file's stored (gzip) bytes, in manifest order, back to back
//! ```
//!
//! The signature covers the header; the header holds each file's stored and
//! unpacked SHA-256 and size, so it binds every byte. Nothing past the header
//! is trusted until its stored hash matches (pass 1), and nothing is
//! decompressed before that (pass 2). Files go to a closed list of targets
//! (`Target`): a package can replace waved and its deploy files, nothing else.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::version::Version;

pub const MAGIC: &[u8; 8] = b"WAVEDUP1";
pub const FORMAT: u32 = 1;
pub const MAX_HEADER: u32 = 64 * 1024;
pub const SIGNATURE_LEN: usize = 64;
/// Larger than any waved binary so far (~63 MB stripped), small enough for `/data`.
pub const MAX_FILE: u64 = 192 * 1024 * 1024;

/// Where a file in a package may go. Device paths are fixed here, not in the
/// package, so a package cannot write anywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "waved")]
    Waved,
    #[serde(rename = "S60waved")]
    S60waved,
    #[serde(rename = "S03splash")]
    S03splash,
    #[serde(rename = "splash.fb")]
    SplashFb,
    #[serde(rename = "iptables.rules")]
    IptablesRules,
    #[serde(rename = "rsyslog.conf")]
    RsyslogConf,
}

impl Target {
    pub const ALL: [Target; 6] = [
        Target::Waved,
        Target::S60waved,
        Target::S03splash,
        Target::SplashFb,
        Target::IptablesRules,
        Target::RsyslogConf,
    ];

    /// The file name in `deploy/` and in the staging directory.
    pub fn name(self) -> &'static str {
        match self {
            Target::Waved => "waved",
            Target::S60waved => "S60waved",
            Target::S03splash => "S03splash",
            Target::SplashFb => "splash.fb",
            Target::IptablesRules => "iptables.rules",
            Target::RsyslogConf => "rsyslog.conf",
        }
    }

    pub fn from_name(name: &str) -> Option<Target> {
        Target::ALL.into_iter().find(|target| target.name() == name)
    }

    /// Absolute path on the device. `/etc` is an overlay whose upper dir is
    /// on `/data`, so writes there persist (`docs/UPDATES.md`).
    pub fn device_path(self) -> &'static str {
        match self {
            Target::Waved => "/data/waved/waved",
            Target::S60waved => "/etc/init.d/S60waved",
            Target::S03splash => "/etc/init.d/S03splash",
            Target::SplashFb => "/data/waved/splash.fb",
            Target::IptablesRules => "/data/waved/iptables.rules",
            Target::RsyslogConf => "/etc/rsyslog.conf",
        }
    }

    pub fn mode(self) -> u32 {
        match self {
            Target::Waved | Target::S60waved | Target::S03splash => 0o755,
            _ => 0o644,
        }
    }

    /// Shell scripts the init system runs; syntax-checked before install.
    pub fn is_init_script(self) -> bool {
        matches!(self, Target::S60waved | Target::S03splash)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub target: Target,
    /// Unpacked size and SHA-256 (hex).
    pub size: u64,
    pub sha256: String,
    /// Size and SHA-256 of the gzip bytes in the package.
    pub stored_size: u64,
    pub stored_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub version: String,
    /// Git build id, for display and sideways reinstalls. May be empty.
    #[serde(default)]
    pub build: String,
    pub created_unix_s: i64,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn installed_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    pub fn stored_size(&self) -> u64 {
        self.files.iter().map(|file| file.stored_size).sum()
    }

    fn validate(&self) -> Result<Version, FormatError> {
        let bad = |why: &str| Err(FormatError::BadManifest(why.into()));
        if self.format != FORMAT {
            return bad("unknown format");
        }
        let version: Version = self
            .version
            .parse()
            .map_err(|_| FormatError::BadManifest("bad version".into()))?;
        if self.build.len() > 40 || !self.build.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return bad("bad build id");
        }
        if !self.files.iter().any(|file| file.target == Target::Waved) {
            return bad("no waved binary");
        }
        for (i, file) in self.files.iter().enumerate() {
            if self.files[..i]
                .iter()
                .any(|seen| seen.target == file.target)
            {
                return bad("a target appears twice");
            }
            if file.size > MAX_FILE || file.stored_size > MAX_FILE {
                return bad("a file is too large");
            }
            if !is_sha256_hex(&file.sha256) || !is_sha256_hex(&file.stored_sha256) {
                return bad("a hash is not SHA-256 hex");
            }
        }
        Ok(version)
    }
}

pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("not a waved update package")]
    NotAPackage,
    #[error("header length {0} is out of range")]
    HeaderSize(u32),
    #[error("the signature does not match this Wave's key")]
    BadSignature,
    #[error("bad manifest: {0}")]
    BadManifest(String),
    #[error("the package ends early")]
    Truncated,
    #[error("the package has bytes after its last file")]
    TrailingBytes,
    #[error("{0:?} does not match its signed hash")]
    StoredHashMismatch(Target),
    #[error("{0:?} unpacks to the wrong size")]
    SizeMismatch(Target),
    #[error("{0:?} unpacks to the wrong content")]
    HashMismatch(Target),
}

impl FormatError {
    /// A tampered, foreign or broken package, as opposed to a local i/o problem.
    pub fn is_rejection(&self) -> bool {
        !matches!(self, FormatError::Io(_))
    }
}

fn read_exact_or_truncated<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), FormatError> {
    reader.read_exact(buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => FormatError::Truncated,
        _ => FormatError::Io(e),
    })
}

/// Lay a package out: magic, header, signature, stored bytes. Exposed so
/// tests can sign manifests `pack` would never write.
pub fn assemble(manifest: &Manifest, stored: &[Vec<u8>], key: &SigningKey) -> Vec<u8> {
    let header = serde_json::to_vec(manifest).expect("a manifest serializes");
    let mut out =
        Vec::with_capacity(header.len() + 76 + stored.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    let signature = key.sign(&out);
    out.extend_from_slice(&signature.to_bytes());
    for bytes in stored {
        out.extend_from_slice(bytes);
    }
    out
}

/// Build and sign a package (host side, `waved-pack`).
pub fn pack(
    files: &[(Target, Vec<u8>)],
    version: Version,
    build: &str,
    created_unix_s: i64,
    key: &SigningKey,
) -> Result<(Manifest, Vec<u8>), FormatError> {
    let mut entries = Vec::new();
    let mut stored = Vec::new();
    for (target, bytes) in files {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(9));
        encoder.write_all(bytes)?;
        let gz = encoder.finish()?;
        entries.push(FileEntry {
            target: *target,
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            stored_size: gz.len() as u64,
            stored_sha256: sha256_hex(&gz),
        });
        stored.push(gz);
    }
    let manifest = Manifest {
        format: FORMAT,
        version: version.to_string(),
        build: build.into(),
        created_unix_s,
        files: entries,
    };
    manifest.validate()?;
    Ok((manifest.clone(), assemble(&manifest, &stored, key)))
}

/// A verified header and where the stored bytes start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub manifest: Manifest,
    pub version: Version,
    pub payload_offset: u64,
}

/// Read and verify the signed header. Only the length is read before the
/// signature is checked, and it is bounded.
pub fn read_header<R: Read>(reader: &mut R, key: &VerifyingKey) -> Result<Header, FormatError> {
    let mut magic = [0u8; 8];
    read_exact_or_truncated(reader, &mut magic)?;
    if &magic != MAGIC {
        return Err(FormatError::NotAPackage);
    }
    let mut len = [0u8; 4];
    read_exact_or_truncated(reader, &mut len)?;
    let len = u32::from_le_bytes(len);
    if len == 0 || len > MAX_HEADER {
        return Err(FormatError::HeaderSize(len));
    }
    let mut signed = Vec::with_capacity(12 + len as usize);
    signed.extend_from_slice(&magic);
    signed.extend_from_slice(&len.to_le_bytes());
    signed.resize(12 + len as usize, 0);
    read_exact_or_truncated(reader, &mut signed[12..])?;
    let mut signature = [0u8; SIGNATURE_LEN];
    read_exact_or_truncated(reader, &mut signature)?;
    key.verify_strict(&signed, &Signature::from_bytes(&signature))
        .map_err(|_| FormatError::BadSignature)?;
    let manifest: Manifest = serde_json::from_slice(&signed[12..])
        .map_err(|e| FormatError::BadManifest(e.to_string()))?;
    let version = manifest.validate()?;
    Ok(Header {
        manifest,
        version,
        payload_offset: 12 + u64::from(len) + SIGNATURE_LEN as u64,
    })
}

/// Read and verify only the header of the package at `path`.
pub fn read_header_at(path: &Path, key: &VerifyingKey) -> Result<Header, FormatError> {
    read_header(&mut BufReader::new(File::open(path)?), key)
}

/// A package whose files are unpacked, checked and waiting in the staging
/// directory under their target names, with their modes set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub manifest: Manifest,
    pub version: Version,
    pub dir: PathBuf,
}

impl Staged {
    pub fn path(&self, target: Target) -> PathBuf {
        self.dir.join(target.name())
    }

    pub fn targets(&self) -> impl Iterator<Item = Target> + '_ {
        self.manifest.files.iter().map(|file| file.target)
    }
}

/// Hash `len` bytes from `reader` in 64 KiB pieces.
fn hash_exact<R: Read>(
    reader: &mut R,
    len: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<String, FormatError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut left = len;
    while left > 0 {
        let take = left.min(buf.len() as u64) as usize;
        read_exact_or_truncated(reader, &mut buf[..take])?;
        hasher.update(&buf[..take]);
        left -= take as u64;
        progress(take as u64);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify the package at `package` and unpack it into `staging` (emptied
/// first). `progress(done, total)` counts bytes over both passes. On any
/// error the staging directory is left empty.
pub fn verify_and_stage(
    package: &Path,
    key: &VerifyingKey,
    staging: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Staged, FormatError> {
    let result = stage_inner(package, key, staging, progress);
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn stage_inner(
    package: &Path,
    key: &VerifyingKey,
    staging: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Staged, FormatError> {
    let mut reader = BufReader::new(File::open(package)?);
    let header = read_header(&mut reader, key)?;
    let stored_total = header.manifest.stored_size();
    let total = stored_total + header.manifest.installed_size();
    let mut done = 0u64;

    // Pass 1: every stored byte matches the signed header, and nothing follows.
    for file in &header.manifest.files {
        let hash = hash_exact(&mut reader, file.stored_size, &mut |n| {
            done += n;
            progress(done, total);
        })?;
        if hash != file.stored_sha256 {
            return Err(FormatError::StoredHashMismatch(file.target));
        }
    }
    let mut probe = [0u8; 1];
    if reader.read(&mut probe)? != 0 {
        return Err(FormatError::TrailingBytes);
    }

    // Pass 2: unpack the now-trusted bytes, checking size and content.
    let _ = fs::remove_dir_all(staging);
    fs::create_dir_all(staging)?;
    reader.seek(SeekFrom::Start(header.payload_offset))?;
    for file in &header.manifest.files {
        let part = staging.join(format!("{}.part", file.target.name()));
        let mut out = File::create(&part)?;
        let mut decoder = flate2::read::GzDecoder::new((&mut reader).take(file.stored_size));
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = decoder
                .read(&mut buf)
                .map_err(|_| FormatError::HashMismatch(file.target))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > file.size {
                return Err(FormatError::SizeMismatch(file.target));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        if written != file.size {
            return Err(FormatError::SizeMismatch(file.target));
        }
        if hex::encode(hasher.finalize()) != file.sha256 {
            return Err(FormatError::HashMismatch(file.target));
        }
        // The decoder may stop before the stored bytes end; skip what is left.
        io::copy(&mut decoder.into_inner(), &mut io::sink())?;
        set_mode(&out, file.target.mode())?;
        out.sync_all()?;
        drop(out);
        fs::rename(&part, staging.join(file.target.name()))?;
    }
    File::open(staging)?.sync_all()?;
    Ok(Staged {
        manifest: header.manifest,
        version: header.version,
        dir: staging.to_path_buf(),
    })
}

#[cfg(unix)]
fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::key;

    pub(crate) fn binary() -> Vec<u8> {
        let mut bytes = b"\x7fELF waved-version:0.10.0\0".to_vec();
        bytes.extend((0..200_000u32).map(|i| (i % 251) as u8));
        bytes
    }

    pub(crate) fn sample(key: &SigningKey) -> Vec<u8> {
        let files = vec![
            (Target::Waved, binary()),
            (Target::S60waved, b"#!/bin/sh\necho hi\n".to_vec()),
        ];
        pack(
            &files,
            Version::new(0, 10, 0),
            "abc1234",
            1_760_000_000,
            key,
        )
        .unwrap()
        .1
    }

    struct Fixture {
        dir: tempfile::TempDir,
        key: SigningKey,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                key: key::generate().unwrap(),
            }
        }

        fn stage(&self, package: &[u8]) -> Result<Staged, FormatError> {
            self.stage_with(package, &self.key.verifying_key())
        }

        fn stage_with(&self, package: &[u8], key: &VerifyingKey) -> Result<Staged, FormatError> {
            let path = self.dir.path().join("package.wup");
            fs::write(&path, package).unwrap();
            verify_and_stage(&path, key, &self.staging(), &mut |_, _| {})
        }

        fn staging(&self) -> PathBuf {
            self.dir.path().join("staged")
        }

        fn staged_names(&self) -> Vec<String> {
            fs::read_dir(self.staging())
                .map(|entries| {
                    entries
                        .map(|e| e.unwrap().file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    /// Offset of the first stored byte in `package`.
    fn payload_offset(package: &[u8]) -> usize {
        12 + u32::from_le_bytes(package[8..12].try_into().unwrap()) as usize + SIGNATURE_LEN
    }

    #[test]
    fn a_signed_package_round_trips_into_staging() {
        let f = Fixture::new();
        let mut seen = Vec::new();
        let path = f.dir.path().join("package.wup");
        fs::write(&path, sample(&f.key)).unwrap();
        let staged = verify_and_stage(&path, &f.key.verifying_key(), &f.staging(), &mut |d, t| {
            seen.push((d, t))
        })
        .unwrap();
        assert_eq!(staged.version, Version::new(0, 10, 0));
        assert_eq!(staged.manifest.build, "abc1234");
        assert_eq!(fs::read(staged.path(Target::Waved)).unwrap(), binary());
        assert_eq!(
            fs::read(staged.path(Target::S60waved)).unwrap(),
            b"#!/bin/sh\necho hi\n"
        );
        let mut names = f.staged_names();
        names.sort();
        assert_eq!(names, ["S60waved", "waved"]);
        let (done, total) = *seen.last().unwrap();
        assert_eq!(done, total);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |t: Target| fs::metadata(staged.path(t)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(Target::Waved), 0o755);
        }
    }

    #[test]
    fn a_package_signed_by_another_key_is_rejected() {
        let f = Fixture::new();
        let other = key::generate().unwrap();
        let err = f
            .stage_with(&sample(&other), &f.key.verifying_key())
            .unwrap_err();
        assert!(matches!(err, FormatError::BadSignature), "{err}");
        assert!(f.staged_names().is_empty());
    }

    #[test]
    fn a_tampered_header_or_signature_is_rejected() {
        let f = Fixture::new();
        let package = sample(&f.key);
        let header_end = payload_offset(&package) - SIGNATURE_LEN;
        let mut header = package.clone();
        let at = header[12..header_end]
            .windows(6)
            .position(|w| w == b"0.10.0")
            .unwrap();
        header[12 + at..12 + at + 6].copy_from_slice(b"9.10.0");
        assert!(matches!(
            f.stage(&header).unwrap_err(),
            FormatError::BadSignature
        ));
        let mut signature = package.clone();
        signature[header_end + 5] ^= 1;
        assert!(matches!(
            f.stage(&signature).unwrap_err(),
            FormatError::BadSignature
        ));
    }

    #[test]
    fn a_tampered_payload_is_caught_before_anything_is_unpacked() {
        let f = Fixture::new();
        let mut package = sample(&f.key);
        let at = payload_offset(&package) + 100;
        package[at] ^= 0x40;
        let err = f.stage(&package).unwrap_err();
        assert!(
            matches!(err, FormatError::StoredHashMismatch(Target::Waved)),
            "{err}"
        );
        assert!(f.staged_names().is_empty());
    }

    #[test]
    fn truncated_and_padded_packages_are_rejected() {
        let f = Fixture::new();
        let package = sample(&f.key);
        let short = &package[..package.len() - 10];
        assert!(matches!(
            f.stage(short).unwrap_err(),
            FormatError::Truncated
        ));
        let mut long = package.clone();
        long.push(0);
        assert!(matches!(
            f.stage(&long).unwrap_err(),
            FormatError::TrailingBytes
        ));
        assert!(matches!(
            f.stage(&package[..20]).unwrap_err(),
            FormatError::Truncated
        ));
    }

    #[test]
    fn foreign_files_and_absurd_headers_are_rejected_before_any_crypto() {
        let f = Fixture::new();
        assert!(matches!(
            f.stage(b"!C#\x01 an insta360 swu").unwrap_err(),
            FormatError::NotAPackage
        ));
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&(MAX_HEADER + 1).to_le_bytes());
        assert!(matches!(
            f.stage(&huge).unwrap_err(),
            FormatError::HeaderSize(_)
        ));
        let mut empty = MAGIC.to_vec();
        empty.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            f.stage(&empty).unwrap_err(),
            FormatError::HeaderSize(0)
        ));
    }

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn entry(target: Target, bytes: &[u8], stored: &[u8]) -> FileEntry {
        FileEntry {
            target,
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            stored_size: stored.len() as u64,
            stored_sha256: sha256_hex(stored),
        }
    }

    fn manifest(files: Vec<FileEntry>) -> Manifest {
        Manifest {
            format: FORMAT,
            version: "0.10.0".into(),
            build: String::new(),
            created_unix_s: 0,
            files,
        }
    }

    #[test]
    fn a_signed_manifest_that_lies_about_sizes_or_content_is_rejected() {
        let f = Fixture::new();
        let bytes = b"real content".to_vec();
        let stored = gz(&bytes);
        let mut lie = entry(Target::Waved, &bytes, &stored);
        lie.size -= 1;
        let package = assemble(&manifest(vec![lie]), std::slice::from_ref(&stored), &f.key);
        assert!(matches!(
            f.stage(&package).unwrap_err(),
            FormatError::SizeMismatch(Target::Waved)
        ));
        let mut lie = entry(Target::Waved, &bytes, &stored);
        lie.sha256 = sha256_hex(b"other content");
        let package = assemble(&manifest(vec![lie]), &[stored], &f.key);
        assert!(matches!(
            f.stage(&package).unwrap_err(),
            FormatError::HashMismatch(Target::Waved)
        ));
        assert!(f.staged_names().is_empty());
    }

    #[test]
    fn manifests_without_waved_with_duplicates_or_unknown_targets_are_rejected() {
        let f = Fixture::new();
        let bytes = b"x".to_vec();
        let stored = gz(&bytes);
        let only_script = manifest(vec![entry(Target::S60waved, &bytes, &stored)]);
        let package = assemble(&only_script, std::slice::from_ref(&stored), &f.key);
        assert!(matches!(
            f.stage(&package).unwrap_err(),
            FormatError::BadManifest(_)
        ));
        let twice = manifest(vec![
            entry(Target::Waved, &bytes, &stored),
            entry(Target::Waved, &bytes, &stored),
        ]);
        let package = assemble(&twice, &[stored.clone(), stored.clone()], &f.key);
        assert!(matches!(
            f.stage(&package).unwrap_err(),
            FormatError::BadManifest(_)
        ));

        // A target outside the closed list does not even parse.
        let mut json =
            serde_json::to_string(&manifest(vec![entry(Target::Waved, &bytes, &stored)])).unwrap();
        json = json.replace("\"waved\"", "\"../../etc/passwd\"");
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&(json.len() as u32).to_le_bytes());
        raw.extend_from_slice(json.as_bytes());
        let signature = f.key.sign(&raw);
        raw.extend_from_slice(&signature.to_bytes());
        raw.extend_from_slice(&stored);
        assert!(matches!(
            f.stage(&raw).unwrap_err(),
            FormatError::BadManifest(_)
        ));
    }

    #[test]
    fn targets_have_fixed_device_paths_and_modes() {
        for target in Target::ALL {
            assert_eq!(Target::from_name(target.name()), Some(target));
            assert!(target.device_path().starts_with('/'));
            assert!(target.device_path().ends_with(target.name()));
        }
        assert_eq!(Target::Waved.device_path(), "/data/waved/waved");
        assert_eq!(Target::S60waved.mode(), 0o755);
        assert_eq!(Target::IptablesRules.mode(), 0o644);
        assert!(Target::S60waved.is_init_script());
        assert!(!Target::Waved.is_init_script());
        assert_eq!(Target::from_name("../waved"), None);
    }
}
