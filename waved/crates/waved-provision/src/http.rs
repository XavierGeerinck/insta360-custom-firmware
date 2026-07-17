//! Bounded HTTP/1.1 framing and one-page provisioning responses.

use std::collections::HashMap;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_HEADER_BYTES: usize = 8 * 1024;
pub const MAX_BODY_BYTES: usize = 32 * 1024;
const OTHER_NETWORK_VALUE: &str = "__other_network__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpError {
    BadRequest,
    PayloadTooLarge,
    NotFound,
    MethodNotAllowed,
}

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn content_length(head: &str) -> Result<usize, HttpError> {
    let mut found = None;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(HttpError::BadRequest);
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if found.is_some() {
                return Err(HttpError::BadRequest);
            }
            found = Some(value.trim().parse().map_err(|_| HttpError::BadRequest)?);
        }
    }
    Ok(found.unwrap_or(0))
}

fn request_line(head: &str) -> Result<(String, String), HttpError> {
    let mut parts = head
        .lines()
        .next()
        .ok_or(HttpError::BadRequest)?
        .split_whitespace();
    let method = parts.next().ok_or(HttpError::BadRequest)?;
    let path = parts.next().ok_or(HttpError::BadRequest)?;
    let version = parts.next().ok_or(HttpError::BadRequest)?;
    if parts.next().is_some()
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || !path.starts_with('/')
    {
        return Err(HttpError::BadRequest);
    }
    Ok((method.to_string(), path.to_string()))
}

/// Read exactly one bounded HTTP request, including bodies split across TCP
/// reads. Callers apply the per-connection wall-clock timeout.
pub async fn read_request<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Request, HttpError> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(end) = find_header_end(&buffer) {
            if end > MAX_HEADER_BYTES {
                return Err(HttpError::PayloadTooLarge);
            }
            break end;
        }
        if buffer.len() >= MAX_HEADER_BYTES {
            return Err(HttpError::PayloadTooLarge);
        }
        let count = reader
            .read(&mut chunk)
            .await
            .map_err(|_| HttpError::BadRequest)?;
        if count == 0 {
            return Err(HttpError::BadRequest);
        }
        buffer.extend_from_slice(&chunk[..count]);
    };

    let head = std::str::from_utf8(&buffer[..header_end - 4]).map_err(|_| HttpError::BadRequest)?;
    let (method, path) = request_line(head)?;
    let body_length = content_length(head)?;
    if body_length > MAX_BODY_BYTES {
        return Err(HttpError::PayloadTooLarge);
    }
    let request_length = header_end
        .checked_add(body_length)
        .ok_or(HttpError::PayloadTooLarge)?;
    while buffer.len() < request_length {
        let remaining = request_length - buffer.len();
        let read_length = remaining.min(chunk.len());
        let count = reader
            .read(&mut chunk[..read_length])
            .await
            .map_err(|_| HttpError::BadRequest)?;
        if count == 0 {
            return Err(HttpError::BadRequest);
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
    let body = std::str::from_utf8(&buffer[header_end..request_length])
        .map_err(|_| HttpError::BadRequest)?
        .to_string();
    Ok(Request { method, path, body })
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn urldecode(value: &str) -> Result<String, HttpError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'+' => out.push(b' '),
            b'%' => {
                let high = *bytes.get(cursor + 1).ok_or(HttpError::BadRequest)?;
                let low = *bytes.get(cursor + 2).ok_or(HttpError::BadRequest)?;
                let decoded = hex_nibble(high)
                    .zip(hex_nibble(low))
                    .map(|(high, low)| high << 4 | low)
                    .ok_or(HttpError::BadRequest)?;
                if decoded == 0 {
                    return Err(HttpError::BadRequest);
                }
                out.push(decoded);
                cursor += 2;
            }
            0 => return Err(HttpError::BadRequest),
            byte => out.push(byte),
        }
        cursor += 1;
    }
    String::from_utf8(out).map_err(|_| HttpError::BadRequest)
}

/// Decode an `application/x-www-form-urlencoded` body into key/value pairs.
pub fn parse_form(body: &str) -> Result<HashMap<String, String>, HttpError> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').ok_or(HttpError::BadRequest)?;
            Ok((urldecode(key)?, urldecode(value)?))
        })
        .collect()
}

