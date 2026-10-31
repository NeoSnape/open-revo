// In-process NVIDIA telemetry through the driver's nvml.dll (System32).
// Replaces spawning nvidia-smi.exe every 1.5 s; falls back to nvidia-smi when NVML is unavailable.

use libloading::Library;
use std::ffi::c_void;
use std::sync::OnceLock;

type NvmlReturn = i32;
type Device = *mut c_void;

#[repr(C)]
struct Utilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

const NVML_SUCCESS: NvmlReturn = 0;
const NVML_TEMPERATURE_GPU: u32 = 0;
const NVML_CLOCK_GRAPHICS: u32 = 0;
const NVML_CLOCK_MEM: u32 = 2;

struct Nvml {
    _lib: Library,
    device: Device,
    get_temperature: unsafe extern "C" fn(Device, u32, *mut u32) -> NvmlReturn,
    get_power_usage: unsafe extern "C" fn(Device, *mut u32) -> NvmlReturn,
    get_utilization: unsafe extern "C" fn(Device, *mut Utilization) -> NvmlReturn,
    get_clock: unsafe extern "C" fn(Device, u32, *mut u32) -> NvmlReturn,
    get_memory: unsafe extern "C" fn(Device, *mut Memory) -> NvmlReturn,
    get_enforced_limit: Option<unsafe extern "C" fn(Device, *mut u32) -> NvmlReturn>,
    get_throttle_reasons: Option<unsafe extern "C" fn(Device, *mut u64) -> NvmlReturn>,
}

// The NVML device handle is process-global and NVML is thread-safe.
unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSample {
    pub temp_c: u32,
    pub power_w: f32,
    pub usage_pct: u32,
    pub core_mhz: u32,
    pub mem_mhz: u32,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
    /// Enforced board power limit (W), 0 if unknown.
    pub power_limit_w: f32,
    /// NVML clocks throttle reason bitmask.
    pub throttle_reasons: u64,
}

/// Short label for what is currently holding the GPU clock down ("" when nothing / idle).
pub fn throttle_label(reasons: u64) -> &'static str {
    const SW_POWER_CAP: u64 = 0x4;
    const HW_SLOWDOWN: u64 = 0x8;
    const SW_THERMAL: u64 = 0x20;
    const HW_THERMAL: u64 = 0x40;
    const HW_POWER_BRAKE: u64 = 0x80;
    if reasons & (SW_THERMAL | HW_THERMAL) != 0 {
        "THERM"
    } else if reasons & (HW_SLOWDOWN | HW_POWER_BRAKE) != 0 {
        "HWSLOW"
    } else if reasons & SW_POWER_CAP != 0 {
        "PWR"
    } else {
        ""
    }
}

static LAST: parking_lot::Mutex<Option<GpuSample>> = parking_lot::Mutex::new(None);

/// Most recent sample taken by the GPU poller.
pub fn last_sample() -> Option<GpuSample> {
    *LAST.lock()
}

fn load() -> Option<Nvml> {
    unsafe {
        let lib = Library::new("nvml.dll").ok()?;
        let init = *lib.get::<unsafe extern "C" fn() -> NvmlReturn>(b"nvmlInit_v2\0").ok()?;
        if init() != NVML_SUCCESS {
            return None;
        }
        let by_index = *lib
            .get::<unsafe extern "C" fn(u32, *mut Device) -> NvmlReturn>(b"nvmlDeviceGetHandleByIndex_v2\0")
            .ok()?;
        let mut device: Device = std::ptr::null_mut();
        if by_index(0, &mut device) != NVML_SUCCESS || device.is_null() {
            return None;
        }
        let nvml = Nvml {
            device,
            get_temperature: *lib.get(b"nvmlDeviceGetTemperature\0").ok()?,
            get_power_usage: *lib.get(b"nvmlDeviceGetPowerUsage\0").ok()?,
            get_utilization: *lib.get(b"nvmlDeviceGetUtilizationRates\0").ok()?,
            get_clock: *lib.get(b"nvmlDeviceGetClockInfo\0").ok()?,
            get_memory: *lib.get(b"nvmlDeviceGetMemoryInfo\0").ok()?,
            get_enforced_limit: lib.get(b"nvmlDeviceGetEnforcedPowerLimit\0").ok().map(|f| *f),
            get_throttle_reasons: lib.get(b"nvmlDeviceGetCurrentClocksThrottleReasons\0").ok().map(|f| *f),
            _lib: lib,
        };
        crate::log_info!("GPU_BRIDGE", "NVML loaded: in-process GPU telemetry active (nvidia-smi polling disabled)");
        Some(nvml)
    }
}

fn instance() -> Option<&'static Nvml> {
    static NVML: OnceLock<Option<Nvml>> = OnceLock::new();
    NVML.get_or_init(load).as_ref()
}

pub fn is_available() -> bool {
    instance().is_some()
}

/// One telemetry sample; None if NVML is unavailable or the temperature query failed.
pub fn sample() -> Option<GpuSample> {
    let n = instance()?;
    let mut s = GpuSample::default();
    unsafe {
        if (n.get_temperature)(n.device, NVML_TEMPERATURE_GPU, &mut s.temp_c) != NVML_SUCCESS {
            return None;
        }
        let mut mw = 0u32;
        if (n.get_power_usage)(n.device, &mut mw) == NVML_SUCCESS {
            s.power_w = mw as f32 / 1000.0;
        }
        let mut u = Utilization { gpu: 0, memory: 0 };
        if (n.get_utilization)(n.device, &mut u) == NVML_SUCCESS {
            s.usage_pct = u.gpu;
        }
        let _ = (n.get_clock)(n.device, NVML_CLOCK_GRAPHICS, &mut s.core_mhz);
        let _ = (n.get_clock)(n.device, NVML_CLOCK_MEM, &mut s.mem_mhz);
        let mut m = Memory { total: 0, free: 0, used: 0 };
        if (n.get_memory)(n.device, &mut m) == NVML_SUCCESS {
            s.vram_total_mb = m.total / (1024 * 1024);
            s.vram_used_mb = m.used / (1024 * 1024);
        }
        if let Some(f) = n.get_enforced_limit {
            let mut mw = 0u32;
            if f(n.device, &mut mw) == NVML_SUCCESS {
                s.power_limit_w = mw as f32 / 1000.0;
            }
        }
        if let Some(f) = n.get_throttle_reasons {
            let _ = f(n.device, &mut s.throttle_reasons);
        }
    }
    *LAST.lock() = Some(s);
    Some(s)
}
