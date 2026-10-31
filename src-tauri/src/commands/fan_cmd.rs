// Preset-mode fan curve commands (machine-specific, see crate::machine).

use serde::Serialize;

#[derive(Serialize)]
pub struct CurvePoint {
    pub temp: u8,
    pub duty: u8,
}

#[derive(Serialize)]
pub struct ModeCurves {
    pub mode: u8,
    pub cpu: Vec<CurvePoint>,
    pub gpu: Vec<CurvePoint>,
}

#[derive(Serialize)]
pub struct PresetFanInfo {
    /// "tuned" | "official"
    pub policy: String,
    pub modes: Vec<ModeCurves>,
    /// Official OEM curve for comparison.
    pub official: ModeCurves,
}

fn points(v: Vec<(u8, u8)>) -> Vec<CurvePoint> {
    v.into_iter().map(|(temp, duty)| CurvePoint { temp, duty }).collect()
}

fn curves(mode: u8, policy: &str) -> ModeCurves {
    let (cpu, gpu) = crate::machine::preset_curve_points(mode, policy);
    ModeCurves { mode, cpu: points(cpu), gpu: points(gpu) }
}

#[tauri::command]
pub fn get_preset_fan_info() -> PresetFanInfo {
    let cfg = crate::system::config::load_config();
    PresetFanInfo {
        modes: (1..=3).map(|m| curves(m, &cfg.preset_fan_policy)).collect(),
        official: curves(2, crate::machine::FAN_POLICY_OFFICIAL),
        policy: cfg.preset_fan_policy,
    }
}

/// Switch preset fan policy and re-apply the table immediately when a preset mode is active.
#[tauri::command]
pub fn set_preset_fan_policy(policy: String) -> Result<PresetFanInfo, String> {
    if policy != crate::machine::FAN_POLICY_TUNED && policy != crate::machine::FAN_POLICY_OFFICIAL {
        return Err(format!("unknown fan policy '{}'", policy));
    }
    let mut cfg = crate::system::config::load_config();
    cfg.preset_fan_policy = policy;
    crate::system::config::save_config(&cfg);
    if (1..=3).contains(&cfg.power_mode) && !cfg.fan_boost {
        crate::system::power::apply_mode_fan_table(cfg.power_mode);
    }
    Ok(get_preset_fan_info())
}

#[derive(Serialize)]
pub struct EcFanTableSnapshot {
    pub cpu_upt: Vec<u8>,
    pub cpu_downt: Vec<u8>,
    pub cpu_duty_pct: Vec<u8>,
    pub gpu_upt: Vec<u8>,
    pub gpu_downt: Vec<u8>,
    pub gpu_duty_pct: Vec<u8>,
}

/// Read-only dump of the fan table currently held in EC RAM (diagnostics).
#[tauri::command]
pub fn read_ec_fan_table() -> Result<EcFanTableSnapshot, String> {
    use crate::driver::wmi_ec;
    let read16 = |base: u16| -> Result<Vec<u8>, String> {
        let addrs: Vec<u16> = (0..16).map(|i| base + i).collect();
        wmi_ec::wmi_batch_read_ec(&addrs).ok_or_else(|| "EC read failed".to_string())
    };
    let pct = |v: Vec<u8>| v.into_iter().map(|d| ((d as u16 * 100 + 100) / 200) as u8).collect();
    Ok(EcFanTableSnapshot {
        cpu_upt: read16(wmi_ec::ADDR_RAMFAN_CPU_UPT_BASE)?,
        cpu_downt: read16(wmi_ec::ADDR_RAMFAN_CPU_DOWNT_BASE)?,
        cpu_duty_pct: pct(read16(wmi_ec::ADDR_RAMFAN_CPU_DUTY_BASE)?),
        gpu_upt: read16(wmi_ec::ADDR_RAMFAN_GPU_UPT_BASE)?,
        gpu_downt: read16(wmi_ec::ADDR_RAMFAN_GPU_DOWNT_BASE)?,
        gpu_duty_pct: pct(read16(wmi_ec::ADDR_RAMFAN_GPU_DUTY_BASE)?),
    })
}

#[derive(Serialize)]
pub struct FanApplyStatus {
    /// "applied" | "mismatch" | "fan_boost" | "unreadable"
    pub state: String,
    /// What the expected table is based on, e.g. "办公 · 分模式调校" / "自定义曲线".
    pub source: String,
    pub ec: Option<EcFanTableSnapshot>,
}

/// Compare the EC fan table with what the current mode should have written.
#[tauri::command]
pub fn get_fan_apply_status() -> FanApplyStatus {
    use crate::system::power::{build_custom_fan_table, get_mode_fan_table};
    let cfg = crate::system::config::load_config();
    let ec = read_ec_fan_table().ok();

    if cfg.fan_boost {
        return FanApplyStatus { state: "fan_boost".into(), source: "一键强冷".into(), ec };
    }

    let policy_name = if cfg.preset_fan_policy == crate::machine::FAN_POLICY_OFFICIAL { "官方原厂" } else { "分模式调校" };
    let (expected, source) = match cfg.power_mode {
        1 => (get_mode_fan_table(1), format!("办公 · {}", policy_name)),
        3 => (get_mode_fan_table(3), format!("狂暴 · {}", policy_name)),
        4 => {
            let p = &cfg.custom_profile;
            if p.fan_curve_enabled || p.fan_isolated_output {
                (build_custom_fan_table(&p.cpu_fan_curve, &p.gpu_fan_curve), "自定义曲线".to_string())
            } else {
                (get_mode_fan_table(3), format!("自定义 · 狂暴{}表", policy_name))
            }
        }
        _ => (get_mode_fan_table(2), format!("均衡 · {}", policy_name)),
    };

    let Some(snap) = ec else {
        return FanApplyStatus { state: "unreadable".into(), source, ec: None };
    };

    let pct = |d: u8| ((d as u16 * 100 + 100) / 200) as u8;
    let cpu_ok = snap.cpu_upt[..] == expected.cpu_upt[..]
        && (1..16).all(|i| snap.cpu_duty_pct[i] == pct(expected.cpu_duty[i]));
    let gpu_ok = snap.gpu_upt[..] == expected.gpu_upt[..]
        && (1..13).all(|i| snap.gpu_duty_pct[i] == pct(expected.gpu_duty[i]));

    FanApplyStatus {
        state: if cpu_ok && gpu_ok { "applied".into() } else { "mismatch".into() },
        source,
        ec: Some(snap),
    }
}
