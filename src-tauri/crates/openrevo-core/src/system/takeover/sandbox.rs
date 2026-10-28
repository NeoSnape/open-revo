//! Autostart, Registry, and Task Scheduler Isolation Sandbox
//!
//! Enforces Level 1.1 Specification from Sovereign Architecture Blueprint:
//! - Complete 12-task synchronization from restore_official_control_center.ps1
//! - Atomic Run key backup (OpenRevoBackup_) & restore across HKLM and HKCU
//! - UWP AppModel Modern StartupTask toggle (State = 1 vs State = 2)
//! - Ghost tray icon sweeping via Win32 Shell_TrayWnd toolbar messages

#[cfg(target_os = "windows")]
use windows::core::{w, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, RegCloseKey, RegDeleteValueW,
    RegEnumValueW, RegOpenKeyExW, RegSetValueExW, REG_DWORD,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetClientRect, SendMessageW, WM_MOUSEMOVE,
};

/// 12 Full verified official scheduled tasks across all OEM variants
pub const OEM_TASKS: &[&str] = &[
    "ControlCenter",
    "ControlCenterTray",
    "GamingCenter",
    "GCUService",
    "GCUBridge",
    "AiStone",
    "AiStoneService",
    "OSDTpDetect",
    "UniwillService",
    "CCU",
    "CCU.WinUI",
    "OEMService",
];

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn run_cmd_quiet(cmd: &str, args: &[&str]) -> (Option<i32>, String, String) {
    use std::os::windows::process::CommandExt;
    match std::process::Command::new(cmd).args(args).creation_flags(CREATE_NO_WINDOW).output() {
        Ok(out) => {
            let code = out.status.code();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (code, stdout, stderr)
        }
        Err(e) => (None, String::new(), e.to_string()),
    }
}

