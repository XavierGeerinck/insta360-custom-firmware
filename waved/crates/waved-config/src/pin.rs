//! The device PIN (D10): 4 to 6 digits, set on the setup page. The PIN
//! screen and the Wave Controller (`verify_dev_passwd`, J3) both check it with
//! [`verify_device_pin`]; [`PinAttempts`] adds the wait after five wrong tries.
//!
//! config.toml keeps `security.device_password` as a record, never the
//! digits: `pin1$<digits>$<iterations>$<salt hex>$<hash hex>`, where the hash
//! is PBKDF2-HMAC-SHA256 of the digits with a 16-byte random salt. An empty
//! value means no PIN. A plain 4–6 digit value (a hand edit, or a file from
//! before K2) is hashed when the file loads; anything else is dropped.
//!
//! Four to six digits are at most 10^6 guesses, so the hash only keeps the
//! PIN out of the file and its copies; the file's 0600 mode protects it.

use crate::SecurityConfig;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{info, warn};

pub const PIN_MIN_DIGITS: usize = 4;
pub const PIN_MAX_DIGITS: usize = 6;
/// Wrong tries before the keypad waits (D10).
pub const PIN_MAX_TRIES: u32 = 5;
/// How long it waits.
pub const PIN_LOCKOUT_MS: u64 = 60_000;

const TAG: &str = "pin1";
/// PBKDF2 rounds for new records. Each record names its own count, so this can
/// rise later without a migration.
const ITERATIONS: u32 = 10_000;
/// A hand-edited count above this is refused, so a check can't stall the UI.
const MAX_ITERATIONS: u32 = 1_000_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

/// 4 to 6 ASCII digits.
pub fn is_valid_pin(pin: &str) -> bool {
    (PIN_MIN_DIGITS..=PIN_MAX_DIGITS).contains(&pin.len())
        && pin.bytes().all(|b| b.is_ascii_digit())
}

/// The stored record for `pin`, with a fresh random salt. `pin` must be valid.
pub fn hash_pin(pin: &str) -> String {
    debug_assert!(is_valid_pin(pin), "hash_pin takes 4 to 6 digits");
    let salt = random_salt();
    let hash = pbkdf2(pin.as_bytes(), &salt, ITERATIONS);
    format!(
        "{TAG}${}${ITERATIONS}${}${}",
        pin.len(),
        to_hex(&salt),
        to_hex(&hash)
    )
}

/// How many digits the saved PIN has; `None` when no PIN is set.
pub fn pin_digits(security: &SecurityConfig) -> Option<usize> {
    Record::parse(&security.device_password).map(|record| record.digits)
}

/// Whether `candidate` is the device PIN. False when no PIN is set.
pub fn verify_device_pin(security: &SecurityConfig, candidate: &str) -> bool {
    let Some(record) = Record::parse(&security.device_password) else {
        return false;
    };
    // The length is no secret: the record and the PIN screen's dots show it.
    if !is_valid_pin(candidate) || candidate.len() != record.digits {
        return false;
    }
    let hash = pbkdf2(candidate.as_bytes(), &record.salt, record.iterations);
    same_bytes(&hash, &record.hash)
}

/// The outcome of one try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinCheck {
    Accepted,
    /// Wrong; `tries_left` more before the wait (0: the wait starts now).
    Wrong {
        tries_left: u32,
    },
    /// Waiting after five wrong tries: the candidate was not checked.
    Locked {
        until_ms: u64,
    },
}

/// Wrong tries and the one-minute wait. Time is any monotonic millisecond
/// clock the caller keeps (the UI's `now_ms`).
#[derive(Clone, Debug, Default)]
pub struct PinAttempts {
    wrong: u32,
    locked_until_ms: Option<u64>,
}

impl PinAttempts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check `candidate` unless the keypad is waiting. The fifth wrong try in a
    /// row starts the wait; when it ends, five more tries are allowed. A right
    /// PIN clears the count.
    pub fn check(&mut self, security: &SecurityConfig, candidate: &str, now_ms: u64) -> PinCheck {
        if let Some(until_ms) = self.locked_until(now_ms) {
            return PinCheck::Locked { until_ms };
        }
        if verify_device_pin(security, candidate) {
            self.wrong = 0;
            return PinCheck::Accepted;
        }
        self.wrong += 1;
        let tries_left = PIN_MAX_TRIES.saturating_sub(self.wrong);
        if tries_left == 0 {
            self.wrong = 0;
            self.locked_until_ms = Some(now_ms + PIN_LOCKOUT_MS);
        }
        PinCheck::Wrong { tries_left }
    }

    /// When the wait ends, while it lasts.
    pub fn locked_until(&self, now_ms: u64) -> Option<u64> {
        self.locked_until_ms.filter(|until| now_ms < *until)
    }
}

