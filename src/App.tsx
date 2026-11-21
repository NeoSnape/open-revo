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

  const [toastMessage, setToastMessage] = useState<string | null>(null);
  const toastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const setWindowVisibleRef = useRef<((visible: boolean, immediate?: boolean) => void) | null>(null);

  const showToast = useCallback((message: string) => {
    const text = message.trim();
    if (!text) return;
    setToastMessage(text);
    if (toastTimerRef.current) {
      clearTimeout(toastTimerRef.current);
    }
    toastTimerRef.current = setTimeout(() => {
      setToastMessage(null);
      toastTimerRef.current = null;
    }, 3500);
  }, []);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current) {
        clearTimeout(toastTimerRef.current);
        toastTimerRef.current = null;
      }
    };
  }, []);

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
    let disposed = false;
    let windowVisible = document.visibilityState !== 'hidden';
    let visibilitySyncedByEvent = false;
    let fallbackTimer: ReturnType<typeof setInterval> | null = null;
    let lastImmediateFetchAt = 0;

    const stopFallbackPolling = () => {
      if (fallbackTimer) {
        clearInterval(fallbackTimer);
        fallbackTimer = null;
      }
    };

    const startFallbackPolling = () => {
      if (!disposed && !fallbackTimer) {
        fallbackTimer = setInterval(fetchStatus, 10000);
      }
    };

    const applyVisibility = (visible: boolean, fetchImmediately = false, fromEvent = true) => {
      if (disposed) return;
      if (fromEvent) {
        visibilitySyncedByEvent = true;
      } else if (visibilitySyncedByEvent) {
        return;
      }
      const wasVisible = windowVisible;
      windowVisible = visible;
      if (visible) {
        const now = Date.now();
        if ((fetchImmediately || !wasVisible) && now - lastImmediateFetchAt > 250) {
          lastImmediateFetchAt = now;
          fetchStatus();
        }
        startFallbackPolling();
      } else {
        stopFallbackPolling();
      }
    };

    setWindowVisibleRef.current = (visible, immediate = false) =>
      applyVisibility(visible, immediate, true);

    const registerUnlisten = (
      promise: Promise<() => void>,
      setter: (fn: () => void) => void
    ) => {
      promise
        .then((fn) => {
          if (disposed) {
            fn();
          } else {
            setter(fn);
          }
        })
        .catch(() => {});
    };

    // Initial sync from persistent config on boot
    const bootSync = async () => {
      try {
        const cfg = await invoke<any>('get_app_config');
        if (!disposed && cfg) {
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

      let initiallyVisible = document.visibilityState !== 'hidden';
      try {
        const win = getCurrentWindow();
        const [vis, min] = await Promise.all([win.isVisible(), win.isMinimized()]);
        initiallyVisible = vis && !min && document.visibilityState !== 'hidden';
      } catch (_) {}

      try {
        await invoke('frontend_ready');
        if (!disposed && !visibilitySyncedByEvent) {
          await invoke('report_window_visibility', { visible: initiallyVisible });
        }
      } catch (_) {}

      if (!disposed && !visibilitySyncedByEvent) {
        applyVisibility(initiallyVisible, initiallyVisible, false);
      }
    };
    bootSync();

    // Liveness ping from the tray click; no answer means the WebView died and the backend relaunches.
    let unlistenPing: (() => void) | undefined;
    registerUnlisten(
      listen<number>('ui-ping', (event) => {
        invoke('ui_pong', { seq: event.payload }).catch(() => {});
      }),
      (un) => { unlistenPing = un; }
    );

    // Responsive hardware state event bus subscription (zero polling overhead)
    let unlistenTelemetry: (() => void) | undefined;
    registerUnlisten(
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
      }),
      (un) => { unlistenTelemetry = un; }
    );

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
    let unlistenVisibility: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    let unlistenModeRejected: (() => void) | undefined;
    let unlistenFan: (() => void) | undefined;
    let unlistenWinLock: (() => void) | undefined;
    let unlistenDcStatus: (() => void) | undefined;
    let unlistenUnlockBattery: (() => void) | undefined;
    let unlistenGpuLimits: (() => void) | undefined;
    let unlistenPowerPresets: (() => void) | undefined;

    registerUnlisten(
      listen<boolean>('window-visibility-changed', (event) => {
        const vis = Boolean(event.payload);
        applyVisibility(vis, vis);
      }),
      (fn) => { unlistenVisibility = fn; }
    );

    registerUnlisten(
      listen<string>('power-mode-rejected', (event) => {
        const msg = typeof event.payload === 'string' ? event.payload : String(event.payload ?? '');
        if (msg) {
          overrideLockRef.current.power_mode = undefined;
          showToast(msg);
          fetchStatus();
        }
      }),
      (fn) => { unlistenModeRejected = fn; }
    );

    registerUnlisten(
      listen('gpu-power-limits-ready', () => {
        if (windowVisible) {
          fetchStatus();
        }
      }),
      (fn) => { unlistenGpuLimits = fn; }
    );

    registerUnlisten(
      listen<any>('factory-power-presets-ready', (event) => {
        if (event.payload) {
          setHardwareState((prev) => ({
            ...prev,
            power_presets: event.payload,
          }));
        }
      }),
      (fn) => { unlistenPowerPresets = fn; }
    );

    try {
      const win = getCurrentWindow();
      registerUnlisten(
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
        }),
        (fn) => { unlistenReset = fn; }
      );

      registerUnlisten(
        win.listen<boolean>('sync-window-mode', (event) => {
          setIsExpanded(event.payload);
        }),
        (fn) => { unlistenSyncMode = fn; }
      );

      registerUnlisten(
        win.listen('tauri://focus', () => {
          syncModeWithWindowSize();
          applyVisibility(true, true);
          invoke('report_window_visibility', { visible: true }).catch(() => {});
        }),
        (fn) => { unlistenFocus = fn; }
      );

      registerUnlisten(
        listen<number>('physical-mode-switched', (event) => {
          const nextMode = event.payload as PowerMode;
          console.log('[Hardware Key] Mode switched by chassis Q-Key to:', nextMode);
          overrideLockRef.current.power_mode = { val: nextMode, expiry: Date.now() + 5000 };
          setHardwareState((prev) => ({ ...prev, power_mode: nextMode }));
        }),
        (fn) => { unlistenMode = fn; }
      );

      registerUnlisten(
        listen<boolean>('physical-fan-boost-switched', (event) => {
          const nextFan = event.payload;
          console.log('[Hardware Key] Fan Boost switched by chassis Q-Key to:', nextFan);
          overrideLockRef.current.fan_boost = { val: nextFan, expiry: Date.now() + 4000 };
          setHardwareState((prev) => ({
            ...prev,
            fan_boost: nextFan,
          }));
        }),
        (fn) => { unlistenFan = fn; }
      );

      registerUnlisten(
        listen<boolean>('win-lock-state-changed', (event) => {
          console.log('[Hardware Watcher] Win lock state changed to:', event.payload);
          setHardwareState((prev) => ({ ...prev, win_key_locked: event.payload }));
        }),
        (fn) => { unlistenWinLock = fn; }
      );

      registerUnlisten(
        listen<boolean>('unlock-battery-power-limit-changed', (event) => {
          console.log('[Hardware Watcher] Unlock battery power limit changed to:', event.payload);
          setHardwareState((prev) => ({ ...prev, unlock_battery_power_limit: event.payload }));
        }),
        (fn) => { unlistenUnlockBattery = fn; }
      );

      registerUnlisten(
        listen<boolean>('dc-status-changed', (event) => {
          console.log('[Hardware Watcher] DC power state changed to:', event.payload);
          setHardwareState((prev) => ({ ...prev, is_dc_online: event.payload }));
        }),
        (fn) => { unlistenDcStatus = fn; }
      );
    } catch (_) {}

    const handleVisibilityChange = () => {
      const vis = document.visibilityState !== 'hidden';
      applyVisibility(vis, vis);
    };

    // Prevent browser default refresh (F5 / Ctrl+R) from breaking window layout
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'F5' || (e.ctrlKey && e.key.toLowerCase() === 'r')) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('resize', syncModeWithWindowSize);
    window.addEventListener('focus', syncModeWithWindowSize);
    syncModeWithWindowSize();

    return () => {
      disposed = true;
      setWindowVisibleRef.current = null;
      stopFallbackPolling();
      if (unlistenPing) unlistenPing();
      if (unlistenTelemetry) unlistenTelemetry();
      if (unlistenReset) unlistenReset();
      if (unlistenSyncMode) unlistenSyncMode();
      if (unlistenFocus) unlistenFocus();
      if (unlistenVisibility) unlistenVisibility();
      if (unlistenMode) unlistenMode();
      if (unlistenModeRejected) unlistenModeRejected();
      if (unlistenFan) unlistenFan();
      if (unlistenWinLock) unlistenWinLock();
      if (unlistenDcStatus) unlistenDcStatus();
      if (unlistenUnlockBattery) unlistenUnlockBattery();
      if (unlistenGpuLimits) unlistenGpuLimits();
      if (unlistenPowerPresets) unlistenPowerPresets();
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('resize', syncModeWithWindowSize);
      window.removeEventListener('focus', syncModeWithWindowSize);
    };
  }, [fetchStatus, showToast]);

  // Power mode
  const handleSetPowerMode = async (mode: PowerMode) => {
    overrideLockRef.current.power_mode = { val: mode, expiry: Date.now() + 5000 };
    setHardwareState((prev) => ({ ...prev, power_mode: mode }));
    if (isTauriEnv) {
      try {
        await invoke('set_power_mode', { mode });
      } catch (e) {
        // Refused (e.g. Beast/Custom on battery): stop showing the mode that was not applied.
        console.error('Failed to set power mode:', e);
        overrideLockRef.current.power_mode = { val: mode, expiry: 0 };
        showToast(String(e));
        fetchStatus();
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
    setWindowVisibleRef.current?.(false, false);
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
    setWindowVisibleRef.current?.(false, false);
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
      {toastMessage && (
        <div className="app-toast" role="status" aria-live="polite">
          {toastMessage}
        </div>
      )}
    </div>
  );
}
