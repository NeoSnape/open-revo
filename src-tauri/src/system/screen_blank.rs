// "Screen off, keep running" for Modern Standby (S0 low power idle) machines.
// On this laptop SC_MONITORPOWER = entering Modern Standby (Kernel-Power 506, reason SC_MONITORPOWER),
// which suspends desktop apps regardless of execution-state requests. So instead of powering the panel
// down, cover every monitor with a black topmost window and drop the backlight to 0; Windows still
// considers the display on, and the watcher holds ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED meanwhile.
// While blanked the panel also drops to 60 Hz and the machine to Office mode (saves power overnight);
// both are put back on restore unless they were changed in the meantime. Neither is written to the
// config, so a restart during the blank keeps the user's own settings.
// Mouse movement and clicks are swallowed by the overlay; a key press (seen by the low-level keyboard
// hook) restores the screen and brightness.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, PostMessageW,
    PostQuitMessage, RegisterClassW, SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, HCURSOR,
    HWND_TOPMOST, MA_NOACTIVATEANDEAT, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SWP_SHOWWINDOW, SW_SHOW, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_MOUSEACTIVATE, WM_SETCURSOR, WM_SYSKEYDOWN, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// Keys are ignored this long after blanking, so the Enter/click that triggered it can't undo it.
const KEY_GRACE_MS: u64 = 800;
const NO_SAVED_BRIGHTNESS: u8 = 255;
const BLANK_HZ: u32 = 60;
const BLANK_MODE: u8 = 1; // Office

static ACTIVE: AtomicBool = AtomicBool::new(false);
static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static SAVED_BRIGHTNESS: AtomicU8 = AtomicU8::new(NO_SAVED_BRIGHTNESS);
static BLANKED_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Refresh rate / power mode to put back (0 = we did not change it).
static SAVED_HZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static SAVED_MODE: AtomicU8 = AtomicU8::new(0);

/// Drop to 60 Hz and Office mode. Runs off the overlay thread (a mode change takes a moment).
fn enter_low_power() {
    if !is_active() {
        return;
    }
    let hz = crate::system::display::get_refresh_rate();
    if hz != BLANK_HZ {
        match crate::system::display::set_refresh_rate(BLANK_HZ) {
            Ok(()) => SAVED_HZ.store(hz, Ordering::SeqCst),
            Err(e) => crate::log_warn!("SCREEN", "Could not switch to {} Hz while blanked: {}", BLANK_HZ, e),
        }
    }
    if !is_active() {
        return;
    }
    if let Some(ec) = crate::system::power_listener::global_ec() {
        let mode = ec.lock().get_power_mode();
        if mode != BLANK_MODE {
            SAVED_MODE.store(mode, Ordering::SeqCst);
            ec.lock().set_power_mode(BLANK_MODE);
        }
    }
    crate::log_info!(
        "SCREEN",
        "Blank low-power: refresh {} -> {} Hz, power mode {} -> {}",
        hz,
        BLANK_HZ,
        SAVED_MODE.load(Ordering::SeqCst).max(BLANK_MODE),
        BLANK_MODE
    );
}

