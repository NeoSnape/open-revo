use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};
use parking_lot::{Condvar, Mutex};
use tauri::{AppHandle, Emitter};

use crate::driver::ec_bridge::SharedEcBridge;
use crate::driver::gpu_bridge::SharedGpuBridge;
use crate::commands::system_cmd::collect_hardware_state;

static ACTIVE_SUBSCRIBERS: AtomicU32 = AtomicU32::new(0);
/// False until the page reports a window that is really on screen.
static MAIN_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(false);
static SCREEN_SUSPENDED: AtomicBool = AtomicBool::new(false);

static TELEMETRY_WAKE: LazyLock<(Mutex<u64>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));

/// Snapshot the current wake generation BEFORE inspecting atomic state, so any
/// `wake_telemetry_loop()` call between the state check and waiting is never lost.
fn current_wake_gen() -> u64 {
    *TELEMETRY_WAKE.0.lock()
}

/// Immediately wake the telemetry heartbeat loop from its standby sleep.
pub fn wake_telemetry_loop() {
    let (lock, cvar) = &*TELEMETRY_WAKE;
    let mut gen = lock.lock();
    *gen = gen.wrapping_add(1);
    cvar.notify_all();
}

fn sleep_or_wake(seen: u64, duration: Duration) {
    let (lock, cvar) = &*TELEMETRY_WAKE;
    let mut gen = lock.lock();
    let deadline = Instant::now() + duration;
    while *gen == seen {
        if cvar.wait_until(&mut gen, deadline).timed_out() {
            break;
        }
    }
}

fn get_subscribed_sensors() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static SENSORS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    SENSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a widget or external consumer with specific sensor needs (idempotent per consumer_id)
pub fn register_consumer(consumer_id: &str, sensors: &[String]) {
    let mut map = get_subscribed_sensors().lock();
    let set: HashSet<String> = sensors.iter().cloned().collect();
    map.insert(consumer_id.to_string(), set);
    let count = map.len() as u32;
    ACTIVE_SUBSCRIBERS.store(count, Ordering::SeqCst);
    drop(map);
    wake_telemetry_loop();
    crate::system::cpu_monitor::wake_cpu_monitor();
    crate::log_info!(
        "TELEMETRY",
        "Registered consumer '{}' with {} sensors. Total active consumers: {}",
        consumer_id,
        sensors.len(),
        count
    );
}

/// Unregister a widget or external consumer when closed
pub fn unregister_consumer(consumer_id: &str) {
    let mut map = get_subscribed_sensors().lock();
    if map.remove(consumer_id).is_some() {
        let count = map.len() as u32;
        ACTIVE_SUBSCRIBERS.store(count, Ordering::SeqCst);
        crate::log_info!(
            "TELEMETRY",
            "Unregistered consumer '{}'. Remaining active consumers: {}",
            consumer_id,
            count
        );
    }
}

/// Report main console window visibility (foreground vs tray)
pub fn set_main_window_visible(visible: bool) {
    let old = MAIN_WINDOW_VISIBLE.swap(visible, Ordering::SeqCst);
    if old != visible {
        crate::log_info!(
            "TELEMETRY",
            "Main window visibility changed to: {}. (Heartbeat policy updated)",
            visible
        );
        if let Some(app) = crate::system::webview_orchestrator::WebviewOrchestrator::global().app() {
            let _ = app.emit_to("main", "window-visibility-changed", visible);
        }
        if visible {
            wake_telemetry_loop();
            crate::system::cpu_monitor::wake_cpu_monitor();
        }
    }
}

static REBOOT_FREEZE: AtomicBool = AtomicBool::new(false);

/// True while telemetry and auxiliary monitors should stay completely paused (reboot or display/system sleep).
pub fn is_paused_for_sleep() -> bool {
    REBOOT_FREEZE.load(Ordering::Relaxed) || SCREEN_SUSPENDED.load(Ordering::Relaxed)
}

/// Report system reboot in progress to completely freeze telemetry polling and GPU hardware access
pub fn set_reboot_freeze(freeze: bool) {
    REBOOT_FREEZE.store(freeze, Ordering::SeqCst);
    if freeze {
        crate::log_info!("TELEMETRY", "System reboot initiated: telemetry heartbeat completely frozen.");
    }
}

