// OpenRevo - Commands Module
// Modularized IPC commands mirroring frontend feature architecture.

pub mod brfx_presets;
pub mod display_cmd;
pub mod hardware_cmd;
pub mod lighting_cmd;
pub mod power_cmd;
pub mod switches_cmd;
pub mod system_cmd;
pub mod tuning_cmd;
pub mod window_cmd;
pub mod shortcut_cmd;

pub use display_cmd::*;
pub use lighting_cmd::*;
pub use power_cmd::*;
pub use switches_cmd::*;
pub use system_cmd::*;
pub use tuning_cmd::*;
pub use window_cmd::*;
pub use shortcut_cmd::*;
