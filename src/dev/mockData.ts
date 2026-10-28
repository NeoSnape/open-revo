/**
 * OpenRevo — Browser development mock data
 *
 * Realistic, machine-specific fixtures for the development-only Tauri IPC mock
 * (see `./mockTauri.ts`). Everything here mirrors the shapes returned by the
 * Rust backend commands (`src-tauri/src/commands/*.rs`, `src-tauri/src/system/*.rs`)
 * with serde's default snake_case field naming.
 *
 * Target machine: MECHREVO 极光 X Pro
 *   - Board: KUANGSHI-JIGUANG Series-X6xx57TW (EC project id 26)
 *   - CPU:   Intel(R) Core(TM) i7-14650HX
 *   - GPU:   NVIDIA GeForce RTX 5070 Ti Laptop GPU (12227 MB VRAM)
 *   - RAM:   32 GB
 *   - Panel: 16" 2560x1600 @ 60 / 300 Hz (currently 300 Hz)
 *
 * This file is only ever loaded through a dynamic `import()` guarded by
 * `import.meta.env.DEV`, so it never reaches the production bundle.
 */

import type {
  CustomProfileConfig,
  CustomProfilePreset,
  FactoryPowerPresets,
  FanCurvePoint,
  GpuTuningCapabilities,
} from '../types/hardware';

// ─── Machine identity ────────────────────────────────────────────────────────

export const MACHINE = {
  cpu_name: 'Intel(R) Core(TM) i7-14650HX',
  gpu_name: 'NVIDIA GeForce RTX 5070 Ti Laptop GPU',
  model_name: 'MECHREVO 极光 X Pro',
  board_name: 'KUANGSHI-JIGUANG Series-X6xx57TW',
  ec_project_id: 26,
  vram_mb: 12227,
  ram_gb: 32,
  max_fan_rpm: 5200,
  display: {
    device_name: '\\\\.\\DISPLAY1',
    friendly_name: '内嵌屏 [2.5K 16:10 300Hz]',
    width: 2560,
    height: 1600,
    current_hz: 300,
    supported_rates: [60, 300],
  },
} as const;

// ─── Hardware support flags ──────────────────────────────────────────────────

export const SUPPORT_FLAGS = {
  has_fan_boost: true,
  has_lightbar: false,
  has_win_lock: true,
  has_fn_lock: false,
  has_battery_limit: true,
  is_rgb_keyboard: true,
  has_water_cooler: false,
};

// ─── Factory power presets (read from EC) ────────────────────────────────────

export const FACTORY_POWER_PRESETS: FactoryPowerPresets = {
  office_pl1: 45,
  office_pl2: 45,
  office_pl4: 100,
  balance_pl1: 75,
  balance_pl2: 85,
  balance_pl4: 100,
  turbo_pl1: 175,
  turbo_pl2: 175,
  turbo_pl4: 175,
  base_tgp: 115,
  max_db: 25,
  hardware_read: true,
};

// ─── GPU tuning capabilities ─────────────────────────────────────────────────

export const GPU_TUNING_CAPS: GpuTuningCapabilities = {
  gpu_generation: 50,
  gpu_name: MACHINE.gpu_name,
  supports_gpu_tuning: true,
  supports_db: true,
  supports_ctgp: true,
  is_full_power: false, // 140W total - 65W cTGP floor = 75W headroom
  ctgp_floor_watts: 65,
  base_tgp_max_watts: 115,
  max_db_watts: 25,
  max_total_tgp: 140,
  is_pending: false,
};

// ─── Fan curves ─────────────────────────────────────────────────────────────

const curve = (pairs: Array<[number, number]>): FanCurvePoint[] =>
  pairs.map(([temp, duty]) => ({ temp, duty }));

/** Linked CPU/GPU curve used by the active "静音狂暴" profile: 40 °C 15 % … 95 °C 90 %. */
export const FAN_CURVE_SILENT: FanCurvePoint[] = curve([
  [40, 15],
  [50, 25],
  [60, 35],
  [68, 45],
  [75, 58],
  [82, 70],
  [88, 80],
  [95, 90],
]);

