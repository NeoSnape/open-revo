/// Synchronize official OEM Control Center profile (if installed).
/// Supports both 40-series (机械革命电竞控制台) and 50-series (机械革命控制中心).
/// If neither is installed, quietly skips without error.
pub fn sync_oem_main_option(ui_mode: u8) {
    let candidate_paths = [
        r"C:\Program Files\OEM\智控中心\AiStoneService\MyControlCenter\UserPofiles\MainOption.json",
        r"C:\Program Files\OEM\机械革命电竞控制台\AiStoneService\MyControlCenter\UserPofiles\MainOption.json",
        r"C:\Program Files\OEM\机械革命控制中心\AiStoneService\MyControlCenter\UserPofiles\MainOption.json",
    ];

    let oem_mode = match ui_mode {
        1 => 0, // Office
        2 => 1, // Gaming / Balance
        3 => 2, // Turbo
        4 => 3, // Custom (GCU2_Define.OperatingMode.Custom = 3)
        _ => 1,
    };

    for oem_json in candidate_paths {
        if let Ok(content) = std::fs::read_to_string(oem_json) {
            let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            let mut modified = false;
            for line in lines.iter_mut() {
                if line.contains("\"OperatingMode\"") {
                    *line = format!("  \"OperatingMode\": {},", oem_mode);
                    modified = true;
                    break;
                }
            }
            if modified {
                let new_content = lines.join("\r\n");
                if let Err(e) = std::fs::write(oem_json, new_content) {
                    crate::log_warn!("POWER_MODE", "Failed to update OEM MainOption.json at {}: {}", oem_json, e);
                } else {
                    crate::log_info!("POWER_MODE", "Synced OEM MainOption.json at {} with OperatingMode={}", oem_json, oem_mode);
                }
            }
        }
    }
}

/// Synchronize user custom parameters into official OEM Mode4_Profile JSON files (if OEM software exists).
/// Prevents official background services (AiStoneService/GCUService) from overwriting user's active settings.
pub fn sync_oem_mode4_profile(profile: &crate::system::config::CustomProfileConfig) {
    let candidate_dirs = [
        r"C:\Program Files\OEM\智控中心\AiStoneService\MyControlCenter\UserPofiles",
        r"C:\Program Files\OEM\机械革命电竞控制台\AiStoneService\MyControlCenter\UserPofiles",
        r"C:\Program Files\OEM\机械革命控制中心\AiStoneService\MyControlCenter\UserPofiles",
        r"C:\Program Files\OEM\Control Center\AiStoneService\MyControlCenter\UserPofiles",
        r"C:\Program Files\OEM\Control Center\UniwillService\MyControlCenter\UserPofiles",
    ];

    for dir_path in candidate_dirs {
        let dir = std::path::Path::new(dir_path);
        if !dir.exists() {
            continue;
        }

        for file_name in &["Mode4_Profile1.json", "Mode4_Profile2.json", "Mode4_Profile3.json", "Mode4_Profile4.json", "Mode4_Profile5.json"] {
            let file_path = dir.join(file_name);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(cpu) = v.get_mut("CPU") {
                        cpu["PL1"] = serde_json::json!(profile.cpu_pl1_watts);
                        cpu["PL2"] = serde_json::json!(profile.cpu_pl2_watts);
                        cpu["PL4"] = serde_json::json!(profile.cpu_pl4_watts);
                        cpu["AmdSPL"] = serde_json::json!(profile.cpu_pl1_watts);
                        cpu["AmdSPPT"] = serde_json::json!(profile.cpu_pl2_watts);
                        cpu["AmdFPPT"] = serde_json::json!(profile.cpu_pl4_watts);
                    }
                    if let Some(gpu) = v.get_mut("GPU") {
                        if profile.gpu_ctgp_enabled && profile.gpu_ctgp_watts > 0 {
                            gpu["ConfigurableTGPTarget"] = serde_json::json!(profile.gpu_ctgp_watts);
                        }
                        gpu["DynamicBoostSwitch"] = serde_json::json!(if profile.gpu_db_enabled { 1 } else { 0 });
                        gpu["DynamicBoost"] = serde_json::json!(profile.gpu_db_watts);
                        gpu["CoreClockOffset"] = serde_json::json!(profile.gpu_core_offset_mhz);
                        gpu["MemoryClockOffset"] = serde_json::json!(profile.gpu_mem_offset_mhz);
                    }
                    if let Ok(new_text) = serde_json::to_string_pretty(&v) {
                        let _ = std::fs::write(&file_path, new_text);
                        crate::log_info!("POWER_MODE", "Synced OpenRevo profile to OEM JSON: {:?}", file_path);
                    }
                }
            }
        }
    }
}
