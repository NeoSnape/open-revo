use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, GetAsyncKeyState, GetKeyState, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_CAPITAL,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_NUMLOCK, VK_RWIN, VK_SCROLL, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
};

/// How often the keyboard hook is re-installed (see `ensure_hook_running`).
const HOOK_REFRESH_MS: u32 = 3 * 60 * 1000;

extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(lpModuleName: *const u16) -> HINSTANCE;
}

static WIN_KEY_LOCKED: AtomicBool = AtomicBool::new(false);
static WIN_PHYSICALLY_DOWN: AtomicBool = AtomicBool::new(false);
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// True while the low-level keyboard hook thread is installed and running.
pub fn is_keyboard_hook_alive() -> bool {
    HOOK_THREAD_ID.load(Ordering::SeqCst) != 0
}
static HOOK_STATE_LOCK: Mutex<()> = Mutex::new(());
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

// Tracked toggle states (Caps Lock, Num Lock, Scroll Lock)
static CAPS_STATE: AtomicBool = AtomicBool::new(false);
static NUM_STATE: AtomicBool = AtomicBool::new(false);
static SCROLL_STATE: AtomicBool = AtomicBool::new(false);

static CAPS_PHYSICALLY_DOWN: AtomicBool = AtomicBool::new(false);
static NUM_PHYSICALLY_DOWN: AtomicBool = AtomicBool::new(false);
static SCROLL_PHYSICALLY_DOWN: AtomicBool = AtomicBool::new(false);


/// Query true live lock key toggle state safely without AttachThreadInput deadlocks
unsafe fn query_hardware_lock_state(vk: u16) -> bool {
    (GetKeyState(vk as i32) & 1) != 0
}

unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let msg = w_param.0 as u32;
        let kbd_struct = *(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk = kbd_struct.vkCode as u16;

        // Screen blanked: the first key restores it and never reaches the app underneath.
        if (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && crate::system::screen_blank::on_key_down() {
            return LRESULT(1);
        }

        // Bypass dummy mask key (0xE8) from interfering with interactive lighting or lock logic
        if vk == 0xE8 {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }

        // 1. Win Key Lock Software Interception
        if WIN_KEY_LOCKED.load(Ordering::SeqCst) {
            if vk == VK_LWIN.0 || vk == VK_RWIN.0 {
                let is_injected = (kbd_struct.flags.0 & 0x10) != 0;
                if !is_injected {
                    // Industry-standard MenuMaskKey technique (AutoHotkey / PowerToys):
                    // When Windows detects a standalone Win key release without any intervening keys,
                    // the Windows Shell (explorer.exe) triggers the Start Menu from the kernel input path,
                    // completely bypassing normal window message queues even if WH_KEYBOARD_LL returns 1!
                    // Furthermore, once popped up, subsequent Win presses cannot dismiss it because the hook eats them.
                    // Emitting unassigned dummy virtual-key 0xE8 down+up breaks the standalone modifier sequence,
                    // guaranteeing 100% that the Start Menu will NEVER be invoked, while swallowing Win key completely.
                    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                        if !WIN_PHYSICALLY_DOWN.swap(true, Ordering::SeqCst) {
                            keybd_event(0xE8, 0, KEYBD_EVENT_FLAGS(0), 0);
                            keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                        }
                    } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                        WIN_PHYSICALLY_DOWN.store(false, Ordering::SeqCst);
                        keybd_event(0xE8, 0, KEYBD_EVENT_FLAGS(0), 0);
                        keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    }
                }
                return LRESULT(1);
            }
        }

        // Per-key interactive lighting needs an ITE keyboard, which this machine does not have,
        // so keystrokes are no longer forwarded to the streamer tracker.
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            // 0.1 Universal Shortcut Bus (Route 2: Hardware & System Shortcut Dispatch)
            if let (true, Some(app)) = (crate::system::shortcuts::is_enabled(), APP_HANDLE.get()) {
                let is_ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                let is_alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                let is_shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                let is_win = (GetAsyncKeyState(VK_LWIN.0 as i32) as u16 & 0x8000) != 0
                    || (GetAsyncKeyState(VK_RWIN.0 as i32) as u16 & 0x8000) != 0;

                if crate::system::shortcuts::check_match_and_trigger(app, vk, is_ctrl, is_alt, is_shift, is_win) {
                    return LRESULT(1);
                }
            }
        }

        // 2. Lock Keys Notification (Caps Lock, Num Lock, Scroll Lock)
        // Only the first key-down is handed to the worker thread; typematic repeats are dropped here.
        // Nothing slow may run inside this callback (see `lock_key_worker`).
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let first_down = if vk == VK_CAPITAL.0 {
                !CAPS_PHYSICALLY_DOWN.swap(true, Ordering::SeqCst)
            } else if vk == VK_NUMLOCK.0 {
                !NUM_PHYSICALLY_DOWN.swap(true, Ordering::SeqCst)
            } else if vk == VK_SCROLL.0 {
                !SCROLL_PHYSICALLY_DOWN.swap(true, Ordering::SeqCst)
            } else {
                false
            };
            if first_down {
                if let Some(tx) = LOCK_KEY_TX.get() {
                    let _ = tx.lock().send(vk);
                }
            }
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            if vk == VK_CAPITAL.0 {
                CAPS_PHYSICALLY_DOWN.store(false, Ordering::SeqCst);
            } else if vk == VK_NUMLOCK.0 {
                NUM_PHYSICALLY_DOWN.store(false, Ordering::SeqCst);
            } else if vk == VK_SCROLL.0 {
                SCROLL_PHYSICALLY_DOWN.store(false, Ordering::SeqCst);
            }
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

