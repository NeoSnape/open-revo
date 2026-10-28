// OpenRevo - Window Management Commands
// Handles full/mini window sizing, position persistence, and maximization.

use tauri::{AppHandle, Manager};

static LAST_FULL_POS: parking_lot::Mutex<Option<(i32, i32)>> = parking_lot::Mutex::new(None);
static LAST_MINI_HEIGHT: parking_lot::Mutex<f64> = parking_lot::Mutex::new(622.0);

pub fn get_last_mini_height() -> f64 {
    *LAST_MINI_HEIGHT.lock()
}

pub fn set_last_mini_height(h: f64) {
    *LAST_MINI_HEIGHT.lock() = h;
}

pub fn update_full_window_pos(x: i32, y: i32) {
    *LAST_FULL_POS.lock() = Some((x, y));
}

pub fn save_full_window_pos(x: i32, y: i32) {
    *LAST_FULL_POS.lock() = Some((x, y));
    let mut cfg = crate::system::config::load_config();
    if cfg.full_window_x != Some(x) || cfg.full_window_y != Some(y) {
        cfg.full_window_x = Some(x);
        cfg.full_window_y = Some(y);
        crate::system::config::save_config(&cfg);
        crate::log_info!("WINDOW", "Persisted full dashboard window position: ({}, {})", x, y);
    }
}

#[tauri::command]
pub fn resize_window(app: AppHandle, expanded: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        if expanded {
            let mut target_x: Option<i32> = None;
            let mut target_y: Option<i32> = None;

            // 1. Try to use saved position from memory or persistent config
            let cfg = crate::system::config::load_config();
            let (target_w, target_h, is_resizable) = (960.0_f64, 740.0_f64, false);
            let saved_pos = (*LAST_FULL_POS.lock()).or_else(|| {
                match (cfg.full_window_x, cfg.full_window_y) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                }
            });

            let available_monitors = window.available_monitors().unwrap_or_default();
            if let Some((sx, sy)) = saved_pos {
                let scale = window.scale_factor().unwrap_or(1.0);
                let win_w = (target_w * scale) as i32;
                let win_h = (target_h * scale) as i32;

                let is_visible = available_monitors.iter().any(|m| {
                    let m_pos = m.position();
                    let m_size = m.size();
                    let overlap_x = (sx + win_w).min(m_pos.x + m_size.width as i32) - sx.max(m_pos.x);
                    let overlap_y = (sy + win_h).min(m_pos.y + m_size.height as i32) - sy.max(m_pos.y);
                    overlap_x >= 200 && overlap_y >= 100
                });

                if is_visible {
                    target_x = Some(sx);
                    target_y = Some(sy);
                    crate::log_info!("WINDOW", "Restoring full dashboard to saved position: ({}, {})", sx, sy);
                } else {
                    crate::log_warn!("WINDOW", "Saved position ({}, {}) is off-screen! Centering on active monitor instead.", sx, sy);
                }
            }

            // 2. If no valid saved position, determine center on the monitor where the window currently is
            if target_x.is_none() || target_y.is_none() {
                let target_monitor = window
                    .current_monitor()
                    .ok()
                    .flatten()
                    .or_else(|| window.primary_monitor().ok().flatten());

                if let Some(monitor) = target_monitor {
                    let m_pos = monitor.position();
                    let m_size = monitor.size();
                    let scale = monitor.scale_factor();
                    let win_w = (target_w * scale) as i32;
                    let win_h = (target_h * scale) as i32;
                    let cx = m_pos.x + (m_size.width as i32 - win_w) / 2;
                    let cy = m_pos.y + (m_size.height as i32 - win_h) / 2;
                    target_x = Some(cx);
                    target_y = Some(cy);
                    crate::log_info!("WINDOW", "Centering full dashboard on monitor: screen_pos=({}, {}), screen_size=({}, {}) -> target=({}, {})",
                        m_pos.x, m_pos.y, m_size.width, m_size.height, cx, cy);
                }
            }

            if window.is_maximized().unwrap_or(false) {
                let _ = window.unmaximize();
            }
            // Adaptive geometry: Driven by active skin manifest, defaulting to 960 x 740 locked
            let _ = window.set_resizable(is_resizable);
            let _ = window.set_min_size(None::<tauri::Size>);
            let _ = window.set_max_size(None::<tauri::Size>);
            let _ = window.set_size(tauri::LogicalSize::new(target_w, target_h));
            if let (Some(x), Some(y)) = (target_x, target_y) {
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)));
                *LAST_FULL_POS.lock() = Some((x, y));
            } else {
                let _ = window.center();
            }
        } else {
            if window.is_maximized().unwrap_or(false) {
                let _ = window.unmaximize();
            }
            // Collapsing from full to mini: save current position before resizing
            let scale = window.scale_factor().unwrap_or(1.0);
            let is_currently_expanded = if let Ok(sz) = window.inner_size() {
                (sz.width as f64 / scale) > 600.0
            } else {
                false
            };
            if is_currently_expanded {
                if let Ok(pos) = window.outer_position() {
                    save_full_window_pos(pos.x, pos.y);
                } else if let Some((x, y)) = *LAST_FULL_POS.lock() {
                    save_full_window_pos(x, y);
                }
            }

            // Mini drawer: non-resizable, remove min/max size constraints
            let _ = window.set_resizable(false);
            let _ = window.set_min_size(None::<tauri::Size>);
            let _ = window.set_max_size(None::<tauri::Size>);
            let mini_h = get_last_mini_height();
            let _ = window.set_size(tauri::LogicalSize::new(395.0, mini_h));
            crate::system::window::align_window_to_tray(&window);
        }
        crate::system::tray::update_tray_window_mode(expanded);
        Ok(())
    } else {
        Err("Main window not found".into())
    }
}

