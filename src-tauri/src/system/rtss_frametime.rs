// Per-frame frametimes from RTSS via its frametime pipe (RTSS SDK RTSSFrametimePipeSample):
// we host \\.\PIPE\RTSS_Frametime and the RTSS hook inside the game writes one
// { dwApp, dwFrametime(us) } record per presented frame. Used for average / 1% low FPS,
// which RTSS itself only computes in benchmark mode.

use std::collections::VecDeque;
use std::time::Duration;

use parking_lot::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, HANDLE,
};
use windows::Win32::Security::{
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR,
};
use windows::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES,
};

/// Sliding window the stats are computed over.
const WINDOW_US: u64 = 30_000_000;
/// Frames older than this without new data are considered stale (game paused / closed).
const STALE_AFTER: Duration = Duration::from_secs(3);

struct Window {
    frames: VecDeque<u32>,
    total_us: u64,
    last_data: Option<std::time::Instant>,
}

static WINDOW: Mutex<Window> = Mutex::new(Window { frames: VecDeque::new(), total_us: 0, last_data: None });

#[derive(Debug, Clone, Copy)]
pub struct FramerateStats {
    pub avg_fps: f32,
    pub low_1pct_fps: f32,
}

fn push(frametime_us: u32) {
    if frametime_us == 0 || frametime_us > 2_000_000 {
        return;
    }
    let mut w = WINDOW.lock();
    w.frames.push_back(frametime_us);
    w.total_us += frametime_us as u64;
    while w.total_us > WINDOW_US {
        match w.frames.pop_front() {
            Some(old) => w.total_us -= old as u64,
            None => break,
        }
    }
    w.last_data = Some(std::time::Instant::now());
}

fn reset() {
    let mut w = WINDOW.lock();
    w.frames.clear();
    w.total_us = 0;
    w.last_data = None;
}

/// Average FPS and integral 1% low FPS over the last 30 s (None without recent frames).
pub fn stats() -> Option<FramerateStats> {
    let w = WINDOW.lock();
    if w.last_data.map_or(true, |t| t.elapsed() > STALE_AFTER) || w.frames.len() < 100 {
        return None;
    }
    let mut sorted: Vec<u32> = w.frames.iter().copied().collect();
    let total = w.total_us;
    let count = sorted.len();
    drop(w);
    // Integral percentile (RTSS default): the frametime at which the slowest frames add up to 1% of total time.
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let budget = total / 100;
    let mut acc = 0u64;
    let mut low_ft = sorted[0];
    for ft in &sorted {
        if acc + *ft as u64 > budget {
            low_ft = *ft;
            break;
        }
        acc += *ft as u64;
    }
    Some(FramerateStats {
        avg_fps: count as f32 * 1_000_000.0 / total as f32,
        low_1pct_fps: 1_000_000.0 / low_ft as f32,
    })
}

fn create_pipe() -> Option<HANDLE> {
    unsafe {
        // NULL DACL as in the SDK sample: game processes run unelevated, OpenRevo elevated.
        let mut sd = SECURITY_DESCRIPTOR::default();
        let psd = PSECURITY_DESCRIPTOR(&mut sd as *mut _ as *mut _);
        InitializeSecurityDescriptor(psd, 1).ok()?;
        SetSecurityDescriptorDacl(psd, true, None, false).ok()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: psd.0,
            bInheritHandle: false.into(),
        };
        let h = CreateNamedPipeW(
            w!(r"\\.\PIPE\RTSS_Frametime"),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_MESSAGE | PIPE_NOWAIT,
            PIPE_UNLIMITED_INSTANCES,
            4096 * 8,
            4096 * 8,
            0,
            Some(&sa),
        );
        if h.is_invalid() {
            None
        } else {
            Some(h)
        }
    }
}

/// Pipe server thread; only does work while RTSS is running and connects to us.
pub fn start() {
    std::thread::Builder::new()
        .name("openrevo-rtss-frametime".into())
        .spawn(|| {
            let Some(pipe) = create_pipe() else {
                crate::log_warn!("RTSS_OSD", "Could not create RTSS frametime pipe; avg / 1% low will use RTSS tags");
                return;
            };
            let mut connected = false;
            let mut rec = [0u8; 8];
            loop {
                if !connected {
                    std::thread::sleep(Duration::from_millis(1000));
                    match unsafe { ConnectNamedPipe(pipe, None) } {
                        Ok(()) => connected = true,
                        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => connected = true,
                        Err(e) if e.code() == ERROR_PIPE_LISTENING.to_hresult() => continue,
                        Err(_) => {
                            unsafe {
                                let _ = DisconnectNamedPipe(pipe);
                            }
                            continue;
                        }
                    }
                    if connected {
                        reset();
                    }
                }
                // Drain everything queued, then sleep a little (non-blocking pipe).
                loop {
                    let mut read = 0u32;
                    match unsafe { ReadFile(pipe, Some(&mut rec), Some(&mut read), None) } {
                        Ok(()) if read == 8 => {
                            push(u32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]));
                        }
                        Ok(()) => break,
                        Err(e) if e.code() == ERROR_NO_DATA.to_hresult() => break,
                        Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => {
                            unsafe {
                                let _ = DisconnectNamedPipe(pipe);
                            }
                            connected = false;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            #[allow(unreachable_code)]
            unsafe {
                let _ = CloseHandle(pipe);
            }
        })
        .ok();
}
