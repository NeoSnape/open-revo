// "Screen off, keep running" for Modern Standby (S0 low power idle) machines.
// On this laptop SC_MONITORPOWER = entering Modern Standby (Kernel-Power 506, reason SC_MONITORPOWER),
// which suspends desktop apps regardless of execution-state requests. So instead of powering the panel
// down, cover every monitor with a black topmost window and drop the backlight to 0; Windows still
// considers the display on, and the watcher holds ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED meanwhile.
// While blanked the panel also drops to 60 Hz (session-only, never written to the display registry)
// and the machine to Office mode (through the EC bridge; Windows power overlay and the OEM
// MainOption.json follow as with any mode switch, config.json keeps the user's choice).
//
// Wake: a key press (low-level keyboard hook; a GetAsyncKeyState poll covers a dead hook), session
// lock, standby, `turn_on_screen` / `orv screen on`, or app exit. Mouse movement and clicks are
// swallowed by the overlay and never wake it.
//
// All transitions run under one mutex with a generation counter, so blank/unblank racing each other
// (e.g. `orv screen off; orv screen on`) can never leave an orphaned overlay or a half-applied state.
// A marker file records the saved brightness; if the process dies while blanked, the next start
// restores the backlight from it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer,
    PostMessageW, PostQuitMessage, RegisterClassW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, HCURSOR, HWND_TOPMOST, MA_NOACTIVATEANDEAT, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_SHOWWINDOW, SW_SHOW, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_KEYDOWN,
    WM_MOUSEACTIVATE, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

/// Keys are ignored this long after blanking, so the Enter/click that triggered it can't undo it.
const KEY_GRACE_MS: u64 = 800;
const BLANK_HZ: u32 = 60;
const BLANK_MODE: u8 = 1; // Office
const KEY_POLL_TIMER: usize = 1;
const WTS_SESSION_LOCK: usize = 0x7;

struct State {
    active: bool,
    generation: u64,
    hwnd: isize,
    saved_brightness: Option<u8>,
    /// The panel was switched to 60 Hz for this blank (undo = reset to the registry mode).
    changed_refresh: bool,
    saved_mode: Option<u8>,
}

static STATE: Mutex<State> = Mutex::new(State {
    active: false,
    generation: 0,
    hwnd: 0,
    saved_brightness: None,
    changed_refresh: false,
    saved_mode: None,
});
/// Lock-free mirror of `State::active` for the keyboard hook (must never block).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Monotonic tick (GetTickCount64) of the last blank; lock-free for the hook.
static BLANKED_AT_TICK: AtomicU64 = AtomicU64::new(0);

fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

fn marker_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"OpenRevo\screen_blank.marker")
}

/// Startup: if the previous process died while blanked, put the backlight back.
pub fn recover_after_crash() {
    let path = marker_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    if let Ok(level) = text.trim().parse::<u8>() {
        let ok = crate::system::display::set_brightness(level).is_ok();
        crate::log_warn!("SCREEN", "Previous run ended while the screen was blanked; brightness restored to {}% (ok={})", level, ok);
    }
    let _ = std::fs::remove_file(path);
}

fn wake_async(reason: &'static str) {
    std::thread::spawn(move || unblank(reason));
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Hide the cursor while it is over the overlay (which is everywhere).
        WM_SETCURSOR => {
            SetCursor(HCURSOR::default());
            LRESULT(1)
        }
        // Clicks land on the overlay and are discarded without activating anything.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATEANDEAT as isize),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            on_key_down();
            LRESULT(0)
        }
        // Fallback for a dead keyboard hook / no foreground: poll the physical key state.
        WM_TIMER if wparam.0 == KEY_POLL_TIMER => {
            if !crate::system::hooks::is_keyboard_hook_alive() && grace_over() && any_key_down() {
                wake_async("key press (polled)");
            }
            LRESULT(0)
        }
        // The lock screen is a separate desktop the hook cannot see; never leave the user at 0 %.
        WM_WTSSESSION_CHANGE if wparam.0 == WTS_SESSION_LOCK => {
            wake_async("session lock");
            LRESULT(0)
        }
        // Monitor plugged / resolution changed: keep covering the whole virtual screen.
        WM_DISPLAYCHANGE => {
            let (x, y, cx, cy) = virtual_screen();
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = KillTimer(hwnd, KEY_POLL_TIMER);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn grace_over() -> bool {
    tick_ms().saturating_sub(BLANKED_AT_TICK.load(Ordering::SeqCst)) >= KEY_GRACE_MS
}

fn any_key_down() -> bool {
    // 0x08..=0xFE covers every keyboard VK; 0x01..0x06 are mouse buttons and are skipped on purpose.
    (0x08..=0xFEi32).any(|vk| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 })
}