/// Dynamically adapts the mini drawer window height to strictly match content height,
/// anchoring the window bottom precisely above the Windows taskbar.
#[tauri::command]
pub fn fit_mini_window_height(app: AppHandle, target_height: f64) -> Result<f64, String> {
    if let Some(window) = app.get_webview_window("main") {
        let is_expanded = crate::system::window::is_window_expanded(&window);
        if is_expanded {
            // Ignore mini height adjustment if currently in full dashboard
            return Ok(740.0);
        }

        let monitor_opt = window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten());

        if let Some(monitor) = monitor_opt {
            let screen_size = monitor.size();
            let screen_pos = monitor.position();
            let scale = monitor.scale_factor();

            let margin = (16.0 * scale) as i32;
            let taskbar_pad = (56.0 * scale) as i32;

            // Maximum logical height available above taskbar on this monitor
            let max_logical_h = ((screen_size.height as i32 - taskbar_pad - margin) as f64 / scale).max(400.0);
            let clamped_h = target_height.min(max_logical_h);

            let final_outer_h = clamped_h.ceil();
            set_last_mini_height(final_outer_h);
            let win_w = (395.0 * scale) as i32;
            let win_h = (final_outer_h * scale) as i32;

            let x = screen_pos.x + screen_size.width as i32 - win_w - margin;
            let y = screen_pos.y + screen_size.height as i32 - win_h - taskbar_pad;

            let _ = window.set_size(tauri::LogicalSize::new(395.0, final_outer_h));
            let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)));

            return Ok(clamped_h);
        }
        Ok(target_height)
    } else {
        Err("Main window not found".into())
    }
}

#[tauri::command]
pub fn toggle_maximize_window(app: AppHandle) -> Result<bool, String> {
    if let Some(window) = app.get_webview_window("main") {
        let is_max = window.is_maximized().unwrap_or(false);
        if is_max {
            let _ = window.unmaximize();
            let _ = window.set_resizable(false);
            let _ = window.set_min_size(None::<tauri::Size>);
            let _ = window.set_max_size(None::<tauri::Size>);
            let _ = window.set_size(tauri::LogicalSize::new(960.0, 740.0));
            if let Some((x, y)) = *LAST_FULL_POS.lock() {
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)));
            } else {
                let _ = window.center();
            }
            Ok(false)
        } else {
            if let Ok(pos) = window.outer_position() {
                save_full_window_pos(pos.x, pos.y);
            }
            let _ = window.maximize();
            Ok(true)
        }
    } else {
        Err("Main window not found".into())
    }
}

#[tauri::command]
pub fn get_wake_window_mode() -> String {
    crate::system::config::load_config().wake_window_mode
}

#[tauri::command]
pub fn set_wake_window_mode(mode: String) -> Result<(), String> {
    let mut cfg = crate::system::config::load_config();
    cfg.wake_window_mode = mode;
    crate::system::config::save_config(&cfg);
    crate::log_info!("WINDOW", "Wake window presentation mode set to: {}", cfg.wake_window_mode);
    Ok(())
}

#[tauri::command]
pub fn get_mini_auto_hide() -> bool {
    crate::system::config::load_config().mini_auto_hide_on_blur
}

#[tauri::command]
pub fn set_mini_auto_hide(enabled: bool) -> Result<(), String> {
    let mut cfg = crate::system::config::load_config();
    cfg.mini_auto_hide_on_blur = enabled;
    crate::system::config::save_config(&cfg);
    crate::log_info!("WINDOW", "Mini drawer auto-hide on blur set to: {}", enabled);
    Ok(())
}

