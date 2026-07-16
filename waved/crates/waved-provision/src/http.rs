//! Bounded HTTP/1.1 framing and one-page provisioning responses.

use std::collections::HashMap;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_HEADER_BYTES: usize = 8 * 1024;
pub const MAX_BODY_BYTES: usize = 4 * 1024;

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
    Connect { ssid: String, psk: String },
}

pub fn route(request: Request) -> Result<Route, HttpError> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Ok(Route::Form),
        ("POST", "/connect") => {
            let form = parse_form(&request.body)?;
            let ssid = form.get("ssid").cloned().unwrap_or_default();
            let psk = form.get("psk").cloned().unwrap_or_default();
            validate_credentials(&ssid, &psk)?;
            Ok(Route::Connect { ssid, psk })
        }
        (_, "/" | "/connect") => Err(HttpError::MethodNotAllowed),
        _ => Err(HttpError::NotFound),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn response(status: &str, content_type: &str, body: &str) -> String {
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

/// Render the provisioning form with one safe option per scanned SSID.
pub fn render_page(ssids: &[String]) -> String {
    let options: String = ssids
        .iter()
        .map(|ssid| format!("<option value=\"{0}\">{0}</option>", html_escape(ssid)))
        .collect();
    let body = format!(
        "<!doctype html><html><head><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Wave Wi-Fi Setup</title></head><body style=\"font-family:sans-serif;max-width:20em;margin:2em auto\">\
<h2>Wave Wi-Fi Setup</h2><form method=POST action=/connect>\
<label>Network<br><input list=nets name=ssid required maxlength=32></label>\
<datalist id=nets>{options}</datalist><br><br>\
<label>Password<br><input type=password name=psk minlength=8 maxlength=63></label><br><br>\
<button type=submit>Connect</button></form></body></html>"
    );
    response("200 OK", "text/html; charset=utf-8", &body)
}

pub fn render_ack(ssid: &str) -> String {
    let body = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif\"><h2>Connecting to {}…</h2>\
<p>You can close this page. The device will join the network.</p></body></html>",
        html_escape(ssid)
    );
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
        let missing = route(Request {
            method: "GET".into(),
            path: "/missing".into(),
            body: String::new(),
        });
        assert!(matches!(missing, Err(HttpError::NotFound)));
    }

    #[test]
    fn page_lists_and_escapes_ssids() {
        let page = render_page(&["HomeNet".to_string(), "<script>".to_string()]);
        assert!(page.contains("HomeNet"));
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn errors_have_explicit_statuses() {
        assert!(render_error(HttpError::BadRequest).starts_with("HTTP/1.1 400"));
        assert!(render_error(HttpError::PayloadTooLarge).starts_with("HTTP/1.1 413"));
        assert!(render_error(HttpError::NotFound).starts_with("HTTP/1.1 404"));
        assert!(render_error(HttpError::MethodNotAllowed).starts_with("HTTP/1.1 405"));
    }
}
