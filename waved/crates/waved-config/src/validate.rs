use crate::{FieldError, UploadMode, WavedConfig};
use url::Url;

pub fn validate_config(config: &WavedConfig) -> Result<(), Vec<FieldError>> {
    let mut errors = Vec::new();
    bounded_text(&mut errors, "device.name", &config.device.name, 1, 64);
    bounded_text(
        &mut errors,
        "device.language",
        &config.device.language,
        2,
        16,
    );

    if config.audio.speaker_volume > 200 {
        push(
            &mut errors,
            "audio.speaker_volume",
            "must be between 0 and 200",
        );
    }
    if config.audio.mic_volume > 800 {
        push(&mut errors, "audio.mic_volume", "must be between 0 and 800");
    }

    validate_endpoint(config, &mut errors);
    bounded_secret(&mut errors, "api.auth_token", &config.api.auth_token, 4096);
    if config.api.retry_max == 0 || config.api.retry_max > 100 {
        push(&mut errors, "api.retry_max", "must be between 1 and 100");
    }
    if !(100..=86_400_000).contains(&config.api.retry_backoff_ms) {
        push(
            &mut errors,
            "api.retry_backoff_ms",
            "must be between 100 and 86400000",
        );
    }
    if config.api.retry_max_backoff_ms < config.api.retry_backoff_ms
        || config.api.retry_max_backoff_ms > 86_400_000
    {
        push(
            &mut errors,
            "api.retry_max_backoff_ms",
            "must be at least retry_backoff_ms and no more than 86400000",
        );
    }

    if !(50..=95).contains(&config.storage.prune_uploaded_at_percent) {
        push(
            &mut errors,
            "storage.prune_uploaded_at_percent",
            "must be between 50 and 95",
        );
    }
    if !(128..=1_048_576).contains(&config.storage.minimum_free_mb) {
        push(
            &mut errors,
            "storage.minimum_free_mb",
            "must be between 128 and 1048576",
        );
    }

    bounded_secret(
        &mut errors,
        "security.device_password",
        &config.security.device_password,
        256,
    );
    if !config.security.ota_server.is_empty() {
        validate_absolute_url(
            &mut errors,
            "security.ota_server",
            &config.security.ota_server,
            true,
        );
    }

    if config.ntp.servers.is_empty() || config.ntp.servers.len() > 8 {
        push(
            &mut errors,
            "ntp.servers",
            "must contain between 1 and 8 servers",
        );
    }
    for server in &config.ntp.servers {
        if server.is_empty()
            || server.len() > 253
            || server
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            push(
                &mut errors,
                "ntp.servers",
                "server names must be non-empty and contain no whitespace",
            );
            break;
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_endpoint(config: &WavedConfig, errors: &mut Vec<FieldError>) {
    if config.api.endpoint.is_empty() {
        return;
    }
    let Some(url) = validate_absolute_url(errors, "api.endpoint", &config.api.endpoint, false)
    else {
        return;
    };
    let scheme_allowed =
        url.scheme() == "https" || (config.api.allow_insecure_http && url.scheme() == "http");
    if !scheme_allowed {
        push(
            errors,
            "api.endpoint",
            "must use HTTPS unless insecure HTTP is explicitly enabled",
        );
    }
    if !url.username().is_empty() || url.password().is_some() {
        push(
            errors,
            "api.endpoint",
            "must not contain embedded credentials",
        );
    }
    if matches!(config.api.upload_mode, UploadMode::Automatic) {
        // Automatic with an empty endpoint is intentionally allowed and queues
        // locally. A non-empty endpoint has already been validated above.
    }
}

fn validate_absolute_url(
    errors: &mut Vec<FieldError>,
    path: &'static str,
    value: &str,
    allow_http: bool,
) -> Option<Url> {
    let url = match Url::parse(value) {
        Ok(url) if url.has_host() => url,
        _ => {
            push(errors, path, "must be an absolute URL with a host");
            return None;
        }
    };
    if allow_http && !matches!(url.scheme(), "https" | "http") {
        push(errors, path, "must use HTTP or HTTPS");
    }
    Some(url)
}

fn bounded_text(
    errors: &mut Vec<FieldError>,
    path: &'static str,
    value: &str,
    min: usize,
    max: usize,
) {
    let len = value.chars().count();
    if len < min || len > max || has_forbidden(value) {
        push(
            errors,
            path,
            &format!("must contain {min} to {max} safe characters"),
        );
    }
}

fn bounded_secret(errors: &mut Vec<FieldError>, path: &'static str, value: &str, max: usize) {
    if value.chars().count() > max || has_forbidden(value) {
        push(
            errors,
            path,
            &format!("must contain at most {max} safe characters"),
        );
    }
}

fn has_forbidden(value: &str) -> bool {
    value.contains(['\0', '\r', '\n'])
}

fn push(errors: &mut Vec<FieldError>, path: &'static str, message: &str) {
    errors.push(FieldError {
        path,
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WavedConfig;

    #[test]
    fn empty_endpoint_is_a_valid_disabled_transport() {
        assert!(validate_config(&WavedConfig::default()).is_ok());
    }

    #[test]
    fn http_requires_explicit_opt_in() {
        let mut config = WavedConfig::default();
        config.api.endpoint = "http://receiver.test/hook".into();
        assert!(validate_config(&config).is_err());
        config.api.allow_insecure_http = true;
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn endpoint_rejects_embedded_credentials() {
        let mut config = WavedConfig::default();
        config.api.endpoint = "https://user:pass@receiver.test/hook".into();
        let errors = validate_config(&config).unwrap_err();
        assert!(errors.iter().any(|error| error.path == "api.endpoint"));
    }
}
