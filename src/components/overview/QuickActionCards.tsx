import React, { useState, useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  Fan,
  Monitor,
  ZapOff,
  ChevronDown,
  Laptop,
  Tv,
} from 'lucide-react';
import { HardwareState } from '../../types/hardware';

import { ModelCapabilities } from '../../plugins/plugin_manifest';

interface QuickActionCardsProps {
  status: HardwareState;
  capabilities?: ModelCapabilities;
  onToggleFanBoost: () => void;
  onToggleBatteryLimit?: () => void;
  onSwitchRefreshRate: (hz: number, invokeBackend?: boolean) => void;
  checkOemGuard: (action: () => void) => void;
}

export const QuickActionCards: React.FC<QuickActionCardsProps> = ({
  status,
  onToggleFanBoost,
  onSwitchRefreshRate,
  checkOemGuard,
}) => {
  const [autoMinRefresh, setAutoMinRefresh] = useState(status.auto_min_refresh_on_battery ?? false);
  useEffect(() => {
    if (typeof status.auto_min_refresh_on_battery === 'boolean') {
      setAutoMinRefresh(status.auto_min_refresh_on_battery);
    }
  }, [status.auto_min_refresh_on_battery]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<number>('refresh-rate-changed', (event) => {
      onSwitchRefreshRate(event.payload, false);
    }).then((fn) => { unlisten = fn; });
    return () => {
      if (unlisten) unlisten();
    };
  }, [onSwitchRefreshRate]);

  const handleToggleAutoMinRefresh = async () => {
    const nextVal = !autoMinRefresh;
    setAutoMinRefresh(nextVal);
    try {
      await invoke('set_auto_min_refresh_on_battery', { enable: nextVal });
    } catch (e) {
      console.error('Failed to toggle auto min refresh:', e);
      setAutoMinRefresh(!nextVal);
    }
  };

  const monitors = useMemo(() => status.monitors || [], [status.monitors]);
  const [selectedMonIdx, setSelectedMonIdx] = useState(0);
  const [showMonMenu, setShowMonMenu] = useState(false);

  // Close dropdown on outside click
  useEffect(() => {
    const handleOutsideClick = () => setShowMonMenu(false);
    window.addEventListener('click', handleOutsideClick);
    return () => window.removeEventListener('click', handleOutsideClick);
  }, []);

  const activeMon = monitors[selectedMonIdx] || monitors[0] || null;
  const currentHz = activeMon ? activeMon.current_hz : status.refresh_rate;
  const supportedRates = useMemo(() => {
    if (activeMon?.supported_rates && activeMon.supported_rates.length > 0) {
      return [...activeMon.supported_rates].sort((a, b) => a - b);
    }
    return status.available_refresh_rates && status.available_refresh_rates.length > 0
      ? status.available_refresh_rates
      : [60, 144, 240];
  }, [activeMon, status.available_refresh_rates]);

  const handleSetMonitorRate = async (deviceName: string, hz: number) => {
    try {
      await invoke('set_display_monitor_refresh_rate', { deviceName, hz });
      const target = monitors.find((m) => m.device_name === deviceName);
      if (target) {
        target.current_hz = hz;
        if (target.is_primary || monitors.length <= 1) {
          onSwitchRefreshRate(hz, false);
        }
      }
    } catch (err) {
      console.warn('Set monitor rate failed:', err);
    }
  };

  return (
    <div style={{ display: 'flex', gap: '12px', alignItems: 'stretch', position: 'relative', zIndex: showMonMenu ? 200 : 1 }}>
      {/* 1. 一键强冷 */}
      {status.support_flags?.has_fan_boost !== false && (
          <div
            className="glass-card"
            style={{
              minWidth: '180px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: '12px',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <Fan className="w-5 h-5 text-cyan-400 flex-shrink-0" />
              <div>
                <div style={{ fontSize: '12px', fontWeight: 600 }}>一键强冷</div>
                <div style={{ fontSize: '10px', color: 'var(--text-dim)' }}>最高转速极限降温</div>
              </div>
            </div>
            <button
              onClick={() => checkOemGuard(onToggleFanBoost)}
              className={`toggle-btn ${status.fan_boost ? 'active-cyan' : ''}`}
            >
              {status.fan_boost ? '已开启' : '关闭'}
            </button>
          </div>
      )}

      {/* 2. 拔电自动低刷 */}
      <div
        className="glass-card"
        title="开启后，当断开电源适配器时自动将屏幕切换至面板支持的最低刷新率以最大化电池续航；若使用电池时手动调整了刷新率，将维持手动设定"
        style={{
          minWidth: '180px',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: '12px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <ZapOff className={`w-5 h-5 ${autoMinRefresh ? 'text-indigo-400' : 'text-slate-400'} flex-shrink-0`} />
          <div>
            <div style={{ fontSize: '12px', fontWeight: 600 }}>拔电自动低刷</div>
            <div style={{ fontSize: '10px', color: 'var(--text-dim)' }}>离电续航最大化</div>
          </div>
        </div>
        <button
          onClick={handleToggleAutoMinRefresh}
          className={`toggle-btn ${autoMinRefresh ? 'active-indigo' : ''}`}
        >
          {autoMinRefresh ? '已开启' : '关闭'}
        </button>
      </div>

      {/* 4. 屏幕刷新率卡片 (固定高度单行，多屏纯黑下拉，杜绝撑大容器) */}
      <div
        className="glass-card"
        style={{
          flex: 1,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: '12px',
          position: 'relative',
          zIndex: showMonMenu ? 250 : 1,
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px', minWidth: '130px', flexShrink: 0 }}>
          <Monitor className="w-5 h-5 text-indigo-400 flex-shrink-0" />
          <div>
            <div style={{ fontSize: '12px', fontWeight: 600 }}>屏幕刷新率</div>
            {monitors.length <= 1 ? (
              <div style={{ fontSize: '10px', color: 'var(--text-dim)' }}>
                {activeMon ? activeMon.friendly_name : '2.5K 屏'}
              </div>
            ) : (
              /* 多屏微型下拉选择器 */
              <div style={{ position: 'relative' }} onClick={(e) => e.stopPropagation()}>
                <button
                  onClick={() => setShowMonMenu(!showMonMenu)}
                  title="点击选择调控的显示器"
                  className="monitor-select-btn"
                  style={{ marginTop: '2px' }}
                >
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: '3px' }}>
                    {activeMon?.is_internal ? <Laptop size={11} /> : <Tv size={11} />}
                    {activeMon?.friendly_name?.split(' ')[0] || '屏幕'}
                  </span>
                  <ChevronDown size={10} />
                </button>

                {showMonMenu && (
                  <div className="monitor-select-dropdown">
                    {monitors.map((m, idx) => (
                      <div
                        key={m.device_name}
                        onClick={() => {
                          setSelectedMonIdx(idx);
                          setShowMonMenu(false);
                        }}
                        className={`monitor-select-item ${idx === selectedMonIdx ? 'active' : ''}`}
                      >
                        <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                          <span style={{ display: 'flex', alignItems: 'center' }}>
                            {m.is_internal ? <Laptop size={12} /> : <Tv size={12} />}
                          </span>
                          <span style={{ fontWeight: 600 }}>{m.friendly_name}</span>
                        </div>
                        <span
                          style={{
                            fontSize: '9.5px',
                            fontFamily: 'monospace',
                            background: 'var(--surface-badge)',
                            padding: '1px 5px',
                            borderRadius: '3px',
                          }}
                        >
                          {m.current_hz}Hz
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>

        <div style={{ display: 'flex', gap: '4px', flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          {supportedRates.map((hz) => (
            <button
              key={hz}
              onClick={() => checkOemGuard(() => activeMon ? handleSetMonitorRate(activeMon.device_name, hz) : onSwitchRefreshRate(hz, true))}
              className={`toggle-btn ${currentHz === hz ? 'active-indigo' : ''}`}
              style={{ minWidth: '44px', padding: '3px 8px' }}
            >
              {hz}Hz
            </button>
          ))}
        </div>
      </div>
    </div>
  );
};
