/**
 * OpenRevo — Development-only Tauri v2 IPC mock
 *
 * The real UI runs inside a Tauri webview. This module fakes just enough of the
 * Tauri v2 IPC surface (`window.__TAURI_INTERNALS__`) so that `pnpm dev` renders
 * the exact same React tree in a normal browser — no Rust process required.
 *
 * Installed from `src/main.tsx` only when `import.meta.env.DEV` is true and no
 * real Tauri runtime is present, so it never ships in production bundles.
 *
 * What is emulated:
 *   - `invoke()` + every backend command used by the frontend (see `handlers`)
 *   - `transformCallback` / `unregisterCallback` (used by `listen`)
 *   - `plugin:event|*` (listen / unlisten / emit / emit_to) with a real registry,
 *     plus a 1 Hz `hardware-state-updated` telemetry stream with jittered values
 *   - `plugin:window|*`, `plugin:webview|*`, `plugin:app|*` (window size queries
 *     report the real browser window, scale factor 1)
 *
 * Unknown commands resolve to `null` with a `console.warn('[mock] unhandled', …)`.
 */

import {
  ACTIVE_PROFILE,
  BIOS_MENU_STATUS,
  BUILTIN_PRESETS,
  COLOR_PROFILES,
  COOLER_CURVE_POINTS,
  DEFAULT_HARDWARE_OVERRIDE,
  DEFAULT_LIGHTING,
  DEFAULT_MODULE_TOGGLES,
  DISCONNECTED_COOLER_TELEMETRY,
  DISPLAY_COLOR_PRESETS,
  FACTORY_POWER_PRESETS,
  GPU_MODE_CAPABILITIES,
  GPU_TUNING_CAPS,
  KEYBOARD_HARDWARE_INFO,
  LOG_FILE_PATH,
  MACHINE,
  MODEL_CAPABILITIES,
  OSD_CONFIG,
  SUPPORT_FLAGS,
  UPDATE_CHECK_RESULT,
  USER_DATA_DIR,
  WINDOWS_POWER_SCHEMES,
  type MockDisplayColorPreset,
  type MockHardwareOverride,
  type MockLightingState,
} from './mockData';
import type {
  CustomProfilePreset,
  FactoryPowerPresets,
  GpuTuningCapabilities,
  HardwareState,
} from '../types/hardware';

/* eslint-disable @typescript-eslint/no-explicit-any */

type Args = Record<string, any>;
type Handler = (args: Args) => unknown;

// ─── Small helpers ──────────────────────────────────────────────────────────

const clone = <T,>(value: T): T =>
  typeof structuredClone === 'function'
    ? structuredClone(value)
    : (JSON.parse(JSON.stringify(value)) as T);

const argNum = (args: Args, fallback: number, ...keys: string[]): number => {
  for (const key of keys) {
    const raw = args[key];
    if (typeof raw === 'number' && Number.isFinite(raw)) return raw;
    if (typeof raw === 'string' && raw.trim() !== '' && Number.isFinite(Number(raw))) {
      return Number(raw);
    }
  }
  return fallback;
};

const argBool = (args: Args, fallback: boolean, ...keys: string[]): boolean => {
  for (const key of keys) {
    const raw = args[key];
    if (typeof raw === 'boolean') return raw;
  }
  return fallback;
};

const argStr = (args: Args, fallback: string, ...keys: string[]): string => {
  for (const key of keys) {
    const raw = args[key];
    if (typeof raw === 'string') return raw;
  }
  return fallback;
};

/**
 * Smooth random walk so the 1 Hz telemetry stream drifts instead of jumping.
 * When the baseline changes (e.g. fan boost toggled), it slews toward the new
 * baseline within a couple of ticks instead of taking ~a minute to converge.
 */
const driftState: Record<string, number> = {};
const drift = (
  key: string,
  base: number,
  spread: number,
  min: number,
  max: number,
): number => {
  const prev = driftState[key] ?? base;
  const gap = prev - base;
  let delta = (Math.random() * 2 - 1) * spread;
  if (Math.abs(gap) > spread * 2) delta -= gap * 0.55;
  const next = Math.min(max, Math.max(min, prev + delta));
  driftState[key] = next;
  return next;
};
const round1 = (v: number) => Math.round(v * 10) / 10;

// ─── Mutable mock state ─────────────────────────────────────────────────────

const params = new URLSearchParams(window.location.search);
const startInFullMode = params.get('full') === '1';

const state = {
  powerMode: 4 as 1 | 2 | 3 | 4,
  fanBoost: false,
  batteryLimit: 100,
  refreshRate: MACHINE.display.current_hz as number,
  brightness: 80,
  winKeyLocked: false,
  autostart: false,
  oemRunning: false,
  dcOnline: true,
  maximized: false,
  expanded: startInFullMode,
  keyboardSleepTimeout: 10,
  miniHeight: 0,
  displayTuningEnabled: false,
  activeColorPresetId: 'preset_vibrant',
  activeCalibrationId: 'default_native',
  calibrationEnabled: false,
  moduleToggles: clone(DEFAULT_MODULE_TOGGLES) as Record<string, boolean>,
  lighting: clone(DEFAULT_LIGHTING) as MockLightingState,
  activeProfileId: 'preset_silent_beast',
  profiles: clone(BUILTIN_PRESETS) as CustomProfilePreset[],
  biosEnabled: BIOS_MENU_STATUS.enabled,
  fanSpeedCapRpm: 0,
  fanRampMs: 500,
  customColorPresets: [] as MockDisplayColorPreset[],
  extraColorProfiles: [] as Array<{
    id: string;
    name: string;
    file_name: string;
    is_builtin: boolean;
    description: string;
  }>,
};

/** Cross-command persistent configuration (stands in for config.json). */
const cfg: Record<string, any> = {
  power_mode: state.powerMode,
  power_mode_ac: 3,
  power_mode_battery: 2,
  fan_boost: false,
  battery_limit: 100,
  win_key_locked: false,
  autostart: false,
  log_enabled: false,
  log_level: 1,
  log_filter: null,
  water_cooler_enabled: false,
  keep_water_cooler: true,
  cooler_led_on: true,
  cooler_led_r: 0,
  cooler_led_g: 128,
  cooler_led_b: 0,
  cooler_led_mode: 2,
  cooler_pump_voltage: 7,
  cooler_fan_duty: 0,
  cooler_strategy: 'smart',
  cooler_curve_points: COOLER_CURVE_POINTS,
  cooler_mac: '',
  cooler_pump_start_temp: 55,
  cooler_pump_turbo_temp: 90,
  refresh_rate: state.refreshRate,
  miniled_enabled: false,
  wifi_enabled: true,
  bluetooth_enabled: true,
  touchpad_enabled: true,
  camera_enabled: true,
  mic_enabled: true,
  keep_awake_enabled: false,
  auto_min_refresh_on_battery: false,
  unlock_battery_power_limit: false,
  auto_fallback_kb_on_battery: true,
  osd_enabled: OSD_CONFIG.enabled,
  osd_theme: OSD_CONFIG.theme,
  osd_scale: OSD_CONFIG.scale,
  osd_position: OSD_CONFIG.position,
  fn_lock_enabled: false,
  wifi_adapter_name: '',
  takeover_oem: true,
  keyboard_sleep_timeout: state.keyboardSleepTimeout,
  lighting: state.lighting,
  gpu_mode: 'dgpu',
  cpu_safety_guard: false,
  cpu_freq_limit: 0,
  cpu_freq_crash_guard: false,
  allow_experimental_igpu_only: false,
  battery_mode: 'long_life',
  battery_hardware_limit_enabled: false,
  usb_charge_enabled: false,
  ac_recovery_enabled: false,
  high_perf_scheme: true,
  preferred_power_scheme: '381b4222-f694-41f0-9685-ff5bb260df2e',
  saved_power_mode_on_battery: null,
  sleep_guard_enabled: false,
  sleep_disconnect_net: false,
  sleep_block_away: false,
  sleep_ignore_sysreq: false,
  sleep_disable_wake_timers: false,
  sleep_deep_sleep: false,
  sleep_hibernate_30m: false,
  sleep_auto_office: false,
  full_window_x: null,
  full_window_y: null,
  gpu_offset_step: 5,
  gpu_core_offset_max: 250,
  gpu_mem_offset_max: 1000,
  cpu_temp_target_min: 85,
  cpu_temp_target_max: 95,
  cpu_pl4_margin: 20,
  cpu_pl1_max: 235,
  cpu_pl2_max: 240,
  cpu_pl4_max: 280,
  cpu_pl1_offset: 0,
  cpu_pl2_offset: 0,
  cpu_pl4_offset: 0,
  cpu_temp_offset: 5,
  cpu_tjmax: 100,
  launch_visible: true,
  wake_window_mode: startInFullMode ? 'full' : 'remember',
  mini_auto_hide_on_blur: true,
  active_profile_id: state.activeProfileId,
  module_toggles: state.moduleToggles,
  custom_plugin_toggles: {},
  widget_positions: {},
  active_skin: null,
  active_background: null,
};

