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

static PING_SEQ: AtomicU64 = AtomicU64::new(0);
static PONG_SEQ: AtomicU64 = AtomicU64::new(0);
static CHECK_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static RELAUNCHING: AtomicBool = AtomicBool::new(false);

/// Answer from the page to a "ui-ping" event.
#[tauri::command]
pub fn ui_pong(webview: tauri::WebviewWindow, seq: u64) {
    if webview.label() == "main" {
        PONG_SEQ.fetch_max(seq, Ordering::SeqCst);
    }
}

/// Tray click: show the main window if its page is alive, otherwise relaunch OpenRevo.
pub fn show_main_checked(app: &AppHandle) {
    // Before the first handshake the page has no listener yet; keep the old behaviour.
    if !crate::system::boot_trace::is_boot_completed() {
        if let Some(window) = app.get_webview_window("main") {
            crate::system::window::restore_and_sync_window(&window);
        }
        return;
    }
    if CHECK_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let seq = PING_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = app.emit_to("main", "ui-ping", seq);
    let app = app.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        while PONG_SEQ.load(Ordering::SeqCst) < seq && start.elapsed() < PONG_TIMEOUT {
            std::thread::sleep(Duration::from_millis(10));
        }
        let alive = PONG_SEQ.load(Ordering::SeqCst) >= seq;
        let waited = start.elapsed().as_millis();
        let main = app.clone();
        let _ = app.run_on_main_thread(move || {
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
            crate::system::webview_orchestrator::WebviewOrchestrator::global().safe_shutdown_all_subviews();
            crate::system::tray::safe_hardware_shutdown_on_exit();
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
    let Some(pid) = std::env::args().find_map(|a| a.strip_prefix(WAIT_PID_ARG).and_then(|p| p.parse::<u32>().ok()))
    else {
        return;
    };
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
}
