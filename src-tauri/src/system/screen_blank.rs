// "Screen off, keep running" for Modern Standby (S0 low power idle) machines.
// On this laptop SC_MONITORPOWER = entering Modern Standby (Kernel-Power 506, reason SC_MONITORPOWER),
// which suspends desktop apps regardless of execution-state requests. So instead of powering the panel
// down, cover every monitor with a black topmost window and drop the backlight to 0; Windows still
// considers the display on, and the watcher holds ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED meanwhile.
// While blanked the panel also drops to 60 Hz (session-only, never written to the display registry)
// and the machine to Office mode (applied synchronously; Windows power overlay and the OEM
// MainOption.json follow as with any mode switch, config.json keeps the user's choice).
//
// Wake: a key press (low-level keyboard hook, plus a GetAsyncKeyState poll in case Windows dropped the
// hook), session lock, standby, `turn_on_screen` / `orv screen on`, or app exit. Mouse movement and
// clicks are swallowed by the overlay and never wake it.
//
// Locking:
// - STATE: quick field updates only (plus the screen-off flag, so it always matches `active`);
//   never held across I/O, a window call or a sleep.
// - TRANSITION: serialises the slow parts (overlay setup, restore). Only worker threads wait on it;
//   the main thread and the keyboard hook never do (exit uses a timeout). A restore takes TRANSITION
//   *before* it deactivates, so no new blank's setup can slip in between and be undone by mistake.
// A marker file records the saved brightness; if the process dies while blanked, the next start
// restores the backlight and the display mode from it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
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

/// Backlight levels as the display driver stores them: (on AC, on battery).
type Brightness = (u8, u8);

struct State {
    active: bool,
    generation: u64,
    hwnd: isize,
    saved_brightness: Option<Brightness>,
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
static TRANSITION: Mutex<()> = Mutex::new(());
/// Lock-free mirror of `State::active` for the keyboard hook (must never block).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Monotonic tick (GetTickCount64) of the last blank; lock-free for the hook.
static BLANKED_AT_TICK: AtomicU64 = AtomicU64::new(0);
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

fn is_current(generation: u64) -> bool {
    let st = STATE.lock();
    st.active && st.generation == generation
}

fn marker_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"OpenRevo\screen_blank.marker")
}

fn parse_marker(text: &str) -> Option<Brightness> {
    let mut it = text.trim().split(',').map(|v| v.trim().parse::<u8>());
    match (it.next(), it.next()) {
        (Some(Ok(ac)), Some(Ok(dc))) => Some((ac, dc)),
        (Some(Ok(both)), None) => Some((both, both)), // older single-value marker
        _ => None,
    }
}

/// Set both backlight levels; falls back to the single-level call if the driver rejects the pair.
fn apply_brightness(level: Brightness) -> bool {
    crate::system::display::set_brightness_levels(level.0, level.1).is_ok()
        || crate::system::display::set_brightness(level.0.max(level.1)).is_ok()
}

fn brightness_is(level: Brightness) -> bool {
    match crate::system::display::get_brightness_levels() {
        Ok(cur) => cur == level || (cur.0.max(cur.1) == level.0.max(level.1) && level.0 != level.1 && cur.0 == cur.1),
        Err(_) => false,
    }
}

/// Startup: if the previous process died while blanked, put the backlight and display mode back.
/// The marker is only removed once the backlight is confirmed, so a failure is retried next start.
pub fn recover_after_crash() {
    let path = marker_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    // The 60 Hz drop was session-only; make sure the panel is back on the registry (user) mode.
    let _ = crate::system::display::reset_display_mode_to_registry();
    match parse_marker(&text) {
        Some(level) => {
            crate::log_warn!("SCREEN", "Previous run ended while the screen was blanked; restoring brightness {:?}", level);
            // Right after start the driver can lag; restore_brightness reads back, retries, and only
            // removes the marker once the level is confirmed.
            restore_brightness(level, STATE.lock().generation);
        }
        None => {
            let _ = std::fs::remove_file(path); // unreadable marker: nothing to restore
        }
    }
}

