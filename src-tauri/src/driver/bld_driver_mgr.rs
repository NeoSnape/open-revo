// Baolongda (BLD) port-I/O fan driver for Jiaolong 16 / Wujie 14 BLD boards.
// Not present on the 极光 X Pro: the upstream manager (which could load a port-I/O kernel
// driver) is replaced by inert stubs so the remaining call sites stay simple no-ops.

pub fn set_bld_fan_manual_clamp(_cpu_rpm: u16, _gpu_rpm: u16) {}

pub fn set_bld_fan_curve(
    _cpu_curve: Vec<crate::system::config::FanCurvePoint>,
    _gpu_curve: Vec<crate::system::config::FanCurvePoint>,
    _ramp_enabled: bool,
    _ramp_speed_ms: u16,
) {
}

pub fn restore_bld_fan_auto() {}

pub fn is_bld_fan_clamped() -> bool {
    false
}

pub fn shutdown_bld_driver_on_exit() {}
