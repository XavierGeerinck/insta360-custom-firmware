use crate::{validate_config, RecordingFormat, UploadMode, WavedConfig};
use std::collections::{BTreeMap, BTreeSet};
use waved_state::{AudioMode, NrLevel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    Device,
    Audio,
    Webhook,
    Storage,
    Advanced,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldKind {
    Text {
        max_len: usize,
    },
    Secret {
        max_len: usize,
    },
    Url,
    Boolean,
    Select {
        options: &'static [(&'static str, &'static str)],
    },
    AudioMode,
    Integer {
        min: i64,
        max: i64,
        step: i64,
    },
    StringList {
        max_items: usize,
        item_max_len: usize,
    },
    /// The device PIN: 4 to 6 digits, write-only like `Secret`, saved as a hash.
    Pin,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldSpec {
    pub path: &'static str,
    pub section: Section,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    pub required: bool,
}

const LANGUAGE_OPTIONS: &[(&str, &str)] = &[("en", "English")];
const UPLOAD_MODE_OPTIONS: &[(&str, &str)] = &[
    ("automatic", "Automatic"),
    ("ask", "Ask after recording"),
    ("local_only", "Keep locally"),
];
const RECORDING_FORMAT_OPTIONS: &[(&str, &str)] = &[
    ("opus", "Opus (compact, about 15 MB per hour)"),
    ("flac", "FLAC (lossless, about 200 MB per hour)"),
];
const NR_LEVEL_OPTIONS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("low", "Low"),
    ("med", "Medium"),
    ("high", "High"),
];
const DIM_AFTER_OPTIONS: &[(&str, &str)] = &[
    ("0", "Never"),
    ("60", "1 min"),
    ("120", "2 min"),
    ("300", "5 min"),
    ("600", "10 min"),
];
const OFF_AFTER_OPTIONS: &[(&str, &str)] = &[
    ("0", "Never"),
    ("300", "5 min"),
    ("600", "10 min"),
    ("1800", "30 min"),
    ("3600", "1 hour"),
];

static FIELD_SPECS: &[FieldSpec] = &[
    FieldSpec {
        path: "device.name",
        section: Section::Device,
        label: "Device name",
        help: "Shown on the Wave and included in webhook metadata.",
        kind: FieldKind::Text { max_len: 64 },
        required: true,
    },
    FieldSpec {
        path: "device.language",
        section: Section::Device,
        label: "Language",
        help: "Language used by the device interface.",
        kind: FieldKind::Select {
            options: LANGUAGE_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "display.brightness_pct",
        section: Section::Device,
        label: "Screen brightness",
        help: "Backlight level in percent.",
        kind: FieldKind::Integer {
            min: 5,
            max: 100,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "display.dim_after_s",
        section: Section::Device,
        label: "Dim after",
        help: "Time without a touch before the resting screen.",
        kind: FieldKind::Select {
            options: DIM_AFTER_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "display.off_after_s",
        section: Section::Device,
        label: "Screen off after",
        help: "Never turns off while recording or in a call.",
        kind: FieldKind::Select {
            options: OFF_AFTER_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "time.zone",
        section: Section::Device,
        label: "Time zone",
        help: "IANA name, for example Europe/Brussels.",
        kind: FieldKind::Text { max_len: 64 },
        required: true,
    },
    FieldSpec {
        path: "time.clock_24h",
        section: Section::Device,
        label: "24-hour clock",
        help: "Show 14:30 instead of 2:30 pm.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "tones.enabled",
        section: Section::Device,
        label: "Alert tones",
        help: "Sounds for mute, volume and recording start and stop.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "audio.mode",
        section: Section::Audio,
        label: "Audio mode",
        help: "Microphone pickup pattern used for meetings.",
        kind: FieldKind::AudioMode,
        required: true,
    },
    FieldSpec {
        path: "audio.speaker_volume",
        section: Section::Audio,
        label: "Speaker volume",
        help: "Default speaker level.",
        kind: FieldKind::Integer {
            min: 0,
            max: 100,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "audio.mic_volume",
        section: Section::Audio,
        label: "Microphone gain",
        help: "100 is unity gain; values above 100 amplify.",
        kind: FieldKind::Integer {
            min: 0,
            max: 800,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "audio.nr_level",
        section: Section::Audio,
        label: "Noise reduction",
        help: "How much background noise to remove.",
        kind: FieldKind::Select {
            options: NR_LEVEL_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "api.endpoint",
        section: Section::Webhook,
        label: "Webhook URL",
        help: "Leave empty to queue recordings locally.",
        kind: FieldKind::Url,
        required: false,
    },
    FieldSpec {
        path: "api.auth_token",
        section: Section::Webhook,
        label: "Bearer token",
        help: "Write-only. Leave blank to keep the saved token.",
        kind: FieldKind::Secret { max_len: 4096 },
        required: false,
    },
    FieldSpec {
        path: "api.upload_mode",
        section: Section::Webhook,
        label: "After recording",
        help: "Upload automatically, ask, or keep only on the Wave.",
        kind: FieldKind::Select {
            options: UPLOAD_MODE_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "recording.pre_recording",
        section: Section::Webhook,
        label: "Pre-recording",
        help: "Keep the last 30 seconds in memory so Record includes them.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "recording.format",
        section: Section::Webhook,
        label: "Recording format",
        help: "Opus keeps uploads small; FLAC keeps every bit. Applies to the next recording.",
        kind: FieldKind::Select {
            options: RECORDING_FORMAT_OPTIONS,
        },
        required: true,
    },
    FieldSpec {
        path: "recording.ask_timeout_s",
        section: Section::Webhook,
        label: "Keep unanswered recordings after (seconds)",
        help: "With Ask, an unanswered recording is kept on the Wave after this time.",
        kind: FieldKind::Integer {
            min: 15,
            max: 600,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "api.allow_insecure_http",
        section: Section::Webhook,
        label: "Allow insecure HTTP",
        help: "Development only. Tokens and recordings are not encrypted.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "api.retry_max",
        section: Section::Webhook,
        label: "Retry attempts",
        help: "Automatic attempts in one failure cycle.",
        kind: FieldKind::Integer {
            min: 1,
            max: 100,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "api.retry_backoff_ms",
        section: Section::Webhook,
        label: "Initial retry delay (ms)",
        help: "Base delay before retrying a transient failure.",
        kind: FieldKind::Integer {
            min: 100,
            max: 86_400_000,
            step: 100,
        },
        required: true,
    },
    FieldSpec {
        path: "api.retry_max_backoff_ms",
        section: Section::Webhook,
        label: "Maximum retry delay (ms)",
        help: "Upper bound for exponential retry delay.",
        kind: FieldKind::Integer {
            min: 100,
            max: 86_400_000,
            step: 100,
        },
        required: true,
    },
    FieldSpec {
        path: "storage.prune_uploaded_at_percent",
        section: Section::Storage,
        label: "Prune uploaded recordings at",
        help: "Storage percentage that starts safe cleanup.",
        kind: FieldKind::Integer {
            min: 50,
            max: 95,
            step: 1,
        },
        required: true,
    },
    FieldSpec {
        path: "storage.minimum_free_mb",
        section: Section::Storage,
        label: "Minimum free space (MB)",
        help: "Wave removes oldest delivered recordings below this value.",
        kind: FieldKind::Integer {
            min: 128,
            max: 1_048_576,
            step: 128,
        },
        required: true,
    },
    FieldSpec {
        path: "security.device_password",
        section: Section::Advanced,
        label: "Device PIN",
        help: "4 to 6 digits. Files, Wi-Fi, Bluetooth, Uploads and System ask for it on the Wave. Write-only. Leave blank to keep the saved PIN.",
        kind: FieldKind::Pin,
        required: false,
    },
    FieldSpec {
        path: "security.ota_server",
        section: Section::Advanced,
        label: "OTA server",
        help: "Optional update service URL.",
        kind: FieldKind::Url,
        required: false,
    },
    FieldSpec {
        path: "security.release_lockdown",
        section: Section::Advanced,
        label: "Release lockdown",
        help: "Keeps developer access (adb) off at boot. Leave off on development units. This page can turn it back off without adb.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "ntp.servers",
        section: Section::Advanced,
        label: "Time servers",
        help: "One NTP hostname per line.",
        kind: FieldKind::StringList {
            max_items: 8,
            item_max_len: 253,
        },
        required: true,
    },
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormSubmission {
    pub values: BTreeMap<String, String>,
    pub clear_secrets: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldError {
    pub path: &'static str,
    pub message: String,
}

pub fn field_specs() -> &'static [FieldSpec] {
    FIELD_SPECS
}

pub fn apply_form(
    current: &WavedConfig,
    submission: &FormSubmission,
) -> Result<WavedConfig, Vec<FieldError>> {
    let mut next = current.clone();
    let mut errors = Vec::new();

    text(&submission.values, "device.name", &mut next.device.name);
    text(
        &submission.values,
        "device.language",
        &mut next.device.language,
    );
    parse_number(
        &submission.values,
        "display.brightness_pct",
        &mut next.display.brightness_pct,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "display.dim_after_s",
        &mut next.display.dim_after_s,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "display.off_after_s",
        &mut next.display.off_after_s,
        &mut errors,
    );
    text(&submission.values, "time.zone", &mut next.time.zone);
    bool_value(
        &submission.values,
        "time.clock_24h",
        &mut next.time.clock_24h,
    );
    bool_value(&submission.values, "tones.enabled", &mut next.tones.enabled);
    if let Some(value) = submission.values.get("audio.nr_level") {
        next.audio.nr_level = match value.as_str() {
            "off" => NrLevel::Off,
            "low" => NrLevel::Low,
            "med" => NrLevel::Med,
            "high" => NrLevel::High,
            _ => {
                errors.push(FieldError {
                    path: "audio.nr_level",
                    message: "unknown noise reduction level; use off, low, med or high".into(),
                });
                next.audio.nr_level.clone()
            }
        };
    }
    bool_value(
        &submission.values,
        "recording.pre_recording",
        &mut next.recording.pre_recording,
    );
    if let Some(value) = submission.values.get("recording.format") {
        next.recording.format = match value.as_str() {
            "opus" => RecordingFormat::Opus,
            "flac" => RecordingFormat::Flac,
            _ => {
                errors.push(FieldError {
                    path: "recording.format",
                    message: "unknown recording format; use opus or flac".into(),
                });
                next.recording.format
            }
        };
    }
    parse_number(
        &submission.values,
        "recording.ask_timeout_s",
        &mut next.recording.ask_timeout_s,
        &mut errors,
    );
    if let Some(value) = submission.values.get("audio.mode") {
        next.audio.mode = parse_audio_mode(value, &submission.values, &mut errors);
    }
    parse_number(
        &submission.values,
        "audio.speaker_volume",
        &mut next.audio.speaker_volume,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "audio.mic_volume",
        &mut next.audio.mic_volume,
        &mut errors,
    );
    text(&submission.values, "api.endpoint", &mut next.api.endpoint);
    secret(submission, "api.auth_token", &mut next.api.auth_token);
    if let Some(value) = submission.values.get("api.upload_mode") {
        next.api.upload_mode = match value.as_str() {
            "automatic" => UploadMode::Automatic,
            "ask" => UploadMode::Ask,
            "local_only" => UploadMode::LocalOnly,
            _ => {
                errors.push(FieldError {
                    path: "api.upload_mode",
                    message: "unknown upload mode".into(),
                });
                next.api.upload_mode
            }
        };
    }
    bool_value(
        &submission.values,
        "api.allow_insecure_http",
        &mut next.api.allow_insecure_http,
    );
    parse_number(
        &submission.values,
        "api.retry_max",
        &mut next.api.retry_max,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "api.retry_backoff_ms",
        &mut next.api.retry_backoff_ms,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "api.retry_max_backoff_ms",
        &mut next.api.retry_max_backoff_ms,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "storage.prune_uploaded_at_percent",
        &mut next.storage.prune_uploaded_at_percent,
        &mut errors,
    );
    parse_number(
        &submission.values,
        "storage.minimum_free_mb",
        &mut next.storage.minimum_free_mb,
        &mut errors,
    );
    pin(submission, &mut next.security.device_password, &mut errors);
    text(
        &submission.values,
        "security.ota_server",
        &mut next.security.ota_server,
    );
    bool_value(
        &submission.values,
        "security.release_lockdown",
        &mut next.security.release_lockdown,
    );
    if let Some(value) = submission.values.get("ntp.servers") {
        next.ntp.servers = value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
    }

    if let Err(mut validation_errors) = validate_config(&next) {
        errors.append(&mut validation_errors);
    }
    if errors.is_empty() {
        Ok(next)
    } else {
        errors.sort_by_key(|error| error.path);
        errors.dedup_by(|left, right| left.path == right.path && left.message == right.message);
        Err(errors)
    }
}

fn parse_audio_mode(
    value: &str,
    values: &BTreeMap<String, String>,
    errors: &mut Vec<FieldError>,
) -> AudioMode {
    match value {
        "omni" => AudioMode::Omni,
        "cardioid" => AudioMode::Cardioid,
        "supercardioid" => AudioMode::Supercardioid,
        "figure8" => AudioMode::Figure8,
        "stereo" => AudioMode::Stereo,
        "steered" => {
            let azimuth = values
                .get("audio.steered_initial_az_deg")
                .and_then(|value| value.parse::<f32>().ok())
                .filter(|value| (-180.0..=180.0).contains(value));
            match azimuth {
                Some(initial_az_deg) => AudioMode::Steered { initial_az_deg },
                None => {
                    errors.push(FieldError {
                        path: "audio.mode",
                        message: "steered mode requires an azimuth from -180 to 180".into(),
                    });
                    AudioMode::Omni
                }
            }
        }
        _ => {
            errors.push(FieldError {
                path: "audio.mode",
                message: "unknown audio mode".into(),
            });
            AudioMode::Omni
        }
    }
}

fn text(values: &BTreeMap<String, String>, path: &str, target: &mut String) {
    if let Some(value) = values.get(path) {
        target.clone_from(value);
    }
}

fn secret(submission: &FormSubmission, path: &str, target: &mut String) {
    if submission.clear_secrets.contains(path) {
        target.clear();
    } else if let Some(value) = submission
        .values
        .get(path)
        .filter(|value| !value.is_empty())
    {
        target.clone_from(value);
    }
}

/// Like `secret`, for the device PIN: a new value must be 4 to 6 digits and
/// is saved as a hash (`crate::pin`).
fn pin(submission: &FormSubmission, target: &mut String, errors: &mut Vec<FieldError>) {
    const PATH: &str = "security.device_password";
    if submission.clear_secrets.contains(PATH) {
        target.clear();
    } else if let Some(value) = submission
        .values
        .get(PATH)
        .filter(|value| !value.is_empty())
    {
        if crate::is_valid_pin(value) {
            *target = crate::hash_pin(value);
        } else {
            errors.push(FieldError {
                path: PATH,
                message: "Enter 4 to 6 digits".into(),
            });
        }
    }
}

fn bool_value(values: &BTreeMap<String, String>, path: &str, target: &mut bool) {
    if let Some(value) = values.get(path) {
        *target = matches!(value.as_str(), "true" | "1" | "on");
    }
}

fn number_error(path: &str) -> String {
    match FIELD_SPECS.iter().find(|spec| spec.path == path) {
        Some(FieldSpec {
            kind: FieldKind::Integer { min, max, .. },
            ..
        }) => format!("Enter a whole number from {min} to {max}"),
        _ => "Enter a whole number".into(),
    }
}

fn parse_number<T>(
    values: &BTreeMap<String, String>,
    path: &'static str,
    target: &mut T,
    errors: &mut Vec<FieldError>,
) where
    T: std::str::FromStr,
{
    if let Some(value) = values.get(path) {
        match value.parse() {
            Ok(parsed) => *target = parsed,
            Err(_) => errors.push(FieldError {
                path,
                message: number_error(path),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_supported_config_paths() {
        let expected = [
            "device.name",
            "device.language",
            "display.brightness_pct",
            "display.dim_after_s",
            "display.off_after_s",
            "time.zone",
            "time.clock_24h",
            "tones.enabled",
            "audio.mode",
            "audio.speaker_volume",
            "audio.mic_volume",
            "audio.nr_level",
            "api.endpoint",
            "api.auth_token",
            "api.upload_mode",
            "recording.pre_recording",
            "recording.format",
            "recording.ask_timeout_s",
            "api.allow_insecure_http",
            "api.retry_max",
            "api.retry_backoff_ms",
            "api.retry_max_backoff_ms",
            "storage.prune_uploaded_at_percent",
            "storage.minimum_free_mb",
            "security.device_password",
            "security.ota_server",
            "security.release_lockdown",
            "ntp.servers",
        ];
        assert_eq!(
            field_specs()
                .iter()
                .map(|spec| spec.path)
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn release_lockdown_is_a_setup_page_switch_both_ways() {
        // K1 way back: the setup page can turn release lockdown on and off
        // without adb. Default is off.
        let current = WavedConfig::default();
        assert!(!current.security.release_lockdown);

        let mut on = FormSubmission::default();
        on.values
            .insert("security.release_lockdown".into(), "true".into());
        let enabled = apply_form(&current, &on).unwrap();
        assert!(enabled.security.release_lockdown);

        let mut off = FormSubmission::default();
        off.values
            .insert("security.release_lockdown".into(), "false".into());
        let disabled = apply_form(&enabled, &off).unwrap();
        assert!(!disabled.security.release_lockdown);
    }

    #[test]
    fn blank_secret_keeps_and_clear_removes() {
        let mut current = WavedConfig::default();
        current.api.auth_token = "saved".into();
        let mut submission = FormSubmission::default();
        submission
            .values
            .insert("api.auth_token".into(), String::new());
        assert_eq!(
            apply_form(&current, &submission).unwrap().api.auth_token,
            "saved"
        );
        submission.clear_secrets.insert("api.auth_token".into());
        assert!(apply_form(&current, &submission)
            .unwrap()
            .api
            .auth_token
            .is_empty());
    }

    #[test]
    fn applies_webhook_and_steered_fields() {
        let mut submission = FormSubmission::default();
        submission
            .values
            .insert("api.endpoint".into(), "https://receiver.test/hook".into());
        submission
            .values
            .insert("api.upload_mode".into(), "ask".into());
        submission
            .values
            .insert("audio.mode".into(), "steered".into());
        submission
            .values
            .insert("audio.steered_initial_az_deg".into(), "90".into());
        let next = apply_form(&WavedConfig::default(), &submission).unwrap();
        assert_eq!(next.api.upload_mode, UploadMode::Ask);
        assert!(matches!(
            next.audio.mode,
            AudioMode::Steered { initial_az_deg } if initial_az_deg == 90.0
        ));
    }

    #[test]
    fn applies_display_time_recording_and_noise_fields() {
        let mut submission = FormSubmission::default();
        for (path, value) in [
            ("display.brightness_pct", "60"),
            ("display.dim_after_s", "300"),
            ("display.off_after_s", "1800"),
            ("time.zone", "Europe/Brussels"),
            ("time.clock_24h", "false"),
            ("tones.enabled", "false"),
            ("audio.nr_level", "med"),
            ("recording.pre_recording", "false"),
            ("recording.ask_timeout_s", "120"),
        ] {
            submission.values.insert(path.into(), value.into());
        }
        let next = apply_form(&WavedConfig::default(), &submission).unwrap();
        assert_eq!(next.display.brightness_pct, 60);
        assert_eq!(
            (next.display.dim_after_s, next.display.off_after_s),
            (300, 1800)
        );
        assert_eq!(next.time.zone, "Europe/Brussels");
        assert!(!next.time.clock_24h && !next.tones.enabled && !next.recording.pre_recording);
        assert_eq!(next.audio.nr_level, NrLevel::Med);
        assert_eq!(next.recording.ask_timeout_s, 120);
    }

    #[test]
    fn the_pin_field_takes_four_to_six_digits_and_saves_a_hash() {
        for bad in ["123", "1234567", "12a4", "12 34", "hunter2"] {
            let errors = submit(&[("security.device_password", bad)]).unwrap_err();
            let error = errors
                .iter()
                .find(|error| error.path == "security.device_password")
                .unwrap_or_else(|| panic!("{bad}: {errors:?}"));
            assert_eq!(error.message, "Enter 4 to 6 digits");
        }
        let next = submit(&[("security.device_password", "4821")]).unwrap();
        assert!(next.security.device_password.starts_with("pin1$4$"));
        assert!(crate::verify_device_pin(&next.security, "4821"));

        // Blank keeps the saved PIN, Clear removes it.
        let mut submission = FormSubmission::default();
        submission
            .values
            .insert("security.device_password".into(), String::new());
        assert_eq!(apply_form(&next, &submission).unwrap(), next);
        submission
            .clear_secrets
            .insert("security.device_password".into());
        let cleared = apply_form(&next, &submission).unwrap();
        assert_eq!(cleared.security.device_password, "");
    }

    #[test]
    fn the_pin_field_is_a_numeric_pin() {
        let spec = field_specs()
            .iter()
            .find(|spec| spec.path == "security.device_password")
            .unwrap();
        assert_eq!(spec.kind, FieldKind::Pin);
        assert!(spec.help.contains("4 to 6 digits"), "{}", spec.help);
    }

    #[test]
    fn rejects_an_unknown_noise_level() {
        let mut submission = FormSubmission::default();
        submission
            .values
            .insert("audio.nr_level".into(), "max".into());
        let errors = apply_form(&WavedConfig::default(), &submission).unwrap_err();
        assert!(errors.iter().any(|error| error.path == "audio.nr_level"));
    }

    fn submit(pairs: &[(&str, &str)]) -> Result<WavedConfig, Vec<FieldError>> {
        let mut submission = FormSubmission::default();
        for (path, value) in pairs {
            submission.values.insert((*path).into(), (*value).into());
        }
        apply_form(&WavedConfig::default(), &submission)
    }

    fn rejects_path(pairs: &[(&str, &str)], path: &str) {
        let errors = submit(pairs).unwrap_err();
        assert!(
            errors.iter().any(|error| error.path == path),
            "expected an error for {path}, got {errors:?}"
        );
    }

    #[test]
    fn rejects_invalid_display_and_time_values() {
        rejects_path(&[("time.zone", "../etc/passwd")], "time.zone");
        rejects_path(&[("time.zone", "-UTC")], "time.zone");
        rejects_path(&[("display.brightness_pct", "0")], "display.brightness_pct");
        rejects_path(
            &[
                ("display.dim_after_s", "600"),
                ("display.off_after_s", "300"),
            ],
            "display.off_after_s",
        );
        rejects_path(
            &[("recording.ask_timeout_s", "5")],
            "recording.ask_timeout_s",
        );
    }

    #[test]
    fn out_of_range_numbers_name_the_range() {
        for value in ["300", "-5", "abc"] {
            let errors = submit(&[("display.brightness_pct", value)]).unwrap_err();
            let error = errors
                .iter()
                .find(|error| error.path == "display.brightness_pct")
                .unwrap();
            assert_eq!(error.message, "Enter a whole number from 5 to 100");
        }
    }

    #[test]
    fn noise_level_error_lists_allowed_values() {
        let errors = submit(&[("audio.nr_level", "max")]).unwrap_err();
        assert!(errors[0].message.contains("off, low, med or high"));
    }

    #[test]
    fn the_recording_format_is_a_select_of_opus_and_flac() {
        let spec = field_specs()
            .iter()
            .find(|spec| spec.path == "recording.format")
            .unwrap();
        assert_eq!(
            spec.kind,
            FieldKind::Select {
                options: RECORDING_FORMAT_OPTIONS
            }
        );
        let values: Vec<&str> = RECORDING_FORMAT_OPTIONS.iter().map(|option| option.0).collect();
        assert_eq!(values, ["opus", "flac"]);
        let next = submit(&[("recording.format", "flac")]).unwrap();
        assert_eq!(next.recording.format, RecordingFormat::Flac);
        let back = submit(&[("recording.format", "opus")]).unwrap();
        assert_eq!(back.recording.format, RecordingFormat::Opus);
        rejects_path(&[("recording.format", "wav")], "recording.format");
    }

    #[test]
    fn omitted_new_keys_leave_values_unchanged() {
        let mut current = WavedConfig::default();
        current.display.brightness_pct = 42;
        current.display.dim_after_s = 900;
        current.display.off_after_s = 7200;
        current.time.zone = "Asia/Tokyo".into();
        current.time.clock_24h = !current.time.clock_24h;
        current.tones.enabled = !current.tones.enabled;
        current.audio.nr_level = NrLevel::High;
        current.recording.pre_recording = !current.recording.pre_recording;
        current.recording.ask_timeout_s = 200;
        current.recording.format = RecordingFormat::Flac;
        let mut submission = FormSubmission::default();
        submission
            .values
            .insert("device.name".into(), "Office".into());
        let next = apply_form(&current, &submission).unwrap();
        assert_eq!(next.display, current.display);
        assert_eq!(next.time, current.time);
        assert_eq!(next.tones, current.tones);
        assert_eq!(next.audio.nr_level, current.audio.nr_level);
        assert_eq!(next.recording, current.recording);
    }
}