/** Backend default curve (config.rs `default_fan_curve`). */
export const FAN_CURVE_DEFAULT: FanCurvePoint[] = curve([
  [40, 20],
  [50, 30],
  [60, 45],
  [68, 55],
  [75, 65],
  [82, 75],
  [88, 85],
  [95, 100],
]);

// ─── Custom tuning profiles ─────────────────────────────────────────────────

function profile(patch: Partial<CustomProfileConfig>): CustomProfileConfig {
  return {
    cpu_temp_target_enabled: true,
    cpu_temp_target: 95,
    cpu_pl1_watts: 75,
    cpu_pl2_watts: 85,
    cpu_pl4_watts: 100,
    bios_oc_enabled: false,
    gpu_super_perf: true,
    gpu_ctgp_enabled: false,
    gpu_ctgp_watts: 115,
    gpu_db_enabled: true,
    gpu_db_watts: 25,
    gpu_core_offset_mhz: 0,
    gpu_mem_offset_mhz: 0,
    fan_ramp_rate: true,
    fan_ramp_speed_ms: 300,
    fan_isolated_output: true,
    fan_curve_enabled: false,
    cpu_fan_curve: FAN_CURVE_DEFAULT.map((p) => ({ ...p })),
    gpu_fan_curve: FAN_CURVE_DEFAULT.map((p) => ({ ...p })),
    fan_curve_linked: false,
    ...patch,
  };
}

const silentBeastCurve = FAN_CURVE_SILENT.map((p) => ({ ...p }));

/**
 * The active profile ("静音狂暴" / preset_silent_beast):
 * PL1 125 W / PL2 175 W / PL4 230 W, 95 °C temp wall, +100 MHz core,
 * +255 MHz mem, Dynamic Boost 25 W, linked fan curve, 500 ms ramp.
 */
export const ACTIVE_PROFILE: CustomProfileConfig = profile({
  cpu_pl1_watts: 125,
  cpu_pl2_watts: 175,
  cpu_pl4_watts: 230,
  gpu_ctgp_enabled: false,
  gpu_ctgp_watts: 115,
  gpu_db_enabled: true,
  gpu_db_watts: 25,
  gpu_core_offset_mhz: 100,
  gpu_mem_offset_mhz: 255,
  fan_ramp_rate: true,
  fan_ramp_speed_ms: 500,
  fan_isolated_output: true,
  fan_curve_enabled: true,
  cpu_fan_curve: silentBeastCurve.map((p) => ({ ...p })),
  gpu_fan_curve: silentBeastCurve.map((p) => ({ ...p })),
  fan_curve_linked: true,
});

export const BUILTIN_PRESETS: CustomProfilePreset[] = [
  {
    id: 'preset_silent_beast',
    name: '静音狂暴',
    file_name: 'preset_silent_beast.json',
    icon_key: 'flame',
    is_builtin: true,
    profile: ACTIVE_PROFILE,
  },
  {
    id: 'preset_balance',
    name: '均衡电竞',
    file_name: 'preset_balance.json',
    icon_key: 'zap',
    is_builtin: true,
    profile: profile({
      cpu_pl1_watts: 75,
      cpu_pl2_watts: 85,
      cpu_pl4_watts: 100,
      fan_ramp_speed_ms: 300,
      fan_curve_linked: false,
    }),
  },
  {
    id: 'preset_quiet',
    name: '极致静音',
    file_name: 'preset_quiet.json',
    icon_key: 'leaf',
    is_builtin: true,
    profile: profile({
      cpu_pl1_watts: 45,
      cpu_pl2_watts: 45,
      cpu_pl4_watts: 100,
      gpu_super_perf: false,
      gpu_ctgp_enabled: true,
      gpu_ctgp_watts: 65,
      gpu_db_enabled: false,
      gpu_db_watts: 0,
      fan_ramp_speed_ms: 300,
    }),
  },
  {
    id: 'preset_extreme',
    name: '极限狂暴',
    file_name: 'preset_extreme.json',
    icon_key: 'rocket',
    is_builtin: true,
    profile: profile({
      cpu_pl1_watts: 175,
      cpu_pl2_watts: 175,
      cpu_pl4_watts: 195,
      gpu_ctgp_enabled: false,
      gpu_db_enabled: true,
      gpu_db_watts: 25,
      fan_ramp_speed_ms: 300,
    }),
  },
];

