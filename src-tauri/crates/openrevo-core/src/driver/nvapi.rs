// Native Pure-Rust NVIDIA NVAPI Driver Interface
// Zero official Mechrevo DLL dependencies. Dynamically links to Windows C:\Windows\System32\nvapi64.dll.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use parking_lot::Mutex;

const NVAPI_OK: i32 = 0;

// Magic IDs for nvapi_QueryInterface (reverse-engineered & verified on physical silicon)
const ID_NVAPI_INITIALIZE: u32 = 0x0150E828;
const ID_NVAPI_UNLOAD: u32 = 0xD22BDD7E;
const ID_NVAPI_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC921F;
const ID_NVAPI_GPU_GET_FULL_NAME: u32 = 0xCEEE8E9F;
const ID_NVAPI_GPU_GET_PSTATES20: u32 = 0x6FF81213;
const ID_NVAPI_GPU_SET_PSTATES20: u32 = 0x0F4DAE6B;
const ID_NVAPI_GPU_SET_THERMAL_POLICIES: u32 = 0x34C0B13D;
const ID_NVAPI_GPU_GET_THERMAL_POLICIES: u32 = 0xE3640A56;

// Structure definitions for PStates20 v1 and v2
// Verified on physical silicon:
// - V1: Size 7316 bytes (0x1C94), Version 0x00011C94 (Strictly required by NVIDIA for Memory Domain 4)
// - V2: Size 7416 bytes (0x1CF8), Version 0x00021CF8 (Required by NVIDIA for Graphics Core Domain 0)
pub const PSTATES20_SIZE_V1: usize = 7316;
pub const PSTATES20_VER1: u32 = 0x00011C94;
pub const PSTATES20_SIZE_V2: usize = 7416;
pub const PSTATES20_VER2: u32 = 0x00021CF8;

pub const CLOCK_DOMAIN_GRAPHICS: u32 = 0; // GPU Core Clock
pub const CLOCK_DOMAIN_MEMORY: u32 = 4;   // VRAM Clock

type FnQueryInterface = unsafe extern "C" fn(u32) -> *const ();
type FnInitialize = unsafe extern "C" fn() -> i32;
type FnUnload = unsafe extern "C" fn() -> i32;
type FnEnumPhysicalGPUs = unsafe extern "C" fn(*mut usize, *mut u32) -> i32;
type FnGPUGetFullName = unsafe extern "C" fn(usize, *mut u8) -> i32;
type FnGPUGetPstates20 = unsafe extern "C" fn(usize, *mut u8) -> i32;
type FnGPUSetPstates20 = unsafe extern "C" fn(usize, *const u8) -> i32;
type FnGPUSetThermalPolicies = unsafe extern "C" fn(usize, *const u8) -> i32;
type FnGPUGetThermalPolicies = unsafe extern "C" fn(usize, *mut u8) -> i32;

struct NvApiBindings {
    _lib: libloading::Library,
    _initialize: FnInitialize,
    _unload: FnUnload,
    _enum_physical_gpus: FnEnumPhysicalGPUs,
    get_full_name: FnGPUGetFullName,
    get_pstates20: FnGPUGetPstates20,
    set_pstates20: FnGPUSetPstates20,
    set_thermal_policies: Option<FnGPUSetThermalPolicies>,
    _get_thermal_policies: Option<FnGPUGetThermalPolicies>,
    primary_gpu_handle: usize,
}

