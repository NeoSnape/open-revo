//! Level 1.1 OEM Dynamic Takeover & Restoration Engine
//!
//! Enforces Level 1.1 Specification from Sovereign Architecture Blueprint:
//! - Multi-tier Truth Sniffer algorithm dynamically locating official frontends
//! - Safe autostart & task isolation sandbox
//! - Full lifecycle pause and resume sequences
//! - Decoupled and hot-pluggable for future laptop models

pub mod lifecycle;
pub mod sandbox;
pub mod sniffer;

pub use lifecycle::{is_oem_installed, is_oem_running, pause_oem_services, resume_oem_services, OEM_PROCESSES, OEM_SERVICES};
pub use sandbox::{disable_scheduled_tasks, enable_scheduled_tasks, neutralize_run_keys, refresh_tray_notification_area, restore_run_keys, OEM_TASKS};
pub use sniffer::{find_official_control_center, ControlCenterLocation, EXECUTABLE_CANDIDATES, SERVICE_ANCHORS, UNINSTALL_KEYWORDS, UWP_CANDIDATES};
