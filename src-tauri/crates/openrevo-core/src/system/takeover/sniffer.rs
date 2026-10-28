//! Triple-Tier Native Truth Sniffer for Official Control Center
//!
//! Enforces Level 1.1 Specification from Sovereign Architecture Blueprint:
//! 1. Tier A (SCM Anchor Inversion): Queries HKLM\SYSTEM\CurrentControlSet\Services\<Anchor>\ImagePath,
//!    inverts directory hierarchy to discover official control center executables (0ms fast path).
//! 2. Tier B (Uninstall Registry Tree): Dynamically traverses 32-bit and 64-bit Uninstall registries
//!    for DisplayName keywords and inspects DisplayIcon / InstallLocation.
//! 3. Tier C (UWP AppModel Protocol): Discovers Modern Windows AppModel Package Family Names.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    RegQueryValueExW, REG_SZ,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCenterLocation {
    Win32Exe(PathBuf),
    UwpApp(String),
}

/// Candidate executable file names across all Tongfang, Mechrevo, ThundeRobot generations
pub const EXECUTABLE_CANDIDATES: &[&str] = &[
    "ControlCenterU.exe",
    "GamingCenterU.exe",
    "ControlCenter.exe",
    "GamingCenter.exe",
    "CCU.exe",
    "CCU.WinUI.exe",
    "MyControlCenter.exe",
    "蛟龙游戏控制中心.exe",
    "JiaolongCC.exe",
    "BLDFnHotkeyUtility.exe",
    "BLDHotKeyService.exe",
];

/// Known service anchors used for reverse directory derivation
pub const SERVICE_ANCHORS: &[&str] = &[
    "GCUBridge",
    "GCUService",
    "AiStoneService",
    "UniwillService",
    "OEMService",
    "BLDHotKeyService",
];

/// Keywords for dynamic Uninstall registry scanning
pub const UNINSTALL_KEYWORDS: &[&str] = &[
    "controlcenter",
    "gamingcenter",
    "机械革命",
    "智控中心",
    "蛟龙游戏控制中心",
    "jiaolong",
    "bldhotkey",
    "uniwill",
    "ccu",
    "thunderobot",
];

/// Known official UWP AppModel Package Family candidates
pub const UWP_CANDIDATES: &[&str] = &[
    "CCU.WinUI_wrbgcf7aesyd8!App",
    "ControlCenter3_wrbgcf7aesyd8!App",
    "ControlCenter3_h329z55cwnj8g!App",
];

/// Helper to read a registry string value safely
#[cfg(target_os = "windows")]
fn read_reg_string(hkey: HKEY, value_name: &str) -> Option<String> {
    let wide_val: Vec<u16> = format!("{}\0", value_name).encode_utf16().collect();
    let mut val_type = REG_SZ;
    let mut data_buf = [0u8; 1024];
    let mut data_len = data_buf.len() as u32;

    unsafe {
        let status = RegQueryValueExW(
            hkey,
            PCWSTR(wide_val.as_ptr()),
            None,
            Some(&mut val_type),
            Some(data_buf.as_mut_ptr()),
            Some(&mut data_len),
        );

        if status.is_ok() && data_len > 0 {
            let u16_slice = std::slice::from_raw_parts(
                data_buf.as_ptr() as *const u16,
                (data_len as usize) / 2,
            );
            let s = String::from_utf16_lossy(u16_slice);
            Some(s.trim_matches('\0').trim().to_string())
        } else {
            None
        }
    }
}

/// Cleans raw ImagePath string (e.g. `\"C:\\Path\\file.exe\" -arg` -> `C:\\Path\\file.exe`)
pub fn clean_image_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') {
        if let Some(end) = trimmed[1..].find('"') {
            return PathBuf::from(&trimmed[1..=end]);
        }
    }
    if let Some(space_idx) = trimmed.find(" -") {
        return PathBuf::from(&trimmed[..space_idx]);
    }
    if let Some(space_idx) = trimmed.find(" /") {
        return PathBuf::from(&trimmed[..space_idx]);
    }
    PathBuf::from(trimmed)
}

