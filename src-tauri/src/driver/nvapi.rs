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

// Use u32 storage for the C structure's alignment, without overlapping
// mutable u32/i32 slices. Casting a signed delta to u32 preserves its bits.
fn clock_offset_buffer(domain_id: u32, delta_mhz: i32) -> Vec<u32> {
    // Preserve the existing V1 memory / V2 graphics write protocol.
    let (size, version) = if domain_id == CLOCK_DOMAIN_MEMORY {
        (PSTATES20_SIZE_V1, PSTATES20_VER1)
    } else {
        (PSTATES20_SIZE_V2, PSTATES20_VER2)
    };
    let mut words = vec![0u32; size / 4];
    words[0] = version;
    words[2] = 1; // numPstates = 1 (P0)
    words[3] = 1; // numClocks = 1
    words[7] = domain_id;
    words[10] = (delta_mhz * 1000) as u32;
    words
}

/// P0 clock-domain allowed offset ranges reported by the driver/VBIOS `ParamDelta` (kHz).
/// Layout per clock entry (44 bytes = 11 u32 words):
///   `[0] DomainId, [1] ClockType, [2] IsEditable, [3] DeltaValue, [4] RangeMin, [5] RangeMax, ...`
/// Rounding tightens both ends (ceil lower bound, floor upper bound) so we never accept an out-of-range offset.
fn parse_pstates20_offset_ranges(words: &[u32]) -> Option<((i32, i32), (i32, i32))> {
    const MAX_PSTATES: usize = 16;
    const MAX_CLOCKS: usize = 8;
    const PSTATE_WORDS: usize = 456 / 4;
    const CLOCK_WORDS: usize = 44 / 4;

    if words.len() != PSTATES20_SIZE_V2 / 4 || words[0] != PSTATES20_VER2 {
        return None;
    }
    let num_pstates = words[2] as usize;
    let num_clocks = words[3] as usize;
    if !(1..=MAX_PSTATES).contains(&num_pstates) || !(1..=MAX_CLOCKS).contains(&num_clocks) {
        return None;
    }

    let ceil_mhz = |khz: i32| -> i32 { (i64::from(khz) + 999).div_euclid(1000) as i32 };
    let floor_mhz = |khz: i32| -> i32 { i64::from(khz).div_euclid(1000) as i32 };

    let mut result = None;
    for pstate in 0..num_pstates {
        let state_start = 5 + pstate * PSTATE_WORDS;
        if words[state_start] != 0 {
            continue; // NVAPI_GPU_PERF_PSTATE_P0 only
        }
        if result.is_some() {
            return None; // Ambiguous P0 entries
        }

        let mut core: Option<(i32, i32)> = None;
        let mut memory: Option<(i32, i32)> = None;
        for clock in 0..num_clocks {
            let start = state_start + 2 + clock * CLOCK_WORDS;
            let slot = match words[start] {
                CLOCK_DOMAIN_GRAPHICS => &mut core,
                CLOCK_DOMAIN_MEMORY => &mut memory,
                _ => continue,
            };
            let min = ceil_mhz(words[start + 4] as i32);
            let max = floor_mhz(words[start + 5] as i32);
            if slot.replace((min, max)).is_some() {
                return None; // Duplicate requested clock domain
            }
        }

        let (core_min, core_max) = core?;
        let (mem_min, mem_max) = memory?;
        if core_max < core_min || mem_max < mem_min {
            return None;
        }
        result = Some(((core_min, core_max), (mem_min, mem_max)));
    }
    result
}

/// Conservative fallback offset bounds (MHz) when the driver does not answer V2 range queries.
pub const GPU_OC_CORE_OFFSET_FALLBACK_MHZ: (i32, i32) = (-500, 500);
pub const GPU_OC_MEM_OFFSET_FALLBACK_MHZ: (i32, i32) = (-1000, 3000);

/// Query driver/VBIOS allowed P0 core and memory clock offset ranges (in MHz).
pub fn get_gpu_clock_offset_ranges_mhz() -> Option<((i32, i32), (i32, i32))> {
    let drv = get_driver()?;
    let _guard = NVAPI_MUTEX.lock();

    let mut words = vec![0u32; PSTATES20_SIZE_V2 / 4];
    words[0] = PSTATES20_VER2;
    let ret = unsafe { (drv.get_pstates20)(drv.primary_gpu_handle, words.as_mut_ptr().cast()) };
    if ret != NVAPI_OK {
        crate::log_warn!("NVAPI", "GetPstates20(V2) for offset ranges failed: code {}", ret);
        return None;
    }
    parse_pstates20_offset_ranges(&words)
}

/// Query driver/VBIOS allowed P0 clock offset ranges (MHz), falling back to conservative bounds.
pub fn gpu_clock_offset_ranges_with_fallback_mhz() -> ((i32, i32), (i32, i32)) {
    get_gpu_clock_offset_ranges_mhz().unwrap_or((
        GPU_OC_CORE_OFFSET_FALLBACK_MHZ,
        GPU_OC_MEM_OFFSET_FALLBACK_MHZ,
    ))
}

