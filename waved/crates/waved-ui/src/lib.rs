pub mod audio_fx;
pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod status_bar;
pub mod theme;
pub mod toast;
mod upload_notice;

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
use upload_notice::UploadNoticeTracker;

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
        Self {
            thread_handle: Some(handle),
        }
    }

    pub fn join(mut self) {
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
    }
}

fn ui_thread_main(
    bus: Arc<StateBus>,
    config: WavedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");

    let audio_rx = bus.audio.subscribe();
    let battery_rx = bus.battery.subscribe();
    let bt_rx = bus.bt.subscribe();
    let wifi_rx = bus.wifi.subscribe();
    let meeting_rx = bus.meeting.subscribe();

    let mut cmd_rx = bus.commands.subscribe();
    let mut event_rx = bus.events.subscribe();

    let _i18n = I18n::load(&config.device.language);

    unsafe {
        lv_init();
        lv_fs_posix_init(); // register 'A:' → POSIX FS for PNG asset loading
    }

    let fb = FramebufferDisplay::init()?;
    let touch_ptr = Box::into_raw(Box::new(TouchInput::init()?));

    unsafe {
        let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
        lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
        lv_display_set_flush_cb(disp, Some(flush_cb));

        let buf1_ptr = fb.buf1.as_ptr() as *mut _;
        let buf2_ptr = fb.buf2.as_ptr() as *mut _;
        let buf_len = fb.buf1.len() as u32;
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
        lv_indev_set_user_data(indev, touch_ptr as *mut _);
    }

    let status_bar = unsafe { StatusBar::create() };
    let mut toast = unsafe { Toast::create() };

    let mut nav = ScreenManager::new();
    let mut home = screens::home::HomeScreen::new(Arc::clone(&bus));
    let mut recording = screens::recording::RecordingScreen::new(Arc::clone(&bus));
    let mut settings = screens::settings::SettingsScreen::new(Arc::clone(&bus));
    let mut audio_mode = screens::audio_mode::AudioModeScreen::new(Arc::clone(&bus));
    let mut noise_red = screens::noise_reduction::NoiseReductionScreen::new(Arc::clone(&bus));
    let mut system = screens::system::SystemScreen::new(Arc::clone(&bus));
    let mut webhook = screens::webhook::WebhookScreen::new(Arc::clone(&bus), config.clone());
    let mut wifi_prov = screens::wifi_provisioning::WifiProvisioningScreen::new(Arc::clone(&bus));
    let mut volume = screens::volume::VolumeScreen::new(Arc::clone(&bus));
    let mut confirmation = screens::confirmation::ConfirmationScreen::new(Arc::clone(&bus));

    // Track which non-settings screen to return to when settings closes.
    let mut return_to_recording = false;
    let mut prev_recording = false;
    let mut upload_notices = UploadNoticeTracker::default();

    let root = home.create();
    unsafe {
        lv_screen_load(root);
    }
    nav.set_root(root);

    let mut last_tick = Instant::now();

    let mut prev_batt: u8 = 255;
    let mut prev_wifi: bool = false;
    let mut prev_bt: bool = false;
    let mut prev_muted: bool = false;
    // Seeded on the first tick, then drives the transient volume toast.
    // `None` means we haven't observed any value yet (suppress the first
    // tick's "change" from default vs real boot state).
    let mut prev_speaker_vol: Option<u16> = None;
    let mut prev_speaker_muted: Option<bool> = None;

    loop {
        let elapsed_ms = last_tick.elapsed().as_millis() as u32;
        last_tick = Instant::now();

        unsafe {
            lv_tick_inc(elapsed_ms);
        }
        unsafe {
            (*touch_ptr).poll();
        }

        {
            let batt = battery_rx.borrow();
            let audio = audio_rx.borrow();
            let bt = bt_rx.borrow();
            let wifi = wifi_rx.borrow();
            let new_batt = batt.level;
            let new_wifi = wifi.connected;
            let new_bt = bt.connected;
            let new_muted = audio.mic_muted;
            if new_batt != prev_batt
                || new_wifi != prev_wifi
                || new_bt != prev_bt
                || new_muted != prev_muted
            {
                // wifi.signal_strength now carries real RSSI (see waved-wifi); icon
                // tiering by strength is a follow-up UI-asset task.
                unsafe {
                    status_bar.update(new_batt, new_wifi, new_bt, new_muted);
                }
                prev_batt = new_batt;
                prev_wifi = new_wifi;
                prev_bt = new_bt;
                prev_muted = new_muted;
            }

            // Transient volume toast: fires on any change to speaker
            // volume or mute, anywhere except when the user is already
            // looking at the Volume screen (which renders its own bar).
            let new_speaker_vol = audio.speaker_volume;
            let new_speaker_muted = audio.speaker_muted;
            let vol_changed = prev_speaker_vol.is_some_and(|p| p != new_speaker_vol);
            let mute_changed = prev_speaker_muted.is_some_and(|p| p != new_speaker_muted);
            if (vol_changed || mute_changed) && nav.current != UiScreen::Volume {
                let msg = if new_speaker_muted || new_speaker_vol == 0 {
                    "Volume: Off".to_string()
                } else {
                    format!("Volume: {}%", new_speaker_vol)
                };
                unsafe {
                    toast.show(&msg, 1200);
                }
            }
            prev_speaker_vol = Some(new_speaker_vol);
            prev_speaker_muted = Some(new_speaker_muted);
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(
                &cmd,
                &bus,
                &mut toast,
                &mut upload_notices,
                &mut nav,
                &mut home,
                &mut recording,
                &mut settings,
                &mut audio_mode,
                &mut noise_red,
                &mut system,
                &mut webhook,
                &mut wifi_prov,
                &mut volume,
                &mut return_to_recording,
                &mut prev_recording,
            );
        }

        while let Ok(event) = event_rx.try_recv() {
            if let Some(upload_toast) = upload_notices.handle(&event) {
                unsafe {
                    toast.show(upload_toast.message(), upload_toast.duration_ms());
                }
            }
        }

        unsafe {
            lv_timer_handler();
        }
        unsafe {
            toast.tick();
        }

        // Auto-navigate based on meeting state. Three states drive the
        // top-level recording flow:
        //   recording=true                            → RecordingScreen
        //   recording=false, pending_decision=Some(_) → ConfirmationScreen
        //   recording=false, pending_decision=None    → HomeScreen
        // Only fires from Home / Recording / Confirmation; settings
        // sub-pages keep the user where they are.
        let snap = meeting_rx.borrow().clone();
        let on_recording_axis = matches!(
            nav.current,
            UiScreen::Home | UiScreen::Recording | UiScreen::Confirmation,
        );
        if on_recording_axis {
            let want = match (snap.recording, snap.pending_decision.is_some()) {
                (true, _) => UiScreen::Recording,
                (false, true) => UiScreen::Confirmation,
                (false, false) => UiScreen::Home,
            };
            if want != nav.current {
                let root = match want {
                    UiScreen::Recording => {
                        prev_recording = true;
                        recording.create()
                    }
                    UiScreen::Confirmation => {
                        prev_recording = false;
                        confirmation.set_decision(snap.pending_decision.clone().unwrap());
                        confirmation.create()
                    }
                    UiScreen::Home => {
                        prev_recording = false;
                        home.create()
                    }
                    _ => unreachable!(),
                };
                unsafe {
                    lv_screen_load(root);
                }
                nav.navigate_to(want);
                nav.set_root(root);
            }
        }

        match &nav.current {
            UiScreen::Home => {
                let audio = audio_rx.borrow().clone();
                let bt = bt_rx.borrow().clone();
                let wifi = wifi_rx.borrow().clone();
                unsafe {
                    home.update_state(&audio, &bt, &wifi);
                }
            }
            UiScreen::Recording => {
                let audio = audio_rx.borrow().clone();
                let meeting = meeting_rx.borrow().clone();
                unsafe {
                    recording.update_state(&audio, &meeting);
                }
            }
            UiScreen::Settings => {}
            UiScreen::System => {}
            UiScreen::Webhook => {
                let meeting = meeting_rx.borrow().clone();
                unsafe {
                    webhook.update_state(&meeting);
                }
            }
            UiScreen::Volume => {
                let audio = audio_rx.borrow().clone();
                unsafe {
                    volume.update_state(&audio);
                }
            }
            UiScreen::AudioMode => {
                let audio = audio_rx.borrow().clone();
                unsafe {
                    audio_mode.update_state(&audio);
                }
            }
            UiScreen::NoiseReduction => {
                let audio = audio_rx.borrow().clone();
                unsafe {
                    noise_red.update_state(&audio);
                }
            }
            UiScreen::Confirmation => {
                let meeting = meeting_rx.borrow().clone();
                unsafe {
                    confirmation.update_state(&meeting);
                }
            }
            UiScreen::WifiProvisioning => {
                let prov = bus.provisioning.borrow().clone();
                let wifi = wifi_rx.borrow().clone();
                unsafe {
                    wifi_prov.update_state(&prov, &wifi);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: &Command,
    bus: &Arc<StateBus>,
    toast: &mut Toast,
    upload_notices: &mut UploadNoticeTracker,
    nav: &mut ScreenManager,
    home: &mut screens::home::HomeScreen,
    recording: &mut screens::recording::RecordingScreen,
    settings: &mut screens::settings::SettingsScreen,
    audio_mode: &mut screens::audio_mode::AudioModeScreen,
    noise_red: &mut screens::noise_reduction::NoiseReductionScreen,
    system: &mut screens::system::SystemScreen,
    webhook: &mut screens::webhook::WebhookScreen,
    wifi_prov: &mut screens::wifi_provisioning::WifiProvisioningScreen,
    volume: &mut screens::volume::VolumeScreen,
    return_to_recording: &mut bool,
    prev_recording: &mut bool,
) {
    // UI-thread audio feedback: pspapp's business daemon plays these stock WAVs
    // on matching IPC commands. waved doesn't run that daemon, so spawn `aplay`
    // inline for the same effect. Fire-and-forget (see audio_fx::play docs).
    match cmd {
        Command::ToggleMicMute => {
            // State watch reflects pre-toggle value here; audio service updates it
            // asynchronously after receiving the command.
            if bus.audio.borrow().mic_muted {
                audio_fx::play(audio_fx::SFX_MIC_UNMUTE);
            } else {
                audio_fx::play(audio_fx::SFX_MIC_MUTE);
            }
        }
        Command::SetSpeakerVolume(_) | Command::ToggleSpeakerMute => {
            audio_fx::play(audio_fx::SFX_SPK_VOL);
        }
        Command::StartRecording => {
            audio_fx::play(audio_fx::SFX_AIRECORD);
        }
        Command::Shutdown => {
            audio_fx::play(audio_fx::SFX_SHUTDOWN);
        }
        _ => {}
    }

    match cmd {
        Command::StopRecording => {
            if let Some(id) = bus.meeting.borrow().meeting_id {
                upload_notices.track(id);
            }
            unsafe {
                toast.show("Saving recording...", 1_500);
            }
        }
        Command::AddBookmark => unsafe {
            toast.show("Bookmark added", 1500);
        },
        Command::Shutdown => unsafe {
            toast.show("Shutting down...", 2000);
        },
        Command::OpenSettings => {
            *return_to_recording = nav.current == UiScreen::Recording;
            let root = settings.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::Settings);
            nav.set_root(root);
        }
        Command::OpenSystemSettings => {
            let root = system.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::System);
            nav.set_root(root);
        }
        Command::OpenWifiSettings => {
            wifi_prov.open_management();
            let root = wifi_prov.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::WifiProvisioning);
            nav.set_root(root);
        }
        Command::OpenWebhookStatus => {
            let root = webhook.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::Webhook);
            nav.set_root(root);
        }
        Command::OpenWifiProvisioning => {
            wifi_prov.begin_provisioning();
            let root = wifi_prov.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::WifiProvisioning);
            nav.set_root(root);
        }
        Command::OpenVolume => {
            // Remember return context (Home vs Recording) just like OpenSettings does.
            *return_to_recording = nav.current == UiScreen::Recording;
            let root = volume.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::Volume);
            nav.set_root(root);
        }
        Command::CloseSettings => {
            match nav.current {
                UiScreen::AudioMode => {
                    // AudioMode → back to More screen.
                    let root = settings.create();
                    unsafe {
                        lv_screen_load(root);
                    }
                    nav.navigate_to(UiScreen::Settings);
                    nav.set_root(root);
                }
                UiScreen::System => {
                    // System → back to More screen.
                    let root = settings.create();
                    unsafe {
                        lv_screen_load(root);
                    }
                    nav.navigate_to(UiScreen::Settings);
                    nav.set_root(root);
                }
                UiScreen::WifiProvisioning => {
                    // Wi-Fi provisioning → back to System settings (entered from there).
                    let root = system.create();
                    unsafe {
                        lv_screen_load(root);
                    }
                    nav.navigate_to(UiScreen::System);
                    nav.set_root(root);
                }
                UiScreen::Webhook => {
                    let root = system.create();
                    unsafe {
                        lv_screen_load(root);
                    }
                    nav.navigate_to(UiScreen::System);
                    nav.set_root(root);
                }
                UiScreen::NoiseReduction => {
                    // NR → back to Home or Recording (entered from Home shortcut).
                    let _ = noise_red;
                    if *return_to_recording {
                        let root = recording.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Recording);
                        nav.set_root(root);
                        *prev_recording = true;
                    } else {
                        let root = home.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Home);
                        nav.set_root(root);
                    }
                }
                UiScreen::Volume => {
                    // Volume → back to Home or Recording (entered from there, not via More).
                    let _ = volume;
                    if *return_to_recording {
                        let root = recording.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Recording);
                        nav.set_root(root);
                        *prev_recording = true;
                    } else {
                        let root = home.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Home);
                        nav.set_root(root);
                    }
                }
                UiScreen::Settings => {
                    // More → back to Home or Recording.
                    if *return_to_recording {
                        let root = recording.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Recording);
                        nav.set_root(root);
                        *prev_recording = true;
                    } else {
                        let root = home.create();
                        unsafe {
                            lv_screen_load(root);
                        }
                        nav.navigate_to(UiScreen::Home);
                        nav.set_root(root);
                    }
                }
                _ => {}
            }
        }
        Command::OpenAudioMode => {
            let root = audio_mode.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::AudioMode);
            nav.set_root(root);
        }
        Command::OpenNoiseReduction => {
            // Remember return context (Home vs Recording) so CloseSettings
            // takes us back to the right place — entered from Home shortcut.
            *return_to_recording = nav.current == UiScreen::Recording;
            let root = noise_red.create();
            unsafe {
                lv_screen_load(root);
            }
            nav.navigate_to(UiScreen::NoiseReduction);
            nav.set_root(root);
        }
        _ => {}
    }
}

unsafe extern "C" fn flush_cb(disp: *mut lv_display_t, area: *const lv_area_t, px_map: *mut u8) {
    let fb = lv_display_get_user_data(disp) as *mut display::FramebufferDisplay;
    if !fb.is_null() {
        (*fb).flush(&*area, px_map as *const u8);
    }
    lv_display_flush_ready(disp);
}

unsafe extern "C" fn touch_read_cb(indev: *mut lv_indev_t, data: *mut lv_indev_data_t) {
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
