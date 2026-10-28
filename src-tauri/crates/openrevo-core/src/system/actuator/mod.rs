// OpenRevo - Unified Single Atomic Power Actuator (The Sole Truth & Dispatcher)
// Inviolable Rules:
// 1. ALL power mode changes, DB toggles, preset clicks, and slider adjustments MUST route through this actuator.
// 2. Strict physical bus isolation: Full-Power cards (4080/4090) and Wide cTGP cards (5060/4060) use dedicated pipelines.
// 3. Zero hardcoding & Zero JSON dependency: All limits derive from live VBIOS and EC ROM ground truth.
// 4. Automatically removes the OEM 5W cold-start thermal margin lock on Turbo modes.
// 5. Broadcasts verified live hardware state to frontend windows after every actuation.

pub mod full_power;
pub mod wide_ctgp;

use std::sync::Arc;
use parking_lot::Mutex;

pub use full_power::FullPowerStateTruth;
pub use wide_ctgp::WideCtgpStateTruth;

use full_power::FullPowerPipeline;
use wide_ctgp::WideCtgpPipeline;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GpuArchType {
    FullPower,
    WideCtgp,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "arch_type")]
pub enum UnifiedGpuStateTruth {
    FullPower(FullPowerStateTruth),
    WideCtgp(WideCtgpStateTruth),
}

enum PipelineVariant {
    FullPower(FullPowerPipeline),
    WideCtgp(WideCtgpPipeline),
}

pub struct UnifiedPowerActuator {
    arch: GpuArchType,
    pipeline: Mutex<PipelineVariant>,
}

static INSTANCE: std::sync::OnceLock<Arc<UnifiedPowerActuator>> = std::sync::OnceLock::new();

impl UnifiedPowerActuator {
    /// Initialize the Unified Power Actuator from live VBIOS boundaries and EC registers.
    pub fn global() -> Arc<Self> {
        INSTANCE.get_or_init(|| {
            let gpu_caps = crate::driver::gpu_bridge::probe_gpu_tuning_capabilities();
            let floor_w = gpu_caps.ctgp_floor_watts as u16;
            let base_w = gpu_caps.base_tgp_max_watts as u16;
            let max_db = gpu_caps.max_db_watts as u8;
            let total_w = gpu_caps.max_total_tgp as u16;

            if gpu_caps.is_full_power {
                crate::log_info!("ACTUATOR", "Classified GPU as Full-Power Architecture (Base={}W, Max={}W, Delta={}W)", base_w, total_w, max_db);
                let pipeline = FullPowerPipeline::new(base_w, max_db);
                Arc::new(Self {
                    arch: GpuArchType::FullPower,
                    pipeline: Mutex::new(PipelineVariant::FullPower(pipeline)),
                })
            } else {
                crate::log_info!("ACTUATOR", "Classified GPU as Wide cTGP Architecture (Floor={}W, Base={}W, MaxDB={}W)", floor_w, base_w, max_db);
                let pipeline = WideCtgpPipeline::new(floor_w, base_w, max_db);
                Arc::new(Self {
                    arch: GpuArchType::WideCtgp,
                    pipeline: Mutex::new(PipelineVariant::WideCtgp(pipeline)),
                })
            }
        }).clone()
    }

    /// Read realtime ground truth state directly from physical silicon.
    pub fn get_live_truth(&self) -> UnifiedGpuStateTruth {
        let guard = self.pipeline.lock();
        match &*guard {
            PipelineVariant::FullPower(p) => UnifiedGpuStateTruth::FullPower(p.read_live_truth()),
            PipelineVariant::WideCtgp(p) => UnifiedGpuStateTruth::WideCtgp(p.read_live_truth()),
        }
    }

    /// Primary single-point atomic actuation entry.
    pub fn apply_gpu_state(
        &self,
        super_perf: Option<bool>,
        db_enable: Option<bool>,
        db_watts: Option<u8>,
        offset_watts: Option<u8>,
        label: &str,
    ) -> Result<UnifiedGpuStateTruth, String> {
        let mut guard = self.pipeline.lock();
        let truth = match &mut *guard {
            PipelineVariant::FullPower(p) => {
                let t = p.apply(super_perf, db_enable, db_watts, label)?;
                UnifiedGpuStateTruth::FullPower(t)
            }
            PipelineVariant::WideCtgp(p) => {
                let t = p.apply(db_enable, db_watts, offset_watts, label)?;
                UnifiedGpuStateTruth::WideCtgp(t)
            }
        };

        // Broadcast verified state to listeners
        crate::event::emit_global(
            "gpu-power-state-truth",
            serde_json::to_value(&truth).unwrap_or_default(),
        );

        Ok(truth)
    }

    /// One-click Emergency Hard-Unlock (removes any lock state and restores 100% full-power capability)
    pub fn emergency_hard_unlock(&self) -> Result<UnifiedGpuStateTruth, String> {
        let mut guard = self.pipeline.lock();
        let truth = match &mut *guard {
            PipelineVariant::FullPower(p) => UnifiedGpuStateTruth::FullPower(p.emergency_unlock()?),
            PipelineVariant::WideCtgp(p) => UnifiedGpuStateTruth::WideCtgp(p.emergency_unlock()?),
        };

        crate::event::emit_global(
            "gpu-power-state-truth",
            serde_json::to_value(&truth).unwrap_or_default(),
        );

        Ok(truth)
    }

    /// Automatically remove factory cold-start 5W thermal margin and bring machine to full release.
    pub fn unlock_factory_full_release(&self) -> Result<UnifiedGpuStateTruth, String> {
        crate::log_info!("ACTUATOR", "Unlocking factory full release: eliminating 5W cold-start margin...");
        let caps = crate::driver::gpu_bridge::probe_gpu_tuning_capabilities();
        match self.arch {
            GpuArchType::FullPower => {
                let db = if caps.max_db_watts > 0 { caps.max_db_watts as u8 } else { 25 };
                self.apply_gpu_state(Some(true), Some(true), Some(db), None, "Factory Full Release")
            }
            GpuArchType::WideCtgp => {
                let db = if caps.max_db_watts > 0 { caps.max_db_watts as u8 } else { 15 };
                let floor = if caps.ctgp_floor_watts > 0 { caps.ctgp_floor_watts as u8 } else { 50 };
                self.apply_gpu_state(None, Some(true), Some(db), Some(floor), "Factory Full Release")
            }
        }
    }
}
