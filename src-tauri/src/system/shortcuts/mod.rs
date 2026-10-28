// OpenRevo - Universal Shortcut Bus (Route 2 Implementation)
// Provides zero-overhead, declarative global shortcuts for plugins and system automation.

pub mod types;
pub mod dispatcher;
pub mod manager;

pub use types::{ParsedKeyCombo, ShortcutAction, ShortcutBinding};
pub use manager::{
    check_match_and_trigger, get_all_shortcuts, load_plugin_shortcuts, register_shortcut,
    unload_plugin_shortcuts, unregister_shortcut,
};

/// Master switch for all global shortcuts, mirrored from `AppConfig::shortcuts_enabled`
/// so the low-level keyboard hook never has to touch the config lock.
pub static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Persist and apply the global shortcut master switch.
pub fn set_enabled(enabled: bool) {
    let mut cfg = crate::system::config::load_config();
    cfg.shortcuts_enabled = enabled;
    crate::system::config::save_config(&cfg);
    ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
    crate::log_info!("SHORTCUTS", "Global shortcuts {}", if enabled { "enabled" } else { "disabled" });
}

/// Initialize default system shortcuts and load plugin shortcuts
pub fn init_shortcuts() {
    ENABLED.store(crate::system::config::load_config().shortcuts_enabled, std::sync::atomic::Ordering::Relaxed);
    crate::log_info!("SHORTCUTS", "Initializing OpenRevo Universal Shortcut Bus...");

    // Register high-value default shortcuts if none exist
    register_shortcut(ShortcutBinding {
        id: "sys_fan_boost".to_string(),
        name: "一键全速强冷切换".to_string(),
        combo: "Ctrl+Shift+F".to_string(),
        action: ShortcutAction::ToggleFanBoost,
        osd: true,
        enabled: true,
        swallow: true,
        plugin_id: None,
    });

    register_shortcut(ShortcutBinding {
        id: "sys_cycle_power_mode".to_string(),
        name: "循环切换性能模式".to_string(),
        combo: "Ctrl+Shift+M".to_string(),
        action: ShortcutAction::CyclePowerMode,
        osd: true,
        enabled: true,
        swallow: true,
        plugin_id: None,
    });

    register_shortcut(ShortcutBinding {
        id: "sys_toggle_refresh_rate".to_string(),
        name: "屏幕刷新率瞬切 (高刷/省电)".to_string(),
        combo: "Ctrl+Shift+R".to_string(),
        action: ShortcutAction::ToggleRefreshRate,
        osd: true,
        enabled: true,
        swallow: true,
        plugin_id: None,
    });
}