/// Lock-key presses queued by the hook callback for `lock_key_worker`.
static LOCK_KEY_TX: OnceLock<Mutex<std::sync::mpsc::Sender<u16>>> = OnceLock::new();

/// Announces Caps / Num / Scroll Lock changes (log line, panel + OSD events), off the hook thread.
/// This used to run inside the hook callback: emitting to the webviews can wait on the main thread,
/// and Windows silently removes a low-level hook whose callback is slow (in the 2026-11-09..11 log
/// every screen wake came from the fallback key poll, i.e. the hook was already gone).
fn lock_key_worker(app: AppHandle, rx: std::sync::mpsc::Receiver<u16>) {
    let mut last_press: [Option<Instant>; 3] = [None; 3];
    while let Ok(vk) = rx.recv() {
        let (slot, state, name, event): (usize, &'static AtomicBool, &str, &str) = if vk == VK_CAPITAL.0 {
            (0, &CAPS_STATE, "Caps Lock", "caps-lock-changed")
        } else if vk == VK_NUMLOCK.0 {
            (1, &NUM_STATE, "Num Lock", "num-lock-changed")
        } else if vk == VK_SCROLL.0 {
            (2, &SCROLL_STATE, "Scroll Lock", "scroll-lock-changed")
        } else {
            continue;
        };
        if last_press[slot].map_or(false, |t| t.elapsed() < std::time::Duration::from_millis(60)) {
            continue;
        }
        last_press[slot] = Some(Instant::now());

        let announce = |val: bool| {
            crate::system::osd_window::wake_osd_if_enabled(&app);
            let _ = app.emit(event, val);
            let _ = app.emit_to("main", event, val);
            let _ = app.emit_to("osd", event, val);
        };
        // Invert immediately for instant visual feedback, then verify against the real key state.
        let new_val = !state.load(Ordering::Relaxed);
        state.store(new_val, Ordering::Relaxed);
        crate::log_info!("HOOKS", "{} key-down: toggled to {}", name, new_val);
        announce(new_val);

        thread::sleep(std::time::Duration::from_millis(35));
        let hw_val = unsafe { query_hardware_lock_state(vk) };
        if hw_val != state.load(Ordering::Relaxed) {
            state.store(hw_val, Ordering::Relaxed);
            crate::log_info!("HOOKS", "{} hardware sync corrected to: {}", name, hw_val);
            announce(hw_val);
        }
    }
}

pub fn is_win_key_locked() -> bool {
    WIN_KEY_LOCKED.load(Ordering::SeqCst)
}

/// Initialize the permanent low-level keyboard hook with its dedicated message pump
pub fn init_hooks(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());

    // Query initial hardware toggle states
    unsafe {
        let init_caps = query_hardware_lock_state(VK_CAPITAL.0);
        let init_num = query_hardware_lock_state(VK_NUMLOCK.0);
        let init_scroll = query_hardware_lock_state(VK_SCROLL.0);

        CAPS_STATE.store(init_caps, Ordering::SeqCst);
        NUM_STATE.store(init_num, Ordering::SeqCst);
        SCROLL_STATE.store(init_scroll, Ordering::SeqCst);

        crate::log_info!(
            "HOOKS",
            "Keyboard lock states initialized: Caps={}, Num={}, Scroll={}",
            init_caps,
            init_num,
            init_scroll
        );
    }

    let (tx, rx) = std::sync::mpsc::channel::<u16>();
    if LOCK_KEY_TX.set(Mutex::new(tx)).is_ok() {
        let app_for_worker = app.clone();
        let _ = thread::Builder::new()
            .name("openrevo-lock-keys".into())
            .spawn(move || lock_key_worker(app_for_worker, rx));
    }

    crate::system::shortcuts::init_shortcuts();

    ensure_hook_running();
}

fn ensure_hook_running() {
    let _guard = HOOK_STATE_LOCK.lock();
    let current_tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
    if current_tid != 0 {
        return;
    }

    thread::spawn(|| {
        unsafe {
            let tid = GetCurrentThreadId();
            HOOK_THREAD_ID.store(tid, Ordering::SeqCst);

            let h_mod = GetModuleHandleW(std::ptr::null());
            let mut hook = match SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                h_mod,
                0,
            ) {
                Ok(h) => h,
                Err(e) => {
                    crate::log_error!("HOOKS", "Failed to install low-level keyboard hook: {:?}", e);
                    HOOK_THREAD_ID.store(0, Ordering::SeqCst);
                    return;
                }
            };

            crate::log_info!("HOOKS", "Low-level keyboard hook installed successfully (TID: {})", tid);

            // Win32 Message Pump for WH_KEYBOARD_LL
            // Windows removes a low-level hook without any notification once its callback has been
            // too slow (a busy machine is enough), and there is no way to ask whether it is still
            // installed. Re-installing it every few minutes makes that self-healing.
            let refresh_timer = SetTimer(None, 0, HOOK_REFRESH_MS, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
                if msg.message == WM_QUIT {
                    break;
                }
                if msg.message == WM_TIMER && msg.hwnd.0.is_null() && msg.wParam.0 == refresh_timer {
                    match SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), h_mod, 0) {
                        Ok(fresh) => {
                            let _ = UnhookWindowsHookEx(hook);
                            hook = fresh;
                        }
                        Err(e) => crate::log_warn!("HOOKS", "Keyboard hook refresh failed: {:?}", e),
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(hook);
            HOOK_THREAD_ID.store(0, Ordering::SeqCst);
            crate::log_info!("HOOKS", "Low-level keyboard hook unhooked");
        }
    });
}

pub fn set_win_key_locked(locked: bool) {
    WIN_KEY_LOCKED.store(locked, Ordering::SeqCst);
    if !locked {
        WIN_PHYSICALLY_DOWN.store(false, Ordering::SeqCst);
    }
    ensure_hook_running();
}