/// Report screen lock / display sleep event from power listener
pub fn set_screen_suspended(suspended: bool) {
    let old = SCREEN_SUSPENDED.swap(suspended, Ordering::SeqCst);
    if old != suspended {
        crate::log_info!(
            "TELEMETRY",
            "Screen suspended changed to: {}. (Telemetry loop parked/resumed)",
            suspended
        );
        if !suspended {
            wake_telemetry_loop();
            crate::system::cpu_monitor::wake_cpu_monitor();
        }
    }
}

/// Check if telemetry heartbeat should be actively pulsing (Level 1)
pub fn is_telemetry_active() -> bool {
    if is_paused_for_sleep() || crate::system::screen_blank::is_active() {
        return false;
    }
    MAIN_WINDOW_VISIBLE.load(Ordering::Relaxed)
        || ACTIVE_SUBSCRIBERS.load(Ordering::Relaxed) > 0
}

/// Determine whether NVIDIA dGPU queries (NVAPI) should be executed
/// CRITICAL DEFENSE: If no GPU metrics are requested and main console is closed (or screen blanked/suspended),
/// NEVER query GPU to prevent waking up NVIDIA dGPU from D3Cold ultra-low power state.
pub fn should_query_gpu() -> bool {
    if is_paused_for_sleep() || crate::system::screen_blank::is_active() {
        return false;
    }
    if MAIN_WINDOW_VISIBLE.load(Ordering::Relaxed) {
        return true;
    }
    let map = get_subscribed_sensors().lock();
    for (_id, sensors) in map.iter() {
        if sensors.is_empty() {
            // Empty sensors list in declaration means "all metrics required"
            return true;
        }
        for s in sensors {
            if s.starts_with("gpu") {
                return true;
            }
        }
    }
    false
}

/// Start the background dynamic telemetry heartbeat loop
pub fn start_telemetry_loop(
    app: AppHandle,
    ec_bridge: SharedEcBridge,
    gpu_bridge: SharedGpuBridge,
) {
    std::thread::Builder::new()
        .name("openrevo-telemetry-heartbeat".to_string())
        .spawn(move || {
            crate::log_info!("TELEMETRY", "Adaptive dynamic telemetry heartbeat loop initialized.");
            loop {
                // Capture generation before checking atomics so a wake arriving between
                // the predicate check and `sleep_or_wake` cannot be missed.
                let wake_gen = current_wake_gen();

                // State 0 / State 3: Reboot or Screen suspended -> Deep Freeze
                if is_paused_for_sleep() {
                    sleep_or_wake(wake_gen, Duration::from_millis(2000));
                    continue;
                }

                if is_telemetry_active() {
                    // State 1: Active Pulse (1.5s interval)
                    let query_gpu = should_query_gpu();
                    let state = collect_hardware_state(&ec_bridge, &gpu_bridge, query_gpu);
                    if !is_paused_for_sleep() && !crate::system::screen_blank::is_active() {
                        let _ = app.emit("hardware-state-updated", &state);
                    }
                    let post_gen = current_wake_gen();
                    sleep_or_wake(post_gen, Duration::from_millis(1500));
                } else {
                    // State 2: Standby Low Power (20s slow heartbeat, zero events emitted; woken instantly via condvar)
                    sleep_or_wake(wake_gen, Duration::from_secs(20));
                }
            }
        })
        .expect("Failed to spawn openrevo telemetry heartbeat thread");
}

#[tauri::command]
pub fn report_window_visibility(app: AppHandle, visible: bool) -> Result<(), String> {
    // Verify both true and false against the real OS window state:
    // - A startup `visible=true` while minimized to tray must not wake the loop;
    // - A delayed `visible=false` after the user already reopened the window must not turn it off.
    let _ = visible;
    let really_visible = {
        use tauri::Manager;
        app.get_webview_window("main")
            .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
    };
    set_main_window_visible(really_visible);
    Ok(())
}

#[tauri::command]
pub fn get_telemetry_heartbeat_status() -> Result<serde_json::Value, String> {
    let consumers = ACTIVE_SUBSCRIBERS.load(Ordering::Relaxed);
    let main_visible = MAIN_WINDOW_VISIBLE.load(Ordering::Relaxed);
    let screen_suspended = SCREEN_SUSPENDED.load(Ordering::Relaxed);
    let active = is_telemetry_active();
    let query_gpu = should_query_gpu();

    Ok(serde_json::json!({
        "active": active,
        "consumers": consumers,
        "main_visible": main_visible,
        "screen_suspended": screen_suspended,
        "query_gpu": query_gpu,
    }))
}

