//! About (B6, E7): device name, firmware version, serial, address and free
//! space. There is no mockup; it uses the settings rhythm (rows under the
//! title, Back at the bottom). Licences are not shown in milestone 2.

use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::{text_width, Obj};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_W, TEXT, TEXT_2, TITLE_Y};

const ROW_X: i32 = 60;
const ROW_TOP: i32 = 66;
const ROW_PITCH: i32 = 32;
const ROW_W: i32 = 280;
const ROW_H: i32 = 30;
const VALUE_CAP: i32 = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutRow {
    Name,
    Version,
    Serial,
    Address,
    Space,
}

const ROWS: [AboutRow; 5] = [
    AboutRow::Name,
    AboutRow::Version,
    AboutRow::Serial,
    AboutRow::Address,
    AboutRow::Space,
];

impl AboutRow {
    fn label(self) -> &'static str {
        match self {
            AboutRow::Name => "Name",
            AboutRow::Version => "Version",
            AboutRow::Serial => "Serial",
            AboutRow::Address => "IP address",
            AboutRow::Space => "Free space",
        }
    }
}

/// "3.2 GB" or "850 MB" (decimal, as Files shows free space).
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", bytes / 1_000_000)
    }
}

/// The firmware version as the system service reports it, or the crate
/// version while it has not reported yet. Shared with the System list.
pub(crate) fn version_line(snap: &UiSnapshot) -> String {
    let version = snap.system.about.version.trim();
    if version.is_empty() {
        format!("waved {}", env!("CARGO_PKG_VERSION"))
    } else {
        version.to_string()
    }
}

/// Free space on the recordings filesystem. The system service's figure is
/// used once it has measured the filesystem (total > 0); until then the
/// Files figure stands in.
fn free_bytes(snap: &UiSnapshot) -> u64 {
    let about = &snap.system.about;
    if about.storage_total_bytes > 0 {
        about.storage_free_bytes
    } else {
        snap.files.free_bytes
    }
}

/// The row's value; `None` leaves the row out.
pub fn about_value(row: AboutRow, snap: &UiSnapshot, name: &str) -> Option<String> {
    match row {
        AboutRow::Name => Some(if name.trim().is_empty() {
            "Wave".into()
        } else {
            name.trim().into()
        }),
        AboutRow::Version => Some(version_line(snap)),
        AboutRow::Serial => {
            let serial = snap.system.about.serial.trim();
            (!serial.is_empty()).then(|| serial.to_string())
        }
        AboutRow::Address => Some(if snap.wifi.connected && !snap.wifi.ip_addr.is_empty() {
            snap.wifi.ip_addr.clone()
        } else {
            "Not connected".into()
        }),
        AboutRow::Space => Some(format_bytes(free_bytes(snap))),
    }
}

#[derive(Default)]
pub struct AboutScreen {
    status: Option<StatusArea>,
    name: String,
    /// Row, its value label and its line. A line without a value (the serial
    /// before E7 reports one) is hidden, and the column closes the gap.
    rows: Vec<(AboutRow, Obj, Obj)>,
    /// The text each value shows (LVGL keeps dotted text in its buffer).
    shown: Vec<String>,
}

