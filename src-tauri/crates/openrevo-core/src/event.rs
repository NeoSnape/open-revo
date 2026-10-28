use std::sync::{Arc, OnceLock};

/// Universal Event Sink for OpenRevo Core Engine.
/// Decouples hardware event notifications (Q-key switches, fan alerts, OSD triggers)
/// from any specific UI framework (Tauri, Direct2D native OSD, WinUI 3, CLI, etc.).
pub trait CoreEventSink: Send + Sync {
    /// Emit a named event with a JSON payload to the UI or subscriber.
    fn emit(&self, event: &str, payload: serde_json::Value);

    /// Notify UI to display or refresh an on-screen display (OSD) notification.
    fn on_osd_notify(&self, feature: &str, value: &str) {
        let payload = serde_json::json!({
            "feature": feature,
            "value": value
        });
        self.emit("osd-notify", payload);
    }

    /// Notify UI when the physical performance mode is changed (via Q-key or AC plug).
    fn on_power_mode_changed(&self, mode: u8) {
        self.emit("physical-mode-switched", serde_json::json!(mode));
        self.on_osd_notify("power_mode", &mode.to_string());
    }

    /// Notify UI when physical fan boost is toggled.
    fn on_fan_boost_changed(&self, enabled: bool) {
        self.emit("fan-boost-changed", serde_json::json!(enabled));
        self.emit("physical-fan-boost-switched", serde_json::json!(enabled));
    }
}

/// A no-op event sink used for headless CLI tests, background daemons, or unattached mode.
#[derive(Debug, Default, Clone)]
pub struct NoopEventSink;

impl CoreEventSink for NoopEventSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

/// Helper type alias for shared event sink instances.
pub type SharedEventSink = Arc<dyn CoreEventSink>;

static GLOBAL_EVENT_SINK: OnceLock<SharedEventSink> = OnceLock::new();

/// Set the process-wide global event sink (e.g. called on application bootstrap).
pub fn set_global_event_sink(sink: SharedEventSink) {
    let _ = GLOBAL_EVENT_SINK.set(sink);
}

/// Retrieve the global event sink, or fallback to NoopEventSink if uninitialized.
pub fn get_global_event_sink() -> SharedEventSink {
    GLOBAL_EVENT_SINK
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(NoopEventSink))
}

/// Convenient helper to emit an event via the global event sink.
pub fn emit_global(event: &str, payload: serde_json::Value) {
    get_global_event_sink().emit(event, payload);
}

/// Convenient helper to trigger an OSD notification via the global event sink.
pub fn osd_notify_global(feature: &str, value: &str) {
    get_global_event_sink().on_osd_notify(feature, value);
}

/// Convenient helper to trigger a power mode change event via the global event sink.
pub fn power_mode_changed_global(mode: u8) {
    get_global_event_sink().on_power_mode_changed(mode);
}

/// Convenient helper to trigger a fan boost change event via the global event sink.
pub fn fan_boost_changed_global(enabled: bool) {
    get_global_event_sink().on_fan_boost_changed(enabled);
}