// ─── Derived snapshots ──────────────────────────────────────────────────────

function currentMonitor() {
  const width = Math.max(window.innerWidth, 640);
  const height = Math.max(window.innerHeight, 480);
  return {
    name: '\\\\.\\DISPLAY1',
    scaleFactor: 1,
    position: { x: 0, y: 0 },
    size: { width, height },
    workArea: {
      position: { x: 0, y: 0 },
      size: { width, height: height - 48 },
    },
  };
}

function displays() {
  return [
    {
      device_name: MACHINE.display.device_name,
      friendly_name: MACHINE.display.friendly_name,
      is_primary: true,
      is_internal: true,
      width: MACHINE.display.width,
      height: MACHINE.display.height,
      current_hz: state.refreshRate,
      supported_rates: [...MACHINE.display.supported_rates],
    },
  ];
}

function hardwareState(): HardwareState {
  const idleFan = state.fanBoost ? MACHINE.max_fan_rpm : 2100;
  const fanSpread = state.fanBoost ? 60 : 45;
  const cpuFan = Math.round(drift('cpuFan', idleFan, fanSpread, 1900, MACHINE.max_fan_rpm));
  const gpuFan = Math.round(drift('gpuFan', idleFan, fanSpread, 1880, MACHINE.max_fan_rpm));

  // Idle GPU clocks: mostly parked at 210 MHz with occasional short boosts.
  const gpuCoreFreq =
    Math.random() < 0.15
      ? Math.round(drift('gpuCoreBoost', 1747, 25, 1600, 1747))
      : Math.round(drift('gpuCoreIdle', 210, 6, 210, 260));

  return {
    power_mode: state.powerMode,
    fan_boost: state.fanBoost,
    battery_limit: state.batteryLimit,
    cpu_temp: Math.round(drift('cpuTemp', 48, 1.4, 43, 57)),
    cpu_usage: Math.round(drift('cpuUsage', 3, 2, 1, 14)),
    cpu_freq: Math.round(drift('cpuFreq', 2400, 260, 1200, 4800)),
    gpu_temp: Math.round(drift('gpuTemp', 45, 1.1, 41, 52)),
    cpu_fan_rpm: cpuFan,
    gpu_fan_rpm: gpuFan,
    gpu_usage: Math.round(drift('gpuUsage', 5, 3, 0, 16)),
    gpu_core_freq: gpuCoreFreq,
    gpu_mem_freq: Math.round(drift('gpuMemFreq', 405, 10, 405, 810)),
    gpu_vram_mb: MACHINE.vram_mb,
    gpu_vram_used_mb: Math.round(drift('vramUsed', 1450, 90, 900, 3200)),
    refresh_rate: state.refreshRate,
    available_refresh_rates: [...MACHINE.display.supported_rates],
    monitors: displays(),
    auto_min_refresh_on_battery: !!cfg.auto_min_refresh_on_battery,
    unlock_battery_power_limit: !!cfg.unlock_battery_power_limit,
    win_key_locked: state.winKeyLocked,
    is_mock: true,
    oem_running: state.oemRunning,
    oem_installed: true,
    autostart: state.autostart,
    is_intel_13_14_gen: true,
    water_cooler_enabled: false,
    support_flags: { ...SUPPORT_FLAGS },
    cpu_name: MACHINE.cpu_name,
    gpu_name: MACHINE.gpu_name,
    model_name: MACHINE.model_name,
    board_name: MACHINE.board_name,
    power_presets: clone(FACTORY_POWER_PRESETS) as FactoryPowerPresets,
    is_dc_online: state.dcOnline,
    cpu_power_w: round1(drift('cpuPower', 18, 2.4, 10, 32)),
    gpu_power_w: round1(drift('gpuPower', 22, 2.2, 12, 36)),
    gpu_tuning_caps: clone(GPU_TUNING_CAPS) as GpuTuningCapabilities,
  };
}

function findPreset(id: string): CustomProfilePreset | undefined {
  return state.profiles.find((p) => p.id === id) ?? state.profiles[0];
}

function activeProfile(): CustomProfilePreset {
  return findPreset(state.activeProfileId) ?? {
    id: 'preset_silent_beast',
    name: '静音狂暴',
    file_name: 'preset_silent_beast.json',
    icon_key: 'flame',
    is_builtin: true,
    profile: clone(ACTIVE_PROFILE),
  };
}

function powerSettings() {
  return {
    battery_percent: 98,
    is_ac_online: state.dcOnline,
    is_charging: false,
    battery_mode: cfg.battery_mode,
    battery_limit: state.batteryLimit,
    battery_hardware_limit_enabled: !!cfg.battery_hardware_limit_enabled,
    usb_charge_enabled: !!cfg.usb_charge_enabled,
    ac_recovery_enabled: !!cfg.ac_recovery_enabled,
    high_perf_scheme: !!cfg.high_perf_scheme,
    sleep_guard_enabled: !!cfg.sleep_guard_enabled,
    sleep_disconnect_net: !!cfg.sleep_disconnect_net,
    sleep_block_away: !!cfg.sleep_block_away,
    sleep_ignore_sysreq: !!cfg.sleep_ignore_sysreq,
    sleep_disable_wake_timers: !!cfg.sleep_disable_wake_timers,
    sleep_deep_sleep: !!cfg.sleep_deep_sleep,
    sleep_hibernate_30m: !!cfg.sleep_hibernate_30m,
    sleep_auto_office: !!cfg.sleep_auto_office,
  };
}

function gpuModeInfo() {
  return {
    current_mode: cfg.gpu_mode,
    has_igpu_present: true,
    has_dgpu: true,
    gpu_name: MACHINE.gpu_name,
    cpu_guard_active: !!cfg.cpu_safety_guard,
    platform: 'intel',
    cpu_name: MACHINE.cpu_name,
    hardware_read: true,
    error: null as string | null,
    capabilities: clone(GPU_MODE_CAPABILITIES),
  };
}

function lightingRuntimeStatus() {
  return {
    keyboard_engine: 'single_zone',
    keyboard_online: true,
    keyboard_active: state.lighting.kb_enabled && state.lighting.kb_brightness > 0,
    keyboard_fps: 0,
    lightbar_online: false,
    lightbar_active: false,
    hinge_online: false,
    hinge_active: false,
    logo_online: false,
    logo_active: false,
    is_sleeping: false,
    hardware_override: clone(state.lighting.hardware_override) as MockHardwareOverride,
    detected_baseline: clone(DEFAULT_HARDWARE_OVERRIDE) as MockHardwareOverride,
  };
}

function allColorPresets(): MockDisplayColorPreset[] {
  return [...DISPLAY_COLOR_PRESETS, ...state.customColorPresets];
}