impl Screen for AboutScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "About", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        // device.name only changes through the setup page, which restarts waved.
        self.name = ctx.config.device.name.clone();
        let column = Obj::plain(root)
            .rect(Rect::new(
                ROW_X,
                ROW_TOP,
                ROW_W,
                ROWS.len() as i32 * ROW_PITCH,
            ))
            .column(ROW_PITCH - ROW_H);
        for row in ROWS {
            let line = Obj::plain(column).size(ROW_W, ROW_H).row_start(8, 16);
            Obj::label(line, row.label(), Font::Small, TEXT_2)
                .grow()
                .one_line();
            let value = Obj::label(line, "", Font::Body, TEXT).one_line();
            self.rows.push((row, value, line));
            self.shown.push(String::new());
        }
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        for ((row, value, line), shown) in self.rows.iter().zip(self.shown.iter_mut()) {
            let Some(text) = about_value(*row, snap, &self.name) else {
                line.hidden(true);
                continue;
            };
            line.hidden(false);
            if *shown != text {
                let width = text_width(&text, Font::Body).min(VALUE_CAP);
                value.width(width).set_text(&text);
                *shown = text;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, fixture_with, snap};
    use waved_config::WavedConfig;

    fn config(name: &str) -> WavedConfig {
        let mut config = WavedConfig::default();
        config.device.name = name.into();
        config
    }

    fn values(screen: &AboutScreen) -> Vec<String> {
        screen
            .rows
            .iter()
            .filter(|(_, _, line)| !line.is_hidden())
            .map(|(_, value, _)| value.text())
            .collect()
    }

    #[test]
    fn about_lists_the_device() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config("Wave Boardroom"));
            let mut snap = snap();
            snap.system.about.serial = "IW1A2B3C4D".into();
            snap.system.about.version = "0.1.0 (374015f)".into();
            snap.system.about.storage_total_bytes = 16_000_000_000;
            snap.system.about.storage_free_bytes = 3_200_000_000;
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_golden("screen_about", &lv.render());
            assert_targets_ok(root);
            assert_eq!(
                values(&screen),
                vec![
                    "Wave Boardroom".to_string(),
                    "0.1.0 (374015f)".into(),
                    "IW1A2B3C4D".into(),
                    "192.168.1.42".into(),
                    "3.2 GB".into(),
                ]
            );
        });
    }

    #[test]
    fn unknown_values_read_plainly() {
        let mut snap = snap();
        assert_eq!(about_value(AboutRow::Serial, &snap, ""), None);
        assert_eq!(
            about_value(AboutRow::Name, &snap, "  ").as_deref(),
            Some("Wave")
        );
        assert_eq!(
            about_value(AboutRow::Version, &snap, "").as_deref(),
            Some(format!("waved {}", env!("CARGO_PKG_VERSION")).as_str())
        );
        snap.wifi.connected = false;
        assert_eq!(
            about_value(AboutRow::Address, &snap, "").as_deref(),
            Some("Not connected")
        );
        assert_eq!(format_bytes(850_000_000), "850 MB");
        assert_eq!(format_bytes(0), "0 MB");
    }

    #[test]
    fn free_space_falls_back_to_the_files_figure() {
        let mut snap = snap();
        snap.files.free_bytes = 2_000_000_000;
        snap.system.about.storage_free_bytes = 5_000_000_000;
        // The system service has not measured the filesystem yet.
        assert_eq!(
            about_value(AboutRow::Space, &snap, "").as_deref(),
            Some("2.0 GB")
        );
        snap.system.about.storage_total_bytes = 16_000_000_000;
        assert_eq!(
            about_value(AboutRow::Space, &snap, "").as_deref(),
            Some("5.0 GB")
        );
    }

    #[test]
    fn the_serial_row_waits_for_a_serial() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_eq!(values(&screen).len(), 4);
            let space_before = screen.rows[4].2.coords().y1;
            snap.system.about.serial = "IW1A2B3C4D".into();
            screen.update(&snap);
            root.update_layout();
            assert_eq!(values(&screen).len(), 5);
            assert_eq!(values(&screen)[2], "IW1A2B3C4D");
            assert!(screen.rows[4].2.coords().y1 > space_before);
            assert_targets_ok(root);
        });
    }

    #[test]
    fn values_follow_the_snapshot_after_build() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap);
            snap.system.about.version = "0.2.0 (abc1234)".into();
            snap.wifi.connected = false;
            screen.update(&snap);
            root.update_layout();
            let shown = values(&screen);
            assert_eq!(shown[1], "0.2.0 (abc1234)");
            assert_eq!(shown[2], "Not connected");
        });
    }

    #[test]
    fn a_long_device_name_ends_in_dots() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config(
                "The large meeting room on the third floor, east wing",
            ));
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap());
            root.update_layout();
            let value = screen.rows[0].1;
            assert!(value.text().ends_with("..."), "{}", value.text());
            assert!(value.coords().x2 <= 340 - 16);
        });
    }
}
