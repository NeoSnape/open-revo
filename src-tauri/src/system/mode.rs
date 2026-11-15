//! The one place that changes the power mode.
//!
//! Two values exist and both are written only here:
//!   * the **selected** mode: what the user chose, persisted as `config.power_mode`;
//!   * the **effective** mode: what the EC is in right now (`EcBridge` cache). It equals the selected
//!     mode unless a temporary override is active (screen blank, sleep-to-Office).
//!
//! Every entry point (panel, `orv`, tray, Q-key, shortcut, power-line watcher, startup restore) calls
//! `select`. Features that borrow the machine for a while call `begin_override` / `end_override` and
//! never touch the config. Each of them used to carry its own copy of "update the EC cache, save the
//! config, tick the tray, emit three events"; the copies had drifted apart.

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::driver::ec_bridge::SharedEcBridge;

/// Who asks for the mode change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Panel button or `orv mode`.
    Panel,
    /// Applying or picking a custom profile (re-applies Custom mode; no OSD pop-up per tweak).
    Profile,
    Tray,
    /// The physical mode key on the chassis.
    QKey,
    Shortcut,
    /// The charger was plugged in or pulled.
    PowerLine,
    /// Restoring the saved mode when the app starts.
    Startup,
    /// Forced step down when a high mode is no longer allowed on battery.
    BatteryGuard,
}

impl Origin {
    /// A choice made by the user: checked against the battery rule and remembered per power source.
    fn is_user(self) -> bool {
        matches!(self, Origin::Panel | Origin::Profile | Origin::Tray | Origin::QKey | Origin::Shortcut)
    }
}

/// A temporary mode that is not the user's choice and is never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    ScreenBlank,
    Sleep,
}

struct Center {
    app: Option<AppHandle>,
    ec: Option<SharedEcBridge>,
    /// Active overrides, oldest first; the last one is in effect.
    overrides: Vec<(Override, u8)>,
}

static CENTER: Mutex<Center> = Mutex::new(Center { app: None, ec: None, overrides: Vec::new() });
/// Serialises whole operations (decide, persist, apply) so two requests cannot interleave.
static OPERATION: Mutex<()> = Mutex::new(());

pub const BATTERY_BLOCK_MESSAGE: &str = "在电池或 Type-C 供电下，狂暴模式与自定义模式不可用。请连接 DC 圆口电源适配器，或在【系统】设置中开启“解除离电限制”。";

pub fn init(app: AppHandle, ec: SharedEcBridge) {
    let mut c = CENTER.lock();
    c.app = Some(app);
    c.ec = Some(ec);
}

fn handles() -> (Option<AppHandle>, Option<SharedEcBridge>) {
    let c = CENTER.lock();
    (c.app.clone(), c.ec.clone())
}

fn valid(mode: u8) -> bool {
    (1..=4).contains(&mode)
}

pub fn mode_label(mode: u8) -> &'static str {
    match mode {
        1 => "Office",
        2 => "Balance",
        3 => "Beast",
        4 => "Custom",
        _ => "Unknown",
    }
}

/// The mode the user chose (persisted).
pub fn selected() -> u8 {
    let m = crate::system::config::load_config().power_mode;
    if valid(m) {
        m
    } else {
        2
    }
}

/// The mode the hardware is in right now.
pub fn effective() -> u8 {
    match handles().1 {
        Some(ec) => ec.lock().get_power_mode(),
        None => selected(),
    }
}

/// Beast and Custom need the barrel-jack charger unless the user lifted the limit.
pub fn blocked_on_battery(mode: u8) -> bool {
    if mode != 3 && mode != 4 {
        return false;
    }
    if crate::commands::power_cmd::is_thin_and_light_or_pure_igpu() {
        return false;
    }
    !crate::commands::power_cmd::is_dc_round_plugged_in()
        && !crate::system::config::load_config().unlock_battery_power_limit
}

/// Tick the tray, wake the OSD and tell both webviews.
fn announce(app: &Option<AppHandle>, mode: u8, with_osd: bool) {
    crate::update_tray_power_mode(mode);
    if let Some(app) = app {
        if with_osd {
            crate::system::osd_window::wake_osd_if_enabled(app);
        }
        let _ = app.emit("physical-mode-switched", mode);
        let _ = app.emit_to("main", "physical-mode-switched", mode);
        let _ = app.emit_to("osd", "physical-mode-switched", mode);
    }
}

