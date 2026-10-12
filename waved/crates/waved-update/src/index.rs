//! `latest.json`: the update server's pointer to its newest package.
//!
//! `security.ota_server` is a base URL; Wave fetches `<base>/latest.json`:
//!
//! ```json
//! {"format": 1, "version": "0.10.0", "build": "374015f",
//!  "package": "waved-0.10.0.wup", "size": 23456789,
//!  "sha256": "<hex of the whole .wup>", "installed_size": 63000000}
//! ```
//!
//! The index is not signed. It only decides whether to download; the
//! package's own signature decides whether to install, and its signed
//! version must equal the index's (`docs/UPDATES.md`, U3).

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::format::{is_sha256_hex, MAX_FILE};
use crate::version::Version;

pub const INDEX_NAME: &str = "latest.json";
pub const MAX_INDEX_BYTES: usize = 16 * 1024;
/// Every file at its largest, plus headers.
pub const MAX_PACKAGE: u64 = 2 * MAX_FILE;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub format: u32,
    pub version: String,
    #[serde(default)]
    pub build: String,
    /// Relative to the index URL, or an absolute http(s) URL.
    pub package: String,
    pub size: u64,
    pub sha256: String,
    /// Unpacked size of every file, for the space check before downloading.
    pub installed_size: u64,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("the update server address is not an http(s) URL")]
    BadServer,
    #[error("the update index is malformed: {0}")]
    Malformed(String),
}

impl Index {
    pub fn parse(bytes: &[u8]) -> Result<(Index, Version), IndexError> {
        let bad = |why: &str| IndexError::Malformed(why.into());
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(bad("too large"));
        }
        let index: Index =
            serde_json::from_slice(bytes).map_err(|e| IndexError::Malformed(e.to_string()))?;
        if index.format != crate::format::FORMAT {
            return Err(bad("unknown format"));
        }
        let version = index.version.parse().map_err(|_| bad("bad version"))?;
        if index.size == 0 || index.size > MAX_PACKAGE {
            return Err(bad("package size out of range"));
        }
        if index.installed_size > MAX_PACKAGE {
            return Err(bad("installed size out of range"));
        }
        if !is_sha256_hex(&index.sha256) {
            return Err(bad("sha256 is not lowercase SHA-256 hex"));
        }
        if index.package.trim().is_empty() {
            return Err(bad("no package"));
        }
        Ok((index, version))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an index serializes") + "\n"
    }
}

fn http(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.has_host()
}

/// `<server>/latest.json`, whether or not `server` ends in a slash.
pub fn index_url(server: &str) -> Result<Url, IndexError> {
    let server = server.trim();
    let base = if server.ends_with('/') {
        server.to_string()
    } else {
        format!("{server}/")
    };
    let base = Url::parse(&base).map_err(|_| IndexError::BadServer)?;
    if !http(&base) || base.query().is_some() || base.fragment().is_some() {
        return Err(IndexError::BadServer);
    }
    base.join(INDEX_NAME).map_err(|_| IndexError::BadServer)
}

pub fn package_url(index_url: &Url, index: &Index) -> Result<Url, IndexError> {
    let url = index_url
        .join(index.package.trim())
        .map_err(|_| IndexError::Malformed("bad package URL".into()))?;
    if !http(&url) {
        return Err(IndexError::Malformed("package URL is not http(s)".into()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_json() -> String {
        format!(
            r#"{{"format":1,"version":"0.10.0","build":"374015f","package":"waved-0.10.0.wup",
               "size":23456789,"sha256":"{}","installed_size":63000000}}"#,
            "ab".repeat(32)
        )
    }

    #[test]
    fn a_good_index_parses() {
        let (index, version) = Index::parse(index_json().as_bytes()).unwrap();
        assert_eq!(version, Version::new(0, 10, 0));
        assert_eq!(index.build, "374015f");
        assert_eq!(index.size, 23_456_789);
        let again = Index::parse(index.to_json().as_bytes()).unwrap().0;
        assert_eq!(again, index);
    }

    #[test]
    fn bad_indexes_are_refused() {
        let good = index_json();
        let cases = [
            good.replace("\"format\":1", "\"format\":2"),
            good.replace("0.10.0\"", "0.10\""),
            good.replace("23456789", "0"),
            good.replace("23456789", "999999999999"),
            good.replace(&"ab".repeat(32), &"AB".repeat(32)),
            good.replace(&"ab".repeat(32), "abc"),
            good.replace("waved-0.10.0.wup", " "),
            good.replace("\"build\"", "\"surprise\":1,\"build\""),
            "not json".to_string(),
            format!("{good}{}", " ".repeat(MAX_INDEX_BYTES)),
        ];
        for case in cases {
            assert!(Index::parse(case.as_bytes()).is_err(), "{case}");
        }
    }

    #[test]
    fn the_index_lives_under_the_server_base() {
        for server in ["http://updates.lan/wave", "http://updates.lan/wave/"] {
            assert_eq!(
                index_url(server).unwrap().as_str(),
                "http://updates.lan/wave/latest.json"
            );
        }
        assert_eq!(
            index_url("https://example.com").unwrap().as_str(),
            "https://example.com/latest.json"
        );
        for bad in ["", "updates.lan", "ftp://updates.lan/", "file:///tmp/x", "http://h/?q=1"] {
            assert_eq!(index_url(bad), Err(IndexError::BadServer), "{bad}");
        }
    }

    #[test]
    fn package_urls_resolve_against_the_index() {
        let at = index_url("http://updates.lan/wave").unwrap();
        let (mut index, _) = Index::parse(index_json().as_bytes()).unwrap();
        assert_eq!(
            package_url(&at, &index).unwrap().as_str(),
            "http://updates.lan/wave/waved-0.10.0.wup"
        );
        index.package = "https://cdn.example.com/w/0.10.0.wup".into();
        assert_eq!(
            package_url(&at, &index).unwrap().as_str(),
            "https://cdn.example.com/w/0.10.0.wup"
        );
        index.package = "file:///etc/passwd".into();
        assert!(package_url(&at, &index).is_err());
    }
}