function displayTuningState() {
  const presets = allColorPresets();
  const active =
    presets.find((p) => p.id === state.activeColorPresetId) ?? presets[0];
  return {
    enabled: state.displayTuningEnabled,
    active_preset_id: active.id,
    active_preset: clone(active),
    presets: clone(presets),
    factory_presets: clone(DISPLAY_COLOR_PRESETS),
  };
}

function colorCalibrationState() {
  const profiles = [...COLOR_PROFILES, ...state.extraColorProfiles];
  const active =
    profiles.find((p) => p.id === state.activeCalibrationId) ?? profiles[0];
  return {
    enabled: state.calibrationEnabled,
    active_profile_id: active.id,
    active_profile_name: active.name,
    profiles: clone(profiles),
    monitor_name: MACHINE.display.friendly_name,
    hdr_active: false,
    current_device_name: MACHINE.display.device_name,
    monitors: displays(),
  };
}

function logStatus() {
  return {
    visible: false,
    enabled: !!cfg.log_enabled,
    level: typeof cfg.log_level === 'number' ? cfg.log_level : 1,
    path: LOG_FILE_PATH,
    filter: cfg.log_filter ?? null,
  };
}

function osdConfig() {
  return {
    enabled: cfg.osd_enabled,
    theme: cfg.osd_theme,
    scale: cfg.osd_scale,
    position: cfg.osd_position,
  };
}

function userDataDirInfo() {
  return { path: USER_DATA_DIR, mode: 'standard' };
}

function coolerTelemetry() {
  return clone({
    ...DISCONNECTED_COOLER_TELEMETRY,
    curve_points: COOLER_CURVE_POINTS,
  });
}

function biosStatus() {
  return { ...BIOS_MENU_STATUS, enabled: state.biosEnabled };
}

function powerSchemes() {
  const preferred = cfg.preferred_power_scheme;
  return WINDOWS_POWER_SCHEMES.map((s) => ({
    ...s,
    is_active: s.guid === preferred,
  }));
}

function moduleToggles() {
  return { ...state.moduleToggles };
}

// ─── Event bus ──────────────────────────────────────────────────────────────

type MockCallback = { fn: (event: unknown) => void; once: boolean };

const callbacks = new Map<number, MockCallback>();
/** event name → eventId → handler callback id */
const eventListeners = new Map<string, Map<number, number>>();
let nextCallbackId = 1;
let nextEventId = 1;

function dispatch(event: string, payload: unknown) {
  const entries = eventListeners.get(event);
  if (!entries) return;
  for (const [eventId, handlerId] of Array.from(entries.entries())) {
    const cb = callbacks.get(handlerId);
    if (!cb) {
      entries.delete(eventId);
      continue;
    }
    try {
      cb.fn({ event, id: eventId, payload });
    } catch (err) {
      console.error(`[mock] event handler for "${event}" threw:`, err);
    }
    if (cb.once) {
      callbacks.delete(handlerId);
      entries.delete(eventId);
    }
  }
}

function removeListener(event: string, eventId: number) {
  const entries = eventListeners.get(event);
  if (!entries) return;
  const handlerId = entries.get(eventId);
  if (handlerId !== undefined) callbacks.delete(handlerId);
  entries.delete(eventId);
}

// ─── Command handlers ───────────────────────────────────────────────────────


// Mirrors src-tauri/src/machine.rs curves (temp °C, duty %).
let fanPolicy: 'tuned' | 'official' = 'tuned';
let shortcutsEnabled = false;
const C = (arr: [number, number][]) => arr.map(([temp, duty]) => ({ temp, duty }));
const FAN_CURVES: Record<string, { cpu: any[]; gpu: any[] }> = {
  official: { cpu: C([[48,30],[52,35],[56,40],[60,50],[64,55],[68,60],[72,70],[76,80],[80,90],[85,100]]), gpu: C([[46,30],[50,35],[54,40],[58,50],[62,55],[66,60],[70,70],[74,80],[78,90],[81,100]]) },
  1: { cpu: C([[55,25],[60,30],[65,35],[70,42],[75,50],[80,62],[85,80],[90,100]]), gpu: C([[52,25],[57,30],[62,35],[67,42],[72,50],[77,62],[82,80],[87,100]]) },
  2: { cpu: C([[50,28],[55,33],[60,38],[65,45],[70,55],[75,65],[80,78],[85,90],[90,100]]), gpu: C([[47,28],[52,33],[57,38],[62,45],[67,55],[72,65],[77,78],[82,90],[86,100]]) },
  3: { cpu: C([[45,30],[50,38],[55,45],[60,55],[65,65],[70,75],[75,85],[80,95],[85,100]]), gpu: C([[43,30],[48,38],[53,45],[58,55],[63,65],[68,75],[72,85],[76,95],[80,100]]) },
};
const presetFanInfo = () => ({
  policy: fanPolicy,
  modes: [1, 2, 3].map((mode) => ({ mode, ...(fanPolicy === 'official' ? FAN_CURVES.official : FAN_CURVES[mode]) })),
  official: { mode: 2, ...FAN_CURVES.official },
});

