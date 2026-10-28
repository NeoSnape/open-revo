use std::sync::atomic::{AtomicBool, Ordering};

static REBOOT_FREEZE: AtomicBool = AtomicBool::new(false);
static TELEMETRY_ACTIVE: AtomicBool = AtomicBool::new(true);

/// Report system reboot in progress to completely freeze telemetry polling and GPU hardware access
pub fn set_reboot_freeze(freeze: bool) {
    REBOOT_FREEZE.store(freeze, Ordering::SeqCst);
    if freeze {
        crate::log_info!("TELEMETRY", "System reboot initiated: telemetry heartbeat completely frozen.");
    }
}

pub fn is_reboot_frozen() -> bool {
    REBOOT_FREEZE.load(Ordering::Relaxed)
}

pub fn set_telemetry_active(active: bool) {
    TELEMETRY_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn is_telemetry_active() -> bool {
    !REBOOT_FREEZE.load(Ordering::Relaxed) && TELEMETRY_ACTIVE.load(Ordering::Relaxed)
}
