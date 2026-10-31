use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::driver::ec_bridge::SharedEcBridge;
use crate::driver::gpu_bridge::SharedGpuBridge;
use crate::commands::system_cmd::collect_hardware_state;

static ACTIVE_SUBSCRIBERS: AtomicU32 = AtomicU32::new(0);
static MAIN_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static SCREEN_SUSPENDED: AtomicBool = AtomicBool::new(false);

fn get_subscribed_sensors() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static SENSORS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    SENSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a widget or external consumer with specific sensor needs
pub fn register_consumer(consumer_id: &str, sensors: &[String]) {
    let mut map = get_subscribed_sensors().lock();
    let set: HashSet<String> = sensors.iter().cloned().collect();
    map.insert(consumer_id.to_string(), set);
    let count = ACTIVE_SUBSCRIBERS.fetch_add(1, Ordering::SeqCst) + 1;
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
        let count = ACTIVE_SUBSCRIBERS.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
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
    }
}

static REBOOT_FREEZE: AtomicBool = AtomicBool::new(false);

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
    }
}

/// Check if telemetry heartbeat should be actively pulsing (Level 1)
pub fn is_telemetry_active() -> bool {
    if REBOOT_FREEZE.load(Ordering::Relaxed) || SCREEN_SUSPENDED.load(Ordering::Relaxed) {
        return false;
    }
    MAIN_WINDOW_VISIBLE.load(Ordering::Relaxed)
        || ACTIVE_SUBSCRIBERS.load(Ordering::Relaxed) > 0
}

/// Determine whether NVIDIA dGPU queries (NVAPI) should be executed
/// CRITICAL DEFENSE: If no GPU metrics are requested and main console is closed,
/// NEVER query GPU to prevent waking up NVIDIA dGPU from D3Cold ultra-low power state.
pub fn should_query_gpu() -> bool {
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
                // State 0: System reboot in progress -> Complete freeze to release bus
                if REBOOT_FREEZE.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(2000));
                    continue;
                }

                // State 3: Screen suspended / Lock screen -> Deep Freeze
                if SCREEN_SUSPENDED.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(2000));
                    continue;
                }

                if is_telemetry_active() {
                    // State 1: Active Pulse (1.5s interval)
                    let query_gpu = should_query_gpu();
                    let state = collect_hardware_state(&ec_bridge, &gpu_bridge, query_gpu);
                    if !SCREEN_SUSPENDED.load(Ordering::SeqCst) && !REBOOT_FREEZE.load(Ordering::SeqCst) {
                        let _ = app.emit("hardware-state-updated", &state);
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                } else {
                    // State 2: Standby Low Power (20s slow heartbeat, zero events emitted)
                    std::thread::sleep(Duration::from_secs(20));
                }
            }
        })
        .expect("Failed to spawn openrevo telemetry heartbeat thread");
}

#[tauri::command]
pub fn report_window_visibility(visible: bool) -> Result<(), String> {
    set_main_window_visible(visible);
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
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            if SCREEN_SUSPENDED.load(Ordering::SeqCst) {
                continue;
            }
            let cfg = crate::system::config::load_config();
            // Only query NVAPI when the dGPU is always on anyway (MUX direct); never wake it in hybrid.
            let query_gpu = cfg.gpu_mode == "dgpu";
            if query_gpu {
                crate::driver::gpu_bridge::GpuBridge::request_telemetry(5000);
                std::thread::sleep(Duration::from_millis(2500));
            }
            let s = collect_hardware_state(&ec_bridge, &gpu_bridge, query_gpu);
            let (bat, _, charging) = crate::commands::power_cmd::get_live_power_status();
            let stop = crate::driver::wmi_ec::wmi_read_ec(crate::driver::wmi_ec::ADDR_BATTERY_LIMIT_UP).unwrap_or(0);
            let resume = crate::driver::wmi_ec::wmi_read_ec(crate::driver::wmi_ec::ADDR_BATTERY_LIMIT_DOWN).unwrap_or(0);
            crate::log_info!(
                "STATUS",
                "mode={} boost={} fanpol={} | CPU {}C {:.1}W {}MHz {}% | GPU {}C {:.1}W {}MHz {}% | fan {}/{} rpm | bat {}% ac={} chg={} window={}/{}",
                s.power_mode, s.fan_boost as u8, cfg.preset_fan_policy,
                s.cpu_temp, s.cpu_power_w, s.cpu_freq, s.cpu_usage,
                s.gpu_temp, s.gpu_power_w, s.gpu_core_freq, s.gpu_usage,
                s.cpu_fan_rpm, s.gpu_fan_rpm,
                bat, s.is_dc_online as u8, charging as u8, stop, resume
            );
        })
        .ok();
}
