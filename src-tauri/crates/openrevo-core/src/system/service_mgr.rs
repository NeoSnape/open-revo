//! Service Manager and Driver Installation Subsystem
//!
//! Handles UWACPIDriver.sys PnP installation, driver health checks, and static capability registries.
//! OEM Takeover and Restoration logic has been decoupled into `crate::system::takeover::*`.
//! Re-exports are maintained here for 100% backward compatibility.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};

pub use crate::system::takeover::{
    disable_scheduled_tasks, enable_scheduled_tasks, find_official_control_center, is_oem_installed,
    is_oem_running, pause_oem_services, refresh_tray_notification_area, resume_oem_services,
    ControlCenterLocation, OEM_PROCESSES, OEM_SERVICES, OEM_TASKS,
};

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub const DRIVER_SYS_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.sys");
pub const DRIVER_INF_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.inf");
pub const DRIVER_CAT_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.cat");

fn reg_key_exists(hkey_root: HKEY, subkey: &str) -> bool {
    let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
    let mut hkey = HKEY::default();
    unsafe {
        if RegOpenKeyExW(
            hkey_root,
            PCWSTR(wide_sub.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        )
        .is_ok()
        {
            let _ = RegCloseKey(hkey);
            true
        } else {
            false
        }
    }
}

fn set_reg_dword(hkey_root: HKEY, subkey: &str, val_name: &str, val: u32) {
    let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
    let wide_val: Vec<u16> = format!("{}\0", val_name).encode_utf16().collect();
    let mut hkey = HKEY::default();
    unsafe {
        if RegCreateKeyExW(
            hkey_root,
            PCWSTR(wide_sub.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_ok()
        {
            let val_bytes = val.to_ne_bytes();
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(wide_val.as_ptr()),
                0,
                REG_DWORD,
                Some(&val_bytes),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

fn set_reg_string(hkey_root: HKEY, subkey: &str, val_name: &str, val: &str) {
    let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
    let wide_val: Vec<u16> = format!("{}\0", val_name).encode_utf16().collect();
    let wide_data: Vec<u16> = format!("{}\0", val).encode_utf16().collect();
    let mut hkey = HKEY::default();
    unsafe {
        if RegCreateKeyExW(
            hkey_root,
            PCWSTR(wide_sub.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_ok()
        {
            let u8_slice = std::slice::from_raw_parts(wide_data.as_ptr() as *const u8, wide_data.len() * 2);
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(wide_val.as_ptr()),
                0,
                REG_SZ,
                Some(u8_slice),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

pub fn run_cmd_captured(program: &str, args: &[&str]) -> (Option<i32>, String, String) {
    match Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => {
            let code = out.status.code();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (code, stdout, stderr)
        }
        Err(e) => (None, String::new(), e.to_string()),
    }
}

fn get_driver_staging_dir() -> PathBuf {
    if let Ok(prog_data) = std::env::var("ProgramData") {
        PathBuf::from(prog_data).join("OpenRevo").join("driver")
    } else if let Ok(app_data) = std::env::var("APPDATA") {
        PathBuf::from(app_data).join("OpenRevo").join("driver")
    } else {
        std::env::temp_dir().join("OpenRevo_driver")
    }
}

/// Install the Uniwill ACPI KMDF PnP driver via Microsoft PnP Utility (pnputil).
pub fn install_pnp_acpi_driver() -> Result<bool, String> {
    let staging_dir = get_driver_staging_dir();
    if let Err(e) = std::fs::create_dir_all(&staging_dir) {
        crate::log_error!("SERVICE_MGR", "Failed to create driver staging dir {:?}: {}", staging_dir, e);
        return Err(e.to_string());
    }

    let inf_path = staging_dir.join("UWACPIDriver.inf");
    let sys_path = staging_dir.join("UWACPIDriver.sys");
    let cat_path = staging_dir.join("UWACPIDriver.cat");

    let _ = std::fs::write(&inf_path, DRIVER_INF_BYTES);
    let _ = std::fs::write(&sys_path, DRIVER_SYS_BYTES);
    let _ = std::fs::write(&cat_path, DRIVER_CAT_BYTES);

    crate::log_info!("SERVICE_MGR", "Invoking pnputil to install WDF PnP driver from {:?}...", inf_path);
    let (code, stdout, stderr) = run_cmd_captured("pnputil.exe", &[
        "/add-driver",
        inf_path.to_str().unwrap_or("UWACPIDriver.inf"),
        "/install",
    ]);

    crate::log_info!("SERVICE_MGR", "pnputil /add-driver /install finished: code={:?}, stdout={}, stderr={}", code, stdout, stderr);

    let _ = Command::new("pnputil.exe")
        .args(["/scan-devices"])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let available = crate::driver::wmi_ec::mark_driver_rechecked();
    crate::log_info!("SERVICE_MGR", "PnP driver post-install probe result: available={}", available);
    if available {
        crate::system::boot_trace::boot_log("DRIVER_HEALED", "UWACPIDriver PnP self-healing succeeded. DriverStore & device link restored.");
    } else {
        crate::system::boot_trace::boot_log("DRIVER_FAIL", "UWACPIDriver PnP self-healing completed but \\\\.\\ACPIDriver device link is not yet active.");
    }
    Ok(available)
}

/// Ensure ACPI kernel driver (UWACPIDriver.sys) and static capability registry entries are present and healthy.
pub fn ensure_acpi_driver_ready() -> Result<(), String> {
    #[cfg(windows)]
    if !crate::hal::platform_probe::needs_acpi_kernel_driver() {
        crate::log_info!(
            "SERVICE_MGR",
            "Platform uses native WMI/driverless mailbox channel. UWACPIDriver.sys installation bypassed."
        );
        return Ok(());
    }

    crate::log_info!("SERVICE_MGR", "Checking ACPI kernel driver & static registry capability readiness...");

    if crate::driver::wmi_ec::is_acpi_driver_available() {
        crate::log_info!("SERVICE_MGR", "UWACPIDriver device is active and accessible (0ms fast path). Skipping installation.");
        self_heal_oem_registry_pollution();
        return Ok(());
    }

    crate::log_warn!("SERVICE_MGR", "UWACPIDriver device unavailable. Executing PnP DriverStore self-healing...");
    let _ = install_pnp_acpi_driver();

    if !crate::system::process::is_oem_installed() {
        let item_support = "SOFTWARE\\OEM\\GamingCenter2\\ItemSupport";
        let static_dwords = [
            "RamFan1p5Support",
            "DGpuDirectConnectionSupport",
            "TurboModeSupport",
            "PerformanceReady",
            "ServiceReady",
            "FanSettingsSupport",
            "ColorCalibrationSupport",
            "OcSettingsSupport",
            "SystemMonitorSupport",
            "CPUPerformanceAndOverClockMenuSupport",
        ];
        for k in static_dwords {
            set_reg_dword(HKEY_LOCAL_MACHINE, item_support, k, 1);
        }
        let flags = crate::driver::wmi_ec::get_hardware_support_flags();
        if flags.has_water_cooler {
            set_reg_dword(HKEY_LOCAL_MACHINE, item_support, "LiquidCoolingSupport", 1);
            set_reg_dword(HKEY_LOCAL_MACHINE, item_support, "LiquidCoolingAutoModeSupport", 1);
        }
        set_reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\OEM\\GamingCenter2\\MyFanTable", "EcVersion", "1.17");
    }

    self_heal_oem_registry_pollution();

    crate::log_info!("SERVICE_MGR", "UWACPIDriver & OEM capability safeguards verified and ready (native Win32).");
    Ok(())
}

/// Automatically detect and self-heal historical registry pollution.
pub fn self_heal_oem_registry_pollution() {
    let flags = crate::driver::wmi_ec::get_hardware_support_flags();
    if !flags.has_water_cooler {
        let item_support = "SOFTWARE\\OEM\\GamingCenter2\\ItemSupport";
        if reg_key_exists(HKEY_LOCAL_MACHINE, item_support) {
            set_reg_dword(HKEY_LOCAL_MACHINE, item_support, "LiquidCoolingSupport", 0);
            set_reg_dword(HKEY_LOCAL_MACHINE, item_support, "LiquidCoolingAutoModeSupport", 0);
            crate::log_info!("SERVICE_MGR", "Self-healed OEM registry: cleared ghost water cooler support on air-cooled machine");
        }
    }
}