/// "" (no PIN) or a well-formed record: what `validate_config` accepts.
pub(crate) fn stored_value_ok(value: &str) -> bool {
    value.is_empty() || Record::parse(value).is_some()
}

/// On load: hash a plain 4–6 digit PIN, drop anything else that is not a
/// record. Returns whether the value changed. Never logs the value.
pub(crate) fn normalise_stored(security: &mut SecurityConfig) -> bool {
    let value = &security.device_password;
    if stored_value_ok(value) {
        return false;
    }
    security.device_password = if is_valid_pin(value) {
        info!("security.device_password was a plain PIN; it is kept as a hash from now on");
        hash_pin(value)
    } else {
        warn!(
            "security.device_password is not a 4 to 6 digit PIN, so the PIN lock is off; \
             set a PIN on the setup page"
        );
        String::new()
    };
    true
}

struct Record {
    digits: usize,
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

impl Record {
    fn parse(value: &str) -> Option<Record> {
        let mut parts = value.split('$');
        if parts.next()? != TAG {
            return None;
        }
        let digits: usize = number(parts.next()?)?;
        let iterations: u32 = number(parts.next()?)?;
        let salt = from_hex(parts.next()?)?;
        let hash = from_hex(parts.next()?)?;
        let ok = parts.next().is_none()
            && (PIN_MIN_DIGITS..=PIN_MAX_DIGITS).contains(&digits)
            && (1..=MAX_ITERATIONS).contains(&iterations)
            && salt.len() == SALT_LEN
            && hash.len() == HASH_LEN;
        ok.then_some(Record {
            digits,
            iterations,
            salt,
            hash,
        })
    }
}

/// Plain decimal digits only (`str::parse` would also take a leading `+`).
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// PBKDF2-HMAC-SHA256 (RFC 8018) for one 32-byte block.
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; HASH_LEN] {
    let key = Hmac::<Sha256>::new_from_slice(password).expect("HMAC takes keys of any length");
    let mut first = key.clone();
    first.update(salt);
    first.update(&1u32.to_be_bytes());
    let mut u = [0u8; HASH_LEN];
    u.copy_from_slice(&first.finalize().into_bytes());
    let mut out = u;
    for _ in 1..iterations {
        let mut round = key.clone();
        round.update(&u);
        u.copy_from_slice(&round.finalize().into_bytes());
        out.iter_mut().zip(u).for_each(|(o, b)| *o ^= b);
    }
    out
}