// ─── Lighting ───────────────────────────────────────────────────────────────

export interface MockHardwareOverride {
  enabled: boolean;
  keyboard: string;
  front_bar: string;
  ambient: string;
  logo: string;
}

export interface MockLightingState {
  kb_enabled: boolean;
  kb_effect: number;
  kb_brightness: number;
  kb_speed: number;
  kb_direction: number;
  kb_color: string;
  kb_engine: string;
  kb_fps: number;
  streamer_effect: number;
  custom_script_id: string;
  kb_zones: [string, string, string, string];
  lb_enabled: boolean;
  lb_effect: number;
  lb_brightness: number;
  lb_speed: number;
  lb_color: string;
  hinge_enabled: boolean;
  hinge_effect: number;
  hinge_brightness: number;
  hinge_speed: number;
  hinge_color: string;
  logo_enabled: boolean;
  logo_effect: number;
  logo_brightness: number;
  logo_color: string;
  sleep_minutes: number;
  ceremony_enabled: boolean;
  hardware_override: MockHardwareOverride;
}

export const DEFAULT_HARDWARE_OVERRIDE: MockHardwareOverride = {
  enabled: false,
  keyboard: 'auto',
  front_bar: 'auto',
  ambient: 'auto',
  logo: 'auto',
};

export const DEFAULT_LIGHTING: MockLightingState = {
  kb_enabled: true,
  kb_effect: 1, // single-zone static
  kb_brightness: 4, // 0..4 steps
  kb_speed: 2,
  kb_direction: 0,
  kb_color: '#00FFFF',
  kb_engine: 'hardware',
  kb_fps: 30,
  streamer_effect: 101,
  custom_script_id: 'kamehameha',
  kb_zones: ['#00FFFF', '#0000FF', '#FF00FF', '#FF0000'],
  lb_enabled: false,
  lb_effect: 1,
  lb_brightness: 0,
  lb_speed: 2,
  lb_color: '#00FFFF',
  hinge_enabled: false,
  hinge_effect: 1,
  hinge_brightness: 0,
  hinge_speed: 2,
  hinge_color: '#00FFFF',
  logo_enabled: false,
  logo_effect: 1,
  logo_brightness: 0,
  logo_color: '#00FFFF',
  sleep_minutes: 10,
  ceremony_enabled: true,
  hardware_override: { ...DEFAULT_HARDWARE_OVERRIDE },
};

/** Single-zone RGB keyboard driven directly over the EC. */
export const KEYBOARD_HARDWARE_INFO = {
  hardware_type: 'SingleZoneRgb',
  has_ite8291: false,
  has_ite7001: false,
  has_ite7000: false,
  has_front_lightbar: false,
  is_ec_lightbar: false,
  has_logo: false,
  has_usb_logo: false,
  is_ec_logo: false,
  has_hinge: false,
  is_ec_hinge: false,
  is_four_zone: false,
  description: 'EC 单区 RGB 背光键盘 (Single-Zone RGB over EC)',
  brightness_level: 4,
  is_on: true,
};

// ─── Display colour tuning / ICC calibration ────────────────────────────────

export interface MockDisplayColorPreset {
  id: string;
  name: string;
  author: string | null;
  brightness_pct: number | null;
  color_temp_k: number;
  gain_r: number;
  gain_g: number;
  gain_b: number;
  saturation: number;
  contrast: number;
  gamma: number;
  is_builtin: boolean;
}