/// One INFO line per minute so a few days of use can be reviewed from the log
/// (fan curve vs. temperature, power limits, battery charge window).
pub fn start_status_logger(
    ec_bridge: crate::driver::ec_bridge::SharedEcBridge,
    gpu_bridge: crate::driver::gpu_bridge::SharedGpuBridge,
) {
    std::thread::Builder::new()
        .name("openrevo-status-log".into())
        .spawn(move || {
            let mut last_cpu = own_cpu_ms();
            let mut last_at = std::time::Instant::now();
            loop {
            std::thread::sleep(Duration::from_secs(60));
            // This process's own CPU use since the previous line (ms of CPU per second), so the cost
            // of the panel, the screen blank and the in-game OSD can be read off the log afterwards.
            let now_cpu = own_cpu_ms();
            let self_ms_per_s = (now_cpu - last_cpu) / last_at.elapsed().as_secs_f64().max(1.0);
            last_cpu = now_cpu;
            last_at = std::time::Instant::now();
            if SCREEN_SUSPENDED.load(Ordering::SeqCst) {
                continue;
            }
            // Refresh CPU metrics before logging a STATUS snapshot (important while cpu_monitor is on slow idle cadence).
            crate::system::cpu_monitor::wake_cpu_monitor();
            let (query_gpu, preset_fan_policy) = crate::system::config::with_config(|cfg| {
                // Only query NVAPI when the dGPU is always on anyway (MUX direct); never wake it in hybrid.
                (cfg.gpu_mode == "dgpu", cfg.preset_fan_policy.clone())
            });
            if query_gpu {
                crate::driver::gpu_bridge::GpuBridge::request_telemetry(5000);
                std::thread::sleep(Duration::from_millis(2500));
            } else {
                std::thread::sleep(Duration::from_millis(150));
            }
            let s = collect_hardware_state(&ec_bridge, &gpu_bridge, query_gpu);
            // Why the GPU is holding back right now ("-" = it is not), so a log review can tell a
            // thermal limit from a power limit.
            let gpu_limit = if query_gpu {
                crate::driver::nvml::sample().map_or("?", |g| {
                    let why = crate::driver::nvml::throttle_label(g.throttle_reasons);
                    if why.is_empty() { "-" } else { why }
                })
            } else {
                "-"
            };
            let (bat, _, charging) = crate::commands::power_cmd::get_live_power_status();
            let stop = crate::driver::wmi_ec::wmi_read_ec(crate::driver::wmi_ec::ADDR_BATTERY_LIMIT_UP).unwrap_or(0);
            let resume = crate::driver::wmi_ec::wmi_read_ec(crate::driver::wmi_ec::ADDR_BATTERY_LIMIT_DOWN).unwrap_or(0);
            crate::log_info!(
                "STATUS",
                "mode={} boost={} fanpol={} | CPU {}C {:.1}W {}MHz {}% | GPU {}C {:.1}W {}MHz {}% | fan {}/{} rpm | bat {}% ac={} chg={} window={}/{} | self {:.1}ms/s panel={} osd={} gpulim={}",
                s.power_mode, s.fan_boost as u8, preset_fan_policy,
                s.cpu_temp, s.cpu_power_w, s.cpu_freq, s.cpu_usage,
                s.gpu_temp, s.gpu_power_w, s.gpu_core_freq, s.gpu_usage,
                s.cpu_fan_rpm, s.gpu_fan_rpm,
                bat, s.is_dc_online as u8, charging as u8, stop, resume,
                self_ms_per_s, MAIN_WINDOW_VISIBLE.load(Ordering::Relaxed) as u8, crate::system::rtss_osd::is_feeding() as u8, gpu_limit
            );
            }
        })
        .ok();
}

/// Kernel + user CPU time this process has used so far, in milliseconds.
fn own_cpu_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.is_err() {
        return 0.0;
    }
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    (ticks(kernel) + ticks(user)) as f64 / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_consumer_registration_is_idempotent() {
        let id = "telemetry-regression";
        let initial = ACTIVE_SUBSCRIBERS.load(Ordering::SeqCst);
        register_consumer(id, &["cpu_temp".into()]);
        register_consumer(id, &["gpu_temp".into()]);
        assert_eq!(ACTIVE_SUBSCRIBERS.load(Ordering::SeqCst), initial + 1);
        assert!(get_subscribed_sensors().lock()[id].contains("gpu_temp"));
        unregister_consumer(id);
        unregister_consumer(id);
        assert_eq!(ACTIVE_SUBSCRIBERS.load(Ordering::SeqCst), initial);
    }
}
