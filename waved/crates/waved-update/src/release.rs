//! What `waved-pack pack` checks before it signs anything, so a release can
//! never lock its owner out: the binary is a stripped AArch64 ELF, it reports
//! its own version, and it trusts the key that signs it.

use ed25519_dalek::SigningKey;

use crate::elf;
use crate::format::{self, FormatError, Manifest, Target};
use crate::index::Index;
use crate::key;
use crate::version;

pub struct ReleaseInput<'a> {
    pub binary: &'a [u8],
    /// Deploy files by target name (`S60waved`, `iptables.rules`, ...).
    pub includes: Vec<(String, Vec<u8>)>,
    pub build: String,
    pub created_unix_s: i64,
    pub allow_unstripped: bool,
}

pub struct Release {
    pub manifest: Manifest,
    pub package: Vec<u8>,
    /// `waved-<version>.wup`
    pub package_name: String,
    pub index: Index,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("the waved binary is not a 64-bit ELF file")]
    NotElf,
    #[error("the waved binary is not built for AArch64 (the Wave)")]
    NotAarch64,
    #[error("the waved binary still has symbols; strip it (llvm-objcopy --strip-all) or pass --allow-unstripped")]
    Unstripped,
    #[error("the waved binary carries no version marker")]
    NoVersion,
    #[error(
        "the waved binary does not trust this signing key: build it with WAVED_UPDATE_PUBKEY={0}"
    )]
    KeyNotEmbedded(String),
    #[error("{0:?} is not a file a package may carry (one of: S60waved, S03splash, splash.fb, iptables.rules, rsyslog.conf)")]
    UnknownFile(String),
    #[error("{0:?} is included twice")]
    Duplicate(String),
    #[error(transparent)]
    Format(#[from] FormatError),
}

pub fn build(input: ReleaseInput, signing: &SigningKey) -> Result<Release, ReleaseError> {
    let info = elf::inspect(input.binary).ok_or(ReleaseError::NotElf)?;
    if !info.aarch64 {
        return Err(ReleaseError::NotAarch64);
    }
    if info.unstripped && !input.allow_unstripped {
        return Err(ReleaseError::Unstripped);
    }
    let version = version::find_marker(input.binary).ok_or(ReleaseError::NoVersion)?;
    let public = signing.verifying_key();
    if !key::binary_trusts(input.binary, &public) {
        return Err(ReleaseError::KeyNotEmbedded(key::public_hex(&public)));
    }

    let mut files = vec![(Target::Waved, input.binary.to_vec())];
    for (name, bytes) in input.includes {
        let target = Target::from_name(&name)
            .filter(|t| *t != Target::Waved)
            .ok_or_else(|| ReleaseError::UnknownFile(name.clone()))?;
        if files.iter().any(|(seen, _)| *seen == target) {
            return Err(ReleaseError::Duplicate(name));
        }
        files.push((target, bytes));
    }
    let (manifest, package) =
        format::pack(&files, version, &input.build, input.created_unix_s, signing)?;
    let package_name = format!("waved-{version}.wup");
    let index = Index {
        format: format::FORMAT,
        version: version.to_string(),
        build: input.build,
        package: package_name.clone(),
        size: package.len() as u64,
        sha256: format::sha256_hex(&package),
        installed_size: manifest.installed_size(),
    };
    Ok(Release {
        manifest,
        package,
        package_name,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::tests::elf;
    use crate::format::read_header;
    use crate::version::Version;

    const AARCH64: u16 = 183;

    fn binary(signing: &SigningKey, sections: &[(&str, u32)]) -> Vec<u8> {
        let mut tail = b"waved-version:0.10.0\0".to_vec();
        tail.extend_from_slice(key::public_hex(&signing.verifying_key()).as_bytes());
        elf(AARCH64, sections, &tail)
    }

    fn input(binary: &[u8]) -> ReleaseInput<'_> {
        ReleaseInput {
            binary,
            includes: vec![("S60waved".into(), b"#!/bin/sh\n".to_vec())],
            build: "374015f".into(),
            created_unix_s: 1_760_000_000,
            allow_unstripped: false,
        }
    }

    #[test]
    fn a_good_binary_becomes_a_package_and_its_index() {
        let signing = key::generate().unwrap();
        let bin = binary(&signing, &[(".text", 1)]);
        let release = build(input(&bin), &signing).unwrap();
        assert_eq!(release.package_name, "waved-0.10.0.wup");
        let header =
            read_header(&mut release.package.as_slice(), &signing.verifying_key()).unwrap();
        assert_eq!(header.version, Version::new(0, 10, 0));
        assert_eq!(header.manifest.files.len(), 2);
        let (index, version) = Index::parse(release.index.to_json().as_bytes()).unwrap();
        assert_eq!(version, Version::new(0, 10, 0));
        assert_eq!(index.size, release.package.len() as u64);
        assert_eq!(index.sha256, format::sha256_hex(&release.package));
        assert_eq!(index.build, "374015f");
    }

    #[test]
    fn a_binary_that_would_lock_the_owner_out_is_refused() {
        let signing = key::generate().unwrap();
        let other = key::generate().unwrap();
        let bin = binary(&other, &[(".text", 1)]);
        assert!(matches!(
            build(input(&bin), &signing),
            Err(ReleaseError::KeyNotEmbedded(_))
        ));
    }

    #[test]
    fn wrong_or_unstripped_or_unversioned_binaries_are_refused() {
        let signing = key::generate().unwrap();
        let unstripped = binary(&signing, &[(".symtab", 2)]);
        assert!(matches!(
            build(input(&unstripped), &signing),
            Err(ReleaseError::Unstripped)
        ));
        let mut allowed = input(&unstripped);
        allowed.allow_unstripped = true;
        assert!(build(allowed, &signing).is_ok());

        let x86 = elf(
            62,
            &[],
            key::public_hex(&signing.verifying_key()).as_bytes(),
        );
        assert!(matches!(
            build(input(&x86), &signing),
            Err(ReleaseError::NotAarch64)
        ));
        assert!(matches!(
            build(input(b"#!/bin/sh"), &signing),
            Err(ReleaseError::NotElf)
        ));
        let no_marker = elf(
            AARCH64,
            &[],
            key::public_hex(&signing.verifying_key()).as_bytes(),
        );
        assert!(matches!(
            build(input(&no_marker), &signing),
            Err(ReleaseError::NoVersion)
        ));
    }

    #[test]
    fn only_known_deploy_files_go_in_and_only_once() {
        let signing = key::generate().unwrap();
        let bin = binary(&signing, &[]);
        for name in ["waved", "S99evil", "../S60waved"] {
            let mut bad = input(&bin);
            bad.includes = vec![(name.into(), vec![])];
            assert!(
                matches!(build(bad, &signing), Err(ReleaseError::UnknownFile(_))),
                "{name}"
            );
        }
        let mut twice = input(&bin);
        twice.includes.push(("S60waved".into(), vec![]));
        assert!(matches!(
            build(twice, &signing),
            Err(ReleaseError::Duplicate(_))
        ));
    }
}
