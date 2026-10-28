use std::os::windows::process::CommandExt;
use std::process::Command;
use windows::core::w;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ,
    HKEY, REG_OPTION_NON_VOLATILE,
};

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Read Windows Camera Group Policy (HKLM\SOFTWARE\Policies\Microsoft\Camera\AllowCamera)
pub fn read_camera_policy_registry() -> Option<bool> {
    unsafe {
        let mut hkey = HKEY::default();
        let sub_key = w!(r"SOFTWARE\Policies\Microsoft\Camera");
        let value_name = w!("AllowCamera");
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub_key, 0, KEY_READ, &mut hkey).is_ok() {
            let mut data_type = REG_DWORD;
            let mut data = 0u32;
            let mut data_size = std::mem::size_of::<u32>() as u32;
            let status = RegQueryValueExW(
                hkey,
                value_name,
                None,
                Some(&mut data_type),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut data_size),
            );
            let _ = RegCloseKey(hkey);
            if status.is_ok() {
                return Some(data != 0);
            }
        }
    }
    None
}

/// Write Windows Camera Group Policy (HKLM\SOFTWARE\Policies\Microsoft\Camera\AllowCamera)
pub fn write_camera_policy_registry(enable: bool) {
    unsafe {
        let mut hkey = HKEY::default();
        let sub_key = w!(r"SOFTWARE\Policies\Microsoft\Camera");
        let value_name = w!("AllowCamera");
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            sub_key,
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ).is_ok() {
            let val = if enable { 1u32 } else { 0u32 };
            let _ = RegSetValueExW(
                hkey,
                value_name,
                0,
                REG_DWORD,
                Some(std::slice::from_raw_parts(&val as *const u32 as *const u8, 4)),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

/// Check camera hardware device status via Windows PnP utility (~10ms)
pub fn get_camera_pnp_status() -> Option<bool> {
    let output = Command::new("pnputil")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["/enum-devices", "/class", "Camera", "/connected"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found_any = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Status:") {
            found_any = true;
            if trimmed.contains("Started") {
                return Some(true);
            }
        }
    }
    if found_any {
        Some(false)
    } else {
        None
    }
}

/// Read Windows Camera Privacy access state (HKCU / HKLM ConsentStore\webcam)
pub fn read_camera_consent_registry() -> Option<bool> {
    unsafe {
        let mut hkey = HKEY::default();
        let sub_key = w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam");
        let value_name = w!("Value");
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub_key, 0, KEY_READ, &mut hkey).is_ok() {
            let mut buf = [0u16; 64];
            let mut data_type = REG_SZ;
            let mut data_size = (buf.len() * 2) as u32;
            let status = RegQueryValueExW(
                hkey,
                value_name,
                None,
                Some(&mut data_type),
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut data_size),
            );
            let _ = RegCloseKey(hkey);
            if status.is_ok() {
                let s = String::from_utf16_lossy(&buf[..(data_size as usize / 2).saturating_sub(1)]);
                return Some(s.trim_matches('\0').eq_ignore_ascii_case("Allow"));
            }
        }
    }
    None
}

/// Write Windows Camera Privacy access state (HKCU and HKLM ConsentStore\webcam, including NonPackaged desktop apps)
pub fn write_camera_consent_registry(enable: bool) {
    let str_val: &[u16] = if enable {
        &[b'A' as u16, b'l' as u16, b'l' as u16, b'o' as u16, b'w' as u16, 0]
    } else {
        &[b'D' as u16, b'e' as u16, b'n' as u16, b'y' as u16, 0]
    };
    let val_name = w!("Value");

    let sub_keys = [
        w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam"),
        w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam\NonPackaged"),
    ];

    for sub_key in sub_keys {
        // HKCU
        unsafe {
            let mut hkey = HKEY::default();
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key,
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            ).is_ok() {
                let _ = RegSetValueExW(
                    hkey,
                    val_name,
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(str_val.as_ptr() as *const u8, str_val.len() * 2)),
                );
                let _ = RegCloseKey(hkey);
            }
        }

        // HKLM
        unsafe {
            let mut hkey_lm = HKEY::default();
            if RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                sub_key,
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey_lm,
                None,
            ).is_ok() {
                let _ = RegSetValueExW(
                    hkey_lm,
                    val_name,
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(str_val.as_ptr() as *const u8, str_val.len() * 2)),
                );
                let _ = RegCloseKey(hkey_lm);
            }
        }
    }
}

/// Read real-time Camera hardware / system status (PnP Device Manager + Policy + ConsentStore + EC)
pub fn get_realtime_camera_state(has_ec_switches: bool, fallback: bool) -> bool {
    // 1. Check PnP Device Manager status via pnputil (~10ms)
    if let Some(started) = get_camera_pnp_status() {
        return started;
    }

    // 2. Check Windows Camera Group Policy (0 = disabled, 1 = enabled)
    if let Some(policy_allowed) = read_camera_policy_registry() {
        if !policy_allowed {
            return false;
        }
    }

    // 3. Check Windows Privacy ConsentStore
    if let Some(consent_allowed) = read_camera_consent_registry() {
        if !consent_allowed {
            return false;
        }
    }

    // 4. Hardware EC check: on supported hardware with physical LDO line
    if has_ec_switches {
        if let Some(val) = crate::driver::wmi_ec::wmi_read_ec(crate::driver::wmi_ec::ADDR_CAMERA_CUTOFF) {
            if (val & crate::driver::wmi_ec::CAMERA_CUTOFF_BITMASK) != 0 {
                return false;
            }
        }
    }

    fallback
}
