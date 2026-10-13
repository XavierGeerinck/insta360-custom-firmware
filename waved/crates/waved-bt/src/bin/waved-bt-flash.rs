//! `waved-bt-flash`: update the Bluetooth module (`BT.bin`) or the USB
//! dongle through it (`DONGLE.bin`), by hand only (TODO F6). Nothing in
//! waved runs this: not the service, not boot, not an update.
//!
//! ```text
//! waved-bt-flash check module|dongle FILE    the image checks and the plan; opens nothing
//! waved-bt-flash version                     the module's and the dongle's versions (read-only)
//! waved-bt-flash flash module|dongle FILE --owner-approved
//! ```
//!
//! waved owns `/dev/ttyS1`: stop it and its watchdog first (`version` and
//! `flash` refuse while a `waved` process runs). The risks and the device
//! procedure are in docs/BLUETOOTH_PROTOCOL.md, "Firmware". Nothing a phone
//! sends (names, MACs, numbers, log lines) is ever printed.

use crossbeam_channel::Receiver;
use std::time::{Duration, Instant};
use waved_bt::command::Command;
use waved_bt::event::{decode, Event};
use waved_bt::frame::Frame;
use waved_bt::ota::{self, FlashError, Image, Link, Progress, Target, PACKET_LEN, STOCK_WAITS};
use waved_uart::UartMux;

const USAGE: &str = "usage:
  waved-bt-flash check module|dongle FILE
  waved-bt-flash version
  waved-bt-flash flash module|dongle FILE --owner-approved";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["check", target, path] => parse_target(target).and_then(|t| check(t, path).map(|_| ())),
        ["version"] => version(),
        ["flash", target, path, "--owner-approved"] => parse_target(target).and_then(|t| flash(t, path)),
        ["flash", ..] => Err(
            "refusing to flash without --owner-approved; read \"Firmware\" in \
             docs/BLUETOOTH_PROTOCOL.md and run `check` first"
                .to_string(),
        ),
        _ => Err(USAGE.to_string()),
    };
    if let Err(e) = result {
        eprintln!("waved-bt-flash: {e}");
        std::process::exit(1);
    }
}

fn parse_target(word: &str) -> Result<Target, String> {
    match word {
        "module" => Ok(Target::Module),
        "dongle" => Ok(Target::Dongle),
        other => Err(format!("unknown target {other:?}: module or dongle")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// The image checks stock makes, and what would go on the wire.
fn check(target: Target, path: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let image = Image::parse(&bytes, target).map_err(|e| format!("{path}: refused: {e:?}"))?;
    let payload = image.payload().len();
    let packets = payload.div_ceil(PACKET_LEN);
    let (version, built) = ota::image_version(&bytes);
    println!("{path}: {target:?} image, {} bytes", bytes.len());
    println!("  version {}, built {}", version.as_deref().unwrap_or("?"), built.as_deref().unwrap_or("?"));
    println!("  sent: {payload} bytes after the 0x8000-byte boot area, in {packets} packets of up to {PACKET_LEN}");
    println!("  announced size {} (u32 at 0x8004 + 0x1000)", image.fw_size());
    if target == Target::Module {
        println!("  request:  {}", hex(&ota::request().encode()));
    }
    println!("  size:     {} (as stock sends it)", hex(&ota::fw_info_as_stock(target, image.fw_size()).encode()));
    if let Some(first) = image.packets().next() {
        let raw = first.encode();
        println!("  packet 1: {} … ({} bytes)", hex(&raw[..16]), raw.len());
    }
    println!("  CRC check {}, then reset {}", hex(&ota::crc_check(target).encode()), hex(&ota::reset(target).encode()));
    println!("  compare the version and build time with `waved-bt-flash version` before flashing");
    Ok(bytes)
}

/// Refuse while waved runs: it owns the port and acknowledges frames too.
fn waved_stopped() -> Result<(), String> {
    let running = std::fs::read_dir("/proc")
        .map(|dir| {
            dir.flatten().any(|entry| {
                std::fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim() == "waved")
            })
        })
        .unwrap_or(false);
    if running {
        Err("waved is running and owns /dev/ttyS1: stop its watchdog and waved first".into())
    } else {
        Ok(())
    }
}

/// The module's UART at stock's speed, acknowledged by the caller.
struct Uart {
    mux: UartMux,
    rx: Receiver<waved_uart::Frame>,
}

impl Uart {
    fn open() -> Result<Self, String> {
        waved_stopped()?;
        let mut mux = UartMux::open(waved_bt::BT_PORT, waved_bt::BT_BAUD)
            .map_err(|e| format!("{}: {e}", waved_bt::BT_PORT))?;
        // A dropped receiver stops the mux's reader: drain the other route.
        let other = mux.take_mcu_receiver();
        std::thread::spawn(move || for _ in other.iter() {});
        let rx = mux.take_bt_receiver();
        Ok(Self { mux, rx })
    }
}

impl Link for Uart {
    fn send(&mut self, frame: &Frame) {
        self.mux.send(frame.encode(), false);
    }

    fn recv(&mut self, timeout: Duration) -> Option<Frame> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let raw = self.rx.recv_timeout(left).ok()?;
            if let Some(frame) = waved_bt::from_uart(&raw) {
                return Some(frame);
            }
        }
    }
}

/// Ask for the versions; print only those answers.
fn version() -> Result<(), String> {
    let mut uart = Uart::open()?;
    for c in [Command::GetVersion, Command::GetBuildTime, Command::GetDongleVersion, Command::GetDongleBuildTime] {
        uart.send(&c.frame());
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while let Some(frame) = uart.recv(deadline.saturating_duration_since(Instant::now())) {
        if let Some(ack) = waved_bt::command::ack_for_frame(&frame) {
            uart.send(&ack);
        }
        match decode(&frame) {
            Event::Version(v) if !v.is_empty() => println!("module firmware {v}"),
            Event::BuildTime(t) if !t.is_empty() => println!("module built {t}"),
            Event::DongleVersion(Some(v)) => println!("dongle firmware {v}"),
            Event::DongleBuildTime(Some(t)) => println!("dongle built {t}"),
            Event::DongleVersion(None) => println!("no dongle connected"),
            _ => {}
        }
    }
    Ok(())
}

fn flash(target: Target, path: &str) -> Result<(), String> {
    let bytes = check(target, path)?;
    let image = Image::parse(&bytes, target).map_err(|e| format!("{e:?}"))?;
    let mut uart = Uart::open()?;
    println!("flashing {target:?} from {path}; keep the Wave on USB power and do not touch it");
    let mut last_percent = None;
    let result = ota::flash(&mut uart, &image, &STOCK_WAITS, &mut |p| match p {
        Progress::Packet { sent, of } => {
            let percent = sent * 100 / of;
            if last_percent != Some(percent / 5) {
                last_percent = Some(percent / 5);
                println!("  {percent:3} %  ({sent} / {of} packets)");
            }
        }
        other => println!("  {other:?}"),
    });
    match result {
        Ok(()) => {
            println!("done: start waved again; its log shows the new `bt module firmware` and `bt module built`");
            Ok(())
        }
        Err(FlashError::NoRequestAnswer) => Err(
            "the module never took the update request; nothing was written. If it stays \
             silent, power-cycle it: echo 1 > /sys/class/leds/btpower/brightness; sleep 1; \
             echo 0 > /sys/class/leds/btpower/brightness"
                .into(),
        ),
        Err(e) => Err(format!(
            "{e:?}. The 0x8000-byte boot area was never touched: run the same command again; \
             see \"Firmware\" in docs/BLUETOOTH_PROTOCOL.md for what to check first"
        )),
    }
}
