//! CoreVideo display-link timing; the existing event-loop timer wakes us.
//!
//! A running display link calls its output callback once per vblank with
//! `inNow`, whose host time is the `mach_absolute_time` of that vblank. The
//! callback only records it, with the refresh period, in atomics. That is the
//! same (last vblank, period) sample DWM provides on Windows, so the next tick
//! is the last vblank advanced by whole periods past now.
//!
//! `CVDisplayLinkGetCurrentTime` is not used: its host time is the time of the
//! call, not of a vblank, so it carries no phase.

use super::VsyncTiming;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

type CVDisplayLinkRef = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct CVSMPTETime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Default)]
struct CVTimeStamp {
    version: u32,
    video_time_scale: i32,
    video_time: i64,
    host_time: u64,
    rate_scalar: f64,
    video_refresh_period: i64,
    smpte_time: CVSMPTETime,
    flags: u64,
    reserved: u64,
}

#[repr(C)]
#[derive(Default)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

type OutputCallback = unsafe extern "C" fn(
    CVDisplayLinkRef,
    *const CVTimeStamp,
    *const CVTimeStamp,
    u64,
    *mut u64,
    *mut c_void,
) -> i32;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVDisplayLinkCreateWithCGDisplay(display: u32, link: *mut CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkSetOutputCallback(
        link: CVDisplayLinkRef,
        callback: OutputCallback,
        context: *mut c_void,
    ) -> i32;
    fn CVDisplayLinkStart(link: CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkStop(link: CVDisplayLinkRef) -> i32;
    fn CVDisplayLinkRelease(link: CVDisplayLinkRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

/// Host time of the latest vblank, its display (high 32 bits of
/// `VBLANK_SOURCE`) and the refresh period in nanoseconds. Written by the
/// CoreVideo thread, read by the event loop; a stale or mixed pair only
/// shifts one wakeup, which the next sample corrects.
static VBLANK_HOST: AtomicU64 = AtomicU64::new(0);
static VBLANK_SOURCE: AtomicU64 = AtomicU64::new(0);
static PERIOD_NANOS: AtomicU64 = AtomicU64::new(0);

/// Records each vblank. Pacing comes from the host's own timer, so the
/// CoreVideo thread does no other work. `context` carries the display ID so a
/// late callback from a replaced link is ignored by readers.
unsafe extern "C" fn record_vblank(
    _link: CVDisplayLinkRef,
    now: *const CVTimeStamp,
    _output: *const CVTimeStamp,
    _flags_in: u64,
    _flags_out: *mut u64,
    context: *mut c_void,
) -> i32 {
    // SAFETY: CoreVideo passes a valid timestamp for the duration of the call.
    let Some(now) = (unsafe { now.as_ref() }) else {
        return 0;
    };
    if now.video_time_scale > 0 && now.video_refresh_period > 0 {
        let period =
            i128::from(now.video_refresh_period) * 1_000_000_000 / i128::from(now.video_time_scale);
        PERIOD_NANOS.store(u64::try_from(period).unwrap_or(0), Ordering::Relaxed);
        VBLANK_HOST.store(now.host_time, Ordering::Relaxed);
        VBLANK_SOURCE.store((context as u64) << 32 | 1, Ordering::Release);
    }
    0
}

struct DisplayLink {
    display: u32,
    link: CVDisplayLinkRef,
}

// SAFETY: CVDisplayLink is a thread-safe CoreFoundation-style object; the
// handle is only used through the mutex below.
unsafe impl Send for DisplayLink {}

impl DisplayLink {
    fn new(display: u32) -> Option<Self> {
        let mut link = std::ptr::null_mut();
        // SAFETY: the out pointer is valid; a failed call leaves it null.
        unsafe {
            if CVDisplayLinkCreateWithCGDisplay(display, &mut link) != 0 || link.is_null() {
                return None;
            }
            let context = display as usize as *mut c_void;
            if CVDisplayLinkSetOutputCallback(link, record_vblank, context) != 0
                || CVDisplayLinkStart(link) != 0
            {
                CVDisplayLinkRelease(link);
                return None;
            }
        }
        Some(Self { display, link })
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        // SAFETY: `link` was created by CVDisplayLinkCreateWithCGDisplay and
        // is released exactly once.
        unsafe {
            CVDisplayLinkStop(self.link);
            CVDisplayLinkRelease(self.link);
        }
    }
}

static LINK: Mutex<Option<DisplayLink>> = Mutex::new(None);

/// Time until the next vblank and the refresh interval, from the host time of
/// the last vblank and the current host time (both in mach ticks).
fn tick_delay(
    now: u64,
    vblank: u64,
    period_nanos: u64,
    timebase: (u32, u32),
) -> Option<(Duration, Duration)> {
    let (numer, denom) = timebase;
    if numer == 0 || denom == 0 || vblank == 0 {
        return None;
    }
    if !(1_000_000..=1_000_000_000).contains(&period_nanos) {
        return None;
    }
    let to_nanos = |ticks: u64| u128::from(ticks) * u128::from(numer) / u128::from(denom);
    let period = u128::from(period_nanos);
    let remaining = if now >= vblank {
        period - to_nanos(now - vblank) % period
    } else {
        let ahead = to_nanos(vblank - now);
        if ahead > period {
            return None;
        }
        ahead
    };
    Some((
        Duration::from_nanos(u64::try_from(remaining).ok()?.max(1)),
        Duration::from_nanos(period_nanos),
    ))
}

fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo::default();
        // SAFETY: the out pointer is a valid MachTimebaseInfo.
        if unsafe { mach_timebase_info(&mut info) } == 0 {
            (info.numer, info.denom)
        } else {
            (0, 0)
        }
    })
}

