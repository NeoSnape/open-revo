use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use crate::driver::wmi_ec;
use crate::system::process;

#[derive(Clone, Debug)]
pub struct HardwareStateCache {
    pub power_mode: u8,
    pub fan_boost: bool,
    pub battery_limit: u8,
    pub cpu_fan_rpm: u16,
    pub gpu_fan_rpm: u16,
    pub cpu_temp: i32,
    // Cooldown protection to prevent background poller from oscillating user state
    pub fan_boost_cooldown_until: Instant,
    pub power_mode_cooldown_until: Instant,
    pub battery_limit_cooldown_until: Instant,
}

impl Default for HardwareStateCache {
    fn default() -> Self {
        Self {
            power_mode: 2, // Default UI mode: 2 (Gaming / Balance)
            fan_boost: false,
            battery_limit: 80,
            cpu_fan_rpm: 0,
            gpu_fan_rpm: 0,
            cpu_temp: 45,
            fan_boost_cooldown_until: Instant::now(),
            power_mode_cooldown_until: Instant::now(),
            battery_limit_cooldown_until: Instant::now(),
        }
    }
}

pub struct EcBridge {
    cache: Arc<Mutex<HardwareStateCache>>,
    pub is_mock: bool,
    pub oem_conflict_detected: bool,
}

/// Store one tachometer reading. A stopped fan reads 0 (the GPU fan a constant 45); it used to be
/// ignored, so the panel kept showing the last spin-down value (~500 rpm) for as long as the fan
/// stood still. Two stopped readings in a row are required so a single glitched read does not
/// flash 0 rpm.
fn settle_fan_rpm(raw: u16, stopped_reads: &mut u8, slot: &mut u16) {
    const STOPPED_BELOW_RPM: u16 = 100;
    if raw >= 8000 {
        return; // not a real reading
    }
    if raw < STOPPED_BELOW_RPM {
        *stopped_reads = stopped_reads.saturating_add(1);
        if *stopped_reads >= 2 {
            *slot = 0;
        }
    } else {
        *stopped_reads = 0;
        *slot = raw;
    }
}

/// The registers `absorb_slow_sample` expects, in this order (read by `system::ec_sampler` every 2 s).
pub const SLOW_REGISTERS: [u16; 8] = [
    wmi_ec::ADDR_SUPER_PERFORMANCE,       // Fan Boost Hardware Clamp (Bit 6: 0x40)
    wmi_ec::ADDR_MODE_INDEX,              // Operating Mode (1: Office, 0: Balance, 2: Turbo)
    wmi_ec::ADDR_BATTERY_PROTECTION_MODE, // Battery Mode | Touchpad mask
    wmi_ec::ADDR_BATTERY_LIMIT_UP,        // Battery Limit Upper
    wmi_ec::ADDR_MAIN_FAN_RPM_B1,         // CPU Fan RPM High
    wmi_ec::ADDR_MAIN_FAN_RPM_B2,         // CPU Fan RPM Low
    wmi_ec::ADDR_SECOND_FAN_RPM_B1,       // GPU Fan RPM High
    wmi_ec::ADDR_SECOND_FAN_RPM_B2,       // GPU Fan RPM Low
];

/// Debounce state for the "fan stopped" readings, owned by the sampler thread.
#[derive(Default)]
pub struct RpmFilter {
    cpu_stopped_reads: u8,
    gpu_stopped_reads: u8,
}

pub type SharedStateCache = Arc<Mutex<HardwareStateCache>>;

