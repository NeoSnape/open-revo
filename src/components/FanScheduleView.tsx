import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Fan, RefreshCw, CheckCircle2, AlertTriangle, Wind } from 'lucide-react';
import { HardwareState } from '../types/hardware';

interface CurvePoint {
  temp: number;
  duty: number;
}

interface ModeCurves {
  mode: number;
  cpu: CurvePoint[];
  gpu: CurvePoint[];
}

interface PresetFanInfo {
  policy: 'tuned' | 'official';
  modes: ModeCurves[];
  official: ModeCurves;
}

interface EcFanTableSnapshot {
  cpu_upt: number[];
  cpu_downt: number[];
  cpu_duty_pct: number[];
  gpu_upt: number[];
  gpu_downt: number[];
  gpu_duty_pct: number[];
}

interface FanApplyStatus {
  state: 'applied' | 'mismatch' | 'fan_boost' | 'unreadable';
  source: string;
  ec: EcFanTableSnapshot | null;
}

const MODE_META: Record<number, { name: string; power: string; cls: string }> = {
  1: { name: '办公', power: 'CPU 45/45 W · GPU 65 W', cls: 'mode-office' },
  2: { name: '均衡', power: 'CPU 75/85 W · GPU 115 W', cls: 'mode-balance' },
  3: { name: '狂暴', power: 'CPU 175 W · GPU 140 W', cls: 'mode-beast' },
};

const T_MIN = 40;
const T_MAX = 100;
const W = 260;
const H = 132;
const PAD_L = 26;
const PAD_B = 18;

const x = (t: number) => PAD_L + ((Math.min(Math.max(t, T_MIN), T_MAX) - T_MIN) / (T_MAX - T_MIN)) * (W - PAD_L - 6);
const y = (d: number) => 6 + (1 - d / 100) * (H - PAD_B - 6);

/** EC tables are step functions: level i holds its duty from UpT[i] until the next level. */
const stepPath = (pts: CurvePoint[]) => {
  if (!pts.length) return '';
  let p = `M ${x(T_MIN)} ${y(0)} L ${x(pts[0].temp)} ${y(0)}`;
  pts.forEach((pt, i) => {
    p += ` L ${x(pt.temp)} ${y(pt.duty)}`;
    const next = pts[i + 1];
    p += ` L ${x(next ? next.temp : T_MAX)} ${y(pt.duty)}`;
  });
  return p;
};

const dutyAt = (pts: CurvePoint[], t: number) => {
  let d = 0;
  for (const p of pts) if (t >= p.temp) d = p.duty;
  return d;
};

const CurveChart: React.FC<{ curves: ModeCurves; official?: ModeCurves }> = ({ curves, official }) => (
  <svg className="fan-chart" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
    {[0, 25, 50, 75, 100].map((d) => (
      <g key={d}>
        <line className="fan-chart-grid" x1={PAD_L} x2={W - 6} y1={y(d)} y2={y(d)} />
        <text className="fan-chart-label" x={PAD_L - 4} y={y(d) + 3} textAnchor="end">
          {d}
        </text>
      </g>
    ))}
    {[40, 50, 60, 70, 80, 90, 100].map((t) => (
      <text key={t} className="fan-chart-label" x={x(t)} y={H - 4} textAnchor="middle">
        {t}°
      </text>
    ))}
    {official && <path className="fan-chart-official" d={stepPath(official.cpu)} />}
    <path className="fan-chart-gpu" d={stepPath(curves.gpu)} />
    <path className="fan-chart-cpu" d={stepPath(curves.cpu)} />
  </svg>
);

interface FanScheduleViewProps {
  status: HardwareState;
}