pub(super) fn next_vsync(display: Option<u32>) -> Option<VsyncTiming> {
    // SAFETY: CGMainDisplayID has no arguments or thread requirements.
    let display = display.unwrap_or_else(|| unsafe { CGMainDisplayID() });
    let mut guard = LINK.lock().ok()?;
    if guard.as_ref().is_none_or(|link| link.display != display) {
        // Drop the previous display's link before starting a new one.
        *guard = None;
        *guard = DisplayLink::new(display);
        if guard.is_none() {
            static CREATE_FAILED: Once = Once::new();
            CREATE_FAILED.call_once(|| {
                log::warn!("CVDisplayLink unavailable; using timer vsync fallback");
            });
        }
    }
    drop(guard);
    // Until the new link's first vblank arrives, the timer fallback paces.
    if VBLANK_SOURCE.load(Ordering::Acquire) != (u64::from(display) << 32 | 1) {
        return None;
    }
    let vblank = VBLANK_HOST.load(Ordering::Relaxed);
    let period = PERIOD_NANOS.load(Ordering::Relaxed);
    let sampled_at = Instant::now();
    // SAFETY: mach_absolute_time has no arguments or thread requirements.
    let now = unsafe { mach_absolute_time() };
    let result = tick_delay(now, vblank, period, timebase()).and_then(|(delay, interval)| {
        Some(VsyncTiming {
            deadline: sampled_at.checked_add(delay)?,
            interval,
        })
    });
    static LINK_REPORTED: Once = Once::new();
    if let Some(timing) = &result {
        LINK_REPORTED.call_once(|| {
            log::info!(
                "macOS display-link vsync enabled: interval_ns={}",
                timing.interval.as_nanos()
            );
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // Apple Silicon's mach timebase: 125/3 ns per tick.
    const TIMEBASE: (u32, u32) = (125, 3);
    const PERIOD: u64 = 16_667_000;

    fn ticks(nanos: u64) -> u64 {
        nanos * 3 / 125
    }

    #[test]
    fn next_tick_keeps_vblank_phase_without_accumulating_lateness() {
        let vblank = ticks(1_000_000_000);
        let (delay, interval) = tick_delay(vblank, vblank, PERIOD, TIMEBASE).unwrap();
        assert_eq!(interval, Duration::from_nanos(PERIOD));
        assert_eq!(delay, Duration::from_nanos(PERIOD));
        let (delay, _) = tick_delay(vblank + ticks(4_000_000), vblank, PERIOD, TIMEBASE).unwrap();
        assert_eq!(delay, Duration::from_nanos(PERIOD - 4_000_000));
        // A stale sample many periods old still yields the next future tick.
        let (delay, _) = tick_delay(
            vblank + ticks(10 * PERIOD + 4_000_000),
            vblank,
            PERIOD,
            TIMEBASE,
        )
        .unwrap();
        assert_eq!(delay, Duration::from_nanos(PERIOD - 4_000_000));
    }

    #[test]
    fn sample_slightly_ahead_of_now_waits_until_that_vblank() {
        let now = ticks(1_000_000_000);
        let (delay, _) = tick_delay(now, now + ticks(3_000_000), PERIOD, TIMEBASE).unwrap();
        assert_eq!(delay, Duration::from_nanos(3_000_000));
    }

    #[test]
    fn invalid_samples_use_the_timer_fallback() {
        for (now, vblank, period, timebase) in [
            (100, 0, PERIOD, TIMEBASE),
            (100, 100, PERIOD, (0, 3)),
            (100, 100, PERIOD, (125, 0)),
            (100, 100, 999_999, TIMEBASE),
            (100, 100, 1_000_000_001, TIMEBASE),
            (100, 100 + ticks(2 * PERIOD), PERIOD, TIMEBASE),
        ] {
            assert!(tick_delay(now, vblank, period, timebase).is_none());
        }
    }
}
