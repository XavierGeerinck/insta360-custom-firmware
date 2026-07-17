use crate::{validate_config, UploadMode, WavedConfig};
use std::collections::{BTreeMap, BTreeSet};
use waved_state::AudioMode;

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
            max: 200,
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
        label: "Device password",
        help: "Write-only. Leave blank to keep the saved password.",
        kind: FieldKind::Secret { max_len: 256 },
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
    secret(
        submission,
        "security.device_password",
        &mut next.security.device_password,
    );
    text(
        &submission.values,
        "security.ota_server",
        &mut next.security.ota_server,
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

fn bool_value(values: &BTreeMap<String, String>, path: &str, target: &mut bool) {
    if let Some(value) = values.get(path) {
        *target = matches!(value.as_str(), "true" | "1" | "on");
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
                message: "must be a number".into(),
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
            "audio.mode",
            "audio.speaker_volume",
            "audio.mic_volume",
            "api.endpoint",
            "api.auth_token",
            "api.upload_mode",
            "api.allow_insecure_http",
            "api.retry_max",
            "api.retry_backoff_ms",
            "api.retry_max_backoff_ms",
            "storage.prune_uploaded_at_percent",
            "storage.minimum_free_mb",
            "security.device_password",
            "security.ota_server",
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
}
