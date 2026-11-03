// "Screen off, keep running" for Modern Standby (S0 low power idle) machines.
// On this laptop SC_MONITORPOWER = entering Modern Standby (Kernel-Power 506, reason SC_MONITORPOWER),
// which suspends desktop apps regardless of execution-state requests. So instead of powering the panel
// down, cover every monitor with a black topmost window and drop the backlight to 0; Windows still
// considers the display on, and the watcher holds ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED meanwhile.
// While blanked the panel also drops to 60 Hz (session-only, never written to the display registry)
// and the machine to Office mode; both are put back on restore unless changed in the meantime.
//
// Wake: a key press (low-level keyboard hook, plus a GetAsyncKeyState poll in case Windows dropped the
// hook), session lock, standby, `turn_on_screen` / `orv screen on`, or app exit. Mouse movement and
// clicks are swallowed by the overlay and never wake it.
//
// Design: one dedicated thread owns everything — the overlay window, its message loop, and every
// blank/restore step, executed in order. Everyone else only posts it a message (never blocks), so
// there are no locks and no interleaving to reason about. A marker file holds the saved backlight so
// the next start can put it back if the process dies while blanked.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer,
    PeekMessageW, PostThreadMessageW, RegisterClassW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos,
    ShowWindow, TranslateMessage, HCURSOR, HWND_TOPMOST, MA_NOACTIVATEANDEAT, MSG, PM_NOREMOVE, PM_REMOVE, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_SHOWWINDOW, SW_SHOW, WM_APP, WM_DISPLAYCHANGE,
    WM_KEYDOWN, WM_MOUSEACTIVATE, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WM_USER, WM_WTSSESSION_CHANGE, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// Keys are ignored this long after blanking, so the Enter/click that triggered it can't undo it.
const KEY_GRACE_MS: u64 = 800;
const BLANK_HZ: u32 = 60;
const BLANK_MODE: u8 = 1; // Office
const KEY_POLL_TIMER: usize = 1;
const WTS_SESSION_LOCK: usize = 0x7;

const MSG_BLANK: u32 = WM_APP + 1;
const MSG_UNBLANK: u32 = WM_APP + 2;
/// Reasons travel as the message's WPARAM.
const REASONS: [&str; 6] = ["command", "key press", "key press (polled)", "session lock", "standby", "exit"];
const REASON_COMMAND: usize = 0;
const REASON_KEY: usize = 1;
const REASON_POLL: usize = 2;
const REASON_LOCK: usize = 3;
const REASON_EXIT: usize = 5;

/// Backlight levels as the display driver stores them: (on AC, on battery).
type Brightness = (u8, u8);

/// Owned by the screen thread only.
#[derive(Default)]
struct Blank {
    hwnd: isize,
    saved_brightness: Option<Brightness>,
    changed_refresh: bool,
    saved_mode: Option<u8>,
}

thread_local! {
    static CURRENT: RefCell<Option<Blank>> = const { RefCell::new(None) };
}

/// Read by the keyboard hook and the rest of the app; written only by the screen thread.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static BLANKED_AT_TICK: AtomicU64 = AtomicU64::new(0);
static THREAD_ID: OnceLock<u32> = OnceLock::new();
/// Incremented by the screen thread after every restore (lets exit wait for it).
static RESTORES: AtomicU32 = AtomicU32::new(0);

fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

fn marker_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"OpenRevo\screen_blank.marker")
}

fn parse_marker(text: &str) -> Option<Brightness> {
    let mut it = text.trim().split(',').map(|v| v.trim().parse::<u8>());
    match (it.next(), it.next()) {
        (Some(Ok(ac)), Some(Ok(dc))) => Some((ac, dc)),
        (Some(Ok(both)), None) => Some((both, both)),
        _ => None,
    }
}

