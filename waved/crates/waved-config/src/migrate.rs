use crate::{ConfigError, CURRENT_CONFIG_VERSION};

pub(crate) fn migrate(mut value: toml::Value) -> Result<toml::Value, ConfigError> {
    let root = value
        .as_table_mut()
        .ok_or_else(|| ConfigError::Validation("root must be a TOML table".into()))?;
    let version = root
        .get("config_version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0);

    if version > CURRENT_CONFIG_VERSION as i64 {
        return Err(ConfigError::UnsupportedVersion(version));
    }
    if version < 0 {
        return Err(ConfigError::UnsupportedVersion(version));
    }

    if version == 0 {
        migrate_v0_to_v1(root);
    }
    if version <= 1 {
        migrate_v1_to_v2(root);
    }
    root.insert(
        "config_version".into(),
        toml::Value::Integer(CURRENT_CONFIG_VERSION as i64),
    );
    Ok(value)
}

fn migrate_v0_to_v1(root: &mut toml::map::Map<String, toml::Value>) {
    if let Some(audio) = root.get_mut("audio").and_then(toml::Value::as_table_mut) {
        if audio.get("mode").and_then(toml::Value::as_str) == Some("noise_reduction") {
            audio.insert("mode".into(), toml::Value::String("omni".into()));
        }
    }

    if let Some(api) = table_or_insert(root, "api") {
        migrate_v0_api(api);
    }
    if let Some(storage) = table_or_insert(root, "storage") {
        storage
            .entry("prune_uploaded_at_percent")
            .or_insert(toml::Value::Integer(85));
        storage
            .entry("minimum_free_mb")
            .or_insert(toml::Value::Integer(2_048));
    }
}

fn migrate_v0_api(api: &mut toml::map::Map<String, toml::Value>) {
    let upload_mode = api
        .remove("auto_upload")
        .and_then(|value| value.as_bool())
        .map(|enabled| if enabled { "automatic" } else { "ask" })
        .unwrap_or("automatic");
    api.entry("upload_mode")
        .or_insert_with(|| toml::Value::String(upload_mode.into()));
    api.entry("allow_insecure_http")
        .or_insert(toml::Value::Boolean(false));
    api.entry("retry_max_backoff_ms")
        .or_insert(toml::Value::Integer(60_000));
}

fn migrate_v1_to_v2(root: &mut toml::map::Map<String, toml::Value>) {
    // v1 allowed speaker_volume 0..=200 but never applied it; v2 is a percent.
    if let Some(audio) = root.get_mut("audio").and_then(toml::Value::as_table_mut) {
        if let Some(volume) = audio
            .get("speaker_volume")
            .and_then(toml::Value::as_integer)
        {
            audio.insert(
                "speaker_volume".into(),
                toml::Value::Integer(volume.clamp(0, 100)),
            );
        }
    }
    // Anyone with a v1 file has already used the device: skip first start.
    if let Some(device) = table_or_insert(root, "device") {
        device.insert("first_run_done".into(), toml::Value::Boolean(true));
    }
}

/// The named section as a table, created when missing. A section of the wrong
/// type yields `None`, so serde reports the parse error instead of a panic.
fn table_or_insert<'a>(
    root: &'a mut toml::map::Map<String, toml::Value>,
    key: &str,
) -> Option<&'a mut toml::map::Map<String, toml::Value>> {
    root.entry(key)
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
}
