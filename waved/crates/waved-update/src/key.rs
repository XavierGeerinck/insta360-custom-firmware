//! The signing key. The public half is compiled in from the
//! `WAVED_UPDATE_PUBKEY` environment variable (64 hex characters) at build
//! time; a build without it trusts no package and shows "Updates are off".
//! The private half is a 64-hex-character seed in a file outside the repo,
//! read only by `waved-pack`.

use ed25519_dalek::{SigningKey, VerifyingKey};

pub const KEY_ENV: &str = "WAVED_UPDATE_PUBKEY";

/// As the build passed it in. Kept as text so `waved-pack` can find it in a
/// binary (`binary_trusts`).
const COMPILED: Option<&str> = option_env!("WAVED_UPDATE_PUBKEY");

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("a key is 64 hex characters")]
    Malformed,
    #[error("not a valid ed25519 public key")]
    NotAPoint,
    #[error("no randomness: {0}")]
    Random(String),
}

/// The key this build trusts, if it was built with one.
pub fn trusted_key() -> Option<VerifyingKey> {
    COMPILED.and_then(|text| parse_public_key(text).ok())
}

/// The compiled-in text when it is set but unusable (logged at start-up).
pub fn compiled_key_problem() -> Option<KeyError> {
    COMPILED.and_then(|text| parse_public_key(text).err())
}

fn parse_32(text: &str) -> Result<[u8; 32], KeyError> {
    let text = text.trim();
    if text.len() != 64 {
        return Err(KeyError::Malformed);
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(text, &mut out).map_err(|_| KeyError::Malformed)?;
    Ok(out)
}

pub fn parse_public_key(text: &str) -> Result<VerifyingKey, KeyError> {
    VerifyingKey::from_bytes(&parse_32(text)?).map_err(|_| KeyError::NotAPoint)
}

/// A private key file: the 32-byte seed as hex, surrounding whitespace allowed.
pub fn parse_signing_key(text: &str) -> Result<SigningKey, KeyError> {
    Ok(SigningKey::from_bytes(&parse_32(text)?))
}

pub fn generate() -> Result<SigningKey, KeyError> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| KeyError::Random(e.to_string()))?;
    Ok(SigningKey::from_bytes(&seed))
}

pub fn public_hex(key: &VerifyingKey) -> String {
    hex::encode(key.as_bytes())
}

pub fn signing_hex(key: &SigningKey) -> String {
    hex::encode(key.to_bytes())
}

/// The binary was built with `key` compiled in, so it will accept the next
/// package signed with it. `waved-pack` refuses to pack a binary that would
/// lock its owner out of over-the-air updates.
pub fn binary_trusts(binary: &[u8], key: &VerifyingKey) -> bool {
    let lower = public_hex(key);
    let upper = lower.to_ascii_uppercase();
    [lower, upper].iter().any(|text| {
        binary
            .windows(text.len())
            .any(|window| window == text.as_bytes())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, Verifier};

    #[test]
    fn a_generated_key_signs_and_its_public_half_verifies() {
        let key = generate().unwrap();
        let again = parse_signing_key(&format!("  {}\n", signing_hex(&key))).unwrap();
        assert_eq!(again.to_bytes(), key.to_bytes());
        let public = parse_public_key(&public_hex(&key.verifying_key())).unwrap();
        let signature = key.sign(b"hello");
        assert!(public.verify(b"hello", &signature).is_ok());
        assert!(public.verify(b"hellO", &signature).is_err());
    }

    #[test]
    fn malformed_keys_are_refused() {
        for bad in [
            "",
            "abc",
            &"zz".repeat(32),
            &"00".repeat(31),
            &"00".repeat(33),
        ] {
            assert_eq!(
                parse_public_key(bad).unwrap_err(),
                KeyError::Malformed,
                "{bad}"
            );
            assert!(parse_signing_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_binary_trusts_a_key_only_when_it_embeds_it() {
        let key = generate().unwrap().verifying_key();
        let other = generate().unwrap().verifying_key();
        let mut binary = vec![0u8; 4096];
        binary.extend_from_slice(public_hex(&key).as_bytes());
        binary.extend_from_slice(&[1u8; 128]);
        assert!(binary_trusts(&binary, &key));
        assert!(!binary_trusts(&binary, &other));
        let upper = public_hex(&other).to_ascii_uppercase();
        assert!(binary_trusts(upper.as_bytes(), &other));
    }
}
