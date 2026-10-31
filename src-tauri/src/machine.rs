//! Fixed hardware profile for the single laptop this fork targets:
//! MECHREVO 极光 X Pro (Tongfang/Uniwill chassis, board "KUANGSHI-JIGUANG Series-X6xx57TW"),
//! i7-14650HX + RTX 5070 Ti Laptop (65 W floor + 50 W cTGP + 25 W Dynamic Boost).
//!
//! Everything that upstream OpenRevo tried to auto-detect across dozens of models
//! (power envelopes, GPU limits, fan tables) is pinned here instead.

use crate::system::power::FanTableData;

/// DMI baseboard product name of the target machine.
pub const BOARD_NAME: &str = "KUANGSHI-JIGUANG Series-X6xx57TW";
/// EC project id reported by this board.
pub const EC_PROJECT_ID: u8 = 26;

/// i7-14650HX junction temperature limit (°C); the EC temp wall is encoded as an offset from it.
pub const CPU_TJMAX_C: u8 = 100;

/// Battery charge window (stop charging at `.0`, resume below `.1`) per limit.
/// Official stationary mode keeps the pack between 40 and 60 % (OEM string strBatteryHealthyInfo).
pub fn battery_charge_window(limit: u8) -> (u8, u8) {
    if limit >= 95 {
        (100, 95)
    } else if limit <= 65 {
        (60, 40)
    } else {
        (80, 70)
    }
}

/// GPU power envelope (W).
pub const GPU_CTGP_FLOOR_W: u8 = 65;
pub const GPU_BASE_TGP_MAX_W: u8 = 115;
pub const GPU_MAX_DB_W: u8 = 25;

pub struct GpuEnvelope {
    pub floor_w: u8,
    pub base_max_w: u8,
    pub max_db_w: u8,
}
pub const GPU: GpuEnvelope = GpuEnvelope { floor_w: GPU_CTGP_FLOOR_W, base_max_w: GPU_BASE_TGP_MAX_W, max_db_w: GPU_MAX_DB_W };

/// Power state the official control center applies for each preset mode.
/// CPU limits are left to the EC factory table (identical to official: 45/45/100, 75/85/100, 175/175/175),
/// only the GPU side has to be asserted explicitly.
#[derive(Debug, Clone, Copy)]
pub struct PresetGpuState {
    pub super_perf: bool,
    pub db_enabled: bool,
    pub db_watts: u8,
    /// cTGP offset above the 65 W floor (0..=50).
    pub ctgp_offset: u8,
}

pub fn preset_gpu_state(ui_mode: u8) -> PresetGpuState {
    let full_offset = GPU_BASE_TGP_MAX_W - GPU_CTGP_FLOOR_W;
    match ui_mode {
        // 办公: 65 W
        1 => PresetGpuState { super_perf: false, db_enabled: false, db_watts: 0, ctgp_offset: 0 },
        // 狂暴: 115 + 25 = 140 W
        3 => PresetGpuState { super_perf: true, db_enabled: true, db_watts: GPU_MAX_DB_W, ctgp_offset: full_offset },
        // 均衡: 115 W
        _ => PresetGpuState { super_perf: false, db_enabled: false, db_watts: 0, ctgp_offset: full_offset },
    }
}

/// Returns true when the running machine is the one this build was made for.
/// Any mismatch disables all EC writes (see `wmi_ec::wmi_write_ec`).
pub fn is_target_machine() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        let board = read_board_name().unwrap_or_default();
        let ok = board.trim() == BOARD_NAME;
        if ok {
            crate::log_info!("MACHINE", "Target machine confirmed: {}", board.trim());
        } else {
            crate::log_error!(
                "MACHINE",
                "Board '{}' is not '{}'. This build only supports 极光 X Pro; hardware writes are disabled.",
                board.trim(),
                BOARD_NAME
            );
        }
        ok
    })
}

