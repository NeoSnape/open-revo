//! The one thread that polls the EC, on one schedule.
//!
//! There used to be two polling threads with their own timers (the EcBridge state poller and the
//! device-switch watcher), plus ad-hoc reads from the telemetry and status-log loops. Now this
//! thread reads everything periodic and the others consume the snapshot:
//!
//!   every 250 ms  status byte (Win-key lock) and Fn-lock byte
//!   every 500 ms  power status (barrel jack / Type-C)
//!   every 2 s     fan boost clamp, mode index, battery bytes, fan rpm (-> EcBridge cache),
//!                 and the APExist heartbeat
//!
//! One-off reads and all writes still call `wmi_ec` directly; they are serialised by its mutex.

use parking_lot::{Condvar, Mutex};
use std::sync::OnceLock;
use std::time::Duration;

use crate::driver::ec_bridge::{absorb_slow_sample, RpmFilter, SharedStateCache, SLOW_REGISTERS};
use crate::driver::wmi_ec;

pub const TICK: Duration = Duration::from_millis(250);
const POWER_EVERY: u64 = 2; // 500 ms
const SLOW_EVERY: u64 = 8; // 2 s

/// Latest periodic readings. A field is `None` until it has been read once, and again whenever the
/// EC could not be read.
#[derive(Debug, Clone, Copy, Default)]
pub struct EcSnapshot {
    /// Counts fast ticks; changes every 250 ms.
    pub tick: u64,
    pub status_byte: Option<u8>,
    pub fn_lock_byte: Option<u8>,
    pub power_status: Option<u8>,
}

impl EcSnapshot {
    pub fn win_key_locked(&self) -> Option<bool> {
        self.status_byte.map(|v| (v & wmi_ec::STATUS_WINLOCK) != 0)
    }

    pub fn fn_locked(&self) -> Option<bool> {
        self.fn_lock_byte.map(|v| (v & 0x10) != 0)
    }
}

struct Shared {
    snapshot: Mutex<EcSnapshot>,
    ticked: Condvar,
    /// The EcBridge state cache currently in use (replaced when the bridge is reloaded).
    cache: Mutex<Option<SharedStateCache>>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        snapshot: Mutex::new(EcSnapshot::default()),
        ticked: Condvar::new(),
        cache: Mutex::new(None),
    })
}

pub fn latest() -> EcSnapshot {
    *shared().snapshot.lock()
}

/// Sleep until the sampler has finished its next tick (or `timeout`, if it is not running), then
/// return the snapshot. Lets a consumer run in step with the sampler instead of on its own timer.
pub fn wait_next(timeout: Duration) -> EcSnapshot {
    let sh = shared();
    let mut snap = sh.snapshot.lock();
    let seen = snap.tick;
    let deadline = std::time::Instant::now() + timeout;
    // Loop: a condition variable may wake without a new tick.
    while snap.tick == seen {
        if sh.ticked.wait_until(&mut snap, deadline).timed_out() {
            break;
        }
    }
    *snap
}

/// Point the sampler at the EcBridge state cache and start it (once per process).
pub fn attach(cache: SharedStateCache) {
    *shared().cache.lock() = Some(cache);
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("openrevo-ec-sampler".into()).spawn(run);
        if spawned.is_err() {
            crate::log_error!("EC_SAMPLER", "Could not start the EC sampler thread");
        }
    });
}

fn run() {
    // Same head start the old poller gave the boot sequence.
    std::thread::sleep(Duration::from_millis(500));
    let fast = [wmi_ec::ADDR_STATUS_BYTE, wmi_ec::ADDR_FN_LOCK];
    let mut rpm = RpmFilter::default();
    let mut tick: u64 = 0;
    let sh = shared();

    loop {
        tick = tick.wrapping_add(1);
        // Leave the EC alone while the machine reboots, and when the driver is not there.
        let readable = !crate::driver::uefi_nvram::is_reboot_in_progress() && wmi_ec::is_acpi_driver_available();

        let mut snap = latest();
        snap.tick = tick;
        if readable {
            match wmi_ec::wmi_batch_read_ec(&fast) {
                Some(b) if b.len() == fast.len() => {
                    snap.status_byte = Some(b[0]);
                    snap.fn_lock_byte = Some(b[1]);
                }
                _ => {
                    snap.status_byte = None;
                    snap.fn_lock_byte = None;
                }
            }
            if tick % POWER_EVERY == 0 {
                snap.power_status = wmi_ec::wmi_read_ec(wmi_ec::ADDR_POWER_STATUS);
            }
        } else {
            snap.status_byte = None;
            snap.fn_lock_byte = None;
            snap.power_status = None;
        }
        {
            let mut guard = sh.snapshot.lock();
            *guard = snap;
        }
        sh.ticked.notify_all();

        if readable && tick % SLOW_EVERY == 0 {
            let cache = sh.cache.lock().clone();
            if let Some(cache) = cache {
                if let Some(bytes) = wmi_ec::wmi_batch_read_ec(&SLOW_REGISTERS) {
                    absorb_slow_sample(&cache, &bytes, &mut rpm);
                }
            }
            // Heartbeat: keep APExist asserted so the EC keeps accepting our tables.
            if let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_AP_OEM_BYTE) {
                if (val & 0x01) == 0 {
                    let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_AP_OEM_BYTE, val | 0x01);
                }
            }
        }

        std::thread::sleep(TICK);
    }
}
