// Main-window liveness check and self-relaunch.
// When the Edge WebView2 runtime updates itself in the background (typically shortly after boot)
// it can take our WebView2 browser process down with it. The native window survives, but since it
// is transparent it then opens as an empty, see-through frame. A tray click therefore pings the
// page first and relaunches OpenRevo (same as Quit + reopen) when nothing answers.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

const WAIT_PID_ARG: &str = "--wait-pid=";
const PONG_TIMEOUT: Duration = Duration::from_millis(1500);
/// Two missed pings in a row (3 s total) before declaring the WebView dead, so a page that is merely
/// slow (e.g. right after resume from standby) is not torn down.
const PING_ATTEMPTS: u32 = 2;
/// If the page never completed its first handshake within this time, the WebView died while loading.
const FIRST_LOAD_DEADLINE: Duration = Duration::from_secs(45);

static PROCESS_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

static PING_SEQ: AtomicU64 = AtomicU64::new(0);
static PONG_SEQ: AtomicU64 = AtomicU64::new(0);
static CHECK_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static RELAUNCHING: AtomicBool = AtomicBool::new(false);

/// Answer from the page to a "ui-ping" event.
#[tauri::command]
pub fn ui_pong(webview: tauri::WebviewWindow, seq: u64) {
    // Only a ping we actually sent counts (a forged huge seq must not disable the check for good).
    if webview.label() == "main" && seq <= PING_SEQ.load(Ordering::SeqCst) {
        PONG_SEQ.fetch_max(seq, Ordering::SeqCst);
    }
}

/// Tray click: show the main window if its page is alive, otherwise relaunch OpenRevo.
pub fn show_main_checked(app: &AppHandle) {
    // Before the first handshake the page has no ping listener yet. Shortly after start that just
    // means "still loading"; long after start it means the WebView died during the first load.
    if !crate::system::boot_trace::is_boot_completed() {
        let uptime = PROCESS_START.get().map_or(Duration::ZERO, |t| t.elapsed());
        if uptime > FIRST_LOAD_DEADLINE {
            crate::log_warn!("WEBVIEW_HEALTH", "Page never finished loading ({}s after start); relaunching", uptime.as_secs());
            relaunch(app);
        } else if let Some(window) = app.get_webview_window("main") {
            crate::system::window::restore_and_sync_window(&window);
        }
        return;
    }
    if CHECK_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut alive = false;
        for _ in 0..PING_ATTEMPTS {
            let seq = PING_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = app.emit_to("main", "ui-ping", seq);
            let attempt = Instant::now();
            while PONG_SEQ.load(Ordering::SeqCst) < seq && attempt.elapsed() < PONG_TIMEOUT {
                std::thread::sleep(Duration::from_millis(10));
            }
            if PONG_SEQ.load(Ordering::SeqCst) >= seq {
                alive = true;
                break;
            }
        }
        let waited = start.elapsed().as_millis();
        let main = app.clone();
        let posted = app.run_on_main_thread(move || {
            CHECK_IN_FLIGHT.store(false, Ordering::SeqCst);
            if alive {
                crate::log_debug!("WEBVIEW_HEALTH", "Main page answered ping in {}ms", waited);
                if let Some(window) = main.get_webview_window("main") {
                    crate::system::window::restore_and_sync_window(&window);
                }
            } else {
                crate::log_warn!(
                    "WEBVIEW_HEALTH",
                    "Main page did not answer within {}ms (WebView2 process gone, e.g. runtime update); relaunching",
                    waited
                );
                relaunch(&main);
            }
        });
        if posted.is_err() {
            CHECK_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    });
}

/// Start a fresh instance that shows its window, then exit this one the same way tray "Quit" does.
fn relaunch(app: &AppHandle) {
    if RELAUNCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            crate::log_error!("WEBVIEW_HEALTH", "Relaunch failed: current_exe: {}", e);
            RELAUNCHING.store(false, Ordering::SeqCst);
            return;
        }
    };
    let mut args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !matches!(a.as_str(), "--minimized" | "--show" | "--visible") && !a.starts_with(WAIT_PID_ARG))
        .collect();
    args.push("--show".into());
    args.push(format!("{}{}", WAIT_PID_ARG, std::process::id()));
    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(child) => {
            crate::log_info!("WEBVIEW_HEALTH", "Spawned replacement instance (pid {}) with args {:?}", child.id(), args);
            // RunEvent::Exit runs the usual shutdown (subviews, hardware handover, screen restore).
            app.exit(0);
        }
        Err(e) => {
            crate::log_error!("WEBVIEW_HEALTH", "Relaunch failed: spawn {:?}: {}", exe, e);
            RELAUNCHING.store(false, Ordering::SeqCst);
        }
    }
}

/// A relaunched instance waits for its predecessor to exit (single-instance lock, EC ownership).
pub fn wait_for_previous_instance() {
    wait_for_predecessor();
    // The first-load deadline counts from here, not from before the wait for the old instance.
    let _ = PROCESS_START.set(Instant::now());
}

fn wait_for_predecessor() {
    let Some(pid) = std::env::args().find_map(|a| a.strip_prefix(WAIT_PID_ARG).and_then(|p| p.parse::<u32>().ok()))
    else {
        return;
    };
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Foundation::WAIT_TIMEOUT;
        use windows::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        // If the old instance is still alive after 30 s its shutdown is stuck; end it, otherwise the
        // single-instance mutex makes this new instance exit and nothing would be left running.
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) {
            if WaitForSingleObject(h, 30_000) == WAIT_TIMEOUT {
                let _ = TerminateProcess(h, 1);
                let _ = WaitForSingleObject(h, 5_000);
            }
            let _ = CloseHandle(h);
        }
    }
}