export const DISPLAY_COLOR_PRESETS: MockDisplayColorPreset[] = [
  {
    id: 'preset_vibrant',
    name: '多彩鲜艳 (Vibrant)',
    author: '机械革命官方',
    brightness_pct: 50,
    color_temp_k: 6500,
    gain_r: 1,
    gain_g: 1,
    gain_b: 1,
    saturation: 1.5,
    contrast: 1,
    gamma: 1,
    is_builtin: true,
  },
  {
    id: 'preset_standard',
    name: '标准 (Standard)',
    author: '机械革命官方',
    brightness_pct: null,
    color_temp_k: 6500,
    gain_r: 1,
    gain_g: 1,
    gain_b: 1,
    saturation: 1,
    contrast: 1,
    gamma: 1,
    is_builtin: true,
  },
  {
    id: 'preset_eye_care',
    name: '护眼暖色 (Eye Care)',
    author: '机械革命官方',
    brightness_pct: null,
    color_temp_k: 5000,
    gain_r: 1.02,
    gain_g: 0.98,
    gain_b: 0.9,
    saturation: 0.92,
    contrast: 1,
    gamma: 1,
    is_builtin: true,
  },
  {
    id: 'preset_custom',
    name: '用户自定义',
    author: '用户',
    brightness_pct: null,
    color_temp_k: 6500,
    gain_r: 1,
    gain_g: 1,
    gain_b: 1,
    saturation: 1,
    contrast: 1,
    gamma: 1,
    is_builtin: false,
  },
];

export const COLOR_PROFILES = [
  {
    id: 'default_native',
    name: '显示器原生 (Native)',
    file_name: '',
    is_builtin: true,
    description: '不做任何 ICC 映射，直接使用面板出厂色域',
  },
  {
    id: 'std_srgb',
    name: 'sRGB (标准色域)',
    file_name: 'sRGB.icm',
    is_builtin: true,
    description: '100% sRGB 标准色域映射，适合网页与日常设计',
  },
  {
    id: 'std_dcip3',
    name: 'DCI-P3 (广色域)',
    file_name: 'DCIP3.icm',
    is_builtin: true,
    description: 'DCI-P3 电影级广色域映射，适合影视调色',
  },
  {
    id: 'std_adobe_rgb',
    name: 'Adobe RGB',
    file_name: 'AdobeRGB.icm',
    is_builtin: true,
    description: 'Adobe RGB 印刷色域映射，适合摄影后期',
  },
];

// ─── Model capabilities (declarative driver contract) ───────────────────────

export const MODEL_CAPABILITIES = {
  id: 'mechrevo_jiguang_x_pro',
  name: 'MECHREVO 极光 X Pro (i7-14650HX / RTX 5070 Ti)',
  form_factor: 'gaming',
  ui_preset: 'gaming_full',
  discrete_gpu: true,
  water_cooling: false,
  fan_custom_curve: true,
  fan_independent_channels: true,
  fan_ramp_rate: true,
  fan_control_mode: 'discrete_table_16',
  fan_boost_mode: 'hardware_toggle',
  max_fan_rpm: MACHINE.max_fan_rpm,
  cpu_vendor: 'intel',
  cpu_tuning_type: 'intel_pl',
  cpu_supports_fppt: true,
  gpu_power_tuning_mode: 'discrete_manual',
  cpu_name: MACHINE.cpu_name,
  battery_charge_limit: true,
  power_modes: ['office', 'balance', 'turbo', 'custom'],
  power_mode_watts: {
    office: 45,
    balance: 75,
    turbo: 175,
    custom: 175,
  },
  rgb_keyboard: {
    supported: true,
    type: 'single_zone_rgb',
    steps: [0, 1, 2, 3, 4],
    sleep_timeout_supported: true,
    hardware_autonomous: false,
    hotkey_osd_only: false,
  },
  is_adapted: true,
  is_debug_mode: false,
};

