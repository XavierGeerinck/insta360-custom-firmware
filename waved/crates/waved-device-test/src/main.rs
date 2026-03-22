use std::time::Instant;

struct TestResult {
    name: &'static str,
    passed: bool,
    message: String,
    duration_ms: u128,
}

fn main() {
    println!("=== waved device test runner ===\n");

    let results = vec![
        run_test("test_uart_responsive", test_uart_responsive),
        run_test("test_hid_gadget", test_hid_gadget),
        run_test("test_wifi_scan", test_wifi_scan),
        run_test("test_battery_read", test_battery_read),
        run_test("test_audio_device", test_audio_device),
        run_test("test_filesystem", test_filesystem),
        run_test("test_config_load", test_config_load),
        run_test("test_recordings_dir", test_recordings_dir),
    ];

    println!();
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();

    for r in &results {
        let status = if r.passed { "\x1b[32mPASS\x1b[0m" } else { "\x1b[31mFAIL\x1b[0m" };
        if r.passed {
            println!("[{status}] {} ({}ms)", r.name, r.duration_ms);
        } else {
            println!("[{status}] {}: {}", r.name, r.message);
        }
    }

    println!("\n{passed}/{total} passed, {} failed", total - passed);
    if passed < total { std::process::exit(1); }
}

fn run_test(name: &'static str, f: fn() -> Result<(), String>) -> TestResult {
    let start = Instant::now();
    match f() {
        Ok(()) => TestResult { name, passed: true, message: String::new(), duration_ms: start.elapsed().as_millis() },
        Err(msg) => TestResult { name, passed: false, message: msg, duration_ms: start.elapsed().as_millis() },
    }
}

fn test_uart_responsive() -> Result<(), String> {
    use std::io::{Read, Write};
    use std::time::Duration;

    let port = serialport::new("/dev/ttyS1", 115200)
        .timeout(Duration::from_millis(500))
        .open();

    match port {
        Ok(mut port) => {
            let query = waved_uart::Frame::build([0xDC, 0xCD], &[0x10])
                .map_err(|e| format!("frame build error: {e}"))?;
            port.write_all(&query).map_err(|e| format!("write error: {e}"))?;
            let mut buf = [0u8; 64];
            match port.read(&mut buf) {
                Ok(n) if n > 0 => Ok(()),
                Ok(_) => Err("no response from MCU".to_string()),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Err("MCU did not respond within 500ms".to_string()),
                Err(e) => Err(format!("read error: {e}")),
            }
        }
        Err(e) => Err(format!("cannot open /dev/ttyS1: {e}")),
    }
}

fn test_hid_gadget() -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open("/dev/hidg0")
        .map_err(|e| format!("cannot open /dev/hidg0: {e}"))?;
    file.write_all(&[0x00, 0x00]).map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

fn test_wifi_scan() -> Result<(), String> {
    let output = std::process::Command::new("wpa_cli").args(["scan_results"]).output()
        .map_err(|e| format!("wpa_cli error: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.lines().count() > 1 { Ok(()) } else { Err("no networks found".to_string()) }
}

fn test_battery_read() -> Result<(), String> {
    use std::io::{Read, Write};
    use std::time::Duration;

    let port = serialport::new("/dev/ttyS1", 115200)
        .timeout(Duration::from_millis(500))
        .open();

    match port {
        Ok(mut port) => {
            let query = waved_uart::Frame::build([0xDC, 0xCD], &[0x10])
                .map_err(|e| format!("frame build error: {e}"))?;
            port.write_all(&query).map_err(|e| format!("write error: {e}"))?;
            let mut buf = [0u8; 64];
            match port.read(&mut buf) {
                Ok(n) if n >= 8 => {
                    let level = buf[4];
                    if level <= 100 { Ok(()) } else { Err(format!("battery level out of range: {level}")) }
                }
                Ok(n) => Err(format!("response too short: {n} bytes")),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Err("MCU did not respond within 500ms".to_string()),
                Err(e) => Err(format!("read error: {e}")),
            }
        }
        Err(e) => Err(format!("cannot open /dev/ttyS1: {e}")),
    }
}

fn test_audio_device() -> Result<(), String> {
    let output = std::process::Command::new("cat").arg("/proc/asound/pcm").output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.is_empty() { Err("no ALSA PCM devices found".to_string()) } else { Ok(()) }
        }
        Ok(_) => Err("/proc/asound/pcm not readable".to_string()),
        Err(e) => Err(format!("failed to check audio device: {e}")),
    }
}

fn test_filesystem() -> Result<(), String> {
    let path = std::path::Path::new("/mnt/udisk");
    if !path.exists() { return Err("/mnt/udisk does not exist".to_string()); }
    let test_file = path.join(".waved_test");
    std::fs::write(&test_file, "test").map_err(|e| format!("/mnt/udisk not writable: {e}"))?;
    std::fs::remove_file(&test_file).map_err(|e| format!("cleanup failed: {e}"))?;
    Ok(())
}

fn test_config_load() -> Result<(), String> {
    let path = std::path::Path::new("/data/waved/config.toml");
    if !path.exists() { return Err("config.toml not found".to_string()); }
    let _config = waved_config::WavedConfig::load(path).map_err(|e| format!("config parse error: {e}"))?;
    Ok(())
}

fn test_recordings_dir() -> Result<(), String> {
    let path = std::path::Path::new("/mnt/udisk/recordings");
    if !path.exists() { return Err("/mnt/udisk/recordings does not exist".to_string()); }
    if !path.is_dir() { return Err("/mnt/udisk/recordings is not a directory".to_string()); }
    Ok(())
}
