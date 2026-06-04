//! Minimal HTTP/1.1 request parsing + page rendering for provisioning.
//! Only what the one-page form needs: request line, and urlencoded body.

use std::collections::HashMap;

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

/// Parse a (small, fully-buffered) HTTP request. Returns None if malformed.
pub fn parse_request(raw: &str) -> Option<Request> {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let mut parts = lines.next()?.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some(Request { method, path, body: body.to_string() })
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Decode an `application/x-www-form-urlencoded` body into key→value.
pub fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (urldecode(k), urldecode(v)))
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the full HTTP response (status line + headers + body) for the
/// provisioning form, with one `<option>` per scanned SSID.
pub fn render_page(ssids: &[String]) -> String {
    let options: String = ssids
        .iter()
        .map(|s| format!("<option value=\"{0}\">{0}</option>", html_escape(s)))
        .collect();
    let body = format!(
        "<!doctype html><html><head><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Wave Wi-Fi Setup</title></head><body style=\"font-family:sans-serif;max-width:20em;margin:2em auto\">\
<h2>Wave Wi-Fi Setup</h2><form method=POST action=/connect>\
<label>Network<br><input list=nets name=ssid required></label>\
<datalist id=nets>{options}</datalist><br><br>\
<label>Password<br><input type=password name=psk></label><br><br>\
<button type=submit>Connect</button></form></body></html>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// A minimal response for the post-submit acknowledgement page.
pub fn render_ack(ssid: &str) -> String {
    let body = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif\"><h2>Connecting to {}…</h2>\
<p>You can close this page. The device will join the network.</p></body></html>",
        html_escape(ssid)
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_get_root() {
        let req = parse_request("GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/");
        assert!(req.body.is_empty());
    }

    #[test]
    fn parse_post_with_body() {
        let raw = "POST /connect HTTP/1.1\r\nContent-Length: 19\r\n\r\nssid=Home&psk=secret";
        let req = parse_request(raw).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/connect");
        assert_eq!(req.body, "ssid=Home&psk=secret");
    }

    #[test]
    fn form_decode_pair() {
        let f = parse_form("ssid=My%20Net&psk=p%40ss%2bword");
        assert_eq!(f.get("ssid").unwrap(), "My Net");
        assert_eq!(f.get("psk").unwrap(), "p@ss+word");
    }

    #[test]
    fn page_lists_ssids() {
        let html = render_page(&["HomeNet".to_string(), "Cafe".to_string()]);
        assert!(html.contains("HomeNet"));
        assert!(html.contains("Cafe"));
        assert!(html.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn page_escapes_html() {
        let html = render_page(&["<script>".to_string()]);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
