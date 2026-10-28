import React from 'react';

/**
 * Temp-wall hint for this machine. Replaces upstream's "advanced BIOS OC menu" toggle,
 * which wrote UEFI NVRAM variables (the hidden BIOS menus are already unlocked here).
 */
export const CpuTempWallHint: React.FC = () => (
  <div className="tuning-sub-card cpu-tempwall-hint">
    <span className="cpu-tempwall-hint-title">温度墙说明</span>
    <div className="cpu-tempwall-hint-text">
      温度墙按 TjMax 100°C 换算成 TCC Offset 写入 EC。ThrottleStop 里若设置了 TCC Offset 会覆盖这里的值，两边只保留一处。
    </div>
  </div>
);