/// Helper to search candidates in a directory and its immediate subdirectories
fn probe_dir_for_candidates(dir: &Path) -> Option<PathBuf> {
    if !dir.exists() || !dir.is_dir() {
        return None;
    }

    // 1. Direct check in target directory
    for candidate in EXECUTABLE_CANDIDATES {
        let target = dir.join(candidate);
        if target.is_file() {
            return Some(target);
        }
    }

    // 2. Immediate subdirectories (e.g. GamingCenter, ControlCenter, bin, App)
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let sub_dir = entry.path();
                    for candidate in EXECUTABLE_CANDIDATES {
                        let target = sub_dir.join(candidate);
                        if target.is_file() {
                            return Some(target);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Tier A: SCM Service Anchor Inversion Algorithm
/// Reads SCM service ImagePath and walks up to 2 parent directories to locate companion frontends.
pub fn sniff_via_scm_anchors() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        for anchor in SERVICE_ANCHORS {
            let subkey = format!("SYSTEM\\CurrentControlSet\\Services\\{}\0", anchor);
            let wide_sub: Vec<u16> = subkey.encode_utf16().collect();
            let mut hkey = HKEY::default();
            unsafe {
                if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ, &mut hkey).is_ok() {
                    let img_opt = read_reg_string(hkey, "ImagePath");
                    let _ = RegCloseKey(hkey);

                    if let Some(raw_path) = img_opt {
                        let clean_path = clean_image_path(&raw_path);
                        if clean_path.exists() {
                            // Check service's parent dir (e.g. ...\UniwillService\)
                            if let Some(parent1) = clean_path.parent() {
                                if let Some(found) = probe_dir_for_candidates(parent1) {
                                    return Some(found);
                                }
                                // Check service's grandparent dir (e.g. ...\OEM\GamingCenterU\)
                                if let Some(parent2) = parent1.parent() {
                                    if let Some(found) = probe_dir_for_candidates(parent2) {
                                        return Some(found);
                                    }
                                    // Check great-grandparent dir (e.g. ...\OEM\)
                                    if let Some(parent3) = parent2.parent() {
                                        if let Some(found) = probe_dir_for_candidates(parent3) {
                                            return Some(found);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Tier B: Dynamic Uninstall Registry Tree Inversion
/// Searches 32-bit and 64-bit Uninstall registries for DisplayName matches.
pub fn sniff_via_uninstall_registry() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let roots = [
            (HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\0"),
            (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\0"),
            (HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\0"),
        ];

        for (h_root, subkey) in roots {
            let wide_sub: Vec<u16> = subkey.encode_utf16().collect();
            let mut h_base = HKEY::default();
            unsafe {
                if RegOpenKeyExW(h_root, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ, &mut h_base).is_ok() {
                    let mut index = 0u32;
                    loop {
                        let mut name_buf = [0u16; 256];
                        let mut name_len = name_buf.len() as u32;
                        if RegEnumKeyExW(
                            h_base,
                            index,
                            windows::core::PWSTR(name_buf.as_mut_ptr()),
                            &mut name_len,
                            None,
                            windows::core::PWSTR::null(),
                            None,
                            None,
                        )
                        .is_err()
                        {
                            break;
                        }

                        let app_key_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                        let item_sub = format!("{}\\{}\0", subkey.trim_matches('\0'), app_key_name);
                        let wide_item: Vec<u16> = item_sub.encode_utf16().collect();
                        let mut h_item = HKEY::default();

                        if RegOpenKeyExW(h_root, PCWSTR(wide_item.as_ptr()), 0, KEY_READ, &mut h_item).is_ok() {
                            let display_name = read_reg_string(h_item, "DisplayName").unwrap_or_default().to_lowercase();
                            let is_match = UNINSTALL_KEYWORDS.iter().any(|&k| display_name.contains(k));

                            if is_match {
                                // Try DisplayIcon first
                                if let Some(icon_raw) = read_reg_string(h_item, "DisplayIcon") {
                                    let clean_icon = clean_image_path(&icon_raw);
                                    if clean_icon.is_file() {
                                        let _ = RegCloseKey(h_item);
                                        let _ = RegCloseKey(h_base);
                                        return Some(clean_icon);
                                    } else if let Some(dir) = clean_icon.parent() {
                                        if let Some(found) = probe_dir_for_candidates(dir) {
                                            let _ = RegCloseKey(h_item);
                                            let _ = RegCloseKey(h_base);
                                            return Some(found);
                                        }
                                    }
                                }

                                // Try InstallLocation
                                if let Some(loc_raw) = read_reg_string(h_item, "InstallLocation") {
                                    let loc_dir = PathBuf::from(loc_raw.trim());
                                    if let Some(found) = probe_dir_for_candidates(&loc_dir) {
                                        let _ = RegCloseKey(h_item);
                                        let _ = RegCloseKey(h_base);
                                        return Some(found);
                                    }
                                }
                            }
                            let _ = RegCloseKey(h_item);
                        }
                        index += 1;
                    }
                    let _ = RegCloseKey(h_base);
                }
            }
        }
    }
    None
}

/// Tier C: Dynamic UWP Modern AppModel Discovery
pub fn sniff_via_uwp_appmodel() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        for candidate in UWP_CANDIDATES {
            let pfn = candidate.split('!').next().unwrap_or("");
            let subkey = format!(
                "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData\\{}\0",
                pfn
            );
            let wide_sub: Vec<u16> = subkey.encode_utf16().collect();
            let mut hkey = HKEY::default();
            unsafe {
                if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide_sub.as_ptr()), 0, KEY_READ, &mut hkey).is_ok() {
                    let _ = RegCloseKey(hkey);
                    return Some(candidate.to_string());
                }
            }
        }
    }
    None
}

/// Master Truth Sniffer: Authoritative multi-tier discovery
pub fn find_official_control_center() -> Option<ControlCenterLocation> {
    static LAST_RESULT: std::sync::Mutex<Option<Option<ControlCenterLocation>>> = std::sync::Mutex::new(None);

    let res = (|| {
        // 1. Tier A (SCM Reverse Derivation - Most accurate for installed services)
        if let Some(exe_path) = sniff_via_scm_anchors() {
            return Some(ControlCenterLocation::Win32Exe(exe_path));
        }

        // 2. Tier B (Uninstall Registry Tree Inversion)
        if let Some(exe_path) = sniff_via_uninstall_registry() {
            return Some(ControlCenterLocation::Win32Exe(exe_path));
        }

        // 3. Tier C (UWP AppModel Protocol)
        if let Some(uwp_app) = sniff_via_uwp_appmodel() {
            return Some(ControlCenterLocation::UwpApp(uwp_app));
        }

        // 4. Conservative Fallback: Well-known installation roots
        let fallback_paths = [
            r"C:\Program Files\OEM\机械革命控制中心\ControlCenter.exe",
            r"C:\Program Files\OEM\机械革命电竞控制台\GamingCenter.exe",
            r"C:\Program Files\OEM\智控中心\ControlCenter.exe",
            r"C:\Program Files\OEM\智控中心\GamingCenter.exe",
            r"C:\Program Files\OEM\GamingCenterU\GamingCenter\GamingCenterU.exe",
            r"C:\Program Files\OEM\GamingCenter\ControlCenterU.exe",
            r"C:\Program Files\OEM\ControlCenter\ControlCenter.exe",
            r"C:\Program Files\OEM\GamingCenter\GamingCenter.exe",
            r"C:\Program Files\Uniwill\GamingCenter.exe",
        ];

        for fb in fallback_paths {
            let p = PathBuf::from(fb);
            if p.is_file() {
                return Some(ControlCenterLocation::Win32Exe(p));
            }
        }

        None
    })();

    if let Ok(mut lock) = LAST_RESULT.lock() {
        if lock.as_ref() != Some(&res) {
            *lock = Some(res.clone());
            if let Some(ref loc) = res {
                crate::log_info!("TAKEOVER_SNIFFER", "Located official control center: {:?}", loc);
            }
        }
    }

    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_image_path() {
        assert_eq!(
            clean_image_path(r#""C:\Program Files\OEM\service.exe" -run"#),
            PathBuf::from(r"C:\Program Files\OEM\service.exe")
        );
        assert_eq!(
            clean_image_path(r"C:\Program Files\OEM\service.exe /svc"),
            PathBuf::from(r"C:\Program Files\OEM\service.exe")
        );
        assert_eq!(
            clean_image_path(r"C:\Uniwill\GCUBridge.exe"),
            PathBuf::from(r"C:\Uniwill\GCUBridge.exe")
        );
    }
}
