import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { HardwareState, PowerMode } from './types/hardware';
import { MiniDrawer } from './components/MiniDrawer';
import { FullDashboard } from './components/FullDashboard';
import { ModelCapabilities } from './plugins/plugin_manifest';

const DEFAULT_STATE: HardwareState = {
  power_mode: 2,
  fan_boost: false,
  battery_limit: 80,
  cpu_temp: 45,
  cpu_usage: 5,
  cpu_freq: 2200,
  gpu_temp: 35,
  cpu_fan_rpm: 1650,
  gpu_fan_rpm: 1600,
  gpu_usage: 0,
  gpu_core_freq: 210,
  gpu_mem_freq: 405,
  gpu_vram_mb: 16384,
  gpu_vram_used_mb: 1024,
  refresh_rate: 240,
  win_key_locked: false,
  is_mock: true,
  oem_running: false,
  autostart: false,
  cpu_name: '',
  gpu_name: '',
  model_name: '',
};

export default function App() {
  const urlParams = typeof window !== 'undefined' ? new URLSearchParams(window.location.search) : null;

  const [isExpanded, setIsExpanded] = useState(false);
  const [activeTab, setActiveTab] = useState<'overview' | 'fan' | 'gpu' | 'tuning' | 'lighting' | 'system'>(
    (import.meta.env.DEV && (urlParams?.get('tab') as any)) || 'overview'
  );
  const [openCustomTuning, setOpenCustomTuning] = useState(import.meta.env.DEV && urlParams?.get('custom') === '1');
  const [hardwareState, setHardwareState] = useState<HardwareState>(DEFAULT_STATE);
  const [isTauriEnv, setIsTauriEnv] = useState(false);

  // Model Capabilities (Declarative Hardware Driver Contract)
  const [capabilities, setCapabilities] = useState<ModelCapabilities | undefined>(undefined);

  useEffect(() => {
    invoke<ModelCapabilities>('get_model_capabilities')
      .then((caps) => {
        if (caps) {
          setCapabilities(caps);
        }
      })
      .catch((err) => console.warn('Failed to load model capabilities in App:', err));
  }, []);

  const [theme, setTheme] = useState<'mono' | 'cyber'>(() => {
    return (localStorage.getItem('openrevo_theme') as 'mono' | 'cyber') ||
      (localStorage.getItem('openrevo_mini_theme') as 'mono' | 'cyber') || 'mono';
  });

  const handleToggleTheme = () => {
    setTheme((prev) => {
      const next = prev === 'mono' ? 'cyber' : 'mono';
      localStorage.setItem('openrevo_theme', next);
      localStorage.setItem('openrevo_mini_theme', next);
      return next;
    });
  };

  // Instant ref lock: bypasses React state batching & closure lag completely
  const overrideLockRef = useRef<{
    fan_boost?: { val: boolean; expiry: number };
    battery_limit?: { val: number; expiry: number };
    power_mode?: { val: PowerMode; expiry: number };
  }>({});

  // Global root synchronization: Penetrate all Portals, Modals and Tooltips
  useEffect(() => {
    const docEl = document.documentElement;
    docEl.setAttribute('data-theme', theme);
    docEl.setAttribute('data-window-mode', isExpanded ? 'full' : 'mini');
  }, [theme, isExpanded]);

  // Poll hardware status
  const fetchStatus = useCallback(async () => {
    try {
      const state = await invoke<HardwareState>('get_hardware_status');
      const now = Date.now();
      const lock = overrideLockRef.current;
      setHardwareState((_prev) => {
        let merged = { ...state };
        if (lock.fan_boost && now < lock.fan_boost.expiry) {
          merged.fan_boost = lock.fan_boost.val;
        }
        if (lock.battery_limit && now < lock.battery_limit.expiry) {
          merged.battery_limit = lock.battery_limit.val;
        }
        if (lock.power_mode && now < lock.power_mode.expiry) {
          merged.power_mode = lock.power_mode.val;
        }
        return merged;
      });
      setIsTauriEnv(true);
    } catch {
      // In standalone web browser dev mode, keep simulation state
      setIsTauriEnv(false);
    }
  }, []);

  useEffect(() => {
    // Initial sync from persistent config on boot
    const bootSync = async () => {
      try {
        const cfg = await invoke<any>('get_app_config');
        if (cfg) {
          setHardwareState((prev) => ({
            ...prev,
            power_mode: cfg.power_mode,
            fan_boost: cfg.fan_boost,
            battery_limit: cfg.battery_limit,
            win_key_locked: cfg.win_key_locked,
            refresh_rate: cfg.refresh_rate,
            autostart: cfg.autostart,
          }));
          if (cfg.wake_window_mode === 'full') {
            setIsExpanded(true);
          }
        }
      } catch (_) {}

      try {
        await invoke('frontend_ready');
        await invoke('report_window_visibility', { visible: true });
      } catch (_) {}
      fetchStatus();
    };
    bootSync();

    // Liveness ping from the tray click; no answer means the WebView died and the backend relaunches.
    let unlistenPing: (() => void) | undefined;
    listen<number>('ui-ping', (event) => {
      invoke('ui_pong', { seq: event.payload }).catch(() => {});
    }).then((un) => {
      unlistenPing = un;
    }).catch(() => {});

    // Responsive hardware state event bus subscription (zero polling overhead)
    let unlistenTelemetry: (() => void) | undefined;
    listen<HardwareState>('hardware-state-updated', (event) => {
      const state = event.payload;
      const now = Date.now();
      const lock = overrideLockRef.current;
      setHardwareState((_prev) => {
        let merged = { ...state };
        if (lock.fan_boost && now < lock.fan_boost.expiry) {
          merged.fan_boost = lock.fan_boost.val;
        }
        if (lock.battery_limit && now < lock.battery_limit.expiry) {
          merged.battery_limit = lock.battery_limit.val;
        }
        if (lock.power_mode && now < lock.power_mode.expiry) {
          merged.power_mode = lock.power_mode.val;
        }
        return merged;
      });
      setIsTauriEnv(true);
    }).then((un) => {
      unlistenTelemetry = un;
    }).catch(() => {});

    // Slow fallback watchdog timer (10s) just in case event stream is quiet
    const timer = setInterval(fetchStatus, 10000);

    // Strictly synchronize layout mode with explicit window events or large width
    // Converts physical pixels to logical pixels via scaleFactor on High-DPI screens
    const syncModeWithWindowSize = async () => {
      let logicalWidth = window.innerWidth;
      try {
        const win = getCurrentWindow();
        const sz = await win.innerSize();
        const scale = await win.scaleFactor();
        if (scale > 0 && sz.width > 0) {
          logicalWidth = sz.width / scale;
        }
      } catch (_) {}

      // Only auto-upgrade to expanded if distinctly in full dashboard width; do not auto-collapse to mini
      if (logicalWidth >= 800) {
        setIsExpanded(true);
      }
    };

    let unlistenReset: (() => void) | undefined;
    let unlistenSyncMode: (() => void) | undefined;
    let unlistenFocus: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    let unlistenFan: (() => void) | undefined;
    let unlistenWinLock: (() => void) | undefined;
    let unlistenDcStatus: (() => void) | undefined;
    let unlistenUnlockBattery: (() => void) | undefined;
    let unlistenGpuLimits: (() => void) | undefined;
    let unlistenPowerPresets: (() => void) | undefined;

    listen('gpu-power-limits-ready', () => {
      fetchStatus();
    }).then((fn) => {
      unlistenGpuLimits = fn;
    }).catch(() => {});

    listen<any>('factory-power-presets-ready', (event) => {
      if (event.payload) {
        setHardwareState((prev) => ({
          ...prev,
          power_presets: event.payload,
        }));
      }
    }).then((fn) => {
      unlistenPowerPresets = fn;
    }).catch(() => {});

    try {
      const win = getCurrentWindow();
      win.listen('reset-to-mini', async () => {
        let logicalWidth = window.innerWidth;
        try {
          const sz = await win.innerSize();
          const scale = await win.scaleFactor();
          if (scale > 0 && sz.width > 0) {
            logicalWidth = sz.width / scale;
          }
        } catch (_) {}
        // If window is currently large (>= 650px logical), refuse to collapse to mini
        if (logicalWidth >= 650) {
          setIsExpanded(true);
          return;
        }
        setIsExpanded(false);
      }).then((fn) => {
        unlistenReset = fn;
      });

      win.listen<boolean>('sync-window-mode', (event) => {
        setIsExpanded(event.payload);
      }).then((fn) => {
        unlistenSyncMode = fn;
      });

      win.listen('tauri://focus', () => {
        syncModeWithWindowSize();
        fetchStatus();
        invoke('report_window_visibility', { visible: true }).catch(() => {});
      }).then((fn) => {
        unlistenFocus = fn;
      });

      listen<number>('physical-mode-switched', (event) => {
        const nextMode = event.payload as PowerMode;
        console.log('[Hardware Key] Mode switched by chassis Q-Key to:', nextMode);
        overrideLockRef.current.power_mode = { val: nextMode, expiry: Date.now() + 5000 };
        setHardwareState((prev) => ({ ...prev, power_mode: nextMode }));
      }).then((fn) => {
        unlistenMode = fn;
      });

      listen<boolean>('physical-fan-boost-switched', (event) => {
        const nextFan = event.payload;
        console.log('[Hardware Key] Fan Boost switched by chassis Q-Key to:', nextFan);
        overrideLockRef.current.fan_boost = { val: nextFan, expiry: Date.now() + 4000 };
        setHardwareState((prev) => ({
          ...prev,
          fan_boost: nextFan,
        }));
      }).then((fn) => {
        unlistenFan = fn;
      });

      listen<boolean>('win-lock-state-changed', (event) => {
        console.log('[Hardware Watcher] Win lock state changed to:', event.payload);
        setHardwareState((prev) => ({ ...prev, win_key_locked: event.payload }));
      }).then((fn) => {
        unlistenWinLock = fn;
      });

      listen<boolean>('unlock-battery-power-limit-changed', (event) => {
        console.log('[Hardware Watcher] Unlock battery power limit changed to:', event.payload);
        setHardwareState((prev) => ({ ...prev, unlock_battery_power_limit: event.payload }));
      }).then((fn) => {
        unlistenUnlockBattery = fn;
      });

      listen<boolean>('dc-status-changed', (event) => {
        console.log('[Hardware Watcher] DC power state changed to:', event.payload);
        setHardwareState((prev) => ({ ...prev, is_dc_online: event.payload }));
      }).then((fn) => {
        unlistenDcStatus = fn;
      });
    } catch (_) {}

    // Prevent browser default refresh (F5 / Ctrl+R) from breaking window layout
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'F5' || (e.ctrlKey && e.key.toLowerCase() === 'r')) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('resize', syncModeWithWindowSize);
    window.addEventListener('focus', syncModeWithWindowSize);
    syncModeWithWindowSize();

    return () => {
      clearInterval(timer);
      if (unlistenPing) unlistenPing();
      if (unlistenTelemetry) unlistenTelemetry();
      if (unlistenReset) unlistenReset();
      if (unlistenSyncMode) unlistenSyncMode();
      if (unlistenFocus) unlistenFocus();
      if (unlistenMode) unlistenMode();
      if (unlistenFan) unlistenFan();
      if (unlistenWinLock) unlistenWinLock();
      if (unlistenDcStatus) unlistenDcStatus();
      if (unlistenUnlockBattery) unlistenUnlockBattery();
      if (unlistenGpuLimits) unlistenGpuLimits();
      if (unlistenPowerPresets) unlistenPowerPresets();
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('resize', syncModeWithWindowSize);
      window.removeEventListener('focus', syncModeWithWindowSize);
    };
  }, [fetchStatus]);

  // Power mode
  const handleSetPowerMode = async (mode: PowerMode) => {
    overrideLockRef.current.power_mode = { val: mode, expiry: Date.now() + 5000 };
    setHardwareState((prev) => ({ ...prev, power_mode: mode }));
    if (isTauriEnv) {
      try {
        await invoke('set_power_mode', { mode });
      } catch (e) {
        console.error('Failed to set power mode:', e);
      }
    }
  };

  // Fan Boost
  const handleToggleFanBoost = async () => {
    const nextVal = !hardwareState.fan_boost;
    overrideLockRef.current.fan_boost = { val: nextVal, expiry: Date.now() + 4000 };
    setHardwareState((prev) => ({
      ...prev,
      fan_boost: nextVal,
    }));
    if (isTauriEnv) {
      try {
        await invoke('set_fan_boost', { enable: nextVal });
      } catch (e) {
        console.error('Failed to toggle fan boost:', e);
      }
    }
  };

  // Battery limit
  const handleToggleBatteryLimit = async (targetLimit?: number) => {
    let nextLimit: number;
    if (typeof targetLimit === 'number') {
      nextLimit = targetLimit;
    } else {
      if (hardwareState.battery_limit === 80) {
        nextLimit = 60;
      } else if (hardwareState.battery_limit === 60) {
        nextLimit = 100;
      } else {
        nextLimit = 80;
      }
    }
    overrideLockRef.current.battery_limit = { val: nextLimit, expiry: Date.now() + 4000 };
    setHardwareState((prev) => ({ ...prev, battery_limit: nextLimit }));
    if (isTauriEnv) {
      try {
        await invoke('set_battery_limit', { limit: nextLimit });
      } catch (e) {
        console.error('Failed to set battery limit:', e);
      }
    }
  };

  // Refresh rate
  const handleSwitchRefreshRate = async (hz: number, invokeBackend = true) => {
    setHardwareState((prev) => ({ ...prev, refresh_rate: hz }));
    if (isTauriEnv && invokeBackend) {
      try {
        await invoke('switch_refresh_rate', { hz });
        await fetchStatus();
      } catch (e) {
        console.error('Failed to switch refresh rate:', e);
      }
    } else if (isTauriEnv) {
      fetchStatus();
    }
  };

  // Win Key Lock
  const handleToggleWinKey = async () => {
    const nextVal = !hardwareState.win_key_locked;
    setHardwareState((prev) => ({ ...prev, win_key_locked: nextVal }));
    if (isTauriEnv) {
      try {
        const res = await invoke<boolean>('set_win_key_locked', { locked: nextVal });
        setHardwareState((prev) => ({ ...prev, win_key_locked: res }));
      } catch (e) {
        console.error('Failed to toggle win key:', e);
      }
    }
  };

  // Autostart checkbox toggle
  const handleToggleAutostart = async () => {
    const nextVal = !hardwareState.autostart;
    setHardwareState((prev) => ({ ...prev, autostart: nextVal }));
    if (isTauriEnv) {
      try {
        const res = await invoke<boolean>('set_autostart_enabled', { enabled: nextVal });
        setHardwareState((prev) => ({ ...prev, autostart: res }));
      } catch (e) {
        console.error('Failed to toggle autostart:', e);
      }
    }
  };

  // Toggle OEM Service
  const handleToggleOemService = async (takeOver: boolean) => {
    if (isTauriEnv) {
      try {
        const updated = await invoke<HardwareState>('toggle_oem_service', { takeOver });
        setHardwareState(updated);
      } catch (e) {
        console.error('Failed to toggle OEM service:', e);
      }
    }
  };

  // Window resize & layout mode
  const handleExpandDashboard = async (targetTab?: 'overview' | 'fan' | 'gpu' | 'tuning' | 'lighting' | 'system' | 'custom_tuning') => {
    if (targetTab === 'custom_tuning') {
      setActiveTab('overview');
      setOpenCustomTuning(true);
    } else {
      setActiveTab(targetTab || 'overview');
      setOpenCustomTuning(false);
    }
    setIsExpanded(true);
    if (isTauriEnv) {
      try {
        await invoke('resize_window', { expanded: true });
      } catch (e) {
        console.error(e);
      }
    }
  };

  const handleCollapseDashboard = async () => {
    setIsExpanded(false);
    setOpenCustomTuning(false);
    if (isTauriEnv) {
      try {
        await invoke('resize_window', { expanded: false });
      } catch (e) {
        console.error(e);
      }
    }
  };

  const handleMinimize = async () => {
    if (isTauriEnv) {
      try {
        const win = getCurrentWindow();
        await win.minimize();
        invoke('report_window_visibility', { visible: false }).catch(() => {});
      } catch (e) {
        console.error(e);
      }
    }
  };

  const handleClose = async () => {
    setOpenCustomTuning(false);
    if (isTauriEnv) {
      try {
        const win = getCurrentWindow();
        await win.hide();
        invoke('report_window_visibility', { visible: false }).catch(() => {});
      } catch (e) {
        console.error(e);
      }
    } else {
      setIsExpanded(false);
    }
  };

  return (
    <div
      className="openrevo-app-root"
      data-theme={theme}
      data-window-mode={isExpanded ? 'full' : 'mini'}
      data-form-factor={capabilities?.form_factor || 'gaming'}
    >
      {isExpanded ? (
        <FullDashboard
          status={hardwareState}
          theme={theme}
          onToggleTheme={handleToggleTheme}
          onSetPowerMode={handleSetPowerMode}
          onToggleFanBoost={handleToggleFanBoost}
          onToggleBatteryLimit={handleToggleBatteryLimit}
          onSwitchRefreshRate={handleSwitchRefreshRate}
          onToggleWinKey={handleToggleWinKey}
          onToggleAutostart={handleToggleAutostart}
          onToggleOemService={handleToggleOemService}
          onCollapseDashboard={handleCollapseDashboard}
          onMinimize={handleMinimize}
          onClose={handleClose}
          initialTab={activeTab}
          initialOpenCustomTuning={openCustomTuning}
          capabilities={capabilities}
        />
      ) : (
        <MiniDrawer
          status={hardwareState}
          theme={theme}
          capabilities={capabilities}
          onToggleTheme={handleToggleTheme}
          onSetPowerMode={handleSetPowerMode}
          onToggleFanBoost={handleToggleFanBoost}
          onSwitchRefreshRate={handleSwitchRefreshRate}
          onToggleWinKey={handleToggleWinKey}
          onToggleAutostart={handleToggleAutostart}
          onToggleOemService={handleToggleOemService}
          onExpandDashboard={handleExpandDashboard}
          onMinimize={handleMinimize}
          onClose={handleClose}
        />
      )}
    </div>
  );
}