fn read_board_name() -> Option<String> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\BIOS"),
            w!("BaseBoardProduct"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

// ---------------------------------------------------------------------------
// Fan tables
//
// Uniwill "RamFan1p5" semantics (from the official DefaultFanTable_*.json):
//   level i is entered when temp >= UpT[i]; the fan drops from level i+1 back to i
//   when temp < DownT[i]. Level 0 is zero-RPM and its duty must stay 0.
// Duty is given in percent here and stored on the EC 0..200 scale.
// ---------------------------------------------------------------------------

/// Fan policy for the three preset modes.
pub const FAN_POLICY_TUNED: &str = "tuned";
pub const FAN_POLICY_OFFICIAL: &str = "official";

/// (UpT °C, duty %) for levels 1.. ; level 0 is implicit (0 %).
type Curve = &'static [(u8, u8)];

// Official EC default (same table for 办公/均衡/狂暴 on this machine).
const OFFICIAL_CPU: Curve = &[(48, 30), (52, 35), (56, 40), (60, 50), (64, 55), (68, 60), (72, 70), (76, 80), (80, 90), (85, 100)];
const OFFICIAL_GPU: Curve = &[(46, 30), (50, 35), (54, 40), (58, 50), (62, 55), (66, 60), (70, 70), (74, 80), (78, 90), (81, 100)];

// 办公: 45 W CPU / 65 W GPU. Zero-RPM up to 55 °C, stays quiet through typical office load.
const OFFICE_CPU: Curve = &[(55, 25), (60, 30), (65, 35), (70, 42), (75, 50), (80, 62), (85, 80), (90, 100)];
const OFFICE_GPU: Curve = &[(52, 25), (57, 30), (62, 35), (67, 42), (72, 50), (77, 62), (82, 80), (87, 100)];

// 均衡: 75-85 W CPU / 115 W GPU. Quieter than official in the 65-80 °C gaming band, still 100 % at 90 °C.
const BALANCED_CPU: Curve = &[(50, 28), (55, 33), (60, 38), (65, 45), (70, 55), (75, 65), (80, 78), (85, 90), (90, 100)];
const BALANCED_GPU: Curve = &[(47, 28), (52, 33), (57, 38), (62, 45), (67, 55), (72, 65), (77, 78), (82, 90), (86, 100)];

// 狂暴: 175 W CPU / 140 W GPU. Ramps earlier than official and reaches 100 % at 85 °C.
const TURBO_CPU: Curve = &[(45, 30), (50, 38), (55, 45), (60, 55), (65, 65), (70, 75), (75, 85), (80, 95), (85, 100)];
const TURBO_GPU: Curve = &[(43, 30), (48, 38), (53, 45), (58, 55), (63, 65), (68, 75), (72, 85), (76, 95), (80, 100)];

/// Build the 16-level UpT/DownT/Duty arrays from a curve.
pub fn levels_from_curve(curve: &[(u8, u8)]) -> ([u8; 16], [u8; 16], [u8; 16]) {
    let mut upt = [255u8; 16];
    let mut downt = [255u8; 16];
    let mut duty = [0u8; 16];
    upt[0] = 0;
    let n = curve.len().min(15);
    for (i, &(t, d)) in curve.iter().take(n).enumerate() {
        upt[i + 1] = t;
        duty[i + 1] = (d.min(100) as u16 * 2) as u8;
    }
    // Hysteresis: 5 °C before stopping the fan entirely, 3 °C between running levels.
    for i in 0..n {
        let next_up = upt[i + 1];
        let gap = if i == 0 { 5 } else { 3 };
        downt[i] = next_up.saturating_sub(gap).max(if i == 0 { 0 } else { upt[i] });
    }
    // Unused tail keeps the top duty (EC requires a monotonic table).
    let top = duty[n];
    for d in duty.iter_mut().skip(n + 1) {
        *d = top;
    }
    (upt, downt, duty)
}

fn table(cpu: Curve, gpu: Curve) -> FanTableData {
    let (cpu_upt, cpu_downt, cpu_duty) = levels_from_curve(cpu);
    let (gpu_upt, gpu_downt, gpu_duty) = levels_from_curve(gpu);
    let max_duty = cpu_duty[15].max(gpu_duty[15]);
    FanTableData { cpu_upt, cpu_downt, cpu_duty, gpu_upt, gpu_downt, gpu_duty, max_duty }
}

/// Byte-exact copy of the OEM DefaultFanTable_*.json (incl. its DownT values).
fn official_table() -> FanTableData {
    let mut t = table(OFFICIAL_CPU, OFFICIAL_GPU);
    t.cpu_downt = [46, 50, 54, 58, 62, 66, 70, 74, 78, 82, 255, 255, 255, 255, 255, 255];
    t.gpu_downt = [44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 255, 255, 255, 255, 255, 255];
    t
}

/// Fan table for a preset mode (1 办公, 2 均衡, 3 狂暴) under the given policy.
pub fn preset_fan_table(ui_mode: u8, policy: &str) -> FanTableData {
    if policy == FAN_POLICY_OFFICIAL {
        return official_table();
    }
    match ui_mode {
        1 => table(OFFICE_CPU, OFFICE_GPU),
        3 => table(TURBO_CPU, TURBO_GPU),
        _ => table(BALANCED_CPU, BALANCED_GPU),
    }
}

/// Preset curves exposed to the UI as (temp, duty%) points: (cpu, gpu).
pub fn preset_curve_points(ui_mode: u8, policy: &str) -> (Vec<(u8, u8)>, Vec<(u8, u8)>) {
    let (c, g) = if policy == FAN_POLICY_OFFICIAL {
        (OFFICIAL_CPU, OFFICIAL_GPU)
    } else {
        match ui_mode {
            1 => (OFFICE_CPU, OFFICE_GPU),
            3 => (TURBO_CPU, TURBO_GPU),
            _ => (BALANCED_CPU, BALANCED_GPU),
        }
    };
    (c.to_vec(), g.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(t: &FanTableData) {
        for (upt, downt, duty) in [(&t.cpu_upt, &t.cpu_downt, &t.cpu_duty), (&t.gpu_upt, &t.gpu_downt, &t.gpu_duty)] {
            assert_eq!(duty[0], 0, "level 0 must be zero-RPM");
            for i in 1..16 {
                assert!(duty[i] >= duty[i - 1], "duty must be monotonic");
                assert!(duty[i] <= 200);
                if upt[i] != 255 {
                    assert!(upt[i] > upt[i - 1], "UpT must increase");
                    assert!(downt[i - 1] < upt[i], "DownT[i-1] must be below UpT[i]");
                }
            }
        }
    }

    #[test]
    fn official_table_matches_oem_json() {
        let t = preset_fan_table(2, FAN_POLICY_OFFICIAL);
        assert_eq!(&t.cpu_upt[..11], &[0, 48, 52, 56, 60, 64, 68, 72, 76, 80, 85]);
        assert_eq!(t.cpu_duty[1], 60);
        assert_eq!(t.cpu_duty[10], 200);
        assert_eq!(t.cpu_upt[11], 255);
        assert_eq!(&t.cpu_downt[..11], &[46, 50, 54, 58, 62, 66, 70, 74, 78, 82, 255]);
        check(&t);
    }

    #[test]
    fn tuned_tables_are_valid() {
        for m in 1..=3 {
            check(&preset_fan_table(m, FAN_POLICY_TUNED));
        }
    }

    #[test]
    fn gpu_presets() {
        assert_eq!(preset_gpu_state(1).ctgp_offset, 0);
        assert_eq!(preset_gpu_state(2).ctgp_offset, 50);
        assert!(!preset_gpu_state(2).db_enabled);
        assert_eq!(preset_gpu_state(3).db_watts, 25);
    }
}
