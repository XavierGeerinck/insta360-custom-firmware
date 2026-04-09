pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod status_bar;
pub mod theme;
pub mod toast;

use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info};
use waved_config::WavedConfig;
use waved_state::{Command, StateBus};

use bindings::*;
use display::FramebufferDisplay;
use i18n::I18n;
use input::TouchInput;
use nav::{ScreenManager, UiScreen};
use screens::Screen;
use status_bar::StatusBar;
use toast::Toast;

pub struct UiService {
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl UiService {
    pub fn new(bus: Arc<StateBus>, config: WavedConfig) -> Self {
        let handle = std::thread::Builder::new()
            .name("ui-lvgl".into())
            .spawn(move || {
                if let Err(e) = ui_thread_main(bus, config) {
                    error!("UI thread error: {e}");
                }
            })
            .expect("failed to spawn ui-lvgl thread");
        Self { thread_handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.thread_handle.take() { let _ = h.join(); }
    }
}

fn ui_thread_main(
    bus: Arc<StateBus>,
    config: WavedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");

    let audio_rx   = bus.audio.subscribe();
    let battery_rx = bus.battery.subscribe();
    let bt_rx      = bus.bt.subscribe();
    let wifi_rx    = bus.wifi.subscribe();
    let meeting_rx = bus.meeting.subscribe();

    let mut cmd_rx = bus.commands.subscribe();

    let _i18n = I18n::load(&config.device.language);

    unsafe {
        lv_init();
        lv_fs_posix_init(); // register 'A:' → POSIX FS for PNG asset loading
    }

    let fb    = FramebufferDisplay::init()?;
    let mut touch = TouchInput::init()?;

    unsafe {
        let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
        lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
        lv_display_set_rotation(disp, lv_display_rotation_t_LV_DISPLAY_ROTATION_180);
        lv_display_set_flush_cb(disp, Some(flush_cb));

        // Extract buffer pointers before boxing fb (Box doesn't move heap data).
        let buf1_ptr = fb.buf1.as_ptr() as *mut _;
        let buf2_ptr = fb.buf2.as_ptr() as *mut _;
        let buf_len  = fb.buf1.len() as u32;
        // Leak fb onto the heap so flush_cb can borrow it via user_data.
        let fb_raw = Box::into_raw(Box::new(fb));
        lv_display_set_user_data(disp, fb_raw as *mut _);

        lv_display_set_buffers(
            disp,
            buf1_ptr,
            buf2_ptr,
            buf_len,
            lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
        );

        let indev = lv_indev_create();
        lv_indev_set_type(indev, lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        lv_indev_set_read_cb(indev, Some(touch_read_cb));
    }

    let status_bar = unsafe { StatusBar::create() };
    let mut toast  = unsafe { Toast::create() };

    let mut nav = ScreenManager::new();
    let mut home = screens::home::HomeScreen::new(Arc::clone(&bus));

    let root = unsafe { home.create() };
    unsafe { lv_screen_load(root); }
    nav.set_root(root);

    let mut last_tick = Instant::now();

    loop {
        let elapsed_ms = last_tick.elapsed().as_millis() as u32;
        last_tick = Instant::now();

        unsafe { lv_tick_inc(elapsed_ms); }

        touch.poll();

        unsafe {
            let batt  = battery_rx.borrow();
            let audio = audio_rx.borrow();
            let bt    = bt_rx.borrow();
            let wifi  = wifi_rx.borrow();
            status_bar.update(batt.level, wifi.connected, bt.connected, audio.mic_muted);
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(&cmd, &mut nav, &mut toast);
        }

        unsafe { lv_timer_handler(); }
        unsafe { toast.tick(); }

        match &nav.current {
            UiScreen::Home => {
                let audio   = audio_rx.borrow().clone();
                let meeting = meeting_rx.borrow().clone();
                unsafe { home.update_state(&audio, &meeting); }
            }
            _ => {}
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn handle_command(cmd: &Command, _nav: &mut ScreenManager, toast: &mut Toast) {
    match cmd {
        Command::AddBookmark => {
            unsafe { toast.show("Bookmark added", 1500); }
        }
        _ => {}
    }
}

unsafe extern "C" fn flush_cb(
    disp: *mut lv_display_t,
    area: *const lv_area_t,
    px_map: *mut u8,
) {
    let fb = lv_display_get_user_data(disp) as *mut display::FramebufferDisplay;
    if !fb.is_null() {
        (*fb).flush(&*area, px_map as *const u8);
    }
    lv_display_flush_ready(disp);
}

unsafe extern "C" fn touch_read_cb(
    _indev: *mut lv_indev_t,
    data: *mut lv_indev_data_t,
) {
    (*data).state = lv_indev_state_t_LV_INDEV_STATE_RELEASED;
}

#[cfg(test)]
mod tests {
    use super::nav::*;

    #[test]
    fn screen_manager_starts_at_home() {
        let nav = ScreenManager::new();
        assert_eq!(nav.current, UiScreen::Home);
    }
}