/// Set both backlight levels (falls back to the single-level call) and confirm by reading back.
fn set_backlight(level: Brightness) -> bool {
    for _ in 0..4 {
        let set = crate::system::display::set_brightness_levels(level.0, level.1).is_ok()
            || crate::system::display::set_brightness(level.0.max(level.1)).is_ok();
        if set {
            for _ in 0..3 {
                if let Ok(cur) = crate::system::display::get_brightness_levels() {
                    // Drivers that ignore the AC/DC split report one value for both.
                    if cur == level || (cur.0 == cur.1 && cur.0 == level.0.max(level.1)) {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

/// Startup: if the previous process died while blanked, put the backlight and display mode back.
pub fn recover_after_crash() {
    let path = marker_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let _ = crate::system::display::reset_display_mode_to_registry();
    let ok = parse_marker(&text).map_or(true, set_backlight);
    crate::log_warn!("SCREEN", "Previous run ended while the screen was blanked; backlight {} restored (ok={})", text.trim(), ok);
    if ok {
        let _ = std::fs::remove_file(path);
    }
}

// ---- requests from other threads (never block) -------------------------------------------------

fn post(msg: u32, reason: usize) -> bool {
    let tid = *THREAD_ID.get_or_init(start_thread);
    tid != 0 && unsafe { PostThreadMessageW(tid, msg, WPARAM(reason), LPARAM(0)) }.is_ok()
}

/// Black out every monitor. Returns as soon as the request is queued.
pub fn blank() -> Result<(), String> {
    if post(MSG_BLANK, 0) {
        Ok(())
    } else {
        Err("screen-blank thread is not available".into())
    }
}

pub fn unblank(reason: &str) {
    let r = REASONS.iter().position(|x| *x == reason).unwrap_or(REASON_COMMAND);
    post(MSG_UNBLANK, r);
}

/// App exit (main thread): ask the screen thread to restore and wait briefly. If it can't finish in
/// time (it may be inside a display-mode broadcast that needs this very thread), set the backlight
/// directly from the marker; the next start resets refresh rate and mode.
pub fn unblank_on_exit() {
    if !is_active() {
        return;
    }
    let before = RESTORES.load(Ordering::SeqCst);
    post(MSG_UNBLANK, REASON_EXIT);
    for _ in 0..30 {
        if RESTORES.load(Ordering::SeqCst) != before {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(level) = std::fs::read_to_string(marker_path()).ok().as_deref().and_then(parse_marker) {
        let _ = set_backlight(level);
    }
    crate::log_warn!("SCREEN", "Exit before the screen restore finished; backlight set back directly (marker kept)");
}

/// Called by the low-level keyboard hook for every key-down. Returns true if the key should be
/// swallowed (it never reaches the app underneath). Never blocks.
pub fn on_key_down() -> bool {
    if !is_active() {
        return false;
    }
    if grace_over() {
        post(MSG_UNBLANK, REASON_KEY);
    }
    true
}

fn grace_over() -> bool {
    tick_ms().saturating_sub(BLANKED_AT_TICK.load(Ordering::SeqCst)) >= KEY_GRACE_MS
}

// ---- the screen thread ----------------------------------------------------------------------------

fn start_thread() -> u32 {
    let (tx, rx) = std::sync::mpsc::channel::<u32>();
    let spawned = std::thread::Builder::new().name("openrevo-screen".into()).spawn(move || unsafe {
        // Create this thread's message queue before announcing the id, so no posted request is lost.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        let _ = tx.send(GetCurrentThreadId());
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0).0;
            if r == 0 || r == -1 {
                break;
            }
            match msg.message {
                MSG_BLANK | MSG_UNBLANK => {
                    // Coalesce a burst of requests (e.g. repeated toggles): only the last one counts.
                    let mut last = msg;
                    let mut next = MSG::default();
                    while PeekMessageW(&mut next, None, MSG_BLANK, MSG_UNBLANK, PM_REMOVE).as_bool() {
                        last = next;
                    }
                    if last.message == MSG_BLANK {
                        do_blank();
                    } else {
                        do_unblank(REASONS.get(last.wParam.0).copied().unwrap_or("command"));
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    });
    if spawned.is_err() {
        crate::log_error!("SCREEN", "Could not start the screen-blank thread");
        return 0;
    }
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or(0)
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SETCURSOR => {
            SetCursor(HCURSOR::default()); // hide the cursor over the overlay
            LRESULT(1)
        }
        // Clicks land on the overlay and are discarded without activating anything.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATEANDEAT as isize),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if grace_over() {
                do_unblank(REASONS[REASON_KEY]);
            }
            LRESULT(0)
        }
        // Independent of the hook: Windows silently removes a low-level hook that ever times out, and
        // a key swallowed by a live hook never reaches the async key state.
        WM_TIMER if wparam.0 == KEY_POLL_TIMER => {
            if grace_over() && any_key_down() {
                do_unblank(REASONS[REASON_POLL]);
            }
            LRESULT(0)
        }
        // The lock screen is a separate desktop the hook cannot see; never leave the user at 0 %.
        WM_WTSSESSION_CHANGE if wparam.0 == WTS_SESSION_LOCK => {
            do_unblank(REASONS[REASON_LOCK]);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            let (x, y, cx, cy) = virtual_screen();
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
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

fn any_key_down() -> bool {
    // 0x08..=0xFE covers every keyboard VK; 0x01..0x06 are mouse buttons and are skipped on purpose.
    (0x08..=0xFEi32).any(|vk| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 })
}

fn apply_mode_now(mode: u8) {
    if let Some(ec) = crate::system::power_listener::global_ec() {
        ec.lock().set_power_mode_cache(mode);
    }
    crate::system::power_mode::apply_full_power_mode(mode);
}

fn do_blank() {
    if CURRENT.with(|c| c.borrow().is_some()) {
        return;
    }
    unsafe {
        static REGISTERED: AtomicBool = AtomicBool::new(false);
        if !REGISTERED.load(Ordering::SeqCst) {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                lpszClassName: w!("OpenRevoScreenBlank"),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            REGISTERED.store(RegisterClassW(&wc) != 0, Ordering::SeqCst);
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
                return;
            }
        };
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
        if SetTimer(hwnd, KEY_POLL_TIMER, 150, None) == 0 {
            crate::log_warn!("SCREEN", "Key poll timer could not be started; wake relies on the keyboard hook");
        }
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);

        let mut b = Blank { hwnd: hwnd.0 as isize, ..Default::default() };
        BLANKED_AT_TICK.store(tick_ms(), Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        // Watcher switches the execution state to "system + display required" on its next tick.
        crate::system::switches::set_screen_off(true);

        match crate::system::display::get_brightness_levels() {
            Ok(level) => {
                if let Err(e) = std::fs::write(marker_path(), format!("{},{}", level.0, level.1)) {
                    crate::log_warn!("SCREEN", "Could not write the blank marker: {}", e);
                }
                b.saved_brightness = Some(level);
                let _ = crate::system::display::set_brightness_levels(0, 0)
                    .or_else(|_| crate::system::display::set_brightness(0));
            }
            Err(e) => crate::log_warn!("SCREEN", "Could not read brightness ({}); overlay only", e),
        }
        // Decide on the user's (registry) rate, not the current one: right after a restore the current
        // rate can still read 60 Hz, and the blank would then never put 300 Hz back.
        let hz = crate::system::display::get_registry_refresh_rate();
        if hz != BLANK_HZ {
            match crate::system::display::set_refresh_rate_temporary(BLANK_HZ) {
                Ok(()) => b.changed_refresh = true,
                Err(e) => crate::log_warn!("SCREEN", "Could not switch to {} Hz while blanked: {}", BLANK_HZ, e),
            }
        }
        if let Some(ec) = crate::system::power_listener::global_ec() {
            let mode = ec.lock().get_power_mode();
            if mode != BLANK_MODE {
                b.saved_mode = Some(mode);
                apply_mode_now(BLANK_MODE);
            }
        }
        crate::log_info!(
            "SCREEN",
            "Screen blanked: overlay {}x{}, backlight {:?} -> 0, refresh {} Hz{}, mode {:?} -> {}; system stays awake. Any key restores.",
            cx,
            cy,
            b.saved_brightness,
            hz,
            if b.changed_refresh { " -> 60 Hz" } else { "" },
            b.saved_mode,
            BLANK_MODE
        );
        CURRENT.with(|c| *c.borrow_mut() = Some(b));
    }
}

fn do_unblank(reason: &str) {
    let Some(b) = CURRENT.with(|c| c.borrow_mut().take()) else { return };
    ACTIVE.store(false, Ordering::SeqCst);

    // Refresh rate first, still behind the black overlay, so the mode-switch flicker is not seen.
    if b.changed_refresh {
        if let Err(e) = crate::system::display::reset_display_mode_to_registry() {
            crate::log_warn!("SCREEN", "Could not restore the display mode: {}", e);
        }
    }
    let brightness_ok = b.saved_brightness.map_or(true, set_backlight);
    unsafe {
        let hwnd = HWND(b.hwnd as _);
        let _ = KillTimer(hwnd, KEY_POLL_TIMER);
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
    }
    crate::system::switches::set_screen_off(false);

    // Power mode last; only if nobody changed it meanwhile, and never Beast/Custom on battery
    // (same guard as the set_power_mode command).
    if let (Some(mode), Some(ec)) = (b.saved_mode, crate::system::power_listener::global_ec()) {
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
    if brightness_ok {
        let _ = std::fs::remove_file(marker_path());
    } else {
        crate::log_error!("SCREEN", "Backlight restore to {:?} not confirmed; marker kept for the next start", b.saved_brightness);
    }
    RESTORES.fetch_add(1, Ordering::SeqCst);
    crate::log_info!("SCREEN", "Screen restored ({}); brightness {:?} (ok={})", reason, b.saved_brightness, brightness_ok);
}