/// Make `mode` the selected mode: persist it, apply it to the hardware and announce it.
/// Ends any temporary override (the user's new choice wins over a screen blank's Office mode).
pub fn select(mode: u8, origin: Origin) -> Result<(), String> {
    if !valid(mode) {
        return Err(format!("invalid power mode {}", mode));
    }
    let _op = OPERATION.lock();
    let is_dc = crate::commands::power_cmd::is_dc_round_plugged_in();
    if origin.is_user() && blocked_on_battery(mode) {
        crate::log_warn!("MODE", "{} blocked for {:?}: not on the barrel-jack charger", mode_label(mode), origin);
        // The tray ticks its item before asking; put the tick back.
        crate::update_tray_power_mode(effective());
        return Err(BATTERY_BLOCK_MESSAGE.to_string());
    }

    let (app, ec) = {
        let mut c = CENTER.lock();
        c.overrides.clear();
        (c.app.clone(), c.ec.clone())
    };
    let previous = selected();
    let remember_for_source = origin.is_user();
    crate::system::config::modify_config(|c| {
        c.power_mode = mode;
        if remember_for_source {
            if is_dc {
                c.power_mode_ac = mode;
            } else {
                c.power_mode_battery = mode;
            }
        }
    });
    crate::log_info!("MODE", "Selected {} ({}) <- {} by {:?} (charger={})", mode, mode_label(mode), previous, origin, is_dc);
    if let Some(ec) = ec {
        ec.lock().set_power_mode(mode);
    }
    announce(&app, mode, !matches!(origin, Origin::Startup | Origin::Profile));
    Ok(())
}

/// Put the hardware into `mode` now and wait until it is applied. Does not touch the config.
fn apply_now(mode: u8) {
    if let Some(ec) = handles().1 {
        ec.lock().set_power_mode_cache(mode);
    }
    crate::system::power_mode::apply_full_power_mode(mode);
}

/// Temporarily run in `mode` without changing the user's selection. Blocks until the mode is
/// applied. The override is registered even when the hardware is already in that mode: another
/// override may be what put it there (lid closed while the screen is blanked), and when that one
/// ends this one must still hold the mode.
pub fn begin_override(kind: Override, mode: u8) {
    let _op = OPERATION.lock();
    let current = effective();
    {
        let mut c = CENTER.lock();
        c.overrides.retain(|(k, _)| *k != kind);
        c.overrides.push((kind, mode));
    }
    if current == mode {
        crate::log_info!("MODE", "Override {:?}: already in {} (selected stays {})", kind, mode, selected());
        return;
    }
    crate::log_info!("MODE", "Override {:?}: {} -> {} (selected stays {})", kind, current, mode, selected());
    apply_now(mode);
}

/// End a temporary override and go back to the selected mode (or to an older override still active).
/// Does nothing if the override is no longer there: a `select` in the meantime already replaced it.
pub fn end_override(kind: Override) {
    let _op = OPERATION.lock();
    let (ended, still_active) = {
        let mut c = CENTER.lock();
        let ended = c.overrides.iter().find(|(k, _)| *k == kind).map(|(_, m)| *m);
        c.overrides.retain(|(k, _)| *k != kind);
        (ended, c.overrides.last().map(|(_, m)| *m))
    };
    let Some(override_mode) = ended else { return };
    let current = effective();
    if current != override_mode {
        crate::log_info!("MODE", "Override {:?} ended; mode is {} now (changed meanwhile), keeping it", kind, current);
        return;
    }
    let target = still_active.unwrap_or_else(selected);
    if still_active.is_none() && blocked_on_battery(target) {
        crate::log_info!("MODE", "Override {:?} ended on battery; not returning to {}, staying in {}", kind, mode_label(target), mode_label(current));
        return;
    }
    if target == current {
        return;
    }
    crate::log_info!("MODE", "Override {:?} ended: {} -> {}", kind, current, target);
    apply_now(target);
}
