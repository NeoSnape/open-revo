// OpenRevo - Lighting State & Types Definition
// Multi-zone configuration presets and runtime status structs.

use serde::{Deserialize, Serialize};

// Speed UI: 0..=4 -> Chip Speed: 10, 7, 5, 3, 1
pub const SPEED_MAP: [u8; 5] = [10, 7, 5, 3, 1];
// Brightness UI: 0..=4 -> Chip Brightness: 0, 8, 22, 36, 50 (Keyboard)
pub const BRIGHTNESS_MAP: [u8; 5] = [0, 8, 22, 36, 50];
// Lightbar & Logo Brightness UI: 0..=4 -> Chip: 0, 25, 50, 75, 100
pub const LIGHTBAR_BRIGHTNESS_MAP: [u8; 5] = [0, 25, 50, 75, 100];
// Direction UI: 0: LeftRight, 1: RightLeft, 2: DownUp, 3: UpDown -> Chip: 1, 2, 3, 4
pub const DIRECTION_MAP: [u8; 4] = [1, 2, 3, 4];

// Standard 8-Color Rainbow Palette for circular multi-color effects (Slots 1..=8)
pub const RAINBOW_COLORS: [(u8, u8, u8); 8] = [
    (255, 0, 0),     // Red
    (255, 128, 0),   // Orange
    (255, 255, 0),   // Yellow
    (0, 255, 0),     // Green
    (0, 255, 255),   // Cyan
    (0, 0, 255),     // Blue
    (139, 0, 255),   // Purple
    (255, 0, 128),   // Magenta
];

pub fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() >= 6 {
        let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(255);
        (r, g, b)
    } else {
        (255, 255, 255)
    }
}

fn default_kb_engine() -> String {
    "hardware".to_string()
}
fn default_kb_fps() -> u32 {
    30
}
fn default_streamer_effect() -> u8 {
    101
}
fn default_custom_script_id() -> String {
    "kamehameha".to_string()
}
fn default_true() -> bool {
    true
}
fn default_three() -> u8 {
    3
}
fn default_two() -> u8 {
    2
}
fn default_hinge_effect() -> u8 {
    3
}
fn default_cyan() -> String {
    "#00FFFF".to_string()
}
fn default_kb_zones() -> [String; 4] {
    [
        "#6366F1".to_string(),
        "#6366F1".to_string(),
        "#6366F1".to_string(),
        "#6366F1".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightingPresetState {
    // Keyboard
    pub kb_enabled: bool,
    pub kb_effect: u8,
    pub kb_brightness: u8,
    pub kb_speed: u8,
    pub kb_direction: u8,
    pub kb_color: String, // hex "#RRGGBB"

    // Streamer Engine (BetterRGB 60FPS per-key streaming)
    #[serde(default = "default_kb_engine")]
    pub kb_engine: String, // "hardware" | "streamer"
    #[serde(default = "default_kb_fps")]
    pub kb_fps: u32, // 15, 30, 45, 60
    #[serde(default = "default_streamer_effect")]
    pub streamer_effect: u8, // 0..=7
    #[serde(default = "default_custom_script_id")]
    pub custom_script_id: String,

    // Four-Zone Keyboard Colors
    #[serde(default = "default_kb_zones")]
    pub kb_zones: [String; 4],

    // Lightbar (Front / Base)
    pub lb_enabled: bool,
    pub lb_effect: u8,
    pub lb_brightness: u8,
    pub lb_speed: u8,
    pub lb_color: String,

    // Air Vent / Hinge Coast Light (尾部出风口海岸灯)
    #[serde(default = "default_true")]
    pub hinge_enabled: bool,
    #[serde(default = "default_hinge_effect")]
    pub hinge_effect: u8,
    #[serde(default = "default_three")]
    pub hinge_brightness: u8,
    #[serde(default = "default_two")]
    pub hinge_speed: u8,
    #[serde(default = "default_cyan")]
    pub hinge_color: String,

    // Logo
    pub logo_enabled: bool,
    pub logo_effect: u8,
    pub logo_brightness: u8,
    pub logo_color: String,

    // Sleep timer (minutes, 0 = never)
    pub sleep_minutes: u8,

    // Startup & Wake Ceremony (开机与唤醒全域灯效仪式开关)
    #[serde(default = "default_true")]
    pub ceremony_enabled: bool,

    // Hardware Topology Manual Override (硬件拓扑手动纠偏)
    #[serde(default)]
    pub hardware_override: LightingHardwareOverride,
}

fn default_auto() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LightingHardwareOverride {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_auto")]
    pub keyboard: String,   // "auto" | "per_key" | "four_zone" | "single_zone" | "single_white" | "none"
    #[serde(default = "default_auto", alias = "frontBar")]
    pub front_bar: String,  // "auto" | "present" | "none"
    #[serde(default = "default_auto")]
    pub ambient: String,    // "auto" | "vent" | "shoulder" | "none"
    #[serde(default = "default_auto")]
    pub logo: String,       // "auto" | "letter" | "shield" | "none"
}

impl Default for LightingHardwareOverride {
    fn default() -> Self {
        Self {
            enabled: false,
            keyboard: "auto".to_string(),
            front_bar: "auto".to_string(),
            ambient: "auto".to_string(),
            logo: "auto".to_string(),
        }
    }
}

impl Default for LightingPresetState {
    fn default() -> Self {
        Self {
            kb_enabled: true,
            kb_effect: 3, // Wave
            kb_brightness: 3,
            kb_speed: 2,
            kb_direction: 0,
            kb_color: "#6366F1".to_string(),

            kb_engine: "hardware".to_string(),
            kb_fps: 30,
            streamer_effect: 101,
            custom_script_id: "kamehameha".to_string(),

            kb_zones: default_kb_zones(),

            lb_enabled: true,
            lb_effect: 3, // Wave
            lb_brightness: 3,
            lb_speed: 2,
            lb_color: "#00FFFF".to_string(),

            hinge_enabled: true,
            hinge_effect: 3,
            hinge_brightness: 3,
            hinge_speed: 2,
            hinge_color: "#00FFFF".to_string(),

            logo_enabled: true,
            logo_effect: 1, // Static
            logo_brightness: 3,
            logo_color: "#00FFFF".to_string(),

            sleep_minutes: 10,
            ceremony_enabled: true,
            hardware_override: LightingHardwareOverride::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightingRuntimeStatus {
    pub keyboard_engine: String,       // "streamer" | "hardware" | "four_zone" | "single_zone" | "single_color" | "none"
    pub keyboard_online: bool,         // USB/EC is connected
    pub keyboard_active: bool,         // Currently emitting light
    pub keyboard_fps: u32,             // Render FPS for streamer, 0 for static/hw
    pub lightbar_online: bool,         // ITE 7001 Coastline lightbar present
    pub lightbar_active: bool,         // Coastline lightbar active
    pub hinge_online: bool,            // ITE 7000 or Air Vent lightbar present
    pub hinge_active: bool,            // Air Vent lightbar active
    pub logo_online: bool,             // A-Logo present
    pub logo_active: bool,             // A-Logo active
    pub is_sleeping: bool,             // System/Screen in sleep mode
    #[serde(default)]
    pub hardware_override: LightingHardwareOverride,
    #[serde(default)]
    pub detected_baseline: LightingHardwareOverride,
}
