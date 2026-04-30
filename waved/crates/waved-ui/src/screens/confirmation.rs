//! ConfirmationScreen — Save / Send / Discard after recording stop.
//!
//! Layout (mockup A): title at top, duration + size, then path, then
//! endpoint, then a row of three 64×64 icon buttons at the bottom. See
//! `docs/superpowers/specs/2026-04-30-waved-recording-save-discard-design.md`
//! §7 for visual specifications.

use std::path::Path;

/// Format a recording's audio path for the confirmation screen.
///
/// Takes the absolute path (e.g. `/mnt/udisk/recordings/4f3a8e2c-…/audio.wav`),
/// truncates the immediate parent dir name to 8 chars + `…`, and keeps the
/// filename. Returns e.g. `4f3a8e2c…/audio.wav`.
///
/// If the path has no parent dir or the parent name is shorter than 8
/// chars, it's returned without truncation.
pub fn format_path(audio_path: &Path) -> String {
    let filename = audio_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = audio_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent_disp = if parent.chars().count() > 8 {
        let truncated: String = parent.chars().take(8).collect();
        format!("{truncated}…")
    } else {
        parent
    };
    if parent_disp.is_empty() {
        filename
    } else {
        format!("{parent_disp}/{filename}")
    }
}

/// Format an endpoint URL for display.
///
/// Strips `http://` or `https://`. If the remaining string is ≤ 32 chars,
/// returns it unchanged. Otherwise ellipsizes the *path* part, keeping the
/// host:port and the last path segment intact:
///
///   `https://example.com/api/v2/very/long/path/upload`
///   → `example.com/…/upload`
pub fn format_endpoint(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    if without_scheme.chars().count() <= 32 {
        return without_scheme.to_string();
    }
    let (host, path) = match without_scheme.split_once('/') {
        Some((h, p)) => (h, p),
        None => return without_scheme.to_string(),
    };
    let last_seg = path.rsplit('/').next().unwrap_or("");
    if last_seg.is_empty() {
        without_scheme.to_string()
    } else {
        format!("{host}/…/{last_seg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_path_truncates_uuid_parent_dir() {
        let p = PathBuf::from("/mnt/udisk/recordings/4f3a8e2c-1234-5678-9abc-def012345678/audio.wav");
        assert_eq!(format_path(&p), "4f3a8e2c…/audio.wav");
    }

    #[test]
    fn format_path_keeps_short_parent_name_intact() {
        let p = PathBuf::from("/tmp/abc/audio.wav");
        assert_eq!(format_path(&p), "abc/audio.wav");
    }

    #[test]
    fn format_path_no_parent_returns_filename_only() {
        let p = PathBuf::from("audio.wav");
        assert_eq!(format_path(&p), "audio.wav");
    }

    #[test]
    fn format_endpoint_strips_https_scheme() {
        assert_eq!(
            format_endpoint("https://localhost:8080/meetings/upload"),
            "localhost:8080/meetings/upload"
        );
    }

    #[test]
    fn format_endpoint_strips_http_scheme() {
        assert_eq!(
            format_endpoint("http://localhost:8080/x"),
            "localhost:8080/x"
        );
    }

    #[test]
    fn format_endpoint_short_url_unchanged() {
        assert_eq!(format_endpoint("localhost:8080/x"), "localhost:8080/x");
    }

    #[test]
    fn format_endpoint_long_path_is_ellipsised_keeping_last_segment() {
        let r = format_endpoint("https://meetings.example.com/api/v2/very/long/path/upload");
        assert_eq!(r, "meetings.example.com/…/upload");
    }

    #[test]
    fn format_endpoint_no_scheme_no_path_unchanged() {
        assert_eq!(format_endpoint("localhost:8080"), "localhost:8080");
    }
}