static NVAPI: OnceLock<Option<NvApiBindings>> = OnceLock::new();
static NVAPI_MUTEX: Mutex<()> = Mutex::new(());
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn load_nvapi() -> Option<NvApiBindings> {
    unsafe {
        // Look in System32
        let lib = libloading::Library::new("nvapi64.dll").ok()?;
        let query_interface: libloading::Symbol<FnQueryInterface> = lib.get(b"nvapi_QueryInterface\0").ok()?;

        let get_proc = |id: u32| -> Option<*const ()> {
            let ptr = query_interface(id);
            if ptr.is_null() {
                None
            } else {
                Some(ptr)
            }
        };

        let init_ptr = get_proc(ID_NVAPI_INITIALIZE)?;
        let unload_ptr = get_proc(ID_NVAPI_UNLOAD)?;
        let enum_ptr = get_proc(ID_NVAPI_ENUM_PHYSICAL_GPUS)?;
        let name_ptr = get_proc(ID_NVAPI_GPU_GET_FULL_NAME)?;
        let get_pstates_ptr = get_proc(ID_NVAPI_GPU_GET_PSTATES20)?;
        let set_pstates_ptr = get_proc(ID_NVAPI_GPU_SET_PSTATES20)?;

        let initialize: FnInitialize = std::mem::transmute(init_ptr);
        let unload: FnUnload = std::mem::transmute(unload_ptr);
        let enum_physical_gpus: FnEnumPhysicalGPUs = std::mem::transmute(enum_ptr);
        let get_full_name: FnGPUGetFullName = std::mem::transmute(name_ptr);
        let get_pstates20: FnGPUGetPstates20 = std::mem::transmute(get_pstates_ptr);
        let set_pstates20: FnGPUSetPstates20 = std::mem::transmute(set_pstates_ptr);

        let set_thermal_policies = get_proc(ID_NVAPI_GPU_SET_THERMAL_POLICIES)
            .map(|p| std::mem::transmute(p));
        let get_thermal_policies = get_proc(ID_NVAPI_GPU_GET_THERMAL_POLICIES)
            .map(|p| std::mem::transmute(p));

        let ret = initialize();
        if ret != NVAPI_OK {
            crate::log_warn!("NVAPI", "NvAPI_Initialize returned error code: {}", ret);
            return None;
        }

        let mut handles = [0usize; 64];
        let mut count = 0u32;
        let enum_ret = enum_physical_gpus(handles.as_mut_ptr(), &mut count);
        if enum_ret != NVAPI_OK || count == 0 {
            crate::log_warn!("NVAPI", "NvAPI_EnumPhysicalGPUs found 0 GPUs or returned {}", enum_ret);
            return None;
        }

        let primary_gpu_handle = handles[0];
        let mut name_buf = [0u8; 64];
        if get_full_name(primary_gpu_handle, name_buf.as_mut_ptr()) == NVAPI_OK {
            let name = String::from_utf8_lossy(&name_buf)
                .trim_matches(char::from(0))
                .to_string();
            crate::log_info!("NVAPI", "Native NVAPI initialized successfully for GPU: {} (handle: 0x{:X})", name, primary_gpu_handle);
        }

        IS_INITIALIZED.store(true, Ordering::SeqCst);

        Some(NvApiBindings {
            _lib: lib,
            _initialize: initialize,
            _unload: unload,
            _enum_physical_gpus: enum_physical_gpus,
            get_full_name,
            get_pstates20,
            set_pstates20,
            set_thermal_policies,
            _get_thermal_policies: get_thermal_policies,
            primary_gpu_handle,
        })
    }
}

fn get_driver() -> Option<&'static NvApiBindings> {
    NVAPI.get_or_init(load_nvapi).as_ref()
}

/// Query whether NVAPI is available on the current machine
pub fn is_nvapi_available() -> bool {
    get_driver().is_some()
}

/// Retrieve the GPU model name directly from NVIDIA driver
pub fn get_gpu_full_name() -> Option<String> {
    let drv = get_driver()?;
    let _guard = NVAPI_MUTEX.lock();
    let mut name_buf = [0u8; 64];
    unsafe {
        if (drv.get_full_name)(drv.primary_gpu_handle, name_buf.as_mut_ptr()) == NVAPI_OK {
            let name = String::from_utf8_lossy(&name_buf)
                .trim_matches(char::from(0))
                .to_string();
            Some(name)
        } else {
            None
        }
    }
}