// ─── GPU MUX ────────────────────────────────────────────────────────────────

export const GPU_MODE_CAPABILITIES = {
  supports_dgpu: true,
  supports_hybrid: true,
  supports_igpu_only: true,
  reason_igpu_unsupported: null as string | null,
  platform: 'intel',
  is_50_series: true,
  is_30_series: false,
  baseboard: MACHINE.board_name,
  gpu_name: MACHINE.gpu_name,
  is_safety_locked: false,
  user_unlocked: false,
  supports_hotswitch: true,
  acpi_igpu_active: false,
};

// ─── BIOS / UEFI NVRAM ──────────────────────────────────────────────────────

export const BIOS_MENU_STATUS = {
  supported: true,
  enabled: false,
  ap_version: 1,
  is_50_series: true,
  is_amd: false,
  hardware_read: true,
  error: null as string | null,
  cpu_oc_supported: true,
  cpu_oc_enabled: false,
  mem_oc_supported: true,
  mem_oc_enabled: false,
};

// ─── Windows power schemes ──────────────────────────────────────────────────

export const WINDOWS_POWER_SCHEMES = [
  { guid: '961cc777-2547-4f9d-8174-7d86181b8a7a', name: '最佳能效', is_active: false },
  { guid: '381b4222-f694-41f0-9685-ff5bb260df2e', name: '平衡', is_active: true },
  { guid: '8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c', name: '高性能', is_active: false },
  { guid: 'e9a42b02-d5df-448d-aa00-03f14749eb61', name: '卓越性能', is_active: false },
];

// ─── Misc paths / defaults ──────────────────────────────────────────────────

export const USER_DATA_DIR = 'C:\\Users\\NullKasa\\AppData\\Roaming\\OpenRevo';
export const LOG_FILE_PATH = `${USER_DATA_DIR}\\logs\\openrevo.log`;

export const DEFAULT_MODULE_TOGGLES: Record<string, boolean> = {
  overview: true,
  gpu: true,
  tuning: true,
  lighting: true,
  water: false,
  system: true,
  plugins: true,
  sponsor: true,
};

export const OSD_CONFIG = {
  enabled: true,
  theme: 'cinema',
  scale: '100%',
  position: 'bottom-center',
};

export const COOLER_CURVE_POINTS = [
  { temp: 45, fan_duty: 0, pump_voltage: 0 },
  { temp: 55, fan_duty: 35, pump_voltage: 2 },
  { temp: 65, fan_duty: 50, pump_voltage: 2 },
  { temp: 80, fan_duty: 60, pump_voltage: 2 },
  { temp: 90, fan_duty: 75, pump_voltage: 2 },
  { temp: 95, fan_duty: 100, pump_voltage: 3 },
];

/** Water cooler hardware is absent on this machine, so the telemetry is a
 *  well-formed "disconnected" snapshot (the UI renders the module as unplugged). */
export const DISCONNECTED_COOLER_TELEMETRY = {
  connected: false,
  is_flow_ok: false,
  meter_state: 0,
  pump_voltage: 0,
  pump_tier: 0,
  fan_duty: 0,
  pump_rpm: 0,
  running: false,
  safety_tripped: false,
  is_priming: false,
  led_on: false,
  led_r: 0,
  led_g: 128,
  led_b: 0,
  led_mode: 2,
  mac_address: '',
  strategy: 'smart',
  curve_points: COOLER_CURVE_POINTS,
  pump_start_temp: 55,
  pump_turbo_temp: 90,
};

/** `check_for_updates_cmd` result — dev mock always reports "up to date". */
export const UPDATE_CHECK_RESULT = {
  has_update: false,
  current_version: '0.9.3',
  current_commit: 'dev',
  latest_version: '0.9.3',
  latest_commit: 'dev',
  display_version: 'v0.9.3 (dev mock)',
  notes: '',
  chosen_source: 'mock',
  asset_type: 'portable',
  filename: '',
  download_url: '',
  size: 0,
  sha256: '',
};
