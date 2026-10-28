// OpenRevo Mock Hardware HAL Implementation
// For open-source public distribution skeleton, local testing, and non-supported machines.

use super::traits::*;

pub struct MockHAL;

impl MockHAL {
    pub fn new() -> Self {
        Self
    }
}

impl MechrevoHardwareHAL for MockHAL {
    fn set_performance_mode(&self, _mode: HardwarePowerMode) -> Result<(), HalError> {
        Ok(())
    }

    fn set_battery_health(&self, _mode: BatteryHealthMode) -> Result<(), HalError> {
        Ok(())
    }

    fn set_fan_boost(&self, _active: bool) -> Result<(), HalError> {
        Ok(())
    }

    fn set_device_switch(&self, _switch: DeviceSwitchType, _enable: bool) -> Result<(), HalError> {
        Ok(())
    }

    fn read_fan_rpm(&self) -> Result<(u16, u16), HalError> {
        Ok((2200, 2000))
    }

    fn is_hardware_supported(&self) -> bool {
        false
    }
}

impl HardwareDriver for MockHAL {
    fn name(&self) -> &'static str {
        "Mock-Generic"
    }

    fn kind(&self) -> DriverKind {
        DriverKind::Mock
    }

    fn probe(&self) -> bool {
        false
    }

    fn get_performance_mode(&self) -> Result<HardwarePowerMode, HalError> {
        Ok(HardwarePowerMode::Balance)
    }

    fn read_cpu_temp(&self) -> Result<u8, HalError> {
        Ok(45)
    }
}