/// Apply a single clock offset (Core or Memory) via NvAPI_GPU_SetPstates20
/// domain_id: 0 = Graphics (Core), 4 = Memory
/// delta_mhz: Frequency offset in MHz (-1000 ~ +1000)
unsafe fn set_single_clock_offset_raw(
    drv: &NvApiBindings,
    domain_id: u32,
    delta_mhz: i32,
) -> Result<(), i32> {
    // Memory uses V1 (7316 bytes, 0x00011C94); Graphics uses V2 (7416 bytes, 0x00021CF8)
    let (buf_size, ver_magic) = if domain_id == CLOCK_DOMAIN_MEMORY {
        (PSTATES20_SIZE_V1, PSTATES20_VER1)
    } else {
        (PSTATES20_SIZE_V2, PSTATES20_VER2)
    };

    let mut buf = vec![0u8; buf_size];
    let u32_slice = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u32, buf_size / 4);
    let i32_slice = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut i32, buf_size / 4);

    u32_slice[0] = ver_magic;
    u32_slice[2] = 1;                     // numPstates = 1 (P0 only)
    u32_slice[3] = 1;                     // numClocks = 1
    u32_slice[7] = domain_id;             // domainId: 0 for Graphics, 4 for Memory
    i32_slice[10] = delta_mhz * 1000;     // freqDelta_kHz = MHz * 1000

    let ret = (drv.set_pstates20)(drv.primary_gpu_handle, buf.as_ptr());
    if ret == NVAPI_OK {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Apply GPU Core Clock and Memory Clock offsets directly to NVIDIA hardware
/// core_offset_mhz: Offset in MHz (e.g. +100 MHz, or 0 to reset)
/// mem_offset_mhz:  Offset in MHz (e.g. +200 MHz, or 0 to reset)
pub fn set_gpu_clock_offsets(core_offset_mhz: i32, mem_offset_mhz: i32) -> Result<(), String> {
    let drv = get_driver().ok_or_else(|| "NVIDIA NVAPI driver not found or not supported".to_string())?;
    let _guard = NVAPI_MUTEX.lock();

    unsafe {
        // 1. Set Core Clock Offset (Graphics Core, Domain 0)
        if let Err(code) = set_single_clock_offset_raw(drv, CLOCK_DOMAIN_GRAPHICS, core_offset_mhz) {
            crate::log_warn!("NVAPI", "Set Core Clock Offset ({} MHz) failed with NVAPI code: {}", core_offset_mhz, code);
            return Err(format!("NVAPI Set Core Clock failed: error code {}", code));
        }

        // 2. Set Memory Clock Offset (VRAM Memory, Domain 4)
        if let Err(code) = set_single_clock_offset_raw(drv, CLOCK_DOMAIN_MEMORY, mem_offset_mhz) {
            crate::log_warn!("NVAPI", "Set Memory Clock Offset ({} MHz) failed with NVAPI code: {}", mem_offset_mhz, code);
            return Err(format!("NVAPI Set Memory Clock failed: error code {}", code));
        }

        crate::log_info!(
            "NVAPI",
            "GPU Clock Offsets applied successfully: Core={:+>3} MHz, Memory={:+>4} MHz",
            core_offset_mhz, mem_offset_mhz
        );
        Ok(())
    }
}

/// Reset all GPU Clock Offsets to factory default (+0 MHz)
pub fn reset_gpu_clock_offsets() -> Result<(), String> {
    set_gpu_clock_offsets(0, 0)
}

/// Read current clock offsets from the GPU driver
pub fn get_gpu_clock_offsets() -> Option<(i32, i32)> {
    let drv = get_driver()?;
    let _guard = NVAPI_MUTEX.lock();

    let mut buf = vec![0u8; PSTATES20_SIZE_V1];
    let u32_slice = unsafe {
        std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u32, PSTATES20_SIZE_V1 / 4)
    };
    u32_slice[0] = PSTATES20_VER1;

    unsafe {
        let ret = (drv.get_pstates20)(drv.primary_gpu_handle, buf.as_mut_ptr());
        if ret != NVAPI_OK {
            return None;
        }

        let i32_slice = std::slice::from_raw_parts(buf.as_ptr() as *const i32, PSTATES20_SIZE_V1 / 4);
        // Extract P0 Graphics (offset 0x28 -> i32[10]) and Memory (offset 0x54 -> i32[21]) delta in kHz
        let core_khz = i32_slice[10];
        let mem_khz = i32_slice[21];
        Some((core_khz / 1000, mem_khz / 1000))
    }
}

/// Set GPU Thermal Limit / Temperature Wall in Celsius (e.g. 87°C)
pub fn set_gpu_thermal_limit(celsius: u32) -> Result<(), String> {
    let drv = get_driver().ok_or_else(|| "NVIDIA NVAPI driver not found".to_string())?;
    let set_thermal = drv.set_thermal_policies.ok_or_else(|| "Thermal policies not supported".to_string())?;
    let _guard = NVAPI_MUTEX.lock();

    // Struct: size 780 bytes (0x030C), version 0x0002030C
    let mut buf = vec![0u8; 780];
    let u32_slice = unsafe {
        std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u32, 780 / 4)
    };
    u32_slice[0] = 0x0002030C; // Version 2
    u32_slice[2] = 1;          // numPolicies = 1
    u32_slice[3] = 6;          // Target: GPU
    u32_slice[5] = celsius.clamp(65, 95); // Thermal limit Celsius

    unsafe {
        let ret = set_thermal(drv.primary_gpu_handle, buf.as_ptr());
        if ret == NVAPI_OK {
            crate::log_info!("NVAPI", "GPU Thermal limit set to {}°C", celsius);
            Ok(())
        } else {
            Err(format!("NVAPI Set Thermal Policies failed: error code {}", ret))
        }
    }
}
