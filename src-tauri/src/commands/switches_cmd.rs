// OpenRevo - Device Switches Commands
// Handles quick hardware switches (Touchpad, Camera, Mic, WiFi, BT, Keep-Awake, Win-Lock, OSD).

use tauri::{AppHandle, Emitter};
use crate::system::hooks;

#[tauri::command]
pub fn set_win_key_locked(locked: bool) -> bool {
    // Same path as the overview switch (hook + EC trigger with the re-send check + config); the
    // mini drawer used to have its own copy here without the re-send.
    let _ = crate::system::device_switches::set_switch("win_key", locked);
    hooks::is_win_key_locked()
}

#[tauri::command]
pub fn get_device_switches() -> crate::system::device_switches::QuickSwitchesState {
    crate::system::device_switches::get_switches_state()
}

#[tauri::command]
pub fn set_device_switch(app: AppHandle, switch_name: String, enable: bool) -> Result<bool, String> {
    let res = crate::system::device_switches::set_switch(&switch_name, enable)?;
    let event_name = format!("{}-state-changed", switch_name.replace('_', "-"));
    let _ = app.emit(&event_name, res);
    if switch_name == "osd" {
        let cfg = crate::system::config::load_config();
        let payload = serde_json::json!({
            "enabled": enable,
            "theme": cfg.osd_theme,
            "scale": cfg.osd_scale,
            "position": cfg.osd_position,
        });
        let _ = app.emit("osd-config-changed", &payload);
        let _ = app.emit_to("osd", "osd-config-changed", &payload);
    }
    Ok(res)
}
