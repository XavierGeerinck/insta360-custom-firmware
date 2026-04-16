use std::io::Write;
use std::process::Command;

fn write_i16_pcm(data: &[i16]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for &s in data {
        f.write_all(&s.to_le_bytes()).unwrap();
    }
    f.flush().unwrap();
    f
}

fn bin_path() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target");
    p.push("debug");
    p.push("waved-audio-bench");
    p
}

#[test]
fn bench_on_silence_reports_zero_rms() {
    // Skip if binary not built yet.
    if !bin_path().exists() {
        return;
    }
    let silence: Vec<i16> = vec![0i16; 48_000 * 8]; // 1s
    let f = write_i16_pcm(&silence);
    let output = Command::new(bin_path())
        .arg(f.path())
        .arg("--format")
        .arg("i16_8")
        .output()
        .expect("run bench");
    assert!(
        output.status.success(),
        "bench exited: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let json = String::from_utf8_lossy(&output.stdout);
    assert!(json.contains("\"rms_per_channel\""));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let rms = parsed["rms_per_channel"].as_array().unwrap();
    for v in rms {
        assert_eq!(v.as_f64().unwrap(), 0.0);
    }
}
