//! CPU accounting for the `perf` diag record: wall time per pipeline stage, and process and thread
//! CPU times (`GetProcessTimes` / `GetThreadTimes`, the same clock `tests\e2e\run.py` uses).
//! Numbers only; written once per second, and only when `--diag-json` is on.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetProcessTimes, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};

/// Capture-thread stages (wall time; the sum is close to the capture thread's CPU time except for
/// `wait`, where the thread sleeps until the GPU copy is done).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Window look-up and geometry (`FindWindowEx`, `describe`).
    Window,
    /// Opening a capture source (D3D device and WGC session, or the GDI DCs).
    Open,
    /// Taking the newest frame from the capture source (WGC frame pool).
    Acquire,
    /// Issuing the GPU copies of the regions to staging textures (GDI: `BitBlt`).
    Copy,
    /// Sleeping until the GPU has done the copies (no CPU used).
    Wait,
    /// Map and read-back of the pixels.
    Read,
    /// `strip::decode_strip`, reassembly and message handling.
    Strip,
    /// `tags::scan`.
    Scan,
    /// `cards::build`.
    Cards,
    /// Covering test and `SceneMachine::update`.
    Scene,
    /// Fingerprint, `cards` diag record, hand-over to the UI thread.
    Publish,
    /// Per-tick diag records.
    Diag,
}

pub const STAGE_NAMES: [&str; 12] = ["window", "open", "acquire", "copy", "wait", "read", "strip", "scan", "cards", "scene", "publish", "diag"];

/// Accumulated wall time and call count per stage.
#[derive(Clone, Debug, Default)]
pub struct Stages {
    ns: [u64; 12],
    n: [u32; 12],
}

impl Stages {
    pub fn add(&mut self, s: Stage, d: Duration) {
        let i = s as usize;
        self.ns[i] += d.as_nanos() as u64;
        self.n[i] += 1;
    }

    /// Adds the time since `t0` and returns now (for chaining stage boundaries).
    pub fn lap(&mut self, s: Stage, t0: Instant) -> Instant {
        let now = Instant::now();
        self.add(s, now - t0);
        now
    }

    pub fn total_ms(&self) -> f64 {
        self.ns.iter().sum::<u64>() as f64 / 1e6
    }

    /// `{"stage": [ms, calls], ...}` for the stages that ran.
    pub fn json(&self) -> Value {
        let mut m = Map::new();
        for (i, name) in STAGE_NAMES.iter().enumerate() {
            if self.n[i] > 0 {
                m.insert((*name).to_string(), Value::from(vec![Value::from(round2(self.ns[i] as f64 / 1e6)), Value::from(self.n[i])]));
            }
        }
        Value::Object(m)
    }
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn ft_ms(k: FILETIME, u: FILETIME) -> f64 {
    let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (t(k) + t(u)) as f64 / 1e4
}

#[link(name = "kernel32")]
extern "system" {
    fn QueryProcessCycleTime(process: HANDLE, cycles: *mut u64) -> i32;
}

/// CPU cycles used by every thread of this process (counted precisely, unlike the process times,
/// which the clock interrupt samples; divide by the cycle-counter rate to get seconds).
pub fn process_cycles() -> u64 {
    let mut c = 0u64;
    unsafe {
        QueryProcessCycleTime(GetCurrentProcess(), &mut c);
    }
    c
}

/// User + kernel CPU milliseconds of this process.
pub fn process_cpu_ms() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    match unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } {
        Ok(()) => ft_ms(k, u),
        Err(_) => 0.0,
    }
}

fn thread_cpu_ms_of(h: HANDLE) -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    match unsafe { GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u) } {
        Ok(()) => ft_ms(k, u),
        Err(_) => 0.0,
    }
}

/// User + kernel CPU milliseconds of the calling thread.
pub fn thread_cpu_ms() -> f64 {
    thread_cpu_ms_of(unsafe { GetCurrentThread() })
}

pub fn current_thread_id() -> u32 {
    unsafe { GetCurrentThreadId() }
}

/// A query handle on another thread of this process (for its CPU time).
pub struct ThreadClock(Option<HANDLE>);

impl ThreadClock {
    pub fn open(thread_id: u32) -> ThreadClock {
        if thread_id == 0 {
            return ThreadClock(None);
        }
        ThreadClock(unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread_id) }.ok())
    }

    pub fn cpu_ms(&self) -> f64 {
        self.0.map(thread_cpu_ms_of).unwrap_or(0.0)
    }
}

impl Drop for ThreadClock {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_accumulate_and_serialize() {
        let mut s = Stages::default();
        s.add(Stage::Scan, Duration::from_micros(1500));
        s.add(Stage::Scan, Duration::from_micros(500));
        s.add(Stage::Copy, Duration::from_micros(250));
        let j = s.json();
        assert_eq!(j["scan"], serde_json::json!([2.0, 2]));
        assert_eq!(j["copy"], serde_json::json!([0.25, 1]));
        assert!(j.get("strip").is_none());
        assert!((s.total_ms() - 2.25).abs() < 1e-9);
    }

    #[test]
    fn cpu_clocks_move_forward() {
        let p0 = process_cpu_ms();
        let t0 = thread_cpu_ms();
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(60) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        assert!(thread_cpu_ms() > t0);
        assert!(process_cpu_ms() > p0);
        let c0 = process_cycles();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(5) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        assert!(process_cycles() > c0);
        let other = ThreadClock::open(current_thread_id());
        assert!(other.cpu_ms() > 0.0);
    }
}