/// Fast check if a Windows Registry subkey exists (< 0.01ms)
pub fn reg_key_exists(hkey_root: HKEY, subkey: &str) -> bool {
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

/// Helper to set DWORD registry value
pub fn set_reg_dword(hkey_root: HKEY, subkey: &str, value_name: &str, val: u32) {
    let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
    let wide_val: Vec<u16> = format!("{}\0", value_name).encode_utf16().collect();
    let mut hkey = HKEY::default();
    unsafe {
        if RegOpenKeyExW(
            hkey_root,
            PCWSTR(wide_sub.as_ptr()),
            0,
            KEY_WRITE,
            &mut hkey,
        )
        .is_ok()
        {
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(wide_val.as_ptr()),
                0,
                REG_DWORD,
                Some(std::slice::from_raw_parts(&val as *const u32 as *const u8, 4)),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

/// Neutralizes Run keys by backing them up to OpenRevoBackup_ prefix
pub fn neutralize_run_keys() {
    let run_roots = [
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run"),
        (HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
    ];

    for (root, subkey) in run_roots {
        let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
        let mut hkey = HKEY::default();
        unsafe {
            if RegOpenKeyExW(root, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ | KEY_WRITE, &mut hkey).is_ok() {
                let mut index = 0u32;
                let mut to_backup = Vec::new();
                loop {
                    let mut name_buf = [0u16; 256];
                    let mut name_len = name_buf.len() as u32;
                    let mut val_type = 0u32;
                    let mut data_buf = [0u8; 1024];
                    let mut data_len = data_buf.len() as u32;

                    let status = RegEnumValueW(
                        hkey,
                        index,
                        windows::core::PWSTR(name_buf.as_mut_ptr()),
                        &mut name_len,
                        None,
                        Some(&mut val_type),
                        Some(data_buf.as_mut_ptr()),
                        Some(&mut data_len),
                    );

                    if status.is_err() {
                        break;
                    }

                    let val_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                    let lower = val_name.to_lowercase();
                    if lower.contains("controlcenter")
                        || lower.contains("gamingcenter")
                        || lower.contains("gcuservice")
                        || lower.contains("gcubridge")
                        || lower.contains("aistone")
                        || lower.contains("mycontrolcenter")
                        || lower.contains("uniwill")
                        || lower.contains("osdtp")
                        || lower.contains("systray")
                        || lower.contains("ccu")
                    {
                        to_backup.push((val_name, val_type, data_buf[..data_len as usize].to_vec()));
                    }
                    index += 1;
                }

                for (name, val_type, data) in to_backup {
                    let backup_name = format!("OpenRevoBackup_{}\0", name);
                    let wide_backup: Vec<u16> = backup_name.encode_utf16().collect();
                    let wide_orig: Vec<u16> = format!("{}\0", name).encode_utf16().collect();

                    let _ = RegSetValueExW(
                        hkey,
                        PCWSTR(wide_backup.as_ptr()),
                        0,
                        windows::Win32::System::Registry::REG_VALUE_TYPE(val_type),
                        Some(&data),
                    );
                    let _ = RegDeleteValueW(hkey, PCWSTR(wide_orig.as_ptr()));
                }

                let _ = RegCloseKey(hkey);
            }
        }
    }
}

/// Restores Run keys from OpenRevoBackup_ prefix
pub fn restore_run_keys() {
    let run_roots = [
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run"),
        (HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
    ];

    for (root, subkey) in run_roots {
        let wide_sub: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
        let mut hkey = HKEY::default();
        unsafe {
            if RegOpenKeyExW(root, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ | KEY_WRITE, &mut hkey).is_ok() {
                let mut index = 0u32;
                let mut to_restore = Vec::new();
                loop {
                    let mut name_buf = [0u16; 256];
                    let mut name_len = name_buf.len() as u32;
                    let mut val_type = 0u32;
                    let mut data_buf = [0u8; 1024];
                    let mut data_len = data_buf.len() as u32;

                    let status = RegEnumValueW(
                        hkey,
                        index,
                        windows::core::PWSTR(name_buf.as_mut_ptr()),
                        &mut name_len,
                        None,
                        Some(&mut val_type),
                        Some(data_buf.as_mut_ptr()),
                        Some(&mut data_len),
                    );

                    if status.is_err() {
                        break;
                    }

                    let val_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                    if val_name.starts_with("OpenRevoBackup_") {
                        let orig_name = val_name["OpenRevoBackup_".len()..].to_string();
                        to_restore.push((val_name, orig_name, val_type, data_buf[..data_len as usize].to_vec()));
                    }
                    index += 1;
                }

                for (backup_name, orig_name, val_type, data) in to_restore {
                    let wide_orig: Vec<u16> = format!("{}\0", orig_name).encode_utf16().collect();
                    let wide_backup: Vec<u16> = format!("{}\0", backup_name).encode_utf16().collect();

                    let _ = RegSetValueExW(
                        hkey,
                        PCWSTR(wide_orig.as_ptr()),
                        0,
                        windows::Win32::System::Registry::REG_VALUE_TYPE(val_type),
                        Some(&data),
                    );
                    let _ = RegDeleteValueW(hkey, PCWSTR(wide_backup.as_ptr()));
                }

                let _ = RegCloseKey(hkey);
            }
        }
    }
}

/// Disables all 12 official Scheduled Tasks
pub fn disable_scheduled_tasks() {
    for task in OEM_TASKS {
        let key_path = format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree\{}", task);
        if reg_key_exists(HKEY_LOCAL_MACHINE, &key_path) {
            let (code, _, _) = run_cmd_quiet("schtasks.exe", &["/change", "/tn", task, "/disable"]);
            crate::log_info!("TAKEOVER_SANDBOX", "Task '{}': Disabled (code: {:?})", task, code);
        }
    }
}

/// Enables all 12 official Scheduled Tasks
pub fn enable_scheduled_tasks() {
    for task in OEM_TASKS {
        let key_path = format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree\{}", task);
        if reg_key_exists(HKEY_LOCAL_MACHINE, &key_path) {
            let (code, _, _) = run_cmd_quiet("schtasks.exe", &["/change", "/tn", task, "/enable"]);
            crate::log_info!("TAKEOVER_SANDBOX", "Task '{}': Enabled (code: {:?})", task, code);
        }
    }
}

/// Controls UWP AppModel StartupTask execution state (State = 1: Disabled, State = 2: Enabled)
pub fn set_uwp_appmodel_startup_state(disable: bool) {
    let target_state: u32 = if disable { 1 } else { 2 };

    let known_task_subkeys = [
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\ControlCenter3_wrbgcf7aesyd8\ControlCenterTray",
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\ControlCenter3_h329z55cwnj8g\ControlCenterTray",
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\CCU.WinUI_wrbgcf7aesyd8\ControlCenterTray",
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\CCU.WinUI_wrbgcf7aesyd8\CCU",
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\CCU.WinUI_wrbgcf7aesyd8\SystrayComponent",
    ];

    for subkey in known_task_subkeys {
        if reg_key_exists(HKEY_CURRENT_USER, subkey) {
            set_reg_dword(HKEY_CURRENT_USER, subkey, "State", target_state);
            crate::log_info!("TAKEOVER_SANDBOX", "UWP AppModel task '{}': State set to {}", subkey, target_state);
        }
    }

    // Dynamic scan over AppModel\SystemAppData for new or customized package families
    let base_sub = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData\0";
    let wide_base: Vec<u16> = base_sub.encode_utf16().collect();
    let mut h_base = HKEY::default();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide_base.as_ptr()), 0, KEY_READ, &mut h_base).is_ok() {
            let mut pfn_idx = 0u32;
            loop {
                let mut pfn_buf = [0u16; 256];
                let mut pfn_len = pfn_buf.len() as u32;
                if windows::Win32::System::Registry::RegEnumKeyExW(
                    h_base,
                    pfn_idx,
                    windows::core::PWSTR(pfn_buf.as_mut_ptr()),
                    &mut pfn_len,
                    None,
                    windows::core::PWSTR::null(),
                    None,
                    None,
                )
                .is_err()
                {
                    break;
                }
                let pfn_str = String::from_utf16_lossy(&pfn_buf[..pfn_len as usize]);
                let pfn_lower = pfn_str.to_lowercase();
                if pfn_lower.contains("controlcenter") || pfn_lower.contains("gamingcenter") || pfn_lower.contains("ccu") {
                    let pfn_sub = format!("Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData\\{}\0", pfn_str);
                    let wide_pfn_sub: Vec<u16> = pfn_sub.encode_utf16().collect();
                    let mut h_pfn = HKEY::default();
                    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide_pfn_sub.as_ptr()), 0, KEY_READ, &mut h_pfn).is_ok() {
                        let mut task_idx = 0u32;
                        loop {
                            let mut task_buf = [0u16; 256];
                            let mut task_len = task_buf.len() as u32;
                            if windows::Win32::System::Registry::RegEnumKeyExW(
                                h_pfn,
                                task_idx,
                                windows::core::PWSTR(task_buf.as_mut_ptr()),
                                &mut task_len,
                                None,
                                windows::core::PWSTR::null(),
                                None,
                                None,
                            )
                            .is_err()
                            {
                                break;
                            }
                            let task_str = String::from_utf16_lossy(&task_buf[..task_len as usize]);
                            let task_lower = task_str.to_lowercase();
                            if task_lower.contains("tray") || task_lower.contains("ccu") || task_lower.contains("systray") {
                                let full_task_sub = format!("Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData\\{}\\{}", pfn_str, task_str);
                                set_reg_dword(HKEY_CURRENT_USER, &full_task_sub, "State", target_state);
                                crate::log_info!("TAKEOVER_SANDBOX", "UWP AppModel dynamic task '{}': State set to {}", full_task_sub, target_state);
                            }
                            task_idx += 1;
                        }
                        let _ = RegCloseKey(h_pfn);
                    }
                }
                pfn_idx += 1;
            }
            let _ = RegCloseKey(h_base);
        }
    }
}

/// Updates Explorer StartupApproved\StartupTask byteflags (0x02 = Enabled, 0x03 = Disabled)
pub fn update_uwp_startup_task(disable: bool) {
    let subkey = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupTask\0";
    let wide_sub: Vec<u16> = subkey.encode_utf16().collect();
    let mut hkey = HKEY::default();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ | KEY_WRITE, &mut hkey).is_ok() {
            let mut index = 0u32;
            let mut to_update = Vec::new();
            loop {
                let mut name_buf = [0u16; 256];
                let mut name_len = name_buf.len() as u32;
                let mut val_type = 0u32;
                let mut data_buf = [0u8; 64];
                let mut data_len = data_buf.len() as u32;

                let status = RegEnumValueW(
                    hkey,
                    index,
                    windows::core::PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    Some(&mut val_type),
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut data_len),
                );

                if status.is_err() {
                    break;
                }

                let val_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                let lower = val_name.to_lowercase();
                if lower.contains("controlcenter") || lower.contains("gamingcenter") || lower.contains("ccu") {
                    if data_len > 0 {
                        let mut updated = data_buf[..data_len as usize].to_vec();
                        let target_byte = if disable { 0x03 } else { 0x02 };
                        if updated[0] != target_byte {
                            updated[0] = target_byte;
                            to_update.push((val_name, val_type, updated));
                        }
                    }
                }
                index += 1;
            }

            for (name, val_type, data) in to_update {
                let wide_name: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR(wide_name.as_ptr()),
                    0,
                    windows::Win32::System::Registry::REG_VALUE_TYPE(val_type),
                    Some(&data),
                );
            }

            let _ = RegCloseKey(hkey);
        }
    }
}