pub fn validate_credentials(ssid: &str, psk: &str) -> Result<(), HttpError> {
    if !(1..=32).contains(&ssid.len())
        || ssid.contains(['\0', '\r', '\n'])
        || psk.contains(['\0', '\r', '\n'])
        || !(psk.is_empty() || (8..=63).contains(&psk.chars().count()))
    {
        return Err(HttpError::BadRequest);
    }
    Ok(())
}

pub enum Route {
    Form,
    CaptiveRedirect { head_only: bool },
    Save { form: HashMap<String, String> },
    SaveConnect { form: HashMap<String, String> },
}

pub fn route(request: Request) -> Result<Route, HttpError> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Ok(Route::Form),
        ("POST", "/save") => Ok(Route::Save {
            form: parse_form(&request.body)?,
        }),
        ("POST", "/save-connect" | "/connect") => Ok(Route::SaveConnect {
            form: parse_form(&request.body)?,
        }),
        ("GET", "/save" | "/save-connect" | "/connect") => Err(HttpError::MethodNotAllowed),
        ("GET", _) => Ok(Route::CaptiveRedirect { head_only: false }),
        ("HEAD", _) => Ok(Route::CaptiveRedirect { head_only: true }),
        (_, "/" | "/save" | "/save-connect" | "/connect") => Err(HttpError::MethodNotAllowed),
        _ => Err(HttpError::NotFound),
    }
}