/// Fold one reading of `SLOW_REGISTERS` into the state cache (fan boost, Turbo detection, battery
/// limit, fan rpm) and run the battery-gate watchdog. Used to be the body of a poller thread that
/// every `EcBridge::new` started again.
pub fn absorb_slow_sample(cache: &SharedStateCache, bytes: &[u8], rpm: &mut RpmFilter) {
    if bytes.len() != SLOW_REGISTERS.len() {
        return;
    }
    let raw_fan_clamp = bytes[0];
    let raw_mode = bytes[1];
    let bat_mode_byte = bytes[2];
    let battery_up = bytes[3];
    let cpu_rpm_high = bytes[4] as u16;
    let cpu_rpm_low = bytes[5] as u16;
    let gpu_rpm_high = bytes[6] as u16;
    let gpu_rpm_low = bytes[7] as u16;

    let now = Instant::now();
    let mut c = cache.lock();

    // 1. Fan Boost Hardware Clamp - Direct zero-latency hardware clamp state
    if now >= c.fan_boost_cooldown_until {
        c.fan_boost = (raw_fan_clamp & wmi_ec::FAN_BOOST_CLAMP_BIT) != 0;
    }

    // 2. Power Mode - Respect user action cooldown window!
    // In Custom Mode (4), raw_mode is purposefully set to base performance (0x00 or 0x10)
    // by OpenRevo custom profile, NOT an autonomous EC mode switch.
    // We must NEVER let the poller misinterpret 0 as Balance and revert Custom Mode!
    if now >= c.power_mode_cooldown_until && c.power_mode != 4 {
        // Upstream switched the cached mode to Turbo here. Only `system::mode` changes the mode
        // now; an EC that reports Turbo on its own is logged (once per occurrence) for the next
        // log review instead of silently making the panel disagree with the selected mode.
        static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let ec_says_turbo = (raw_mode & 0x10) != 0;
        if ec_says_turbo && c.power_mode != 3 {
            if !REPORTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                crate::log_warn!("EC_BRIDGE", "EC mode index 0x{:02X} reports Turbo while the mode is {}; not adopting it", raw_mode, c.power_mode);
            }
        } else {
            REPORTED.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    // 3. Battery Limit - Respect cooldown window!
    if now >= c.battery_limit_cooldown_until {
        let clean_mode = bat_mode_byte & !0x08; // Mask out touchpad enable bit (0x08)
        if clean_mode == 0x21 || (clean_mode & 0xF0) == 0x20 || (battery_up <= 65 && battery_up > 0) {
            c.battery_limit = 60;
        } else if clean_mode == 0x11 || (clean_mode & 0xF0) == 0x10 || (battery_up <= 85 && battery_up > 65) {
            c.battery_limit = 80;
        } else if clean_mode == 0x01 || battery_up >= 95 || (clean_mode == 0 && battery_up == 0) {
            c.battery_limit = 100;
        } else if battery_up > 0 && battery_up <= 100 {
            c.battery_limit = battery_up;
        }
    }

    // 4. Fan RPMs (Always update with real RPM readings)
    let cpu_rpm = (cpu_rpm_high << 8) | cpu_rpm_low;
    settle_fan_rpm(cpu_rpm, &mut rpm.cpu_stopped_reads, &mut c.cpu_fan_rpm);

    let gpu_rpm = (gpu_rpm_high << 8) | gpu_rpm_low;
    settle_fan_rpm(gpu_rpm, &mut rpm.gpu_stopped_reads, &mut c.gpu_fan_rpm);

    // 5. Battery Gate Watchdog: Protect platform override from accidental drop (Tongfang platforms only)
    if now >= c.battery_limit_cooldown_until {
        let cfg_snap = crate::system::config::load_config();
        if cfg_snap.battery_hardware_limit_enabled && c.battery_limit < 95 {
            if let Some(cur_plat_id) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_PLATFORM_REAL_ID) {
                if cur_plat_id != 0x04 {
                    crate::log_warn!(
                        "EC_WATCHDOG",
                        "Battery hardware gate dropped (platform code 0x{:02X})! Auto-healing platform override...",
                        cur_plat_id
                    );
                    let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_PLATFORM_REAL_ID, 0x04);
                    let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_PLATFORM_OVERRIDE, wmi_ec::BATTERY_OVERRIDE_RESET_VALUE);
                    let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_LIMIT_UP, c.battery_limit);
                    let down = if c.battery_limit > 5 { c.battery_limit - 5 } else { c.battery_limit };
                    let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_LIMIT_DOWN, down);
                }
            }
        }
    }
}

impl EcBridge {
    pub fn new(conflict: bool) -> Self {
        let initial_cfg = crate::system::config::load_config();
        let initial_mode = if initial_cfg.power_mode >= 1 && initial_cfg.power_mode <= 4 {
            initial_cfg.power_mode
        } else {
            2
        };
        let mut initial_cache = HardwareStateCache::default();
        initial_cache.power_mode = initial_mode;
        initial_cache.fan_boost = if initial_cfg.fan_boost && wmi_ec::is_acpi_driver_available() {
            wmi_ec::is_fan_boost_active()
        } else {
            false
        };
        initial_cache.battery_limit = initial_cfg.battery_limit;
        // 5 second cooldown on boot so poller never overrides initial configuration during bootup
        initial_cache.power_mode_cooldown_until = Instant::now() + Duration::from_secs(5);
        initial_cache.fan_boost_cooldown_until = Instant::now() + Duration::from_secs(5);
        initial_cache.battery_limit_cooldown_until = Instant::now() + Duration::from_secs(5);

        let cache = Arc::new(Mutex::new(initial_cache));
        crate::system::ec_sampler::attach(Arc::clone(&cache));

        let hal_supported = crate::hal::get_hardware_driver().is_hardware_supported();
        let is_mock = !wmi_ec::is_acpi_driver_available()
            && !crate::driver::wmi_mulong::is_mulong_available()
            && !hal_supported;
        Self {
            cache,
            is_mock,
            oem_conflict_detected: conflict,
        }
    }