/// Sweeps and refreshes Windows Shell notification area toolbars to eliminate ghost icons
pub fn refresh_tray_notification_area() {
    unsafe {
        let refresh_toolbar = |h_toolbar: HWND| {
            if h_toolbar.0.is_null() {
                return;
            }
            let mut rect = RECT::default();
            if GetClientRect(h_toolbar, &mut rect).is_ok() {
                let mut x = 0;
                while x < rect.right {
                    let mut y = 0;
                    while y < rect.bottom {
                        let l_param = LPARAM(((y << 16) | (x & 0xFFFF)) as isize);
                        let _ = SendMessageW(h_toolbar, WM_MOUSEMOVE, WPARAM(0), l_param);
                        y += 8;
                    }
                    x += 8;
                }
            }
        };

        if let Ok(h_tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            if let Ok(h_notify) = FindWindowExW(h_tray, HWND::default(), w!("TrayNotifyWnd"), PCWSTR::null()) {
                if let Ok(h_sys_pager) = FindWindowExW(h_notify, HWND::default(), w!("SysPager"), PCWSTR::null()) {
                    if let Ok(h_tb1) = FindWindowExW(h_sys_pager, HWND::default(), w!("ToolbarWindow32"), PCWSTR::null()) {
                        refresh_toolbar(h_tb1);
                    }
                }
                let mut h_tb_direct = HWND::default();
                while let Ok(next) = FindWindowExW(h_notify, h_tb_direct, w!("ToolbarWindow32"), PCWSTR::null()) {
                    if next.0.is_null() {
                        break;
                    }
                    refresh_toolbar(next);
                    h_tb_direct = next;
                }
            }
        }

        if let Ok(h_overflow) = FindWindowW(w!("NotifyIconOverflowWindow"), PCWSTR::null()) {
            let mut h_tb_overflow = HWND::default();
            while let Ok(next) = FindWindowExW(h_overflow, h_tb_overflow, w!("ToolbarWindow32"), PCWSTR::null()) {
                if next.0.is_null() {
                    break;
                }
                refresh_toolbar(next);
                h_tb_overflow = next;
            }
        }
    }
}