/// Black out every monitor and dim the backlight. Idempotent.
pub fn blank() -> Result<(), String> {
    let generation = {
        let mut st = STATE.lock();
        if st.active {
            return Ok(());
        }
        st.active = true;
        st.generation += 1;
        BLANKED_AT_TICK.store(tick_ms(), Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        // Watcher switches the execution state to "system + display required" on its next tick.
        crate::system::switches::set_screen_off(true);
        st.generation
    };
    if !crate::system::hooks::is_keyboard_hook_alive() {
        crate::log_warn!("SCREEN", "Keyboard hook is not running; keys are detected by polling while blanked");
    }

    std::thread::Builder::new()
        .name("openrevo-screen-blank".into())
        .spawn(move || overlay_thread(generation))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn overlay_thread(generation: u64) {
    unsafe {
        static REGISTER: std::sync::Once = std::sync::Once::new();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                lpszClassName: w!("OpenRevoScreenBlank"),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let (x, y, cx, cy) = virtual_screen();
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("OpenRevoScreenBlank"),
            w!("OpenRevo screen off"),
            WS_POPUP,
            x,
            y,
            cx,
            cy,
            None,
            None,
            None,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                crate::log_error!("SCREEN", "Black overlay could not be created: {}", e);
                abort_blank(generation);
                return;
            }
        };

        // Publish the window and dim, unless this blank was already cancelled.
        {
            let mut st = STATE.lock();
            if !st.active || st.generation != generation {
                drop(st);
                let _ = DestroyWindow(hwnd);
                return;
            }
            st.hwnd = hwnd.0 as isize;
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetTimer(hwnd, KEY_POLL_TIMER, 150, None);
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            // Backlight down only once the overlay is up, so nothing bright flashes at 0 %.
            match crate::system::display::get_brightness() {
                Ok(b) => {
                    st.saved_brightness = Some(b);
                    let _ = std::fs::write(marker_path(), b.to_string());
                    let _ = crate::system::display::set_brightness(0);
                }
                Err(e) => crate::log_warn!("SCREEN", "Could not read brightness ({}); overlay only", e),
            }
            // 60 Hz for this session only.
            let hz = crate::system::display::get_refresh_rate();
            if hz != BLANK_HZ {
                match crate::system::display::set_refresh_rate_temporary(BLANK_HZ) {
                    Ok(()) => st.changed_refresh = true,
                    Err(e) => crate::log_warn!("SCREEN", "Could not switch to {} Hz while blanked: {}", BLANK_HZ, e),
                }
            }
            if let Some(ec) = crate::system::power_listener::global_ec() {
                let mode = ec.lock().get_power_mode();
                if mode != BLANK_MODE {
                    st.saved_mode = Some(mode);
                    ec.lock().set_power_mode(BLANK_MODE);
                }
            }
            crate::log_info!(
                "SCREEN",
                "Screen blanked: overlay {}x{}, backlight {:?} -> 0, refresh {} -> {} Hz, mode {:?} -> {}; system stays awake. Any key restores.",
                cx,
                cy,
                st.saved_brightness,
                hz,
                if st.changed_refresh { BLANK_HZ } else { hz },
                st.saved_mode,
                BLANK_MODE
            );
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let mut st = STATE.lock();
        if st.hwnd == hwnd.0 as isize {
            st.hwnd = 0;
        }
    }
}

/// Overlay creation failed: undo the flags set by `blank()`.
fn abort_blank(generation: u64) {
    let mut st = STATE.lock();
    if st.active && st.generation == generation {
        st.active = false;
        st.generation += 1;
        ACTIVE.store(false, Ordering::SeqCst);
        crate::system::switches::set_screen_off(false);
    }
}

/// Remove the overlay and restore refresh rate, backlight and power mode. Idempotent.
pub fn unblank(reason: &str) {
    let mut st = STATE.lock();
    if !st.active {
        return;
    }
    st.active = false;
    st.generation += 1;
    ACTIVE.store(false, Ordering::SeqCst);

    // Refresh rate first, still behind the black overlay, so the mode-switch flicker is not seen.
    if std::mem::take(&mut st.changed_refresh) {
        if let Err(e) = crate::system::display::reset_display_mode_to_registry() {
            crate::log_warn!("SCREEN", "Could not restore the display mode: {}", e);
        }
    }
    let saved_brightness = st.saved_brightness.take();
    let brightness_ok = match saved_brightness {
        Some(level) => restore_brightness(level),
        None => true,
    };
    if st.hwnd != 0 {
        unsafe {
            let _ = PostMessageW(HWND(st.hwnd as _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    crate::system::switches::set_screen_off(false);

    // Power mode last; only if nobody changed it meanwhile, and never Beast/Custom on battery
    // (same guard as the set_power_mode command).
    if let Some(mode) = st.saved_mode.take() {
        if let Some(ec) = crate::system::power_listener::global_ec() {
            let current = ec.lock().get_power_mode();
            let cfg = crate::system::config::load_config();
            let on_dc = crate::commands::power_cmd::is_dc_round_plugged_in();
            let thin = crate::commands::power_cmd::is_thin_and_light_or_pure_igpu();
            if current != BLANK_MODE {
                crate::log_info!("SCREEN", "Power mode was changed to {} while blanked; keeping it", current);
            } else if !thin && (mode == 3 || mode == 4) && !on_dc && !cfg.unlock_battery_power_limit {
                crate::log_info!("SCREEN", "Not restoring mode {} on battery; staying in Office", mode);
            } else {
                ec.lock().set_power_mode(mode);
            }
        }
    }
    drop(st);
    if brightness_ok {
        let _ = std::fs::remove_file(marker_path());
    }
    crate::log_info!("SCREEN", "Screen restored ({}); brightness {:?} (ok={})", reason, saved_brightness, brightness_ok);
}

/// Set and read back the backlight; keep retrying in the background if the IOCTL is not ready
/// (e.g. right after resume). Returns whether it is confirmed now.
fn restore_brightness(level: u8) -> bool {
    let confirmed = move || crate::system::display::get_brightness().map_or(false, |b| b == level);
    for _ in 0..3 {
        if crate::system::display::set_brightness(level).is_ok() && confirmed() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    crate::log_error!("SCREEN", "Brightness restore to {}% not confirmed; retrying in the background", level);
    std::thread::spawn(move || {
        for _ in 0..15 {
            std::thread::sleep(Duration::from_secs(2));
            if is_active() {
                return; // a new blank owns the backlight now
            }
            if crate::system::display::set_brightness(level).is_ok() && confirmed() {
                let _ = std::fs::remove_file(marker_path());
                crate::log_info!("SCREEN", "Brightness restored to {}% on retry", level);
                return;
            }
        }
        crate::log_error!("SCREEN", "Brightness restore to {}% failed; marker kept for the next start", level);
    });
    false
}

/// Called by the low-level keyboard hook for every key-down while blanked.
/// Returns true if the key should be swallowed (it never reaches the app underneath).
/// Must not block: only reads atomics and spawns the actual restore.
pub fn on_key_down() -> bool {
    if !is_active() {
        return false;
    }
    if grace_over() {
        wake_async("key press");
    }
    true
}
