// RivaTuner Statistics Server (RTSS) in-game OSD feed.
// OpenRevo writes its own sensor text into an RTSS OSD slot (shared memory "RTSSSharedMemoryV2",
// protocol as in RTSS SDK RTSSSharedMemorySample::UpdateOSD); RTSS renders it and adds <FR>/<FT>.
// Nothing is injected into games by OpenRevo itself.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS,
};

const OWNER: &str = "OpenRevo";
const RTSS_SIGNATURE: u32 = 0x5254_5353; // 'RTSS'

// RTSS_SHARED_MEMORY header offsets (v2.x)
const OFF_SIGNATURE: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_APP_ENTRY_SIZE: usize = 8;
const OFF_APP_ARR_OFFSET: usize = 12;
const OFF_APP_ARR_SIZE: usize = 16;
const OFF_OSD_ENTRY_SIZE: usize = 20;
const OFF_OSD_ARR_OFFSET: usize = 24;
const OFF_OSD_ARR_SIZE: usize = 28;
const OFF_OSD_FRAME: usize = 32;
const OFF_BUSY: usize = 36; // v2.14+

// RTSS_SHARED_MEMORY_OSD_ENTRY offsets
const OSD_TEXT: usize = 0; // char[256]
const OSD_OWNER: usize = 256; // char[256]
const OSD_TEXT_EX: usize = 512; // char[4096], v2.7+

// RTSS_SHARED_MEMORY_APP_ENTRY offsets
const APP_PID: usize = 0;
const APP_FLAGS: usize = 4 + 260;
const APPFLAG_API_USAGE_MASK: u32 = 0x0000_FFFF;

static SLOT_HELD: AtomicBool = AtomicBool::new(false);

struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Mapping {
    fn open() -> Option<Self> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, w!("RTSSSharedMemoryV2")).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let m = Mapping { handle, view };
            if m.u32_at(OFF_SIGNATURE) != RTSS_SIGNATURE || m.u32_at(OFF_VERSION) < 0x0002_0000 {
                return None;
            }
            Some(m)
        }
    }

    fn base(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }

    fn u32_at(&self, off: usize) -> u32 {
        unsafe { (self.base().add(off) as *const u32).read_volatile() }
    }

    fn version(&self) -> u32 {
        self.u32_at(OFF_VERSION)
    }

    fn osd_entry(&self, index: u32) -> *mut u8 {
        let off = self.u32_at(OFF_OSD_ARR_OFFSET) as usize + index as usize * self.u32_at(OFF_OSD_ENTRY_SIZE) as usize;
        unsafe { self.base().add(off) }
    }

    fn bump_frame(&self) {
        unsafe {
            let p = self.base().add(OFF_OSD_FRAME) as *mut u32;
            p.write_volatile(p.read_volatile().wrapping_add(1));
        }
    }

    /// True while RTSS has hooked at least one process that is rendering through a 3D API.
    fn any_3d_app(&self) -> bool {
        let size = self.u32_at(OFF_APP_ENTRY_SIZE) as usize;
        let arr = self.u32_at(OFF_APP_ARR_OFFSET) as usize;
        (0..self.u32_at(OFF_APP_ARR_SIZE) as usize).any(|i| {
            let e = arr + i * size;
            self.u32_at(e + APP_PID) != 0 && (self.u32_at(e + APP_FLAGS) & APPFLAG_API_USAGE_MASK) != 0
        })
    }

    /// Find our slot (pass 0) or claim the first free one (pass 1). Slot 0 belongs to Afterburner.
    fn our_slot(&self, claim: bool) -> Option<*mut u8> {
        let count = self.u32_at(OFF_OSD_ARR_SIZE);
        for pass in 0..(if claim { 2 } else { 1 }) {
            for i in 1..count {
                let e = self.osd_entry(i);
                let owner = unsafe { read_cstr(e.add(OSD_OWNER), 256) };
                if pass == 1 && owner.is_empty() {
                    unsafe { write_cstr(e.add(OSD_OWNER), 256, OWNER) };
                    return Some(e);
                }
                if owner == OWNER {
                    return Some(e);
                }
            }
        }
        None
    }

    fn update(&self, text: &str) -> bool {
        let Some(e) = self.our_slot(true) else { return false };
        unsafe {
            if self.version() >= 0x0002_0007 {
                if self.version() >= 0x0002_000E {
                    let busy = &*(self.base().add(OFF_BUSY) as *const AtomicI32);
                    if busy.fetch_or(1, Ordering::SeqCst) & 1 != 0 {
                        return true; // renderer holds the lock; try again next tick
                    }
                    write_cstr(e.add(OSD_TEXT_EX), 4096, text);
                    busy.store(0, Ordering::SeqCst);
                } else {
                    write_cstr(e.add(OSD_TEXT_EX), 4096, text);
                }
            } else {
                write_cstr(e.add(OSD_TEXT), 256, text);
            }
        }
        self.bump_frame();
        true
    }

    fn release(&self) {
        if let Some(e) = self.our_slot(false) {
            let size = self.u32_at(OFF_OSD_ENTRY_SIZE) as usize;
            unsafe { std::ptr::write_bytes(e, 0, size) };
            self.bump_frame();
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

unsafe fn read_cstr(p: *const u8, max: usize) -> String {
    let bytes = std::slice::from_raw_parts(p, max);
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(max);
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

/// Copy ASCII text, always NUL-terminated within `max`.
unsafe fn write_cstr(p: *mut u8, max: usize, s: &str) {
    let src = s.as_bytes();
    let n = src.len().min(max - 1);
    std::ptr::copy_nonoverlapping(src.as_ptr(), p, n);
    *p.add(n) = 0;
}

fn mode_label(mode: u8, boost: bool) -> String {
    let m = match mode {
        1 => "OFFICE",
        3 => "TURBO",
        4 => "CUSTOM",
        _ => "BALANCE",
    };
    if boost { format!("{} +BOOST", m) } else { m.to_string() }
}

fn build_text(ec: &crate::driver::ec_bridge::SharedEcBridge, gpu: &crate::driver::gpu_bridge::SharedGpuBridge) -> String {
    let (mode, boost, cpu_rpm, gpu_rpm) = {
        let e = ec.lock();
        (e.get_power_mode(), e.is_fan_boost(), e.get_cpu_fan_rpm(), e.get_gpu_fan_rpm())
    };
    let cpu = crate::commands::system_cmd::get_cpu_monitor();
    let (g_temp, g_pow, g_clk, g_use, v_used, v_tot) = {
        let g = gpu.lock();
        (g.get_temp(0), g.get_power_w(0), g.get_core_freq(0), g.get_usage(0), g.get_vram_used_mb(0), g.get_vram_mb(0))
    };
    let (bat, _, _) = crate::commands::power_cmd::get_live_power_status();
    let on_ac = crate::commands::hardware_cmd::is_dc_round_plugged_in();

    // RTSS hypertext: <P=x,y> position, <Cn=RRGGBB> colour defs, <Sn=pct> size defs, <FR>/<FT> framerate/frametime.
    let mut t = String::with_capacity(512);
    t.push_str("<P=0,0><C0=F59E0B><C1=8AB4F8><C2=76B900><C3=A3A3A3><C4=FFFFFF><S0=60>");
    t.push_str(&format!("<C0>{}<C>  <C4><FR><C><S0> FPS<S>  <C4><FT><C><S0> ms<S>", mode_label(mode, boost)));
    if !on_ac {
        t.push_str(&format!("  <C3>BAT {}%<C>", bat));
    }
    t.push('\n');
    t.push_str(&format!(
        "<C1>CPU<C>  {}<S0>C<S>  {:.0}<S0>W<S>  {}<S0>MHz<S>  {}<S0>%<S>\n",
        cpu.get_temp(),
        cpu.get_power_w(),
        cpu.get_current_mhz(),
        cpu.get_usage()
    ));
    t.push_str(&format!(
        "<C2>GPU<C>  {}<S0>C<S>  {:.0}<S0>W<S>  {}<S0>MHz<S>  {}<S0>%<S>  {:.1}<S0>/{:.0}G<S>\n",
        g_temp,
        g_pow,
        g_clk,
        g_use,
        v_used as f32 / 1024.0,
        v_tot as f32 / 1024.0
    ));
    t.push_str(&format!("<C3>FAN<C>  {} / {}<S0> rpm<S>", cpu_rpm, gpu_rpm));
    t
}

/// Background feeder: 1 Hz while RTSS runs and a 3D app is hooked, idle otherwise.
pub fn start(ec: crate::driver::ec_bridge::SharedEcBridge, gpu: crate::driver::gpu_bridge::SharedGpuBridge) {
    std::thread::Builder::new()
        .name("openrevo-rtss-osd".into())
        .spawn(move || {
            let mut logged_attach = false;
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                let Some(map) = Mapping::open() else {
                    SLOT_HELD.store(false, Ordering::SeqCst);
                    logged_attach = false;
                    std::thread::sleep(Duration::from_millis(4000));
                    continue;
                };
                let enabled = crate::system::config::load_config().rtss_osd_enabled;
                if !enabled || !map.any_3d_app() {
                    if SLOT_HELD.swap(false, Ordering::SeqCst) {
                        map.release();
                    }
                    continue;
                }
                crate::driver::gpu_bridge::GpuBridge::request_telemetry(3000);
                if map.update(&build_text(&ec, &gpu)) {
                    SLOT_HELD.store(true, Ordering::SeqCst);
                    if !logged_attach {
                        crate::log_info!("RTSS_OSD", "Feeding OpenRevo sensors into RTSS OSD (shared memory v{:X})", map.version());
                        logged_attach = true;
                    }
                }
            }
        })
        .ok();
}

/// Free our OSD slot (on exit or when the feature is turned off).
pub fn release_now() {
    if let Some(map) = Mapping::open() {
        map.release();
    }
    SLOT_HELD.store(false, Ordering::SeqCst);
}