/// Apply a single clock offset (Core or Memory) via NvAPI_GPU_SetPstates20
/// domain_id: 0 = Graphics (Core), 4 = Memory
/// delta_mhz: Frequency offset in MHz (-1000 ~ +1000)
unsafe fn set_single_clock_offset_raw(
    drv: &NvApiBindings,
    domain_id: u32,
    delta_mhz: i32,
) -> Result<(), i32> {
    let words = clock_offset_buffer(domain_id, delta_mhz);
    let ret = (drv.set_pstates20)(drv.primary_gpu_handle, words.as_ptr().cast());
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

// NV_GPU_PERF_PSTATES20_INFO_V1 in NVIDIA's nvapi.h has a 20-byte
// header and 16 P-states. Each P-state is 456 bytes: 8 bytes of ID/flags,
// eight 44-byte clocks, and four 24-byte base voltages. All fields are 32-bit.
fn parse_pstates20_clock_offsets(words: &[u32]) -> Option<(i32, i32)> {
    const MAX_PSTATES: usize = 16;
    const MAX_CLOCKS: usize = 8;
    const PSTATE_WORDS: usize = 456 / 4;
    const CLOCK_WORDS: usize = 44 / 4;

    if words.len() != PSTATES20_SIZE_V1 / 4 || words[0] != PSTATES20_VER1 {
        return None;
    }
    let num_pstates = words[2] as usize;
    let num_clocks = words[3] as usize;
    if !(1..=MAX_PSTATES).contains(&num_pstates) || !(1..=MAX_CLOCKS).contains(&num_clocks) {
        return None;
    }

    let mut offsets = None;
    for pstate in 0..num_pstates {
        let state_start = 5 + pstate * PSTATE_WORDS;
        if words[state_start] != 0 {
            continue; // NVAPI_GPU_PERF_PSTATE_P0
        }
        if offsets.is_some() {
            return None; // Ambiguous P0 entries.
        }
        let mut core = None;
        let mut memory = None;
        for clock in 0..num_clocks {
            let clock_start = state_start + 2 + clock * CLOCK_WORDS;
            let target = match words[clock_start] {
                CLOCK_DOMAIN_GRAPHICS => &mut core,
                CLOCK_DOMAIN_MEMORY => &mut memory,
                _ => continue,
            };
            // Preserve signed kHz-to-MHz truncation from the public getter.
            let mhz = (words[clock_start + 3] as i32) / 1000;
            if target.replace(mhz).is_some() {
                return None; // Duplicate requested clock domain.
            }
        }
        offsets = Some((core?, memory?));
    }
    offsets
}

/// Read P0 graphics and memory offsets from the GPU driver, in MHz.
pub fn get_gpu_clock_offsets() -> Option<(i32, i32)> {
    let drv = get_driver()?;
    let _guard = NVAPI_MUTEX.lock();

    let mut words = vec![0u32; PSTATES20_SIZE_V1 / 4];
    words[0] = PSTATES20_VER1;
    let ret = unsafe { (drv.get_pstates20)(drv.primary_gpu_handle, words.as_mut_ptr().cast()) };
    if ret != NVAPI_OK {
        return None;
    }
    parse_pstates20_clock_offsets(&words)
}

/// Set GPU Thermal Limit / Temperature Wall in Celsius (e.g. 87°C)
pub fn set_gpu_thermal_limit(celsius: u32) -> Result<(), String> {
    let drv = get_driver().ok_or_else(|| "NVIDIA NVAPI driver not found".to_string())?;
    let set_thermal = drv.set_thermal_policies.ok_or_else(|| "Thermal policies not supported".to_string())?;
    let _guard = NVAPI_MUTEX.lock();

    // Struct: size 780 bytes (0x030C), version 0x0002030C
    let mut words = vec![0u32; 780 / 4];
    words[0] = 0x0002030C; // Version 2
    words[2] = 1;          // numPolicies = 1
    words[3] = 6;          // Target: GPU
    words[5] = celsius.clamp(65, 95); // Thermal limit Celsius

    unsafe {
        let ret = set_thermal(drv.primary_gpu_handle, words.as_ptr().cast());
        if ret == NVAPI_OK {
            crate::log_info!("NVAPI", "GPU Thermal limit set to {}°C", celsius);
            Ok(())
        } else {
            Err(format!("NVAPI Set Thermal Policies failed: error code {}", ret))
        }
    }
}

#[cfg(test)]
mod pstates20_tests {
    use super::*;

    fn fixture(states: u32, clocks: u32) -> Vec<u32> {
        let mut words = vec![0; 7316 / 4];
        words[0] = 0x00011c94;
        words[2] = states;
        words[3] = clocks;
        for state in 0..16 {
            words[(20 + state * 456) / 4] = 8;
            for clock in 0..8 {
                words[(28 + state * 456 + clock * 44) / 4] = u32::MAX;
            }
        }
        words
    }

    fn p0(words: &mut [u32], state: usize) {
        words[(20 + state * 456) / 4] = 0;
    }

    fn clock(words: &mut [u32], state: usize, slot: usize, domain: u32, khz: i32) {
        let byte = 28 + state * 456 + slot * 44;
        words[byte / 4] = domain;
        words[(byte + 12) / 4] = khz as u32;
    }

    fn normal() -> Vec<u32> {
        let mut words = fixture(1, 2);
        p0(&mut words, 0);
        clock(&mut words, 0, 0, 0, 105_000);
        clock(&mut words, 0, 1, 4, 500_000);
        words
    }

    #[test]
    fn preserves_ordered_offsets_and_signed_mhz_conversion() {
        let mut words = normal();
        assert_eq!(parse_pstates20_clock_offsets(&words), Some((105, 500)));
        for (khz, mhz) in [
            (0, 0),
            (-105_000, -105),
            (1999, 1),
            (-1999, -1),
            (i32::MIN, -2_147_483),
            (i32::MAX, 2_147_483),
        ] {
            clock(&mut words, 0, 0, 0, khz);
            assert_eq!(parse_pstates20_clock_offsets(&words), Some((mhz, 500)));
        }
    }

    #[test]
    fn finds_p0_and_reads_domains_regardless_of_slot() {
        let mut words = fixture(2, 2);
        clock(&mut words, 0, 0, 0, 0);
        clock(&mut words, 0, 1, 4, 0);
        p0(&mut words, 1);
        clock(&mut words, 1, 0, 4, 500_000);
        clock(&mut words, 1, 1, 0, -105_000);
        assert_eq!(parse_pstates20_clock_offsets(&words), Some((-105, 500)));
    }

    #[test]
    fn rejects_invalid_pstates20_buffers() {
        assert_eq!(parse_pstates20_clock_offsets(&fixture(1, 2)), None);
        let mut dup = normal();
        dup[2] = 2;
        p0(&mut dup, 1);
        clock(&mut dup, 1, 0, 0, 0);
        clock(&mut dup, 1, 1, 4, 0);
        assert_eq!(parse_pstates20_clock_offsets(&dup), None);
        let mut bad_ver = normal();
        bad_ver[0] = PSTATES20_VER2;
        assert_eq!(parse_pstates20_clock_offsets(&bad_ver), None);
    }

    #[test]
    fn aligned_clock_writes_preserve_v1_v2_wire_bytes() {
        for (domain, size, version) in [(0u32, 7416, 0x00021cf8u32), (4, 7316, 0x00011c94)] {
            for mhz in [-1000i32, -1, 0, 1, 1000] {
                let words = clock_offset_buffer(domain, mhz);
                assert_eq!(words.as_ptr() as usize % std::mem::align_of::<u32>(), 0);
                let actual: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
                let mut expected = vec![0u8; size];
                expected[0..4].copy_from_slice(&version.to_le_bytes());
                expected[8..12].copy_from_slice(&1u32.to_le_bytes());
                expected[12..16].copy_from_slice(&1u32.to_le_bytes());
                expected[28..32].copy_from_slice(&domain.to_le_bytes());
                expected[40..44].copy_from_slice(&(mhz * 1000).to_le_bytes());
                assert_eq!(actual, expected);
            }
        }
    }

    fn v2_with_ranges(core_khz: (i32, i32), mem_khz: (i32, i32)) -> Vec<u32> {
        let mut words = vec![0u32; PSTATES20_SIZE_V2 / 4];
        words[0] = PSTATES20_VER2;
        words[2] = 1;
        words[3] = 2;
        words[5] = 0;
        let g = 7;
        words[g] = CLOCK_DOMAIN_GRAPHICS;
        words[g + 4] = core_khz.0 as u32;
        words[g + 5] = core_khz.1 as u32;
        let m = g + 11;
        words[m] = CLOCK_DOMAIN_MEMORY;
        words[m + 4] = mem_khz.0 as u32;
        words[m + 5] = mem_khz.1 as u32;
        words
    }

    #[test]
    fn reads_offset_ranges_per_domain() {
        let words = v2_with_ranges((-1_000_000, 1_000_000), (-1_000_000, 3_000_000));
        assert_eq!(
            parse_pstates20_offset_ranges(&words),
            Some(((-1000, 1000), (-1000, 3000)))
        );
    }

    #[test]
    fn offset_range_rounding_tightens_both_ends() {
        let words = v2_with_ranges((1_500_500, 2_999_999), (-1_500_500, 1_999));
        assert_eq!(
            parse_pstates20_offset_ranges(&words),
            Some(((1501, 2999), (-1500, 1)))
        );
    }
}