pub fn wifi_credentials(form: &HashMap<String, String>) -> Result<(String, String), HttpError> {
    let selected_ssid = form.get("ssid").cloned().unwrap_or_default();
    let ssid = if selected_ssid == OTHER_NETWORK_VALUE {
        form.get("other_ssid").cloned().unwrap_or_default()
    } else {
        selected_ssid
    };
    let psk = form.get("psk").cloned().unwrap_or_default();
    validate_credentials(&ssid, &psk)?;
    Ok((ssid, psk))
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub fn render_error(error: HttpError) -> String {
    let (status, message) = match error {
        HttpError::BadRequest => ("400 Bad Request", "Invalid provisioning request"),
        HttpError::PayloadTooLarge => ("413 Payload Too Large", "Request is too large"),
        HttpError::NotFound => ("404 Not Found", "Not found"),
        HttpError::MethodNotAllowed => ("405 Method Not Allowed", "Method not allowed"),
    };
    response(status, "text/plain; charset=utf-8", message)
}

/// Redirect a captive-network probe to the canonical local setup form. The
/// request host/path are never reflected into either the header or body.
pub fn render_captive_redirect(head_only: bool) -> String {
    let body = if head_only {
        ""
    } else {
        "<!doctype html><html><body><a href=\"http://192.168.2.1/\">Open Wave Wi-Fi Setup</a></body></html>"
    };
    format!(
        "HTTP/1.1 302 Found\r\nLocation: http://192.168.2.1/\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

const PAGE_STYLE: &str = r#"
:root{color-scheme:light;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:#f5f7fa;color:#17222b}
*{box-sizing:border-box}
body{min-height:100vh;margin:0;padding:max(24px,env(safe-area-inset-top)) 18px max(24px,env(safe-area-inset-bottom));display:grid;place-items:center;background:linear-gradient(145deg,#f8fafc 0%,#edf3f2 100%)}
.card{width:min(100%,420px);padding:30px 26px 26px;background:#fff;border:1px solid #e1e7e9;border-radius:24px;box-shadow:0 18px 55px rgba(20,42,48,.12)}
.brand{display:flex;align-items:center;gap:10px;color:#52636a;font-size:12px;font-weight:750;letter-spacing:.15em;text-transform:uppercase}
.mark{width:30px;height:30px;display:grid;place-items:center;border:2px solid #087f73;border-radius:50%;color:#087f73;font-size:12px;letter-spacing:0}
.eyebrow{margin:28px 0 8px;color:#087f73;font-size:12px;font-weight:750;letter-spacing:.09em;text-transform:uppercase}
h1{margin:26px 0 9px;font-size:clamp(27px,7vw,34px);line-height:1.12;letter-spacing:-.035em}
.eyebrow+h1{margin-top:0}
.intro{margin:0 0 26px;color:#617078;font-size:15px;line-height:1.55}
.field{margin-top:18px}
label{display:block;margin-bottom:8px;color:#34464e;font-size:13px;font-weight:700}
input,select{width:100%;height:52px;padding:0 15px;border:1px solid #cfd9dc;border-radius:13px;background:#fff;color:#17222b;font:inherit;font-size:16px;outline:none;transition:border-color .15s,box-shadow .15s}
select{cursor:pointer}
input:focus-visible,select:focus-visible,.toggle:focus-visible,.primary:focus-visible{outline:none;border-color:#087f73;box-shadow:0 0 0 4px rgba(8,127,115,.14)}
.other-network[hidden]{display:none}
.password{position:relative}
.password input{padding-right:76px}
.toggle{position:absolute;top:7px;right:7px;height:38px;padding:0 11px;border:0;border-radius:9px;background:#eef5f4;color:#087f73;font:inherit;font-size:13px;font-weight:750;cursor:pointer}
.primary{width:100%;height:52px;margin-top:24px;border:0;border-radius:14px;background:#087f73;color:#fff;font:inherit;font-size:16px;font-weight:750;cursor:pointer;box-shadow:0 10px 24px rgba(8,127,115,.2);transition:background .15s,transform .15s,box-shadow .15s}
.primary:hover{background:#066e64;transform:translateY(-1px);box-shadow:0 12px 27px rgba(8,127,115,.24)}
.primary:disabled{background:#81aaa5;cursor:wait;transform:none;box-shadow:none}
.security{display:flex;align-items:flex-start;gap:9px;margin:18px 2px 0;color:#718087;font-size:12px;line-height:1.45}
.security-icon{width:18px;height:18px;flex:0 0 18px;display:grid;place-items:center;border:1px solid #9bb0ad;border-radius:50%;color:#087f73;font-size:11px;font-weight:800}
.submit-status{margin:13px 0 0;text-align:center;color:#52636a;font-size:13px}
.status-mark{width:54px;height:54px;margin:28px 0 20px;display:grid;place-items:center;border-radius:50%;background:#e5f4f1;color:#087f73;font-size:25px;font-weight:800}
.network-name{overflow-wrap:anywhere;color:#087f73}
.notice{margin-top:24px;padding:16px;border:1px solid #d8e6e3;border-radius:14px;background:#f3f8f7;color:#40545b;font-size:14px;line-height:1.5}
.foot{margin:20px 0 0;color:#718087;font-size:13px;line-height:1.5}
@media(max-width:460px){body{padding-left:12px;padding-right:12px;align-items:start}.card{margin-top:8px;padding:26px 20px 22px;border-radius:20px}}
@media(prefers-reduced-motion:reduce){*{transition:none!important}}
"#;

const FORM_SCRIPT: &str = r#"
const form=document.getElementById('setup-form');
const password=document.getElementById('psk');
const toggle=document.getElementById('toggle-password');
const submit=document.getElementById('connect-button');
const status=document.getElementById('submit-status');
const network=document.getElementById('ssid');
const otherField=document.getElementById('other-network-field');
const otherSsid=document.getElementById('other-ssid');
network.addEventListener('change',()=>{
  const other=network.value==='__other_network__';
  otherField.hidden=!other;
  otherSsid.disabled=!other;
  otherSsid.required=other;
  if(other)otherSsid.focus();
});
toggle.addEventListener('click',()=>{
  const showing=password.type==='text';
  password.type=showing?'password':'text';
  toggle.textContent=showing?'Show':'Hide';
  toggle.setAttribute('aria-pressed',String(!showing));
  password.focus();
});
form.addEventListener('submit',()=>{
  submit.disabled=true;
  submit.textContent='Connecting…';
  status.hidden=false;
});
"#;

fn render_document(title: &str, content: &str, script: Option<&str>) -> String {
    let script = script
        .map(|source| format!("<script>{source}</script>"))
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><meta name=\"theme-color\" content=\"#f5f7fa\"><title>{title}</title><style>{PAGE_STYLE}</style></head><body>{content}{script}</body></html>"
    )
}

/// Render the provisioning form with one safe option per scanned SSID.
pub fn render_page(ssids: &[String]) -> String {
    let options: String = ssids
        .iter()
        .map(|ssid| format!("<option value=\"{0}\">{0}</option>", html_escape(ssid)))
        .collect();
    let content = format!(
        r#"<main class="card"><div class="brand"><span class="mark" aria-hidden="true">W</span><span>Wave</span></div><h1>Wi-Fi setup</h1><p class="intro">Connect your Wave to a nearby network.</p><form id="setup-form" method="post" action="/connect"><div class="field"><label for="ssid">Network</label><select id="ssid" name="ssid" required><option value="" selected disabled>Choose a network</option>{options}<option value="{OTHER_NETWORK_VALUE}">Other network…</option></select></div><div class="field other-network" id="other-network-field" hidden><label for="other-ssid">Network name</label><input id="other-ssid" name="other_ssid" disabled maxlength="32" autocomplete="off" autocapitalize="none" spellcheck="false" placeholder="Enter the network name"></div><div class="field"><label for="psk">Password</label><div class="password"><input id="psk" name="psk" type="password" minlength="8" maxlength="63" autocomplete="new-password" placeholder="Enter the Wi-Fi password"><button class="toggle" id="toggle-password" type="button" aria-controls="psk" aria-pressed="false">Show</button></div></div><button class="primary" id="connect-button" type="submit">Connect</button><p class="submit-status" id="submit-status" role="status" hidden>Sending credentials to your Wave…</p></form><p class="security"><span class="security-icon" aria-hidden="true">✓</span><span>Your credentials are sent directly to this Wave and stored as an encrypted Wi-Fi key.</span></p></main>"#
    );
    let body = render_document("Wave Wi-Fi Setup", &content, Some(FORM_SCRIPT));
    response("200 OK", "text/html; charset=utf-8", &body)
}

pub fn render_ack(ssid: &str) -> String {
    let content = format!(
        r#"<main class="card"><div class="brand"><span class="mark" aria-hidden="true">W</span><span>Wave</span></div><div class="status-mark" aria-hidden="true">✓</div><p class="eyebrow">Setup received</p><h1>Connecting to <span class="network-name">{}</span></h1><p class="intro">Your Wave is switching from its temporary setup network to your Wi-Fi.</p><div class="notice"><strong>The Wave Wi-Fi will now disappear.</strong><br>This is expected while the device changes networks.</div><p class="foot">You can close this page. Check the Wave screen for the connection result.</p></main>"#,
        html_escape(ssid)
    );
    let body = render_document("Wave is connecting", &content, None);
    response("200 OK", "text/html; charset=utf-8", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncWriteExt};

    #[tokio::test]
    async fn reads_split_post_body() {
        let (mut client, mut server) = duplex(1024);
        let writer = tokio::spawn(async move {
            client
                .write_all(b"POST /connect HTTP/1.1\r\nContent-Length: 25\r\n\r\nssid=Home")
                .await
                .unwrap();
            tokio::task::yield_now().await;
            client.write_all(b"&psk=password123").await.unwrap();
        });
        let request = read_request(&mut server).await.unwrap();
        writer.await.unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.body, "ssid=Home&psk=password123");
    }

    #[tokio::test]
    async fn rejects_oversized_body_before_reading_it() {
        let (mut client, mut server) = duplex(256);
        client
            .write_all(
                format!(
                    "POST /connect HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                    MAX_BODY_BYTES + 1
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            read_request(&mut server).await.unwrap_err(),
            HttpError::PayloadTooLarge
        );
    }

    #[test]
    fn form_decodes_utf8_and_rejects_bad_escapes() {
        let form = parse_form("ssid=Caf%C3%A9&psk=p%40ssword").unwrap();
        assert_eq!(form.get("ssid").unwrap(), "Café");
        assert_eq!(form.get("psk").unwrap(), "p@ssword");
        assert_eq!(
            parse_form("ssid=bad%0&psk=password").unwrap_err(),
            HttpError::BadRequest
        );
        assert_eq!(
            parse_form("ssid=bad%00x&psk=password").unwrap_err(),
            HttpError::BadRequest
        );
    }

    #[test]
    fn credentials_enforce_wifi_limits() {
        assert!(validate_credentials("Home", "password").is_ok());
        assert!(validate_credentials("Open", "").is_ok());
        assert_eq!(
            validate_credentials("", "password"),
            Err(HttpError::BadRequest)
        );
        assert_eq!(
            validate_credentials(&"x".repeat(33), "password"),
            Err(HttpError::BadRequest)
        );
        assert_eq!(
            validate_credentials("Home", "short"),
            Err(HttpError::BadRequest)
        );
    }

    #[test]
    fn other_network_selection_uses_the_typed_ssid() {
        let selected = route(Request {
            method: "POST".into(),
            path: "/connect".into(),
            body: "ssid=__other_network__&other_ssid=Hidden+Lab&psk=password123".into(),
        });
        let Ok(Route::SaveConnect { form }) = selected else {
            panic!("expected save-connect route");
        };
        assert_eq!(
            wifi_credentials(&form).unwrap(),
            ("Hidden Lab".into(), "password123".into())
        );

        let missing = route(Request {
            method: "POST".into(),
            path: "/connect".into(),
            body: "ssid=__other_network__&other_ssid=&psk=password123".into(),
        });
        let Ok(Route::SaveConnect { form }) = missing else {
            panic!("expected save-connect route");
        };
        assert!(matches!(
            wifi_credentials(&form),
            Err(HttpError::BadRequest)
        ));
    }

    #[test]
    fn routes_methods_and_paths_explicitly() {
        let form = route(Request {
            method: "GET".into(),
            path: "/".into(),
            body: String::new(),
        });
        assert!(matches!(form, Ok(Route::Form)));
        let method = route(Request {
            method: "POST".into(),
            path: "/".into(),
            body: String::new(),
        });
        assert!(matches!(method, Err(HttpError::MethodNotAllowed)));
        let save = route(Request {
            method: "POST".into(),
            path: "/save".into(),
            body: "csrf=token".into(),
        });
        assert!(matches!(save, Ok(Route::Save { .. })));
        let missing = route(Request {
            method: "GET".into(),
            path: "/missing".into(),
            body: String::new(),
        });
        assert!(matches!(
            missing,
            Ok(Route::CaptiveRedirect { head_only: false })
        ));
        let head = route(Request {
            method: "HEAD".into(),
            path: "/hotspot-detect.html".into(),
            body: String::new(),
        });
        assert!(matches!(
            head,
            Ok(Route::CaptiveRedirect { head_only: true })
        ));
        let unknown_method = route(Request {
            method: "DELETE".into(),
            path: "/missing".into(),
            body: String::new(),
        });
        assert!(matches!(unknown_method, Err(HttpError::NotFound)));
    }

    #[test]
    fn page_lists_and_escapes_ssids() {
        let page = render_page(&["HomeNet".to_string(), "<script>".to_string()]);
        assert!(page.contains("HomeNet"));
        assert!(!page.contains("<option value=\"<script>"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("<select id=\"ssid\" name=\"ssid\" required>"));
        assert!(page.contains("<option value=\"HomeNet\">HomeNet</option>"));
        assert!(page.contains("Other network…"));
        assert!(page.contains("id=\"other-network-field\" hidden"));
        assert!(page.contains("otherSsid.disabled=!other"));
        assert!(page.contains("id=\"toggle-password\""));
        assert!(page.contains("password.type==='text'"));
        assert!(page.contains("submit.disabled=true"));
        assert!(page.contains("viewport-fit=cover"));
        assert!(page.contains("Your credentials are sent directly"));
        assert!(!page.contains("https://"));
        assert!(!page.contains("<link"));
        assert!(!page.contains("<img"));
    }

    #[test]
    fn acknowledgement_is_polished_and_escapes_ssid() {
        let page = render_ack("Home <script>");
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("Connecting to"));
        assert!(page.contains("Home &lt;script&gt;"));
        assert!(!page.contains("Home <script>"));
        assert!(page.contains("Wave Wi-Fi will now disappear"));
        assert!(page.contains("Check the Wave screen"));
    }

    #[test]
    fn errors_have_explicit_statuses() {
        assert!(render_error(HttpError::BadRequest).starts_with("HTTP/1.1 400"));
        assert!(render_error(HttpError::PayloadTooLarge).starts_with("HTTP/1.1 413"));
        assert!(render_error(HttpError::NotFound).starts_with("HTTP/1.1 404"));
        assert!(render_error(HttpError::MethodNotAllowed).starts_with("HTTP/1.1 405"));
    }

    #[test]
    fn captive_redirect_is_canonical_and_head_has_no_body() {
        let get = render_captive_redirect(false);
        assert!(get.starts_with("HTTP/1.1 302 Found"));
        assert!(get.contains("\r\nLocation: http://192.168.2.1/\r\n"));
        assert!(get.ends_with("</html>"));

        let head = render_captive_redirect(true);
        assert!(head.starts_with("HTTP/1.1 302 Found"));
        assert!(head.ends_with("\r\n\r\n"));
        assert!(head.contains("\r\nContent-Length: 0\r\n"));
    }
}
