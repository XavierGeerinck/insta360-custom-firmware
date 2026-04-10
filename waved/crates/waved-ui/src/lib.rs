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
    // Leak TouchInput onto the heap so touch_read_cb can borrow it via indev user_data.
    let touch_ptr = Box::into_raw(Box::new(TouchInput::init()?));

    unsafe {
        let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
        lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
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
        // Pass TouchInput pointer so touch_read_cb can deliver actual touch state.
        lv_indev_set_user_data(indev, touch_ptr as *mut _);
    }

    let status_bar = unsafe { StatusBar::create() };
    let mut toast  = unsafe { Toast::create() };

    let mut nav = ScreenManager::new();
    let mut home = screens::home::HomeScreen::new(Arc::clone(&bus));
    let mut recording = screens::recording::RecordingScreen::new(Arc::clone(&bus));
    let mut cc = screens::control_center::ControlCenter::new(Arc::clone(&bus));
    unsafe { cc.init(); }

    let mut prev_recording = false;

    let root = home.create();
    unsafe { lv_screen_load(root); }
    nav.set_root(root);

    let mut last_tick = Instant::now();

    // Track previous status-bar state to avoid redrawing every frame.
    let mut prev_batt:  u8   = 255; // impossible sentinel
    let mut prev_wifi:  bool = false;
    let mut prev_bt:    bool = false;
    let mut prev_muted: bool = false;

    loop {
        let elapsed_ms = last_tick.elapsed().as_millis() as u32;
        last_tick = Instant::now();

        unsafe { lv_tick_inc(elapsed_ms); }

        unsafe { (*touch_ptr).poll(); }

        {
            let batt  = battery_rx.borrow();
            let audio = audio_rx.borrow();
            let bt    = bt_rx.borrow();
            let wifi  = wifi_rx.borrow();
            let new_batt  = batt.level;
            let new_wifi  = wifi.connected;
            let new_bt    = bt.connected;
            let new_muted = audio.mic_muted;
            if new_batt != prev_batt || new_wifi != prev_wifi
                || new_bt != prev_bt || new_muted != prev_muted
            {
                unsafe { status_bar.update(new_batt, new_wifi, new_bt, new_muted); }
                prev_batt  = new_batt;
                prev_wifi  = new_wifi;
                prev_bt    = new_bt;
                prev_muted = new_muted;
            }
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(&cmd, &mut toast, &mut cc);
        }

        unsafe { lv_timer_handler(); }
        unsafe { toast.tick(); }

        // Navigate Home ↔ Recording based on meeting.recording state
        {
            let recording_active = meeting_rx.borrow().recording;
            if recording_active && !prev_recording {
                // Started recording → switch to Recording screen.
                // Hide CC first so it doesn't occlude the recording screen.
                // SAFETY: lv_screen_load is called before navigate_to deletes the old root.
                // This is safe without animations because LVGL completes the load synchronously.
                // If transition animations are ever added, this order must be revisited.
                if cc.is_visible() { unsafe { cc.hide(); } }
                let root = recording.create();
                unsafe { lv_screen_load(root); }
                nav.navigate_to(UiScreen::Recording);
                nav.set_root(root);
                prev_recording = true;
            } else if !recording_active && prev_recording {
                // Stopped recording → switch back to Home screen.
                if cc.is_visible() { unsafe { cc.hide(); } }
                let root = home.create();
                unsafe { lv_screen_load(root); }
                nav.navigate_to(UiScreen::Home);
                nav.set_root(root);
                prev_recording = false;
            }
        }

        match &nav.current {
            UiScreen::Home => {
                let audio = audio_rx.borrow().clone();
                let bt    = bt_rx.borrow().clone();
                let wifi  = wifi_rx.borrow().clone();
                unsafe { home.update_state(&audio, &bt, &wifi); }
            }
            UiScreen::Recording => {
                let audio   = audio_rx.borrow().clone();
                let meeting = meeting_rx.borrow().clone();
                unsafe { recording.update_state(&audio, &meeting); }
            }
            _ => {}
        }

        // Update control center state whenever it is visible
        if cc.is_visible() {
            let audio = audio_rx.borrow().clone();
            let wifi  = wifi_rx.borrow().clone();
            let bt    = bt_rx.borrow().clone();
            unsafe { cc.update_state(&audio, &wifi, &bt); }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn handle_command(
    cmd: &Command,
    toast: &mut Toast,
    cc: &mut screens::control_center::ControlCenter,
) {
    match cmd {
        Command::AddBookmark => {
            unsafe { toast.show("Bookmark added", 1500); }
        }
        Command::Shutdown => {
            // The actual shutdown is handled by waved-mcu which also subscribes
            // to this command. Here we just show feedback.
            unsafe { toast.show("Shutting down...", 2000); }
        }
        Command::ToggleSpeakerMute => {
            // Reused as "toggle control center" UI signal from gestures/buttons.
            // handle_command intercepts this before any audio service sees it
            // (the command channel has multiple subscribers; the audio service
            // also subscribes, but on the Wave there is currently no speaker
            // hardware, so toggling speaker mute is a no-op there).
            unsafe {
                if cc.is_visible() {
                    cc.hide();
                } else {
                    cc.show();
                }
            }
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
    indev: *mut lv_indev_t,
    data: *mut lv_indev_data_t,
) {
    let touch = lv_indev_get_user_data(indev) as *const input::TouchInput;
    if touch.is_null() {
        (*data).state = lv_indev_state_t_LV_INDEV_STATE_RELEASED;
        return;
    }
    let touch = &*touch;
    (*data).point.x = touch.x;
    (*data).point.y = touch.y;
    (*data).state = if touch.pressed {
        lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
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