const handlers: Record<string, Handler> = {
  // --- thetcx fork: preset fan schedule + shortcut master switch ---
  get_preset_fan_info: () => presetFanInfo(),
  set_preset_fan_policy: (a) => {
    fanPolicy = a?.policy === 'official' ? 'official' : 'tuned';
    return presetFanInfo();
  },
  read_ec_fan_table: () => {
    const m = presetFanInfo().modes.find((x: any) => x.mode === state.powerMode) || presetFanInfo().modes[1];
    const row = (pts: { temp: number; duty: number }[], key: 'temp' | 'duty') =>
      [0, ...pts.map((p) => p[key])].concat(Array(16).fill(key === 'temp' ? 255 : pts[pts.length - 1].duty)).slice(0, 16);
    return {
      cpu_upt: row(m.cpu, 'temp'), cpu_downt: row(m.cpu, 'temp').map((t: number) => (t === 255 ? 255 : Math.max(0, t - 3))),
      cpu_duty_pct: row(m.cpu, 'duty'),
      gpu_upt: row(m.gpu, 'temp'), gpu_downt: row(m.gpu, 'temp').map((t: number) => (t === 255 ? 255 : Math.max(0, t - 3))),
      gpu_duty_pct: row(m.gpu, 'duty'),
    };
  },
  get_shortcuts_enabled: () => shortcutsEnabled,
  set_shortcuts_enabled: (a) => {
    shortcutsEnabled = !!a?.enabled;
    return shortcutsEnabled;
  },
  // -- Hardware status / core setters --------------------------------------
  get_hardware_status: () => hardwareState(),
  get_model_capabilities: () => clone(MODEL_CAPABILITIES),
  get_gpu_tuning_capabilities: () => clone(GPU_TUNING_CAPS),

  set_power_mode: (a) => {
    const mode = Math.min(4, Math.max(1, Math.round(argNum(a, state.powerMode, 'mode'))));
    state.powerMode = mode as 1 | 2 | 3 | 4;
    cfg.power_mode = mode;
    return null;
  },
  set_fan_boost: (a) => {
    state.fanBoost = argBool(a, !state.fanBoost, 'enable', 'enabled');
    cfg.fan_boost = state.fanBoost;
    return null;
  },
  set_battery_limit: (a) => {
    state.batteryLimit = Math.round(argNum(a, state.batteryLimit, 'limit'));
    cfg.battery_limit = state.batteryLimit;
    return null;
  },
  switch_refresh_rate: (a) => {
    state.refreshRate = Math.round(argNum(a, state.refreshRate, 'hz'));
    cfg.refresh_rate = state.refreshRate;
    return null;
  },
  set_display_monitor_refresh_rate: (a) => {
    state.refreshRate = Math.round(argNum(a, state.refreshRate, 'hz'));
    cfg.refresh_rate = state.refreshRate;
    return null;
  },
  get_display_monitors: () => displays(),
  set_auto_min_refresh_on_battery: (a) => {
    cfg.auto_min_refresh_on_battery = argBool(a, false, 'enable', 'enabled');
    return cfg.auto_min_refresh_on_battery;
  },
  set_unlock_battery_power_limit: (a) => {
    cfg.unlock_battery_power_limit = argBool(a, false, 'enable', 'enabled');
    return cfg.unlock_battery_power_limit;
  },
  get_display_brightness: () => state.brightness,
  set_display_brightness: (a) => {
    state.brightness = Math.min(100, Math.max(0, Math.round(argNum(a, state.brightness, 'brightness'))));
    dispatch('display-brightness-changed', state.brightness);
    return null;
  },
  get_miniled_support: () => false,
  get_miniled_status: () => false,
  set_miniled_status: () => false,
  turn_off_screen: () => null,
  set_win_key_locked: (a) => {
    state.winKeyLocked = argBool(a, !state.winKeyLocked, 'locked');
    cfg.win_key_locked = state.winKeyLocked;
    dispatch('win-lock-state-changed', state.winKeyLocked);
    return state.winKeyLocked;
  },
  set_autostart_enabled: (a) => {
    state.autostart = argBool(a, false, 'enabled', 'enable');
    cfg.autostart = state.autostart;
    return state.autostart;
  },
  get_device_switches: () => ({
    wifi: !!cfg.wifi_enabled,
    bluetooth: !!cfg.bluetooth_enabled,
    touchpad: !!cfg.touchpad_enabled,
    camera: !!cfg.camera_enabled,
    mic: !!cfg.mic_enabled,
    osd: !!cfg.osd_enabled,
    win_key: state.winKeyLocked,
    fn_lock: !!cfg.fn_lock_enabled,
    keep_awake: !!cfg.keep_awake_enabled,
  }),
  set_device_switch: (a) => {
    const name = argStr(a, '', 'switchName', 'switch_name');
    const enable = argBool(a, false, 'enable', 'enabled');
    if (name === 'camera' || name === 'mic' || name === 'keep_awake' ||
        name === 'wifi' || name === 'bluetooth' || name === 'touchpad') {
      cfg[`${name}_enabled`] = enable;
    }
    dispatch(`${name.replace(/_/g, '-')}-state-changed`, enable);
    return true;
  },

  // -- Window / layout -----------------------------------------------------
  resize_window: (a) => {
    state.expanded = argBool(a, state.expanded, 'expanded');
    return null;
  },
  fit_mini_window_height: (a) => {
    state.miniHeight = argNum(a, window.innerHeight, 'targetHeight', 'target_height');
    return state.miniHeight;
  },
  toggle_maximize_window: () => {
    state.maximized = !state.maximized;
    return state.maximized;
  },
  get_wake_window_mode: () => cfg.wake_window_mode,
  set_wake_window_mode: (a) => {
    cfg.wake_window_mode = argStr(a, 'remember', 'mode');
    return null;
  },
  get_mini_auto_hide: () => !!cfg.mini_auto_hide_on_blur,
  set_mini_auto_hide: (a) => {
    cfg.mini_auto_hide_on_blur = argBool(a, true, 'enabled', 'enable');
    return null;
  },
  toggle_oem_service: (a) => {
    const takeOver = argBool(a, true, 'takeOver', 'take_over');
    cfg.takeover_oem = takeOver;
    state.oemRunning = false;
    return hardwareState();
  },

  // -- App config ----------------------------------------------------------
  get_app_config: () => clone(cfg),
  save_app_config: (a) => {
    const next = a.cfg ?? a.config;
    if (next && typeof next === 'object') Object.assign(cfg, next);
    return null;
  },

  // -- Lighting ------------------------------------------------------------
  get_lighting_status: () => clone(state.lighting),
  get_lighting_state: () => clone(state.lighting),
  get_lighting_runtime_status: () => lightingRuntimeStatus(),
  get_lighting_hardware_override: () => clone(state.lighting.hardware_override),
  apply_lighting_hardware_override: (a) => {
    const override = (a.overrideCfg ?? a.override_cfg) as MockHardwareOverride | undefined;
    if (override && typeof override === 'object') {
      state.lighting.hardware_override = { ...DEFAULT_HARDWARE_OVERRIDE, ...override };
    }
    dispatch('lighting://hardware_info_changed', null);
    return lightingRuntimeStatus();
  },
  get_keyboard_hardware_info: () => ({
    ...KEYBOARD_HARDWARE_INFO,
    brightness_level: state.lighting.kb_brightness,
    is_on: state.lighting.kb_enabled && state.lighting.kb_brightness > 0,
  }),
  get_keyboard_sleep_timeout: () => state.keyboardSleepTimeout,
  set_keyboard_sleep_timeout: (a) => {
    state.keyboardSleepTimeout = Math.round(argNum(a, state.keyboardSleepTimeout, 'seconds'));
    cfg.keyboard_sleep_timeout = state.keyboardSleepTimeout;
    dispatch('keyboard-sleep-timeout-changed', state.keyboardSleepTimeout);
    return null;
  },
  set_keyboard_engine: (a) => {
    state.lighting.kb_engine = argStr(a, state.lighting.kb_engine, 'engine');
    dispatch('kb-engine-changed', state.lighting.kb_engine);
    return null;
  },
  set_streamer_fps: (a) => {
    state.lighting.kb_fps = Math.round(argNum(a, state.lighting.kb_fps, 'fps'));
    return null;
  },
  apply_keyboard_lighting: (a) => {
    const l = state.lighting;
    l.kb_enabled = argBool(a, l.kb_enabled, 'enabled');
    l.kb_effect = Math.round(argNum(a, l.kb_effect, 'effect'));
    l.kb_brightness = Math.round(argNum(a, l.kb_brightness, 'brightness'));
    l.kb_speed = Math.round(argNum(a, l.kb_speed, 'speed'));
    l.kb_direction = Math.round(argNum(a, l.kb_direction, 'direction'));
    l.kb_color = argStr(a, l.kb_color, 'color');
    l.kb_engine = argStr(a, l.kb_engine, 'engine');
    l.kb_fps = Math.round(argNum(a, l.kb_fps, 'fps'));
    l.streamer_effect = Math.round(argNum(a, l.streamer_effect, 'streamer_effect', 'streamerEffect'));
    l.custom_script_id = argStr(a, l.custom_script_id, 'customScriptId', 'custom_script_id');
    dispatch('keyboard-brightness-changed', l.kb_enabled ? l.kb_brightness : 0);
    return null;
  },
  apply_lightbar_lighting: (a) => {
    const l = state.lighting;
    l.lb_enabled = argBool(a, l.lb_enabled, 'enabled');
    l.lb_effect = Math.round(argNum(a, l.lb_effect, 'effect'));
    l.lb_brightness = Math.round(argNum(a, l.lb_brightness, 'brightness'));
    l.lb_speed = Math.round(argNum(a, l.lb_speed, 'speed'));
    l.lb_color = argStr(a, l.lb_color, 'color');
    return null;
  },
  apply_hinge_lighting: (a) => {
    const l = state.lighting;
    l.hinge_enabled = argBool(a, l.hinge_enabled, 'enabled');
    l.hinge_effect = Math.round(argNum(a, l.hinge_effect, 'effect'));
    l.hinge_brightness = Math.round(argNum(a, l.hinge_brightness, 'brightness'));
    l.hinge_speed = Math.round(argNum(a, l.hinge_speed, 'speed'));
    l.hinge_color = argStr(a, l.hinge_color, 'color');
    return null;
  },
  apply_four_zone_colors: (a) => {
    const zones = a.zones;
    if (Array.isArray(zones) && zones.length === 4) {
      state.lighting.kb_zones = [String(zones[0]), String(zones[1]), String(zones[2]), String(zones[3])];
    }
    return null;
  },
  apply_logo_lighting: (a) => {
    const l = state.lighting;
    l.logo_enabled = argBool(a, l.logo_enabled, 'enabled');
    l.logo_effect = Math.round(argNum(a, l.logo_effect, 'effect'));
    l.logo_brightness = Math.round(argNum(a, l.logo_brightness, 'brightness'));
    l.logo_color = argStr(a, l.logo_color, 'color');
    return null;
  },
  set_lighting_sleep_timer: (a) => {
    state.lighting.sleep_minutes = Math.round(argNum(a, state.lighting.sleep_minutes, 'minutes'));
    return null;
  },
  trigger_lighting_welcome: () => null,
  set_ceremony_enabled: (a) => {
    state.lighting.ceremony_enabled = argBool(a, state.lighting.ceremony_enabled, 'enabled');
    return null;
  },
  save_current_lighting_as_startup: () => null,
  get_auto_fallback_kb_on_battery: () => !!cfg.auto_fallback_kb_on_battery,
  set_auto_fallback_kb_on_battery: (a) => {
    cfg.auto_fallback_kb_on_battery = argBool(a, true, 'enable', 'enabled');
    return cfg.auto_fallback_kb_on_battery;
  },

  // -- Custom tuning profiles ---------------------------------------------
  get_custom_profile: () => clone(activeProfile().profile),
  get_profiles_list: () => clone(state.profiles),
  get_default_builtin_presets: () => clone(BUILTIN_PRESETS),
  get_active_profile_id: () => state.activeProfileId,
  set_active_profile_id: (a) => {
    const id = argStr(a, state.activeProfileId, 'id');
    state.activeProfileId = id;
    cfg.active_profile_id = id;
    return clone(activeProfile());
  },
  save_profile_preset: (a) => {
    const preset = a.preset as CustomProfilePreset | undefined;
    if (preset && preset.id) {
      const idx = state.profiles.findIndex((p) => p.id === preset.id);
      if (idx >= 0) state.profiles[idx] = clone(preset);
      else state.profiles.push(clone(preset));
    }
    return null;
  },
  create_profile_preset: (a) => {
    const name = argStr(a, '新建方案', 'name');
    const baseId = argStr(a, state.activeProfileId, 'baseId', 'base_id');
    const initial = (a.initialProfile ?? a.initial_profile) as CustomProfilePreset['profile'] | undefined;
    const base = findPreset(baseId);
    const id = `preset_user_${Date.now().toString(36)}`;
    const preset: CustomProfilePreset = {
      id,
      name,
      file_name: `${id}.json`,
      icon_key: 'sparkles',
      is_builtin: false,
      profile: clone(initial ?? base?.profile ?? ACTIVE_PROFILE),
    };
    state.profiles.push(preset);
    return clone(preset);
  },
  reset_profile_preset: (a) => {
    const id = argStr(a, state.activeProfileId, 'id');
    const builtin = BUILTIN_PRESETS.find((p) => p.id === id);
    const idx = state.profiles.findIndex((p) => p.id === id);
    if (idx >= 0) {
      const existing = state.profiles[idx];
      const restored: CustomProfilePreset = {
        ...existing,
        profile: clone(builtin ? builtin.profile : ACTIVE_PROFILE),
      };
      state.profiles[idx] = restored;
      return clone(restored);
    }
    return clone(activeProfile());
  },
  rename_profile_preset: (a) => {
    const id = argStr(a, state.activeProfileId, 'id');
    const newName = argStr(a, '', 'newName', 'new_name');
    const idx = state.profiles.findIndex((p) => p.id === id);
    if (idx >= 0 && newName) state.profiles[idx] = { ...state.profiles[idx], name: newName };
    return clone(findPreset(id) ?? activeProfile());
  },
  delete_profile_preset: (a) => {
    const id = argStr(a, '', 'id');
    state.profiles = state.profiles.filter((p) => p.id !== id || p.is_builtin);
    return null;
  },
  import_profile_preset: (a) => {
    const raw = argStr(a, '{}', 'jsonContent', 'json_content');
    const fileName = argStr(a, 'imported.json', 'fileName', 'file_name');
    let parsed: Partial<CustomProfilePreset> = {};
    try {
      parsed = JSON.parse(raw) as Partial<CustomProfilePreset>;
    } catch {
      parsed = {};
    }
    const id = parsed.id || `preset_user_${Date.now().toString(36)}`;
    const preset: CustomProfilePreset = {
      id,
      name: parsed.name || fileName.replace(/\.json$/i, ''),
      file_name: fileName,
      icon_key: parsed.icon_key || 'download',
      is_builtin: false,
      profile: clone((parsed.profile as CustomProfilePreset['profile']) ?? ACTIVE_PROFILE),
    };
    state.profiles.push(preset);
    return clone(preset);
  },
  export_profile_preset: (a) => {
    const id = argStr(a, state.activeProfileId, 'id');
    return `${USER_DATA_DIR}\\profiles\\${id}.json`;
  },
  open_profiles_folder: () => null,
  apply_custom_profile: (a) => {
    const profile = a.profile;
    const idx = state.profiles.findIndex((p) => p.id === state.activeProfileId);
    if (profile && idx >= 0) {
      state.profiles[idx] = { ...state.profiles[idx], profile: clone(profile) };
    }
    return null;
  },
  apply_fan_curve_live: (a) => {
    const idx = state.profiles.findIndex((p) => p.id === state.activeProfileId);
    if (idx >= 0) {
      const p = { ...state.profiles[idx], profile: { ...state.profiles[idx].profile } };
      if (Array.isArray(a.cpuCurve)) p.profile.cpu_fan_curve = clone(a.cpuCurve);
      if (Array.isArray(a.gpuCurve)) p.profile.gpu_fan_curve = clone(a.gpuCurve);
      state.profiles[idx] = p;
    }
    return null;
  },
  toggle_fan_isolated_output: () => true,
  toggle_fan_ramp_rate: (a) => {
    state.fanRampMs = Math.round(argNum(a, state.fanRampMs, 'speedMs', 'speed_ms'));
    const enable = argBool(a, true, 'enable', 'enabled');
    const idx = state.profiles.findIndex((p) => p.id === state.activeProfileId);
    if (idx >= 0) {
      state.profiles[idx] = {
        ...state.profiles[idx],
        profile: {
          ...state.profiles[idx].profile,
          fan_ramp_rate: enable,
          fan_ramp_speed_ms: state.fanRampMs,
        },
      };
    }
    return enable;
  },
  toggle_fan_curve_control: (a) => {
    const enable = argBool(a, true, 'enable', 'enabled');
    const idx = state.profiles.findIndex((p) => p.id === state.activeProfileId);
    if (idx >= 0) {
      state.profiles[idx] = {
        ...state.profiles[idx],
        profile: { ...state.profiles[idx].profile, fan_curve_enabled: enable },
      };
    }
    return null;
  },
  apply_live_custom_tweak: (a) => {
    const idx = state.profiles.findIndex((p) => p.id === state.activeProfileId);
    if (idx >= 0) {
      const p = state.profiles[idx];
      const next = { ...p.profile };
      if (typeof a.pl1 === 'number') next.cpu_pl1_watts = a.pl1;
      if (typeof a.pl2 === 'number') next.cpu_pl2_watts = a.pl2;
      if (typeof a.pl4 === 'number') next.cpu_pl4_watts = a.pl4;
      if (typeof a.temp_target === 'number') next.cpu_temp_target = a.temp_target;
      if (typeof a.tempTarget === 'number') next.cpu_temp_target = a.tempTarget;
      if (typeof a.ctgp_watts === 'number') next.gpu_ctgp_watts = a.ctgp_watts;
      if (typeof a.ctgpWatts === 'number') next.gpu_ctgp_watts = a.ctgpWatts;
      if (typeof a.db_enabled === 'boolean') next.gpu_db_enabled = a.db_enabled;
      if (typeof a.dbEnabled === 'boolean') next.gpu_db_enabled = a.dbEnabled;
      if (typeof a.db_watts === 'number') next.gpu_db_watts = a.db_watts;
      if (typeof a.dbWatts === 'number') next.gpu_db_watts = a.dbWatts;
      if (typeof a.super_perf === 'boolean') next.gpu_super_perf = a.super_perf;
      if (typeof a.superPerf === 'boolean') next.gpu_super_perf = a.superPerf;
      if (typeof a.gpu_core_offset === 'number') next.gpu_core_offset_mhz = a.gpu_core_offset;
      if (typeof a.gpuCoreOffset === 'number') next.gpu_core_offset_mhz = a.gpuCoreOffset;
      if (typeof a.gpu_mem_offset === 'number') next.gpu_mem_offset_mhz = a.gpu_mem_offset;
      if (typeof a.gpuMemOffset === 'number') next.gpu_mem_offset_mhz = a.gpuMemOffset;
      state.profiles[idx] = { ...p, profile: next };
    }
    const cap = argNum(a, NaN, 'fanSpeedCap', 'fan_speed_cap');
    if (Number.isFinite(cap)) state.fanSpeedCapRpm = cap;
    return null;
  },
  heal_gpu_power_contract: () => true,
  heal_cpu_power_contract: () => true,
  set_bld_fan_speed_cap: (a) => {
    state.fanSpeedCapRpm = Math.round(argNum(a, 0, 'rpm'));
    state.fanBoost = state.fanSpeedCapRpm > 0;
    return null;
  },

  // -- BIOS / UEFI NVRAM ---------------------------------------------------
  get_bios_advanced_menu_status: () => biosStatus(),
  toggle_bios_advanced_menu: (a) => {
    state.biosEnabled = argBool(a, !state.biosEnabled, 'enable', 'enabled');
    return biosStatus();
  },

  // -- GPU MUX -------------------------------------------------------------
  get_gpu_mode_info: () => gpuModeInfo(),
  set_gpu_mode: (a) => {
    cfg.gpu_mode = argStr(a, cfg.gpu_mode, 'mode');
    return gpuModeInfo();
  },
  hot_switch_gpu_mode: (a) => {
    cfg.gpu_mode = argStr(a, cfg.gpu_mode, 'mode');
    return gpuModeInfo();
  },
  set_experimental_igpu_unlock: (a) => {
    cfg.allow_experimental_igpu_only = argBool(a, false, 'unlock');
    return gpuModeInfo();
  },

  // -- CPU frequency -------------------------------------------------------
  get_cpu_freq_info: () => ({
    enabled: (cfg.cpu_freq_limit ?? 0) > 0,
    current_limit_mhz: cfg.cpu_freq_limit ?? 0,
    min_mhz: 800,
    max_mhz: 5200,
    recommended_mhz: 4800,
    is_intel_13_14: true,
    cpu_brand: MACHINE.cpu_name,
    cpu_tjmax: 100,
  }),
  set_cpu_freq_limit: (a) => {
    const mhz = Math.round(argNum(a, 0, 'mhz'));
    cfg.cpu_freq_limit = argBool(a, true, 'enable', 'enabled') ? mhz : 0;
    return mhz;
  },
  reset_cpu_freq_limit: () => {
    cfg.cpu_freq_limit = 0;
    return true;
  },
  set_cpu_safety_guard: (a) => {
    cfg.cpu_safety_guard = argBool(a, false, 'enable', 'enabled');
    return cfg.cpu_safety_guard;
  },

  // -- Power & battery -----------------------------------------------------
  get_power_settings: () => powerSettings(),
  set_battery_mode: (a) => {
    const mode = argStr(a, cfg.battery_mode, 'mode');
    const limit = mode === 'long_life' ? 100 : mode === 'station' ? 60 : 80;
    cfg.battery_mode = mode;
    cfg.battery_limit = limit;
    state.batteryLimit = limit;
    return powerSettings();
  },
  set_battery_hardware_limit: (a) => {
    cfg.battery_hardware_limit_enabled = argBool(a, false, 'enable', 'enabled');
    return powerSettings();
  },
  set_power_option: (a) => {
    const option = argStr(a, '', 'option');
    const enable = argBool(a, false, 'enable', 'enabled');
    const keyMap: Record<string, string> = {
      usb_charge: 'usb_charge_enabled',
      ac_recovery: 'ac_recovery_enabled',
      high_perf: 'high_perf_scheme',
    };
    const key = keyMap[option];
    if (key) cfg[key] = enable;
    return powerSettings();
  },
  set_sleep_guard_option: (a) => {
    const option = argStr(a, '', 'option');
    const enable = argBool(a, false, 'enable', 'enabled');
    const keyMap: Record<string, string> = {
      disconnect_net: 'sleep_disconnect_net',
      block_away: 'sleep_block_away',
      ignore_sysreq: 'sleep_ignore_sysreq',
      disable_wake_timers: 'sleep_disable_wake_timers',
      deep_sleep: 'sleep_deep_sleep',
      hibernate_30m: 'sleep_hibernate_30m',
    };
    const key = keyMap[option];
    if (key) cfg[key] = enable;
    return powerSettings();
  },
  set_master_sleep_guard: (a) => {
    const enable = argBool(a, false, 'enable', 'enabled');
    cfg.sleep_disconnect_net = enable;
    cfg.sleep_block_away = enable;
    cfg.sleep_ignore_sysreq = enable;
    cfg.sleep_disable_wake_timers = enable;
    cfg.sleep_deep_sleep = enable;
    cfg.sleep_hibernate_30m = enable;
    cfg.sleep_guard_enabled = enable;
    return powerSettings();
  },
  set_sleep_auto_office: (a) => {
    cfg.sleep_auto_office = argBool(a, false, 'enable', 'enabled');
    return powerSettings();
  },
  get_windows_power_schemes: () => powerSchemes(),
  set_active_windows_power_scheme: (a) => {
    cfg.preferred_power_scheme = argStr(a, '', 'guid');
    return null;
  },
  restore_official_power_plan: () => '已成功恢复官方出厂电源计划与 PPM 调度策略',

  // -- Display colour tuning ----------------------------------------------
  get_display_tuning_state: () => displayTuningState(),
  set_display_tuning_enabled: (a) => {
    state.displayTuningEnabled = argBool(a, false, 'enable', 'enabled');
    return null;
  },
  apply_display_color_preset: (a) => {
    const preset = a.preset as MockDisplayColorPreset | undefined;
    if (preset && preset.id) {
      state.activeColorPresetId = preset.id;
      const idx = state.customColorPresets.findIndex((p) => p.id === preset.id);
      if (idx >= 0) state.customColorPresets[idx] = clone(preset);
    }
    return displayTuningState();
  },
  import_color_preset_content: (a) => {
    const content = argStr(a, '{}', 'content');
    const filename = argStr(a, '未命名配色.json', 'filename', 'fileName');
    let parsed: Partial<MockDisplayColorPreset> = {};
    try {
      parsed = JSON.parse(content) as Partial<MockDisplayColorPreset>;
    } catch {
      parsed = {};
    }
    const preset: MockDisplayColorPreset = {
      id: parsed.id || `preset_imported_${Date.now().toString(36)}`,
      name: parsed.name || filename.replace(/\.json$/i, ''),
      author: parsed.author ?? '用户',
      brightness_pct: parsed.brightness_pct ?? null,
      color_temp_k: parsed.color_temp_k ?? 6500,
      gain_r: parsed.gain_r ?? 1,
      gain_g: parsed.gain_g ?? 1,
      gain_b: parsed.gain_b ?? 1,
      saturation: parsed.saturation ?? 1,
      contrast: parsed.contrast ?? 1,
      gamma: parsed.gamma ?? 1,
      is_builtin: false,
    };
    state.customColorPresets = [...state.customColorPresets.filter((p) => p.id !== preset.id), preset];
    state.activeColorPresetId = preset.id;
    return clone(preset);
  },
  reset_display_tuning: () => {
    state.displayTuningEnabled = false;
    state.activeColorPresetId = 'preset_vibrant';
    state.customColorPresets = [];
    return displayTuningState();
  },
  reset_color_preset_to_default: (a) => {
    const id = argStr(a, state.activeColorPresetId, 'presetId', 'preset_id');
    const factory = DISPLAY_COLOR_PRESETS.find((p) => p.id === id);
    state.customColorPresets = state.customColorPresets.filter((p) => p.id !== id);
    if (factory) state.activeColorPresetId = factory.id;
    return displayTuningState();
  },

  // -- ICC colour calibration ---------------------------------------------
  get_color_calibration_state: () => colorCalibrationState(),
  apply_color_profile: (a) => {
    state.activeCalibrationId = argStr(a, state.activeCalibrationId, 'profileId', 'profile_id');
    state.calibrationEnabled = state.activeCalibrationId !== 'default_native';
    return colorCalibrationState();
  },
  set_color_calibration_enabled: (a) => {
    state.calibrationEnabled = argBool(a, false, 'enable', 'enabled');
    if (!state.calibrationEnabled) state.activeCalibrationId = 'default_native';
    return colorCalibrationState();
  },
  import_custom_color_profile: (a) => {
    const fileName = argStr(a, 'custom.icc', 'fileName', 'file_name');
    const id = `custom_${fileName.replace(/\.[^.]+$/, '').replace(/[^A-Za-z0-9_]/g, '_')}`;
    if (!state.extraColorProfiles.some((p) => p.id === id)) {
      state.extraColorProfiles = [
        ...state.extraColorProfiles,
        {
          id,
          name: fileName.replace(/\.[^.]+$/, ''),
          file_name: fileName,
          is_builtin: false,
          description: '用户导入的自定义 ICC 校色配置',
        },
      ];
    }
    state.activeCalibrationId = id;
    state.calibrationEnabled = true;
    return colorCalibrationState();
  },
  delete_custom_color_profile: (a) => {
    const id = argStr(a, '', 'profileId', 'profile_id');
    state.extraColorProfiles = state.extraColorProfiles.filter((p) => p.id !== id);
    if (state.activeCalibrationId === id) state.activeCalibrationId = 'default_native';
    return colorCalibrationState();
  },

  // -- OSD -----------------------------------------------------------------
  get_osd_config: () => osdConfig(),
  set_osd_config: (a) => {
    applyOsdArgs(a);
    return null;
  },
  save_osd_config: (a) => {
    applyOsdArgs(a);
    return null;
  },
  trigger_osd_preview: () => null,
  show_osd_window: () => null,
  hide_osd_window: () => null,

  // -- Logging & diagnostics ----------------------------------------------
  get_log_status: () => logStatus(),
  set_log_level: (a) => {
    const level = Math.round(argNum(a, 1, 'level'));
    cfg.log_level = level;
    cfg.log_enabled = level > 0;
    return logStatus();
  },
  set_log_filter: (a) => {
    const filter = a.filter;
    cfg.log_filter = typeof filter === 'string' && filter.trim() !== '' ? filter.trim() : null;
    return logStatus();
  },
  open_log_file: () => null,
  export_device_diagnostic_report: () => `${USER_DATA_DIR}\\diagnostic_report.txt`,
  reboot_computer: () => null,

  // -- User data directory -------------------------------------------------
  get_user_data_dir_info: () => userDataDirInfo(),
  migrate_user_data_dir: (a) => argStr(a, USER_DATA_DIR, 'targetPath', 'target_path'),
  open_user_data_dir: () => null,
  clear_user_data_dir: () => '用户数据目录已清空',

  // -- Module toggles ------------------------------------------------------
  get_module_toggles: () => moduleToggles(),
  set_module_enabled: (a) => {
    const id = argStr(a, '', 'id');
    const enabled = argBool(a, true, 'enabled');
    if (id) state.moduleToggles[id] = enabled;
    return enabled;
  },

  // -- Shortcuts -----------------------------------------------------------
  get_registered_shortcuts: () => [],
  register_custom_shortcut: () => true,
  unregister_custom_shortcut: () => true,
  trigger_shortcut_action: () => null,

  // -- Boot / telemetry ----------------------------------------------------
  frontend_ready: () => Date.now(),
  get_boot_log: () => '[mock] dev browser boot trace\n',
  report_window_visibility: () => null,
  get_telemetry_heartbeat_status: () => ({
    active: true,
    interval_ms: 1000,
    gpu_query_enabled: true,
    main_window_visible: true,
    consumers: [],
  }),

  // -- Skins / backgrounds (feature removed upstream) ----------------------
  get_active_skin: () => cfg.active_skin ?? null,
  set_active_skin: (a) => {
    cfg.active_skin = a.skinId ?? null;
    return null;
  },
  get_active_background: () => cfg.active_background ?? null,
  set_active_background: (a) => {
    cfg.active_background = a.backgroundId ?? null;
    return null;
  },
  load_plugin_theme_css: () => '',
  load_plugin_background_css: () => '',
  load_plugin_html: () => '',

  // -- Custom plugins (feature removed upstream) ---------------------------
  get_custom_plugins: () => [],
  set_custom_plugin_enabled: () => true,
  open_plugins_dir: () => null,
  open_custom_widget: () => null,
  close_custom_widget: () => null,
  widget_ready: () => null,
  get_plugin_service_status: (a) => ({
    plugin_id: argStr(a, '', 'pluginId', 'plugin_id'),
    status: 'stopped' as const,
  }),
  start_plugin_service: (a) => ({
    plugin_id: argStr(a, '', 'pluginId', 'plugin_id'),
    status: 'stopped' as const,
    message: '插件服务已在浏览器 mock 中禁用',
  }),
  stop_plugin_service: (a) => ({
    plugin_id: argStr(a, '', 'pluginId', 'plugin_id'),
    status: 'stopped' as const,
  }),
  get_raw_ec_capabilities: () => null,
  raw_ec_read: () => 0,
  raw_ec_write: () => true,
  raw_mailbox_invoke: () => null,
  get_registered_watchers: () => [],
  register_custom_watcher: () => true,
  unregister_custom_watcher: () => true,

  // -- Custom BRFX engine (feature removed upstream) -----------------------
  get_custom_brfx_list: () => [],
  save_custom_brfx: (a) => `${USER_DATA_DIR}\\brfx\\${argStr(a, 'custom.brfx', 'filename')}`,
  open_brfx_in_notepad: () => null,
  apply_custom_brfx_script: () => null,
  clear_custom_brfx_script: () => null,
  set_custom_fx_text: () => null,
  get_custom_fx_text: () => '',

  // -- Water cooler (hardware absent on this machine) ----------------------
  get_water_cooler_status: () => coolerTelemetry(),
  set_water_cooler_enabled: (a) => {
    cfg.water_cooler_enabled = argBool(a, false, 'enabled', 'enable');
    return null;
  },
  start_water_cooler: () => coolerTelemetry(),
  stop_water_cooler: () => coolerTelemetry(),
  pair_water_cooler: () => coolerTelemetry(),
  reset_water_cooler: () => coolerTelemetry(),
  select_water_cooler: () => coolerTelemetry(),
  set_water_cooler_speed: () => coolerTelemetry(),
  set_water_cooler_strategy: () => coolerTelemetry(),
  set_water_cooler_led: () => coolerTelemetry(),
  set_water_cooler_pump_thresholds: () => coolerTelemetry(),
  fill_water_cooler: () => null,
  scan_water_cooler_devices: () => [],

  // -- Updater -------------------------------------------------------------
  check_for_updates_cmd: () => ({ ...UPDATE_CHECK_RESULT }),
  download_and_install_update_cmd: () => null,

  // -- Model manifests (read-only diagnostics) -----------------------------
  get_models_file_path: () => `${USER_DATA_DIR}\\models.json`,
  reload_model_profiles: () => clone(FACTORY_POWER_PRESETS),
  open_models_in_notepad: () => null,
  export_models_json: () => null,
  import_models_json: () => '成功导入 0 项机型配置！(mock)',
  get_model_profile_info: () => ({
    matched_profile: {
      model_id: MACHINE.board_name,
      display_name: MACHINE.model_name,
      alias: '极光 X Pro',
    },
    is_trusted: true,
    confidence: 1,
    confidence_reason: 'browser dev mock',
  }),
};