/// Equal without an early exit.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn random_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut salt));
    if read.is_ok() {
        return salt;
    }
    // A salt only has to differ between records; it is not a secret.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}:{}:{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    salt.copy_from_slice(&Sha256::digest(seed.as_bytes())[..SALT_LEN]);
    salt
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_pin(pin: &str) -> SecurityConfig {
        SecurityConfig {
            device_password: hash_pin(pin),
            ..SecurityConfig::default()
        }
    }

    #[test]
    fn pbkdf2_matches_the_published_vectors() {
        // PBKDF2-HMAC-SHA256, P = "password", S = "salt", dkLen 32.
        assert_eq!(
            to_hex(&pbkdf2(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            to_hex(&pbkdf2(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn a_pin_is_four_to_six_digits() {
        for good in ["0000", "4821", "12345", "999999"] {
            assert!(is_valid_pin(good), "{good}");
        }
        for bad in [
            "",
            "123",
            "1234567",
            "12a4",
            "12 34",
            "１２３４",
            "-123",
            "+1234",
        ] {
            assert!(!is_valid_pin(bad), "{bad}");
        }
    }

    #[test]
    fn a_hashed_pin_verifies_only_itself() {
        let security = with_pin("4821");
        assert!(security.device_password.starts_with("pin1$4$"));
        assert!(
            !security.device_password.contains("4821$"),
            "no digits at rest"
        );
        assert!(verify_device_pin(&security, "4821"));
        for wrong in [
            "4820",
            "48210",
            "482",
            "",
            "pin1",
            &security.device_password,
        ] {
            assert!(!verify_device_pin(&security, wrong), "{wrong}");
        }
    }

    #[test]
    fn every_hash_has_its_own_salt() {
        let (a, b) = (hash_pin("4821"), hash_pin("4821"));
        assert_ne!(a, b);
        let six = with_pin("482100");
        assert!(verify_device_pin(&six, "482100"));
        assert!(!verify_device_pin(&six, "4821"));
    }

    #[test]
    fn no_pin_or_a_broken_record_never_verifies() {
        assert!(!verify_device_pin(&SecurityConfig::default(), ""));
        assert!(!verify_device_pin(&SecurityConfig::default(), "0000"));
        let record = hash_pin("4821");
        let parts: Vec<&str> = record.split('$').collect();
        for broken in [
            "4821".to_string(),
            record.replacen("pin1", "pin2", 1),
            record[..record.len() - 2].to_string(),
            format!("{}${}$0${}${}", parts[0], parts[1], parts[3], parts[4]),
            format!("{}${}$+9${}${}", parts[0], parts[1], parts[3], parts[4]),
            format!(
                "{}${}$2000000${}${}",
                parts[0], parts[1], parts[3], parts[4]
            ),
            format!("{}${}${}$zz${}", parts[0], parts[1], parts[2], parts[4]),
            format!("{record}$x"),
        ] {
            let security = SecurityConfig {
                device_password: broken.clone(),
                ..SecurityConfig::default()
            };
            assert!(!verify_device_pin(&security, "4821"), "{broken}");
            assert!(!stored_value_ok(&broken), "{broken}");
        }
        assert!(stored_value_ok(&record) && stored_value_ok(""));
        // A well-formed record whose length is wrong can't be caught without
        // the PIN; it just never verifies.
        let five = format!("{}$5${}${}${}", parts[0], parts[2], parts[3], parts[4]);
        let security = SecurityConfig {
            device_password: five,
            ..SecurityConfig::default()
        };
        assert!(!verify_device_pin(&security, "4821"));
        assert!(!verify_device_pin(&security, "48210"));
    }

    #[test]
    fn the_record_tells_the_pin_length() {
        assert_eq!(pin_digits(&SecurityConfig::default()), None);
        assert_eq!(pin_digits(&with_pin("4821")), Some(4));
        assert_eq!(pin_digits(&with_pin("482100")), Some(6));
        let plain = SecurityConfig {
            device_password: "4821".into(),
            ..SecurityConfig::default()
        };
        assert_eq!(pin_digits(&plain), None, "only records count");
    }

    #[test]
    fn five_wrong_tries_wait_one_minute() {
        let security = with_pin("4821");
        let mut attempts = PinAttempts::new();
        for left in (0..PIN_MAX_TRIES).rev() {
            assert_eq!(
                attempts.check(&security, "0000", 1_000),
                PinCheck::Wrong { tries_left: left }
            );
        }
        let until = 1_000 + PIN_LOCKOUT_MS;
        assert_eq!(attempts.locked_until(1_000), Some(until));
        assert_eq!(
            attempts.check(&security, "4821", until - 1),
            PinCheck::Locked { until_ms: until },
            "the right PIN is not even checked"
        );
        assert_eq!(attempts.locked_until(until), None);
        assert_eq!(attempts.check(&security, "4821", until), PinCheck::Accepted);
    }

    #[test]
    fn the_wait_ends_with_five_new_tries_and_a_right_pin_clears_the_count() {
        let security = with_pin("4821");
        let mut attempts = PinAttempts::new();
        for _ in 0..PIN_MAX_TRIES {
            attempts.check(&security, "1111", 0);
        }
        assert_eq!(
            attempts.check(&security, "1111", PIN_LOCKOUT_MS),
            PinCheck::Wrong { tries_left: 4 }
        );
        assert_eq!(
            attempts.check(&security, "4821", PIN_LOCKOUT_MS),
            PinCheck::Accepted
        );
        assert_eq!(
            attempts.check(&security, "1111", PIN_LOCKOUT_MS),
            PinCheck::Wrong { tries_left: 4 },
            "accepted cleared the count"
        );
    }

    #[test]
    fn normalising_hashes_a_plain_pin_and_drops_anything_else() {
        let mut plain = SecurityConfig {
            device_password: "4821".into(),
            ..SecurityConfig::default()
        };
        assert!(normalise_stored(&mut plain));
        assert!(verify_device_pin(&plain, "4821"));
        let record = plain.device_password.clone();
        assert!(!normalise_stored(&mut plain), "a record stays as it is");
        assert_eq!(plain.device_password, record);

        for bad in ["hunter2", "123", "1234567", "pin1$4$garbage"] {
            let mut security = SecurityConfig {
                device_password: bad.into(),
                ..SecurityConfig::default()
            };
            assert!(normalise_stored(&mut security), "{bad}");
            assert_eq!(security.device_password, "", "{bad}");
        }
        let mut none = SecurityConfig::default();
        assert!(!normalise_stored(&mut none));
    }
}