    pub fn dummy() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HardwareStateCache::default())),
            is_mock: true,
            oem_conflict_detected: false,
        }
    }

    pub fn auto_load() -> Self {
        #[cfg(windows)]
        {
            let hal = crate::hal::get_hardware_driver();
            if !crate::hal::platform_probe::is_tongfang_family() && !hal.is_hardware_supported() {
                crate::log_info!(
                    "EC_BRIDGE",
                    "Non-Tongfang platform without active HAL detected. Falling back to dummy."
                );
                return Self::dummy();
            }
        }

        let oem_running = process::is_oem_service_running();
        Self::new(oem_running)
    }

    pub fn reload(&mut self) {
        *self = Self::auto_load();
    }

    pub fn get_cpu_temp(&self) -> i32 {
        self.cache.lock().cpu_temp
    }

    pub fn get_gpu_temp(&self) -> i32 {
        42
    }

    pub fn get_power_mode(&self) -> u8 {
        self.cache.lock().power_mode
    }

    /// Called by `system::mode` only. Records the mode and applies it on a worker thread.
    pub fn set_power_mode(&self, ui_mode: u8) {
        crate::log_info!("EC_BRIDGE", "set_power_mode: {}", ui_mode);
        {
            let mut c = self.cache.lock();
            c.power_mode = ui_mode;
            c.power_mode_cooldown_until = Instant::now() + Duration::from_secs(5);
        }

        // Apply whatever is the newest request when the worker gets to run: two quick requests
        // start two workers in no guaranteed order, and the older one must not win.
        let cache = Arc::clone(&self.cache);
        thread::spawn(move || loop {
            let newest = cache.lock().power_mode;
            crate::system::power_mode::apply_full_power_mode(newest);
            // Workers are not ordered: if a newer request was applied before this one, the
            // hardware now holds the older mode. Whoever finishes last checks and corrects it.
            if cache.lock().power_mode == newest {
                break;
            }
        });
    }

    /// Called by `system::mode` only: update the cached mode (the hardware mode is applied by the caller).
    pub fn set_power_mode_cache(&self, ui_mode: u8) {
        let mut c = self.cache.lock();
        c.power_mode = ui_mode;
        c.power_mode_cooldown_until = Instant::now() + Duration::from_secs(5);
    }


    pub fn is_fan_boost(&self) -> bool {
        self.cache.lock().fan_boost
    }

    pub fn is_in_fan_boost_cooldown(&self) -> bool {
        Instant::now() < self.cache.lock().fan_boost_cooldown_until
    }

    pub fn set_fan_boost(&self, enable: bool) {
        let support_flags = crate::driver::wmi_ec::get_hardware_support_flags();
        if !support_flags.has_fan_boost {
            crate::log_info!("EC_BRIDGE", "Platform does not support fan boost: ignoring request.");
            return;
        }
        {
            let mut c = self.cache.lock();
            c.fan_boost = enable;
            // Cooldown prevents background poller and event echoes from oscillating status
            c.fan_boost_cooldown_until = Instant::now() + Duration::from_secs(3);
        }

        // Proactively align user config and tray icon
        crate::system::config::modify_config(|c| c.fan_boost = enable);
        crate::update_tray_fan_boost(enable);

        // Apply fan boost to hardware driver (Tongfang WMI EC or Declarative HAL)
        thread::spawn(move || {
            wmi_ec::set_fan_boost_hardware(enable);
        });
    }

    pub fn set_fan_boost_from_hardware(&self, enable: bool) {
        crate::log_info!("EC_BRIDGE", "Hardware aligned set_fan_boost: {}", enable);
        {
            let mut c = self.cache.lock();
            c.fan_boost = enable;
            c.fan_boost_cooldown_until = Instant::now() + Duration::from_secs(3);
        }

        // Proactively align user config and tray icon without re-pulsing hardware EC
        crate::system::config::modify_config(|c| c.fan_boost = enable);
        crate::update_tray_fan_boost(enable);
    }

    pub fn get_battery_limit(&self) -> u8 {
        self.cache.lock().battery_limit
    }

    pub fn set_battery_limit(&self, limit: u8) {
        let cfg = crate::system::config::load_config();
        self.apply_battery_limit_with_override(limit, cfg.battery_hardware_limit_enabled);
    }

    pub fn set_battery_hardware_limit_override(&self, hardware_override: bool) {
        let cfg = crate::system::config::load_config();
        self.apply_battery_limit_with_override(cfg.battery_limit, hardware_override);
    }

    pub fn apply_battery_limit_with_override(&self, limit: u8, hardware_override: bool) {
        {
            let mut c = self.cache.lock();
            c.battery_limit = limit;
            c.battery_limit_cooldown_until = Instant::now() + Duration::from_secs(5);
        }

        thread::spawn(move || {
            let is_tongfang_hardware = crate::driver::wmi_ec::is_acpi_driver_available();

            if !is_tongfang_hardware {
                let health_mode = if limit >= 95 {
                    crate::hal::BatteryHealthMode::LongLife
                } else if limit <= 65 {
                    crate::hal::BatteryHealthMode::Station
                } else {
                    crate::hal::BatteryHealthMode::Balance
                };
                let hal = crate::hal::get_hardware_driver();
                let _ = hal.set_battery_health(health_mode);
                return;
            }

            let mode_code = if limit >= 95 {
                wmi_ec::BATTERY_MODE_LONGLIFE
            } else if limit <= 65 {
                wmi_ec::BATTERY_MODE_STATION
            } else {
                wmi_ec::BATTERY_MODE_BALANCE
            };

            let (limit, down_limit) = crate::machine::battery_charge_window(limit);

            // 0. Platform4 gate
            if hardware_override && limit < 95 {
                wmi_ec::wmi_write_ec(wmi_ec::ADDR_PLATFORM_REAL_ID, 0x04);
            } else {
                let cur_id = wmi_ec::wmi_read_ec(wmi_ec::ADDR_PLATFORM_REAL_ID).unwrap_or(0x07);
                let restore_id = if cur_id == 0x04 || cur_id == 0x05 { 0x07 } else { cur_id };
                wmi_ec::wmi_write_ec(wmi_ec::ADDR_PLATFORM_REAL_ID, restore_id);
            }
            wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_PLATFORM_OVERRIDE, wmi_ec::BATTERY_OVERRIDE_RESET_VALUE);

            // 1. First write cut-off thresholds (Upper, Lower)
            wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_LIMIT_UP, limit);
            wmi_ec::wmi_write_ec(wmi_ec::ADDR_BATTERY_LIMIT_DOWN, down_limit);

            // 2. Atomically update battery protection mode while strictly preserving Touchpad state bit
            let final_mode_byte = wmi_ec::wmi_modify_ec(wmi_ec::ADDR_BATTERY_PROTECTION_MODE, |cur_bat_mode| {
                let tp_bit = cur_bat_mode & wmi_ec::TOUCHPAD_BITMASK;
                mode_code | tp_bit
            }).unwrap_or(mode_code);

            // 3. Immediately align Q-Key LED clamp for active Office or Balance mode under platform override (Gaming laptops only)
            let current_power_mode = crate::system::config::load_config().power_mode;
            if current_power_mode == 1 || current_power_mode == 2 {
                let fan_boost = crate::system::config::load_config().fan_boost;
                let boost_bit = if fan_boost { wmi_ec::FAN_BOOST_CLAMP_BIT } else { 0 };
                let target_clamp = (if current_power_mode == 1 { 0xA0 } else { 0x00 }) | boost_bit;
                wmi_ec::wmi_write_ec(wmi_ec::ADDR_FAN_BOOST_CLAMP, target_clamp);
            }

            crate::log_info!(
                "EC_BATTERY",
                "Hardware battery limit applied [Platform4-REAL_ID]: limit={}%, down={}, hw_override={}, mode_byte=0x{:02X}",
                limit, down_limit, hardware_override, final_mode_byte
            );
        });
    }

    pub fn get_cpu_fan_rpm(&self) -> u16 {
        self.cache.lock().cpu_fan_rpm
    }

    pub fn get_gpu_fan_rpm(&self) -> u16 {
        self.cache.lock().gpu_fan_rpm
    }

    pub fn update_fan_rpm(&self, cpu: u16, gpu: u16) {
        let mut c = self.cache.lock();
        if cpu > 0 { c.cpu_fan_rpm = cpu; }
        if gpu > 0 { c.gpu_fan_rpm = gpu; }
    }
}

pub type SharedEcBridge = Arc<Mutex<EcBridge>>;
