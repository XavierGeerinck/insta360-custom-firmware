use crate::http::{html_escape, response};
use waved_config::{
    field_specs, FieldError, FieldKind, Section, UploadMode, WavedConfig, PIN_MAX_DIGITS,
    PIN_MIN_DIGITS,
};
use waved_state::{AudioMode, NrLevel, RecordingFormat};

const OTHER_NETWORK_VALUE: &str = "__other_network__";

const STYLE: &str = r#"
:root{color-scheme:light;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:#eef3f3;color:#17222b}
*{box-sizing:border-box}
body{margin:0;padding:max(20px,env(safe-area-inset-top)) 14px max(28px,env(safe-area-inset-bottom));background:linear-gradient(145deg,#f8fafc,#eaf2f0)}
main{width:min(100%,620px);margin:auto}
.brand{display:flex;align-items:center;gap:10px;margin:4px 2px 22px;color:#52636a;font-size:12px;font-weight:800;letter-spacing:.15em;text-transform:uppercase}
.mark{width:32px;height:32px;display:grid;place-items:center;border:2px solid #087f73;border-radius:50%;color:#087f73}
h1{margin:0 0 8px;font-size:34px;letter-spacing:-.035em}
.intro{margin:0 0 24px;color:#617078;line-height:1.5}
fieldset{margin:0 0 16px;padding:22px 20px 24px;border:1px solid #dce5e6;border-radius:20px;background:#fff;box-shadow:0 10px 30px rgba(20,42,48,.07)}
legend{padding:0 7px;color:#087f73;font-size:13px;font-weight:800;letter-spacing:.08em;text-transform:uppercase}
.field{margin-top:18px}.field:first-of-type{margin-top:4px}
label{display:block;margin-bottom:7px;color:#34464e;font-size:13px;font-weight:750}
.help,.configured{margin:7px 1px 0;color:#718087;font-size:12px;line-height:1.4}
.configured{color:#087f73}
input,select,textarea{width:100%;min-height:50px;padding:12px 14px;border:1px solid #cbd7da;border-radius:12px;background:#fff;color:#17222b;font:inherit;font-size:16px;outline:none}
textarea{min-height:96px;resize:vertical}
input:focus-visible,select:focus-visible,textarea:focus-visible,button:focus-visible{border-color:#087f73;box-shadow:0 0 0 4px rgba(8,127,115,.14);outline:none}
.check{display:flex;align-items:center;gap:10px}.check input{width:20px;min-height:20px;margin:0}.check label{margin:0}
.error{margin:7px 0 0;color:#b42318;font-size:12px;font-weight:700}
.global-error{margin:0 0 18px;padding:14px;border:1px solid #f1b8b2;border-radius:13px;background:#fff4f2;color:#8f2118}
.other-network[hidden],.steered[hidden]{display:none}
.password{display:grid;grid-template-columns:1fr auto;gap:9px}.password button,.secondary{border:0;border-radius:11px;background:#eaf3f1;color:#087f73;font:inherit;font-weight:750;padding:0 15px}
.actions{display:grid;gap:10px;margin-top:18px}
.primary{min-height:54px;border:0;border-radius:14px;background:#087f73;color:#fff;font:inherit;font-size:16px;font-weight:800;box-shadow:0 10px 24px rgba(8,127,115,.2)}
.secondary-action{min-height:50px;border:1px solid #9bbab5;border-radius:14px;background:#fff;color:#087f73;font:inherit;font-size:15px;font-weight:800}
.security{margin:18px 3px;color:#718087;font-size:12px;line-height:1.5}
@media(max-width:460px){body{padding-left:10px;padding-right:10px}fieldset{padding:20px 16px;border-radius:17px}h1{font-size:30px}}
"#;

const SCRIPT: &str = r#"
const ssid=document.getElementById('ssid');
const other=document.getElementById('other-network-field');
const otherInput=document.getElementById('other_ssid');
const mode=document.querySelector('[name="audio.mode"]');
const steered=document.getElementById('steered-field');
function updateNetwork(){const show=ssid.value==='__other_network__';other.hidden=!show;otherInput.disabled=!show}
function updateMode(){if(mode)steered.hidden=mode.value!=='steered'}
ssid.addEventListener('change',updateNetwork);if(mode)mode.addEventListener('change',updateMode);updateNetwork();updateMode();
document.querySelectorAll('[data-secret-toggle]').forEach(button=>button.addEventListener('click',()=>{const input=document.getElementById(button.dataset.secretToggle);const showing=input.type==='text';input.type=showing?'password':'text';button.textContent=showing?'Show':'Hide'}));
document.getElementById('setup-form').addEventListener('submit',event=>{const button=event.submitter;if(button){button.disabled=true;button.textContent='Saving…'}});
"#;

pub fn render_page(
    config: &WavedConfig,
    ssids: &[String],
    csrf: &str,
    errors: &[FieldError],
    global_error: Option<&str>,
) -> String {
    let mut fields = String::new();
    for section in [
        Section::Device,
        Section::Audio,
        Section::Webhook,
        Section::Storage,
        Section::Advanced,
    ] {
        fields.push_str("<fieldset><legend>");
        fields.push_str(section_name(section));
        fields.push_str("</legend>");
        for spec in field_specs().iter().filter(|spec| spec.section == section) {
            fields.push_str(&render_field(config, spec, errors));
        }
        fields.push_str("</fieldset>");
    }

    let options: String = ssids
        .iter()
        .map(|ssid| format!("<option value=\"{0}\">{0}</option>", html_escape(ssid)))
        .collect();
    let global_error = global_error
        .map(|message| {
            format!(
                "<div class=\"global-error\" role=\"alert\">{}</div>",
                html_escape(message)
            )
        })
        .unwrap_or_default();
    let content = format!(
        r#"<main><div class="brand"><span class="mark">W</span><span>Wave</span></div><h1>Device setup</h1><p class="intro">Configure your Wave, webhook, storage, and Wi-Fi. Settings are saved directly on this device.</p>{global_error}<form id="setup-form" method="post"><input type="hidden" name="csrf" value="{}">{fields}<fieldset><legend>Wi-Fi</legend><p class="help">Leave these fields empty to reconnect using a saved network.</p><div class="field"><label for="ssid">Network</label><select id="ssid" name="ssid"><option value="">Use a saved network</option>{options}<option value="{OTHER_NETWORK_VALUE}">Other network…</option></select></div><div class="field other-network" id="other-network-field" hidden><label for="other_ssid">Network name</label><input id="other_ssid" name="other_ssid" disabled maxlength="32" autocomplete="off"></div><div class="field"><label for="psk">Wi-Fi password</label><div class="password"><input id="psk" name="psk" type="password" minlength="8" maxlength="63" autocomplete="new-password"><button type="button" data-secret-toggle="psk">Show</button></div></div><div class="actions"><button class="primary" type="submit" formaction="/save-connect">Save and connect to selected network</button><button class="secondary-action" type="submit" formaction="/save">Save settings and reconnect</button></div></fieldset></form><p class="security">The temporary setup network is WPA2-protected. Saved secrets are never shown again. The setup network disappears while Wave reconnects.</p></main>"#,
        html_escape(csrf)
    );
    document("Wave Device Setup", &content, Some(SCRIPT))
}

/// `kept_as` is the file name an unreadable config.toml was moved to.
pub fn render_ack(ssid: Option<&str>, webhook_test: bool, kept_as: Option<&str>) -> String {
    let target = ssid
        .map(|value| format!("Connecting to <strong>{}</strong>.", html_escape(value)))
        .unwrap_or_else(|| "Reconnecting to a saved network.".into());
    let test = if webhook_test {
        " After reconnecting, Wave will send a small test webhook."
    } else {
        ""
    };
    let kept = kept_as
        .map(|name| {
            format!(
                "<p class=\"intro\">Wave couldn't read the old settings file, so it kept that file as <strong>{}</strong> and saved the settings you submitted. Anything you didn't change on this page came from the last working backup or the defaults.</p>",
                html_escape(name)
            )
        })
        .unwrap_or_default();
    let content = format!(
        r#"<main><div class="brand"><span class="mark">W</span><span>Wave</span></div><fieldset><legend>Settings saved</legend><h1>Wave is reconnecting</h1><p class="intro">{target}{test}</p>{kept}<p class="security">The Wave setup network will now disappear. You can close this page and follow progress on the Wave screen.</p></fieldset></main>"#
    );
    document("Wave is reconnecting", &content, None)
}

fn render_field(
    config: &WavedConfig,
    spec: &waved_config::FieldSpec,
    errors: &[FieldError],
) -> String {
    let id = spec.path.replace('.', "_");
    let value = value(config, spec.path);
    let error = errors
        .iter()
        .find(|error| error.path == spec.path)
        .map(|error| {
            format!(
                "<p class=\"error\" role=\"alert\">{}</p>",
                html_escape(&error.message)
            )
        })
        .unwrap_or_default();
    let required = if spec.required { " required" } else { "" };
    let control = match spec.kind {
        FieldKind::Text { max_len } => format!(
            "<input id=\"{id}\" name=\"{}\" value=\"{}\" maxlength=\"{max_len}\"{required}>",
            spec.path,
            html_escape(&value)
        ),
        FieldKind::Secret { max_len } => {
            secret_control(config, spec.path, &id, &format!("maxlength=\"{max_len}\""))
        }
        FieldKind::Pin => secret_control(
            config,
            spec.path,
            &id,
            &format!(
                "inputmode=\"numeric\" pattern=\"[0-9]{{{PIN_MIN_DIGITS},{PIN_MAX_DIGITS}}}\" minlength=\"{PIN_MIN_DIGITS}\" maxlength=\"{PIN_MAX_DIGITS}\""
            ),
        ),
        FieldKind::Url => format!(
            "<input id=\"{id}\" name=\"{}\" type=\"url\" value=\"{}\"{required}>",
            spec.path,
            html_escape(&value)
        ),
        FieldKind::Boolean => format!(
            "<input type=\"hidden\" name=\"{}\" value=\"false\"><div class=\"check\"><input id=\"{id}\" name=\"{}\" type=\"checkbox\" value=\"true\"{}><label for=\"{id}\">Enabled</label></div>",
            spec.path,
            spec.path,
            if value == "true" { " checked" } else { "" }
        ),
        FieldKind::Select { options: choices } => {
            let mut options: String = choices
                .iter()
                .map(|(option, label)| {
                    format!(
                        "<option value=\"{option}\"{}>{}</option>",
                        if value == *option { " selected" } else { "" },
                        html_escape(label)
                    )
                })
                .collect();
            // A saved value outside the fixed list must stay selected, or the
            // browser would submit the first option and silently replace it.
            if !value.is_empty() && !choices.iter().any(|(option, _)| value == *option) {
                options.push_str(&format!(
                    "<option value=\"{}\" selected>{}</option>",
                    html_escape(&value),
                    html_escape(&saved_option_label(spec.path, &value))
                ));
            }
            format!(
                "<select id=\"{id}\" name=\"{}\"{required}>{options}</select>",
                spec.path
            )
        }
        FieldKind::AudioMode => render_audio_mode(config, &id),
        FieldKind::Integer { min, max, step } => format!(
            "<input id=\"{id}\" name=\"{}\" type=\"number\" value=\"{}\" min=\"{min}\" max=\"{max}\" step=\"{step}\"{required}>",
            spec.path,
            html_escape(&value)
        ),
        FieldKind::StringList { .. } => format!(
            "<textarea id=\"{id}\" name=\"{}\"{required}>{}</textarea>",
            spec.path,
            html_escape(&value)
        ),
    };
    format!(
        "<div class=\"field\"><label for=\"{id}\">{}</label>{control}<p class=\"help\">{}</p>{error}</div>",
        html_escape(spec.label),
        html_escape(spec.help)
    )
}

/// A write-only input with Show and "Clear saved value"; `limits` are the
/// input's length (and pattern) attributes.
fn secret_control(config: &WavedConfig, path: &str, id: &str, limits: &str) -> String {
    let configured = if secret_configured(config, path) {
        "<p class=\"configured\">A value is saved. Leave blank to keep it.</p>"
    } else {
        ""
    };
    format!(
        "<div class=\"password\"><input id=\"{id}\" name=\"{path}\" type=\"password\" value=\"\" {limits} autocomplete=\"new-password\"><button type=\"button\" data-secret-toggle=\"{id}\">Show</button></div>{configured}<div class=\"check\"><input id=\"clear_{id}\" name=\"clear.{path}\" type=\"checkbox\" value=\"true\"><label for=\"clear_{id}\">Clear saved value</label></div>"
    )
}

fn render_audio_mode(config: &WavedConfig, id: &str) -> String {
    let (selected, azimuth) = match config.audio.mode {
        AudioMode::Omni => ("omni", 0.0),
        AudioMode::Cardioid => ("cardioid", 0.0),
        AudioMode::Supercardioid => ("supercardioid", 0.0),
        AudioMode::Figure8 => ("figure8", 0.0),
        AudioMode::Stereo => ("stereo", 0.0),
        AudioMode::Steered { initial_az_deg } => ("steered", initial_az_deg),
    };
    let options = [
        ("omni", "Omni"),
        ("cardioid", "Cardioid"),
        ("supercardioid", "Supercardioid"),
        ("figure8", "Figure 8"),
        ("stereo", "Stereo"),
        ("steered", "Steered"),
    ]
    .iter()
    .map(|(value, label)| {
        format!(
            "<option value=\"{value}\"{}>{label}</option>",
            if selected == *value { " selected" } else { "" }
        )
    })
    .collect::<String>();
    format!(
        "<select id=\"{id}\" name=\"audio.mode\" required>{options}</select><div class=\"field steered\" id=\"steered-field\"{}><label for=\"audio_steered_initial_az_deg\">Initial azimuth</label><input id=\"audio_steered_initial_az_deg\" name=\"audio.steered_initial_az_deg\" type=\"number\" min=\"-180\" max=\"180\" step=\"1\" value=\"{azimuth}\"></div>",
        if selected == "steered" { "" } else { " hidden" }
    )
}

fn saved_option_label(path: &str, value: &str) -> String {
    let seconds = matches!(path, "display.dim_after_s" | "display.off_after_s")
        .then(|| value.parse::<u32>().ok())
        .flatten();
    match seconds {
        Some(s) if s < 60 || s % 60 != 0 => format!("{s} seconds"),
        Some(s) if s < 3600 || s % 3600 != 0 => format!("{} min", s / 60),
        Some(s) => format!("{} hour{}", s / 3600, if s == 3600 { "" } else { "s" }),
        None => value.to_string(),
    }
}

fn value(config: &WavedConfig, path: &str) -> String {
    match path {
        "device.name" => config.device.name.clone(),
        "device.language" => config.device.language.clone(),
        "audio.mode" => String::new(),
        "audio.speaker_volume" => config.audio.speaker_volume.to_string(),
        "audio.mic_volume" => config.audio.mic_volume.to_string(),
        "api.endpoint" => config.api.endpoint.clone(),
        "api.auth_token" => String::new(),
        "api.upload_mode" => match config.api.upload_mode {
            UploadMode::Automatic => "automatic",
            UploadMode::Ask => "ask",
            UploadMode::LocalOnly => "local_only",
        }
        .into(),
        "api.allow_insecure_http" => config.api.allow_insecure_http.to_string(),
        "api.retry_max" => config.api.retry_max.to_string(),
        "api.retry_backoff_ms" => config.api.retry_backoff_ms.to_string(),
        "api.retry_max_backoff_ms" => config.api.retry_max_backoff_ms.to_string(),
        "storage.prune_uploaded_at_percent" => config.storage.prune_uploaded_at_percent.to_string(),
        "storage.minimum_free_mb" => config.storage.minimum_free_mb.to_string(),
        "security.device_password" => String::new(),
        "security.ota_server" => config.security.ota_server.clone(),
        "security.release_lockdown" => config.security.release_lockdown.to_string(),
        "ntp.servers" => config.ntp.servers.join("\n"),
        "display.brightness_pct" => config.display.brightness_pct.to_string(),
        "display.dim_after_s" => config.display.dim_after_s.to_string(),
        "display.off_after_s" => config.display.off_after_s.to_string(),
        "time.zone" => config.time.zone.clone(),
        "time.clock_24h" => config.time.clock_24h.to_string(),
        "tones.enabled" => config.tones.enabled.to_string(),
        "audio.nr_level" => match config.audio.nr_level {
            NrLevel::Off => "off",
            NrLevel::Low => "low",
            NrLevel::Med => "med",
            NrLevel::High => "high",
        }
        .into(),
        "recording.pre_recording" => config.recording.pre_recording.to_string(),
        "recording.format" => match config.recording.format {
            RecordingFormat::Opus => "opus",
            RecordingFormat::Flac => "flac",
        }
        .into(),
        "recording.ask_timeout_s" => config.recording.ask_timeout_s.to_string(),
        _ => String::new(),
    }
}

fn secret_configured(config: &WavedConfig, path: &str) -> bool {
    match path {
        "api.auth_token" => !config.api.auth_token.is_empty(),
        "security.device_password" => !config.security.device_password.is_empty(),
        _ => false,
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::Device => "Device",
        Section::Audio => "Audio",
        Section::Webhook => "Webhook",
        Section::Storage => "Storage",
        Section::Advanced => "Advanced",
    }
}

fn document(title: &str, content: &str, script: Option<&str>) -> String {
    let script = script
        .map(|source| format!("<script>{source}</script>"))
        .unwrap_or_default();
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><meta name=\"theme-color\" content=\"#eef3f3\"><title>{title}</title><style>{STYLE}</style></head><body>{content}{script}</body></html>"
    );
    response("200 OK", "text/html; charset=utf-8", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_fields_without_saved_secrets() {
        let mut config = WavedConfig::default();
        config.api.auth_token = "never-render-me".into();
        config.security.device_password = "also-secret".into();
        config.api.endpoint = "https://receiver.test/hook?a=1&b=2".into();
        let page = render_page(&config, &["Home <5G>".into()], "csrf-token", &[], None);
        for spec in field_specs() {
            assert!(page.contains(&format!("name=\"{}\"", spec.path)));
        }
        assert!(!page.contains("never-render-me"));
        assert!(!page.contains("also-secret"));
        assert!(page.contains("A value is saved"));
        assert!(page.contains("Home &lt;5G&gt;"));
        assert!(page.contains("csrf-token"));
        assert!(page.contains("https://receiver.test/hook?a=1&amp;b=2"));
        assert!(page.contains("formaction=\"/save\""));
        assert!(page.contains("formaction=\"/save-connect\""));
    }

    #[test]
    fn the_pin_field_takes_digits_and_never_shows_the_saved_pin() {
        let mut config = WavedConfig::default();
        let page = render_page(&config, &[], "csrf", &[], None);
        let field = page
            .split("<div class=\"field\">")
            .find(|field| field.contains("name=\"security.device_password\""))
            .expect("the PIN field");
        for attribute in [
            "type=\"password\"",
            "inputmode=\"numeric\"",
            "pattern=\"[0-9]{4,6}\"",
            "minlength=\"4\"",
            "maxlength=\"6\"",
            "value=\"\"",
            "name=\"clear.security.device_password\"",
        ] {
            assert!(field.contains(attribute), "{attribute} in {field}");
        }
        assert!(!field.contains("A value is saved"));

        config.security.device_password = waved_config::hash_pin("4821");
        let page = render_page(&config, &[], "csrf", &[], None);
        assert!(!page.contains("pin1$"), "the record stays on the Wave");
        let field = page
            .split("<div class=\"field\">")
            .find(|field| field.contains("name=\"security.device_password\""))
            .unwrap();
        assert!(field.contains("A value is saved"));
    }

    #[test]
    fn renders_field_and_global_errors_safely() {
        let errors = vec![FieldError {
            path: "api.endpoint",
            message: "bad <endpoint>".into(),
        }];
        let page = render_page(
            &WavedConfig::default(),
            &[],
            "token",
            &errors,
            Some("write <failed>"),
        );
        assert!(page.contains("bad &lt;endpoint&gt;"));
        assert!(page.contains("write &lt;failed&gt;"));
        assert!(!page.contains("bad <endpoint>"));
    }

    #[test]
    fn every_registry_field_renders_its_saved_value() {
        let config = WavedConfig::default();
        for spec in field_specs() {
            let may_be_blank = matches!(spec.kind, FieldKind::Secret { .. } | FieldKind::Pin)
                || matches!(
                    spec.path,
                    "audio.mode" | "api.endpoint" | "security.ota_server"
                );
            if !may_be_blank {
                assert!(
                    !value(&config, spec.path).is_empty(),
                    "no value for {}",
                    spec.path
                );
            }
        }
    }

    #[test]
    fn saved_value_outside_the_option_list_stays_selected() {
        let mut config = WavedConfig::default();
        config.display.dim_after_s = 900;
        config.display.off_after_s = 7200;
        let page = render_page(&config, &[], "csrf", &[], None);
        assert!(page.contains("<option value=\"900\" selected>15 min</option>"));
        assert!(page.contains("<option value=\"7200\" selected>2 hours</option>"));
        // Listed values are not duplicated.
        let page = render_page(&WavedConfig::default(), &[], "csrf", &[], None);
        assert!(page.contains("<option value=\"120\" selected>2 min</option>"));
        assert_eq!(page.matches("value=\"120\"").count(), 1);
    }

    #[test]
    fn the_recording_format_select_shows_the_saved_format() {
        let page = render_page(&WavedConfig::default(), &[], "csrf", &[], None);
        assert!(page.contains("name=\"recording.format\""));
        assert!(page.contains("<option value=\"opus\" selected>"));
        let mut config = WavedConfig::default();
        config.recording.format = RecordingFormat::Flac;
        let page = render_page(&config, &[], "csrf", &[], None);
        assert!(page.contains("<option value=\"flac\" selected>"));
        assert!(!page.contains("<option value=\"opus\" selected>"));
    }

    #[test]
    fn odd_saved_seconds_are_labelled_plainly() {
        assert_eq!(
            saved_option_label("display.dim_after_s", "30"),
            "30 seconds"
        );
        assert_eq!(
            saved_option_label("display.off_after_s", "90"),
            "90 seconds"
        );
        assert_eq!(saved_option_label("device.language", "fr"), "fr");
    }

    #[test]
    fn the_ack_says_where_an_unreadable_settings_file_went() {
        let page = render_ack(None, false, Some("config.toml.unloadable"));
        assert!(page.contains("Wave couldn't read the old settings file"));
        assert!(page.contains("config.toml.unloadable"));
        assert!(!render_ack(None, false, None).contains("unloadable"));
    }
}
