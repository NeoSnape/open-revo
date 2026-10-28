// OpenRevo - ITE Lighting Facade
// Re-exports modularized components from `crate::driver::lighting`
// providing 100% backward compatibility for all existing callers across OpenRevo.

pub use super::lighting::*;
pub use crate::driver::hid_ffi::{read_hklm_dword, set_hklm_dword, set_hklm_sz};

pub const ITE_PID_KEYBOARD: u16 = ITE_PID_KEYBOARD_PERKEY;
pub const ITE_PID_LIGHTBAR: u16 = ITE_PID_LIGHTBAR_BASE;
