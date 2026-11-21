use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, POWERBROADCAST_SETTING,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassExW, TranslateMessage, MSG, WINDOW_EX_STYLE, WNDCLASSEXW,
    WS_OVERLAPPED,
};

const WM_POWERBROADCAST: u32 = 0x0218;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const WM_DESTROY: u32 = 0x0002;

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
const DEVICE_NOTIFY_WINDOW_HANDLE: windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS(0);

// GUID for Lid switch state: {BA3E0F4D-B817-4094-A2D1-D56379E6A0F3}
const GUID_LIDSWITCH_STATE_CHANGE: windows::core::GUID =
    windows::core::GUID::from_u128(0xba3e0f4d_b817_4094_a2d1_d56379e6a0f3);

static IS_LISTENING: AtomicBool = AtomicBool::new(false);
static POWER_WATCHER_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static GLOBAL_EC: parking_lot::RwLock<Option<crate::driver::ec_bridge::SharedEcBridge>> = parking_lot::RwLock::new(None);

/// The EC bridge handed to the power watcher at boot (None before that).
pub fn global_ec() -> Option<crate::driver::ec_bridge::SharedEcBridge> {
    GLOBAL_EC.read().clone()
}

pub fn init_power_event_watcher(ec: crate::driver::ec_bridge::SharedEcBridge) {
    *GLOBAL_EC.write() = Some(ec);
    start_power_event_watcher();
}

pub fn get_power_watcher_hwnd() -> Option<HWND> {
    let val = POWER_WATCHER_HWND.load(Ordering::SeqCst);
    if val != 0 {
        Some(HWND(val as _))
    } else {
        None
    }
}

/// Set between WM_QUERYENDSESSION and the end of the process (cleared if the shutdown is cancelled).
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);

/// True while Windows is shutting down, restarting or signing the user out.
pub fn is_session_ending() -> bool {
    SESSION_ENDING.load(Ordering::SeqCst)
}

/// Hardware hand-back for a confirmed shutdown / restart / sign-out. Runs once; a second caller
/// (the app's exit path) waits until the first one has finished.
pub fn run_session_end_shutdown() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::log_info!("POWER_EVENT", "System is shutting down or restarting: resetting display tuning, stopping lighting.");
        crate::system::telemetry::set_screen_suspended(true);
        crate::system::display_tuning::reset_display_tuning_to_default();
        crate::driver::ite_streamer::get_global_streamer().suspend_for_sleep();
        let lighting = crate::driver::ite_lighting::get_global_lighting();
        lighting.turn_off_all();
        lighting.invalidate_devices();
    });
}

fn handle_sleep_auto_office() {
    if crate::system::config::load_config().sleep_auto_office {
        crate::system::mode::begin_override(crate::system::mode::Override::Sleep, 1);
    }
}

fn handle_wake_auto_office() {
    crate::system::mode::end_override(crate::system::mode::Override::Sleep);
}

fn handle_system_resume_pipeline(event_name: &str) {
    // Windows delivers PBT_APMRESUMEAUTOMATIC and PBT_APMRESUMESUSPEND (and lid-open) back-to-back
    // on a manual wake; debounce within 3 s so we don't spawn duplicate restore threads/writes.
    static LAST_RESUME: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);
    {
        let mut last = LAST_RESUME.lock();
        if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3)) {
            crate::log_debug!("POWER_EVENT", "Duplicate resume event ({}) debounced (<3s)", event_name);
            return;
        }
        *last = Some(std::time::Instant::now());
    }

    crate::system::telemetry::set_screen_suspended(false);
    crate::log_info!("POWER_EVENT", "Laptop {}! Restoring power mode & lighting...", event_name);
    handle_wake_auto_office();

    std::thread::spawn(|| {
        crate::driver::ite_lighting::get_global_lighting().resume_after_wake();
        std::thread::sleep(std::time::Duration::from_millis(500));
        crate::system::display_tuning::init_display_tuning();
    });

    let cfg = crate::system::config::load_config();
    if let Some(ec) = GLOBAL_EC.read().clone() {
        let cfg_limit = cfg.battery_limit;
        let cfg_override = cfg.battery_hardware_limit_enabled;
        crate::log_info!("POWER_EVENT", "System resumed: Re-asserting battery limit ({}%) with override={}",
            cfg_limit, cfg_override
        );
        ec.lock().apply_battery_limit_with_override(cfg_limit, cfg_override);

        // Delayed re-assertion to defend against late ACPI battery driver initialization
        let ec_delayed = ec.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2000));
            let cfg_delayed = crate::system::config::load_config();
            if cfg_delayed.battery_hardware_limit_enabled && cfg_delayed.battery_limit < 95 {
                ec_delayed.lock().apply_battery_limit_with_override(
                    cfg_delayed.battery_limit,
                    true,
                );
            }
        });
    }

    crate::driver::wmi_ec::wmi_modify_ec_mask(
        crate::driver::wmi_ec::ADDR_FN_LOCK,
        0x10,
        cfg.fn_lock_enabled,
    );
    crate::system::switches::sync_winkey_lock_verified(cfg.win_key_locked);
    if !cfg.touchpad_enabled {
        crate::driver::wmi_ec::wmi_modify_ec_mask(
            crate::driver::wmi_ec::ADDR_BATTERY_PROTECTION_MODE,
            crate::driver::wmi_ec::TOUCHPAD_BITMASK,
            false,
        );
        crate::system::switches::touchpad::write_ptp_registry_enabled(false);
    }
    if !cfg.mic_enabled {
        let _ = crate::system::switches::audio::set_microphone_state(false);
    }
    if cfg.keep_awake_enabled {
        crate::system::switches::set_keep_awake(true);
    }
    if !cfg.wifi_enabled {
        let _ = crate::system::switches::radio::set_wifi_radio_state_native(false);
    }
    if !cfg.bluetooth_enabled {
        let _ = crate::system::switches::radio::set_native_bluetooth_radio_state(false);
    }
}

