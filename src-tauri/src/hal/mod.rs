// OpenRevo Hardware Abstraction Layer (HAL) Module

// pub mod baolongda_guard;
pub mod declarative;
pub mod driver_impl;
pub mod platform_probe;
pub mod traits;
pub mod vault;

pub use driver_impl::MechrevoHALImpl;
pub use traits::*;

use std::sync::OnceLock;

static DRIVER_INSTANCE: OnceLock<MechrevoHALImpl> = OnceLock::new();

/// Global accessor for the active Hardware Driver backend (always the Tongfang ACPI driver).
pub fn get_hardware_driver() -> &'static dyn HardwareDriver {
    DRIVER_INSTANCE.get_or_init(MechrevoHALImpl::new)
}

/// Global accessor for the Hardware Abstraction Layer (backward compatibility)
pub fn get_hardware_hal() -> &'static dyn MechrevoHardwareHAL {
    get_hardware_driver()
}