export const FanScheduleView: React.FC<FanScheduleViewProps> = ({ status }) => {
  const [info, setInfo] = useState<PresetFanInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [apply, setApply] = useState<FanApplyStatus | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    invoke<PresetFanInfo>('get_preset_fan_info')
      .then(setInfo)
      .catch((e) => setError(String(e)));
  }, []);

  const setPolicy = async (policy: 'tuned' | 'official') => {
    if (!info || info.policy === policy) return;
    setBusy(true);
    setError(null);
    try {
      setInfo(await invoke<PresetFanInfo>('set_preset_fan_policy', { policy }));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const checkApplied = useCallback(async () => {
    setChecking(true);
    try {
      setApply(await invoke<FanApplyStatus>('get_fan_apply_status'));
    } catch (e) {
      setError(`读取 EC 风扇表失败：${e}`);
    } finally {
      setChecking(false);
    }
  }, []);

  // Verify on entering the page, and again shortly after a mode / policy / boost change
  // (the EC write runs on a background thread).
  useEffect(() => {
    checkApplied();
    const t = setTimeout(checkApplied, 1500);
    return () => clearTimeout(t);
  }, [checkApplied, status.power_mode, status.fan_boost, info?.policy]);

  const ecTable = apply?.ec ?? null;

  const activeMode = status.power_mode;

  return (
    <div className="fan-view">
      <div className="glass-card fan-view-header">
        <div className="fan-view-title">
          <div className="fan-view-icon">
            <Fan className="w-4 h-4" />
          </div>
          <div>
            <div className="fan-view-heading">预设模式风扇调度</div>
            <div className="fan-view-sub">
              办公 / 均衡 / 狂暴 切换时写入 EC 的风扇表。自定义模式使用「自定义调校」里的曲线，一键强冷时风扇全速。
            </div>
          </div>
        </div>
        <div className="fan-policy-switch">
          <button
            className={`toggle-btn ${info?.policy === 'tuned' ? 'active-indigo' : ''}`}
            disabled={busy || !info}
            onClick={() => setPolicy('tuned')}
            title="按模式分别调校：办公更安静，狂暴更早拉满"
          >
            分模式调校
          </button>
          <button
            className={`toggle-btn ${info?.policy === 'official' ? 'active-indigo' : ''}`}
            disabled={busy || !info}
            onClick={() => setPolicy('official')}
            title="官方控制中心的 EC 默认表（三个模式完全相同）"
          >
            官方原厂
          </button>
        </div>
      </div>

      {error && <div className="fan-view-error">{error}</div>}

      <div className="fan-mode-grid">
        {(info?.modes ?? []).map((m) => {
          const meta = MODE_META[m.mode];
          const start = m.cpu[0]?.temp;
          const full = m.cpu.find((p) => p.duty >= 100)?.temp;
          return (
            <div key={m.mode} className={`glass-card fan-mode-card ${meta.cls} ${activeMode === m.mode ? 'active' : ''}`}>
              <div className="fan-mode-head">
                <span className="fan-mode-name">{meta.name}</span>
                {activeMode === m.mode && <span className="fan-mode-live">当前</span>}
                <span className="fan-mode-power">{meta.power}</span>
              </div>
              <CurveChart curves={m} official={info?.policy === 'tuned' ? info.official : undefined} />
              <div className="fan-mode-stats">
                <span>起转 {start}°C</span>
                <span>70°C → {dutyAt(m.cpu, 70)}%</span>
                <span>满速 {full ?? '—'}°C</span>
              </div>
            </div>
          );
        })}
      </div>

      <div className="fan-legend">
        <span className="fan-legend-item cpu">CPU 风扇</span>
        <span className="fan-legend-item gpu">GPU 风扇</span>
        {info?.policy === 'tuned' && <span className="fan-legend-item official">官方原厂（对照）</span>}
      </div>

      <div className="glass-card fan-ec-card">
        <div className="fan-ec-head">
          <div>
            <div className="fan-view-heading">
              EC 当前生效风扇表
              {apply?.state === 'applied' && (
                <span className="fan-apply-badge ok">
                  <CheckCircle2 className="w-3.5 h-3.5" />已应用
                </span>
              )}
              {apply?.state === 'mismatch' && (
                <span className="fan-apply-badge bad">
                  <AlertTriangle className="w-3.5 h-3.5" />与预期不一致
                </span>
              )}
              {apply?.state === 'fan_boost' && (
                <span className="fan-apply-badge info">
                  <Wind className="w-3.5 h-3.5" />一键强冷中
                </span>
              )}
              {apply?.state === 'unreadable' && <span className="fan-apply-badge bad">无法读取 EC</span>}
            </div>
            <div className="fan-view-sub">
              {apply ? `预期：${apply.source}。` : ''}进入本页和切换模式后自动读取主板 EC 并比对。
            </div>
          </div>
          <button className="toggle-btn" onClick={checkApplied} disabled={checking}>
            <RefreshCw className={`w-3.5 h-3.5 ${checking ? 'spin-icon' : ''}`} />
            <span>刷新</span>
          </button>
        </div>
        {ecTable ? (
          <table className="fan-ec-table">
            <tbody>
              {(
                [
                  ['CPU 温度', ecTable.cpu_upt],
                  ['CPU 转速 %', ecTable.cpu_duty_pct],
                  ['GPU 温度', ecTable.gpu_upt],
                  ['GPU 转速 %', ecTable.gpu_duty_pct],
                ] as [string, number[]][]
              ).map(([label, row]) => (
                <tr key={label}>
                  <th>{label}</th>
                  {row.slice(0, 13).map((v, i) => (
                    <td key={i}>{v === 255 ? '—' : v}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="fan-ec-empty">{checking ? '正在读取…' : '暂无数据'}</div>
        )}
      </div>
    </div>
  );
};