function applyOsdArgs(a: Args) {
  const nested = a.config;
  const enabled = argBool(a, cfg.osd_enabled, 'enabled');
  const theme = argStr(a, cfg.osd_theme, 'theme');
  const scale = argStr(a, cfg.osd_scale, 'scale');
  const position = argStr(a, cfg.osd_position, 'position');

  cfg.osd_enabled = typeof nested?.enabled === 'boolean' ? nested.enabled : enabled;
  cfg.osd_theme = typeof nested?.theme === 'string' ? nested.theme : theme;
  cfg.osd_scale = typeof nested?.scale === 'string' ? nested.scale : scale;
  cfg.osd_position = typeof nested?.position === 'string' ? nested.position : position;
  dispatch('osd-config-changed', osdConfig());
}

// ─── Install ────────────────────────────────────────────────────────────────

export function installMockTauri(): void {
  const w = window as any;
  if (w.__TAURI_INTERNALS__) return;

  const invoke = (cmd: string, args: Args = {}): Promise<unknown> => {
    const handler = handlers[cmd];
    if (!handler) {
      console.warn('[mock] unhandled', cmd, args);
      return Promise.resolve(null);
    }
    try {
      return Promise.resolve(handler(args ?? {}));
    } catch (err) {
      console.error(`[mock] command "${cmd}" failed:`, err);
      return Promise.reject(err instanceof Error ? err.message : String(err));
    }
  };

  const transformCallback = (callback: (event: unknown) => void, once = false): number => {
    const id = nextCallbackId++;
    callbacks.set(id, { fn: callback, once });
    return id;
  };

  w.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback,
    unregisterCallback: (id: number) => callbacks.delete(id),
    convertFileSrc: (filePath: string) => filePath,
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main', windowLabel: 'main' },
    },
    plugins: {},
  };

  // The event API calls this before `plugin:event|unlisten`; keep both paths safe.
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event: string, eventId: number) => removeListener(event, eventId),
  };

  // ── Event plugin ────────────────────────────────────────────────────────
  handlers['plugin:event|listen'] = (a) => {
    const event = String(a.event);
    const handlerId = Number(a.handler);
    const eventId = nextEventId++;
    if (!eventListeners.has(event)) eventListeners.set(event, new Map());
    eventListeners.get(event)!.set(eventId, handlerId);
    return eventId;
  };
  handlers['plugin:event|unlisten'] = (a) => {
    removeListener(String(a.event), Number(a.eventId ?? a.event_id));
    return null;
  };
  handlers['plugin:event|emit'] = (a) => {
    dispatch(String(a.event), a.payload);
    return null;
  };
  handlers['plugin:event|emit_to'] = (a) => {
    const target = a.target;
    const label = target && typeof target === 'object' ? target.label : undefined;
    if (!label || label === 'main') dispatch(String(a.event), a.payload);
    return null;
  };

  // ── Window plugin: report the real browser viewport, scale 1 ────────────
  const innerSize = () => ({ width: window.innerWidth, height: window.innerHeight });
  const windowStubs: Record<string, Handler> = {
    inner_size: innerSize,
    outer_size: innerSize,
    scale_factor: () => 1,
    inner_position: () => ({ x: 0, y: 0 }),
    outer_position: () => ({ x: 0, y: 0 }),
    cursor_position: () => ({ x: 0, y: 0 }),
    is_maximized: () => state.maximized,
    maximize: () => {
      state.maximized = true;
      dispatch('tauri://resize', null);
      return null;
    },
    unmaximize: () => {
      state.maximized = false;
      dispatch('tauri://resize', null);
      return null;
    },
    toggle_maximize: () => {
      state.maximized = !state.maximized;
      dispatch('tauri://resize', null);
      return state.maximized;
    },
    is_fullscreen: () => false,
    is_minimized: () => false,
    is_focused: () => document.hasFocus(),
    is_visible: () => true,
    is_decorated: () => false,
    is_resizable: () => true,
    is_maximizable: () => true,
    is_minimizable: () => true,
    is_closable: () => true,
    is_enabled: () => true,
    is_always_on_top: () => false,
    theme: () => 'dark',
    title: () => document.title,
    current_monitor: () => currentMonitor(),
    primary_monitor: () => currentMonitor(),
    available_monitors: () => [currentMonitor()],
    monitor_from_point: () => currentMonitor(),
    get_all_windows: () => ['main'],
    activity_name: () => null,
    scene_identifier: () => null,
    create: () => null,
    start_dragging: () => null,
    start_resize_dragging: () => null,
  };
  for (const [name, fn] of Object.entries(windowStubs)) {
    handlers[`plugin:window|${name}`] = fn;
  }
  // Every remaining window mutation is a harmless no-op.
  const windowNoops = [
    'set_size', 'set_position', 'set_min_size', 'set_max_size', 'set_size_constraints',
    'set_resizable', 'set_maximizable', 'set_minimizable', 'set_closable', 'set_enabled',
    'set_decorations', 'set_shadow', 'set_always_on_top', 'set_always_on_bottom',
    'set_content_protected', 'set_cursor_icon', 'set_cursor_position', 'set_cursor_grab',
    'set_cursor_visible', 'set_focus', 'set_focusable', 'set_fullscreen',
    'set_simple_fullscreen', 'set_icon', 'set_ignore_cursor_events', 'set_skip_taskbar',
    'set_theme', 'set_title', 'set_title_bar_style', 'set_visible_on_all_workspaces',
    'set_progress_bar', 'set_badge_count', 'set_badge_label', 'set_overlay_icon',
    'set_effects', 'set_background_color', 'request_user_attention', 'center',
    'show', 'hide', 'close', 'destroy', 'minimize', 'unminimize',
  ];
  for (const name of windowNoops) {
    handlers[`plugin:window|${name}`] = () => null;
  }

  // ── Webview plugin ─────────────────────────────────────────────────────
  handlers['plugin:webview|get_all_webviews'] = () => [
    { label: 'main', windowLabel: 'main', url: window.location.href },
  ];
  handlers['plugin:webview|webview_size'] = innerSize;
  handlers['plugin:webview|webview_position'] = () => ({ x: 0, y: 0 });
  for (const name of [
    'create_webview', 'create_webview_window', 'reparent', 'set_webview_auto_resize',
    'set_webview_background_color', 'set_webview_focus', 'set_webview_position',
    'set_webview_size', 'set_webview_zoom', 'webview_close', 'webview_hide',
    'webview_show', 'clear_all_browsing_data',
  ]) {
    handlers[`plugin:webview|${name}`] = () => null;
  }

  // ── App plugin ─────────────────────────────────────────────────────────
  handlers['plugin:app|version'] = () => '0.9.3';
  handlers['plugin:app|name'] = () => 'OpenRevo';
  handlers['plugin:app|identifier'] = () => 'com.openrevo.ui';
  handlers['plugin:app|tauri_version'] = () => '2.0.0';
  handlers['plugin:app|bundle_type'] = () => 'app';
  handlers['plugin:app|supports_multiple_windows'] = () => true;
  handlers['plugin:app|default_window_icon'] = () => null;
  handlers['plugin:app|fetch_data_store_identifiers'] = () => [];
  for (const name of [
    'app_hide', 'app_show', 'set_app_theme', 'set_dock_visibility', 'remove_data_store',
  ]) {
    handlers[`plugin:app|${name}`] = () => null;
  }

  // ── Telemetry heartbeat (1 Hz) ─────────────────────────────────────────
  window.setInterval(() => {
    dispatch('hardware-state-updated', hardwareState());
  }, 1000);

  // Handy for poking at the mock from devtools.
  w.__OPENREVO_MOCK__ = { state, cfg, handlers, hardwareState, dispatch };

  console.info(
    '%c[OpenRevo mock]%c Tauri IPC mock installed — browser dev mode, fake data (%s).',
    'color:#22d3ee;font-weight:700',
    'color:inherit',
    startInFullMode ? 'full dashboard' : 'auto layout',
  );
}

export default installMockTauri;