/// Put back refresh rate and power mode, unless the user changed them while blanked.
fn leave_low_power() {
    let hz = SAVED_HZ.swap(0, Ordering::SeqCst);
    if hz != 0 && crate::system::display::get_refresh_rate() == BLANK_HZ {
        if let Err(e) = crate::system::display::set_refresh_rate(hz) {
            crate::log_warn!("SCREEN", "Could not restore {} Hz: {}", hz, e);
        }
    }
    let mode = SAVED_MODE.swap(0, Ordering::SeqCst);
    if mode != 0 {
        if let Some(ec) = crate::system::power_listener::global_ec() {
            if ec.lock().get_power_mode() == BLANK_MODE {
                ec.lock().set_power_mode(mode);
            }
        }
    }
    if hz != 0 || mode != 0 {
        crate::log_info!("SCREEN", "Restored refresh {} Hz, power mode {}", if hz != 0 { hz } else { crate::system::display::get_refresh_rate() }, if mode != 0 { mode } else { BLANK_MODE });
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
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
        // Fallback if the keyboard hook is not running: a key on the focused overlay also wakes.
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            on_key_down();
            LRESULT(0)
        }
        WM_CLOSE => {
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

/// Black out every monitor and dim the backlight. Idempotent.
pub fn blank() -> Result<(), String> {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    BLANKED_AT_MS.store(now_ms(), Ordering::SeqCst);
    // Watcher switches the execution state to "system + display required" on its next tick.
    crate::system::switches::set_screen_off(true);

    std::thread::Builder::new()
        .name("openrevo-screen-blank".into())
        .spawn(|| unsafe {
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
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            let (cx, cy) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
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
                    ACTIVE.store(false, Ordering::SeqCst);
                    crate::system::switches::set_screen_off(false);
                    return;
                }
            };
            OVERLAY.store(hwnd.0 as isize, Ordering::SeqCst);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, cx, cy, SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(hwnd);

            // Backlight down only once the overlay is up, so nothing bright flashes at 0 %.
            match crate::system::display::get_brightness() {
                Ok(b) => {
                    SAVED_BRIGHTNESS.store(b, Ordering::SeqCst);
                    let _ = crate::system::display::set_brightness(0);
                }
                Err(e) => crate::log_warn!("SCREEN", "Could not read brightness ({}); overlay only", e),
            }
            std::thread::spawn(enter_low_power);
            crate::log_info!(
                "SCREEN",
                "Screen blanked (black overlay {}x{}, backlight 0, was {}%); system stays awake. Any key restores.",
                cx,
                cy,
                SAVED_BRIGHTNESS.load(Ordering::SeqCst)
            );

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            OVERLAY.store(0, Ordering::SeqCst);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Remove the overlay and restore the backlight. Idempotent.
pub fn unblank(reason: &str) {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    // Refresh rate first, still behind the black overlay, so the mode switch flicker is not seen.
    let saved_hz = SAVED_HZ.swap(0, Ordering::SeqCst);
    if saved_hz != 0 && crate::system::display::get_refresh_rate() == BLANK_HZ {
        if let Err(e) = crate::system::display::set_refresh_rate(saved_hz) {
            crate::log_warn!("SCREEN", "Could not restore {} Hz: {}", saved_hz, e);
        }
    }
    let saved = SAVED_BRIGHTNESS.swap(NO_SAVED_BRIGHTNESS, Ordering::SeqCst);
    if saved != NO_SAVED_BRIGHTNESS {
        // Retry once: the IOCTL can fail transiently right after resume from a lid-close standby.
        if crate::system::display::set_brightness(saved).is_err() {
            std::thread::sleep(Duration::from_millis(300));
            let _ = crate::system::display::set_brightness(saved);
        }
    }
    let hwnd = OVERLAY.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(HWND(hwnd as _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    crate::system::switches::set_screen_off(false);
    if saved_hz != 0 {
        SAVED_HZ.store(saved_hz, Ordering::SeqCst); // let leave_low_power log it; rate already restored
    }
    leave_low_power();
    crate::log_info!("SCREEN", "Screen restored ({}); brightness back to {}%", reason, saved);
}

/// Called by the low-level keyboard hook for every key-down while blanked.
/// Returns true if the key should be swallowed (it never reaches the app underneath).
pub fn on_key_down() -> bool {
    if !is_active() {
        return false;
    }
    if now_ms().saturating_sub(BLANKED_AT_MS.load(Ordering::SeqCst)) >= KEY_GRACE_MS {
        // Off the hook thread: the brightness IOCTL must not stall the system-wide hook.
        std::thread::spawn(|| unblank("key press"));
    }
    true
}