unsafe extern "system" fn power_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            let pbt = wparam.0 as u32;
            if pbt == PBT_APMSUSPEND {
                // Only Keep-Awake preserves active high-power mode during standby requests; a screen
                // blank must NOT skip sleep_auto_office or lighting suspend (otherwise unblank("standby")
                // ends ScreenBlank without a Sleep override and restores Beast/Custom right before sleep).
                let is_running_tasks = crate::system::switches::is_keep_awake();
                let was_blanked = crate::system::screen_blank::is_active();
                if is_running_tasks {
                    if was_blanked {
                        std::thread::spawn(|| crate::system::screen_blank::unblank("standby"));
                    }
                    crate::log_info!("POWER_EVENT", "Standby event received while Keep-Awake is ACTIVE. Preserving active power mode for background tasks.");
                } else {
                    crate::log_info!("POWER_EVENT", "System is entering Sleep / Hibernate / Standby! Suspending lighting...");
                    crate::system::telemetry::set_screen_suspended(true);
                    // Register Sleep override synchronously BEFORE unblank("standby") runs so ending
                    // ScreenBlank sees the Sleep override and stays in Office mode.
                    handle_sleep_auto_office();
                    if was_blanked {
                        std::thread::spawn(|| crate::system::screen_blank::unblank("standby"));
                    }
                    let streamer = crate::driver::ite_streamer::get_global_streamer();
                    streamer.suspend_for_sleep();
                    let lighting = crate::driver::ite_lighting::get_global_lighting();
                    lighting.turn_off_all();
                    lighting.invalidate_devices();
                    lighting.is_sleeping.store(true, Ordering::SeqCst);
                }
            } else if pbt == PBT_POWERSETTINGCHANGE {
                let setting_ptr = lparam.0 as *const POWERBROADCAST_SETTING;
                if !setting_ptr.is_null() {
                    let setting = &*setting_ptr;
                    if setting.PowerSetting == GUID_LIDSWITCH_STATE_CHANGE && setting.DataLength >= 4 {
                        static LAST_LID_STATE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
                        let lid_state = *(setting.Data.as_ptr() as *const u32);
                        let prev = LAST_LID_STATE.swap(lid_state as i8, Ordering::SeqCst);
                        if prev == -1 {
                            // Windows automatically posts initial state upon RegisterPowerSettingNotification.
                            // Record baseline only; NEVER trigger a false wake/resume pipeline on app startup!
                            crate::log_info!("POWER_EVENT", "Initial lid state registered: {} (startup baseline set)", if lid_state == 0 { "Closed" } else { "Open" });
                        } else if prev != lid_state as i8 {
                            // 0 = Lid Closed, 1 = Lid Open
                            if lid_state == 0 {
                                crate::system::telemetry::set_screen_suspended(true);
                                crate::log_info!("POWER_EVENT", "Laptop lid closed! Suspending lighting...");
                                handle_sleep_auto_office();
                                let streamer = crate::driver::ite_streamer::get_global_streamer();
                                streamer.suspend_for_sleep();
                                let lighting = crate::driver::ite_lighting::get_global_lighting();
                                lighting.turn_off_all();
                                lighting.invalidate_devices();
                                lighting.is_sleeping.store(true, Ordering::SeqCst);
                            } else {
                                handle_system_resume_pipeline("lid opened");
                            }
                        }
                    }
                }
            } else if pbt == PBT_APMRESUMEAUTOMATIC || pbt == PBT_APMRESUMESUSPEND {
                handle_system_resume_pipeline("resumed from Sleep / Hibernate");
            }
            LRESULT(1)
        }
        WM_DISPLAYCHANGE => {
            crate::system::display::invalidate_display_cache();
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        // Only a question at this point: another app (or the user) can still cancel the shutdown,
        // so nothing is torn down here.
        WM_QUERYENDSESSION => {
            SESSION_ENDING.store(true, Ordering::SeqCst);
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                run_session_end_shutdown();
            } else {
                SESSION_ENDING.store(false, Ordering::SeqCst);
                crate::log_info!("POWER_EVENT", "Shutdown / sign-out was cancelled; carrying on.");
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Start background Windows power broadcast watcher thread.
/// Guaranteed to catch:
/// 1. Tray quit / App exit
/// 2. Laptop Lid Close (半闭/合盖)
/// 3. Sleep / Standby (待机睡眠)
/// 4. Hibernate (休眠)
/// 5. System Shutdown / Restart (系统关机)
pub fn start_power_event_watcher() {
    if IS_LISTENING.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(|| unsafe {
        let class_name = w!("OpenRevoPowerWatcherClass");
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(power_wnd_proc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        RegisterClassExW(&wnd_class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("OpenRevoPowerWatcher"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            None,
            None,
        );

        if hwnd.is_err() {
            crate::log_error!("POWER_EVENT", "Failed to create power watcher window");
            return;
        }

        let hwnd = hwnd.unwrap();
        POWER_WATCHER_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

        // Register explicitly for Lid Switch state change notifications
        let _ = RegisterPowerSettingNotification(
            HANDLE(hwnd.0 as _),
            &GUID_LIDSWITCH_STATE_CHANGE,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );

        crate::log_info!("POWER_EVENT", "Dedicated power event listener successfully registered and running.");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