fn wake_async(reason: &'static str) {
    if std::thread::Builder::new().name("openrevo-screen-wake".into()).spawn(move || unblank(reason)).is_err() {
        crate::log_error!("SCREEN", "Could not start a restore thread ({}); use `orv screen on`", reason);
    }
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
        // Independent of the hook: Windows silently removes a low-level hook that ever times out, and
        // a key swallowed by a live hook never reaches the async key state, so this only fires when
        // the hook is not doing its job.
        WM_TIMER if wparam.0 == KEY_POLL_TIMER => {
            if is_active() && grace_over() && any_key_down() {
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
            let _ = WTSUnRegisterSessionNotification(hwnd);
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

/// Apply a power mode synchronously (EcBridge::set_power_mode spawns its own thread, which would let
/// a blank's Office switch and the restore race each other).
fn apply_mode_now(mode: u8) {
    if let Some(ec) = crate::system::power_listener::global_ec() {
        ec.lock().set_power_mode_cache(mode);
    }
    crate::system::power_mode::apply_full_power_mode(mode);
}

/// Black out every monitor and dim the backlight. Idempotent; returns immediately (work runs on a worker).
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
    if let Err(e) = std::thread::Builder::new()
        .name("openrevo-screen-blank".into())
        .spawn(move || overlay_thread(generation))
    {
        abort_blank(generation);
        return Err(format!("could not start the screen-blank thread: {}", e));
    }
    Ok(())
}

fn overlay_thread(generation: u64) {
    let transition = TRANSITION.lock();
    if !is_current(generation) {
        return; // cancelled before we started
    }
    unsafe {
        if !CLASS_REGISTERED.load(Ordering::SeqCst) {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                lpszClassName: w!("OpenRevoScreenBlank"),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            // 0 also covers "class already exists"; CreateWindowExW below is the real check.
            if RegisterClassW(&wc) != 0 {
                CLASS_REGISTERED.store(true, Ordering::SeqCst);
            }
        }
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
        {
            let mut st = STATE.lock();
            if !(st.active && st.generation == generation) {
                drop(st);
                let _ = DestroyWindow(hwnd);
                return;
            }
            st.hwnd = hwnd.0 as isize;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
        if SetTimer(hwnd, KEY_POLL_TIMER, 150, None) == 0 {
            crate::log_warn!("SCREEN", "Key poll timer could not be started; wake relies on the keyboard hook");
        }
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);

        // Slow changes, each recorded as soon as it is made so a restore (waiting on TRANSITION)
        // undoes exactly what was done. Stop early if the blank was cancelled meanwhile.
        if is_current(generation) {
            // Backlight down only once the overlay is up, so nothing bright flashes at 0 %.
            match crate::system::display::get_brightness_levels() {
                Ok(level) => {
                    if let Err(e) = std::fs::write(marker_path(), format!("{},{}", level.0, level.1)) {
                        crate::log_warn!("SCREEN", "Could not write the blank marker: {}", e);
                    }
                    STATE.lock().saved_brightness = Some(level);
                    apply_brightness((0, 0));
                }
                Err(e) => crate::log_warn!("SCREEN", "Could not read brightness ({}); overlay only", e),
            }
        }
        let hz = crate::system::display::get_refresh_rate();
        if is_current(generation) && hz != BLANK_HZ {
            match crate::system::display::set_refresh_rate_temporary(BLANK_HZ) {
                Ok(()) => STATE.lock().changed_refresh = true,
                Err(e) => crate::log_warn!("SCREEN", "Could not switch to {} Hz while blanked: {}", BLANK_HZ, e),
            }
        }
        if is_current(generation) {
            if let Some(ec) = crate::system::power_listener::global_ec() {
                let mode = ec.lock().get_power_mode();
                if mode != BLANK_MODE {
                    STATE.lock().saved_mode = Some(mode);
                    apply_mode_now(BLANK_MODE);
                }
            }
        }
        {
            let st = STATE.lock();
            crate::log_info!(
                "SCREEN",
                "Screen blanked: overlay {}x{}, backlight {:?} -> 0, refresh {} Hz -> {}, mode {:?} -> {}; system stays awake. Any key restores.",
                cx,
                cy,
                st.saved_brightness,
                hz,
                if st.changed_refresh { "60 Hz" } else { "unchanged" },
                st.saved_mode,
                BLANK_MODE
            );
        }
        drop(transition); // setup done; a restore may run now

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0).0;
            if r == 0 || r == -1 {
                break; // WM_QUIT, or an error that would otherwise spin forever
            }
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

/// Flip the state to "not blanked". Returns false if it was not blanked.
fn deactivate() -> bool {
    let mut st = STATE.lock();
    if !st.active {
        return false;
    }
    st.active = false;
    st.generation += 1;
    ACTIVE.store(false, Ordering::SeqCst);
    true
}

/// Remove the overlay and restore refresh rate, backlight and power mode. Idempotent.
/// Runs on worker threads (wake_async / turn_on_screen); waits for an overlay setup in progress.
pub fn unblank(reason: &str) {
    if !is_active() {
        return;
    }
    // TRANSITION first: an overlay setup in progress finishes and records what it changed, and no
    // new blank can start its setup until this restore is done.
    let _transition = TRANSITION.lock();
    if !deactivate() {
        return;
    }
    restore(reason);
}

/// App exit (main thread): never wait long for a worker, which could be inside a display-mode
/// broadcast that needs this very thread. Falls back to restoring the backlight directly.
pub fn unblank_on_exit() {
    if !is_active() {
        return;
    }
    match TRANSITION.try_lock_for(Duration::from_millis(1500)) {
        Some(_transition) => {
            if deactivate() {
                restore("exit");
            }
        }
        None => {
            deactivate();
            let saved = STATE.lock().saved_brightness;
            if let Some(level) = saved {
                apply_brightness(level);
            }
            crate::log_warn!(
                "SCREEN",
                "Exit while a screen-blank change was in progress: backlight set back to {:?}; refresh rate and mode are restored by the next start (marker kept)",
                saved
            );
        }
    }
}

/// Undo what the last blank recorded. Caller holds TRANSITION; STATE is only taken for short sections.
fn restore(reason: &str) {
    let (changed_refresh, saved_brightness, hwnd, saved_mode, generation) = {
        let mut st = STATE.lock();
        (
            std::mem::take(&mut st.changed_refresh),
            st.saved_brightness.take(),
            st.hwnd,
            st.saved_mode.take(),
            st.generation,
        )
    };
    // Refresh rate first, still behind the black overlay, so the mode-switch flicker is not seen.
    if changed_refresh {
        if let Err(e) = crate::system::display::reset_display_mode_to_registry() {
            crate::log_warn!("SCREEN", "Could not restore the display mode: {}", e);
        }
    }
    let brightness_ok = match saved_brightness {
        Some(level) => restore_brightness(level, generation),
        None => true,
    };
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(HWND(hwnd as _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    // Release the display hold, atomically with the state so a new blank can't be overwritten.
    {
        let st = STATE.lock();
        if !st.active {
            crate::system::switches::set_screen_off(false);
        }
    }
    // Power mode last; only if nobody changed it meanwhile, and never Beast/Custom on battery
    // (same guard as the set_power_mode command).
    if let Some(mode) = saved_mode {
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
                apply_mode_now(mode);
            }
        }
    }
    if brightness_ok {
        let _ = std::fs::remove_file(marker_path());
    }
    crate::log_info!("SCREEN", "Screen restored ({}); brightness {:?} (ok={})", reason, saved_brightness, brightness_ok);
}

/// Set and read back the backlight; keep retrying in the background if the IOCTL is not ready
/// (e.g. right after resume). Returns whether it is confirmed now.
fn restore_brightness(level: Brightness, generation: u64) -> bool {
    for _ in 0..3 {
        if apply_brightness(level) {
            // The driver reports the new level with a short delay.
            for _ in 0..3 {
                if brightness_is(level) {
                    let _ = std::fs::remove_file(marker_path());
                    return true;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    crate::log_error!("SCREEN", "Brightness restore to {:?} not confirmed; retrying in the background", level);
    std::thread::spawn(move || {
        for _ in 0..15 {
            std::thread::sleep(Duration::from_secs(2));
            // A newer blank (or its restore) owns the backlight now.
            if STATE.lock().generation != generation || is_active() {
                return;
            }
            if apply_brightness(level) && brightness_is(level) {
                let _ = std::fs::remove_file(marker_path());
                crate::log_info!("SCREEN", "Brightness restored to {:?} on retry", level);
                return;
            }
        }
        crate::log_error!("SCREEN", "Brightness restore to {:?} failed; marker kept for the next start", level);
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
