// OpenRevo - Adaptive High-Precision Frame Pacer (QPC)
// Microsecond-level hardware frame rate stabilization with BetterRGB dynamic backpressure.

use std::thread::sleep;
use std::time::Duration;
use crate::driver::hid_ffi::{QueryPerformanceCounter, QueryPerformanceFrequency, LARGE_INTEGER};

pub struct FramePacer {
    freq: i64,
    pub target_fps: u32,
    pub effective_fps: u32,
    next_tick: i64,
    slow_frame_count: u32,
    healthy_frame_count: u32,
}

impl FramePacer {
    pub fn new(target_fps: u32) -> Self {
        let mut li = LARGE_INTEGER { quad_part: 0 };
        unsafe { QueryPerformanceFrequency(&mut li); }
        let freq = if li.quad_part > 0 { li.quad_part } else { 10_000_000 };
        let fps = target_fps.clamp(15, 120);
        Self {
            freq,
            target_fps: fps,
            effective_fps: fps,
            next_tick: 0,
            slow_frame_count: 0,
            healthy_frame_count: 0,
        }
    }

    pub fn set_fps(&mut self, fps: u32) {
        let clamped = fps.clamp(15, 120);
        self.target_fps = clamped;
        self.effective_fps = clamped;
        self.next_tick = 0;
        self.slow_frame_count = 0;
        self.healthy_frame_count = 0;
    }

    /// Record USB transport result with adaptive relaxation margin backpressure.
    /// Ensures at least 4ms of idle total bus headroom exists between frames so that
    /// the ITE 8291 MCU can cleanly drain its hardware FIFO and latch the PWM matrix.
    /// If write times eat into this relaxation margin for 5 consecutive frames, gently back off
    /// by 1 FPS. Once stable for 5 seconds, gently restore +1 FPS towards target.
    /// Phase is smoothly preserved without resetting next_tick to 0, completely preventing MCU watchdog blackouts.
    pub fn record_transport_result(&mut self, success: bool, call_time_ms: u64) {
        let budget_ms = 1000 / (self.effective_fps as u64).max(1);

        if !success {
            self.slow_frame_count = 0;
            self.healthy_frame_count = 0;
            let new_fps = self.effective_fps.saturating_sub(2).max(20);
            self.set_effective_fps(new_fps);
            return;
        }

        const MIN_RELAXATION_MARGIN_MS: u64 = 4;
        if call_time_ms + MIN_RELAXATION_MARGIN_MS >= budget_ms {
            self.healthy_frame_count = 0;
            self.slow_frame_count += 1;
            if self.slow_frame_count >= 5 {
                self.slow_frame_count = 0;
                let new_fps = self.effective_fps.saturating_sub(1).max(20);
                self.set_effective_fps(new_fps);
            }
            return;
        }

        self.slow_frame_count = 0;
        if self.effective_fps < self.target_fps {
            self.healthy_frame_count += 1;
            if self.healthy_frame_count >= (self.effective_fps * 5) {
                self.healthy_frame_count = 0;
                let new_fps = (self.effective_fps + 1).min(self.target_fps);
                self.set_effective_fps(new_fps);
            }
        }
    }

    fn set_effective_fps(&mut self, fps: u32) {
        let clamped = fps.clamp(20, self.target_fps);
        if clamped != self.effective_fps {
            let old_interval = self.freq / (self.effective_fps as i64);
            let new_interval = self.freq / (clamped as i64);
            crate::log_info!(
                "STREAMER",
                "Adaptive Headroom Pacing: effective FPS {} -> {} (target: {})",
                self.effective_fps,
                clamped,
                self.target_fps
            );
            self.effective_fps = clamped;
            if self.next_tick != 0 {
                // Smooth phase preservation: update next_tick by the interval delta
                // NEVER reset next_tick to 0 to prevent ITE 8291 MCU ~80ms watchdog timeout blackout!
                self.next_tick = self.next_tick - old_interval + new_interval;
            }
        }
    }

    pub fn pace(&mut self) {
        let mut now = LARGE_INTEGER { quad_part: 0 };
        unsafe { QueryPerformanceCounter(&mut now); }

        let interval = self.freq / (self.effective_fps as i64);
        if self.next_tick == 0 || now.quad_part > self.next_tick + interval * 2 {
            self.next_tick = now.quad_part;
        }
        self.next_tick += interval;

        while now.quad_part < self.next_tick {
            let remaining = self.next_tick - now.quad_part;
            let sleep_ms = (remaining * 1000 / self.freq) as u32;
            if sleep_ms > 1 {
                sleep(Duration::from_millis((sleep_ms - 1) as u64));
            } else {
                sleep(Duration::from_millis(0));
            }
            unsafe { QueryPerformanceCounter(&mut now); }
        }
    }
}
