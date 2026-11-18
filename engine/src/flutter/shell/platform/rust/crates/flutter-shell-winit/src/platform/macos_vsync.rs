//! CoreVideo display-link timing; the existing event-loop timer wakes us.
//!
//! A running display link calls its output callback once per refresh. Its
//! `inOutputTime` is the host time (`mach_absolute_time`) of the frame the
//! callback is preparing; CoreVideo calls about one and a half frames ahead of
//! it, so `inNow` is not on the vblank phase (FlutterVSyncWaiter.mm derives
//! frame start from the output timestamp for the same reason). The callback
//! records the output time and the refresh period in atomics, the same
//! (vblank, period) sample DWM provides on Windows; the next tick is that
//! timestamp moved by whole periods to the first instant after now.
//!
//! The link runs only while frames are requested: after `IDLE_NANOS` without
//! a request the callback asks the main thread to stop it, keeping the last
//! phase, and the next request restarts it.
//!
//! `CVDisplayLinkGetCurrentTime` is not used: its host time is the time of the
//! call, not of a vblank, so it carries no phase.

use super::VsyncTiming;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    fn CVDisplayLinkIsRunning(link: CVDisplayLinkRef) -> bool;
    fn CVDisplayLinkRelease(link: CVDisplayLinkRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

/// Host time of the latest vblank, its display (high 32 bits of
/// `VBLANK_SOURCE`) and the refresh period in nanoseconds. Written by the
/// CoreVideo thread, read by the event loop; a stale or mixed pair only
/// shifts one wakeup, which the next sample corrects.
static VBLANK_HOST: AtomicU64 = AtomicU64::new(0);
static VBLANK_SOURCE: AtomicU64 = AtomicU64::new(0);
static PERIOD_NANOS: AtomicU64 = AtomicU64::new(0);
/// Host time of the latest `next_vsync` request, and whether a stop has
/// already been queued for the current idle stretch.
static LAST_REQUEST: AtomicU64 = AtomicU64::new(0);
static STOP_QUEUED: AtomicBool = AtomicBool::new(false);
/// How long the link keeps running without frame requests.
const IDLE_NANOS: u64 = 500_000_000;

/// The phase anchor for one callback: the output (target) time when CoreVideo
/// provides one, otherwise the callback time.
fn vblank_anchor(now_host: u64, output_host: u64) -> u64 {
    if output_host != 0 {
        output_host
    } else {
        now_host
    }
}

/// Runs on the main thread: stops the link if no frame was requested since
/// the callback that queued this.
extern "C" fn stop_if_idle(_context: *mut c_void) {
    STOP_QUEUED.store(false, Ordering::Release);
    let Ok(guard) = LINK.lock() else {
        return;
    };
    if let Some(link) = guard.as_ref() {
        // SAFETY: mach_absolute_time has no arguments or thread requirements.
        let idle = elapsed_nanos(LAST_REQUEST.load(Ordering::Acquire), unsafe {
            mach_absolute_time()
        }) >= IDLE_NANOS;
        if idle {
            // SAFETY: the link is live while the guard is held; stopping a
            // display link from outside its callback thread is supported.
            unsafe { CVDisplayLinkStop(link.link) };
        }
    }
}

fn elapsed_nanos(from: u64, to: u64) -> u64 {
    let (numer, denom) = timebase();
    if numer == 0 || denom == 0 || to <= from {
        return 0;
    }
    u64::try_from(u128::from(to - from) * u128::from(numer) / u128::from(denom)).unwrap_or(u64::MAX)
}

/// Records each vblank. Pacing comes from the host's own timer, so the
/// CoreVideo thread does no other work. `context` carries the display ID so a
/// late callback from a replaced link is ignored by readers.
unsafe extern "C" fn record_vblank(
    _link: CVDisplayLinkRef,
    now: *const CVTimeStamp,
    output: *const CVTimeStamp,
    _flags_in: u64,
    _flags_out: *mut u64,
    context: *mut c_void,
) -> i32 {
    // SAFETY: CoreVideo passes valid timestamps for the duration of the call.
    let Some(now) = (unsafe { now.as_ref() }) else {
        return 0;
    };
    // SAFETY: as above; a null output pointer falls back to `now`.
    let output_host = unsafe { output.as_ref() }.map_or(0, |output| output.host_time);
    if now.video_time_scale > 0 && now.video_refresh_period > 0 {
        let period =
            i128::from(now.video_refresh_period) * 1_000_000_000 / i128::from(now.video_time_scale);
        PERIOD_NANOS.store(u64::try_from(period).unwrap_or(0), Ordering::Relaxed);
        VBLANK_HOST.store(vblank_anchor(now.host_time, output_host), Ordering::Relaxed);
        VBLANK_SOURCE.store((context as u64) << 32 | 1, Ordering::Release);
    }
    if elapsed_nanos(LAST_REQUEST.load(Ordering::Acquire), now.host_time) >= IDLE_NANOS
        && !STOP_QUEUED.swap(true, Ordering::AcqRel)
    {
        // Stop from the main thread rather than from inside this callback.
        // SAFETY: the main queue is a static object, and stop_if_idle takes
        // no context.
        unsafe {
            dispatch_async_f(
                &raw const _dispatch_main_q,
                std::ptr::null_mut(),
                stop_if_idle,
            )
        };
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
        // Output timestamps lie up to about two frames ahead of now.
        let ahead = to_nanos(vblank - now);
        if ahead > 4 * period {
            return None;
        }
        match ahead % period {
            0 => period,
            remaining => remaining,
        }
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
    // SAFETY: mach_absolute_time has no arguments or thread requirements.
    LAST_REQUEST.store(unsafe { mach_absolute_time() }, Ordering::Release);
    let mut guard = LINK.lock().ok()?;
    if let Some(link) = guard.as_ref().filter(|link| link.display == display) {
        // SAFETY: the link is live while the guard is held.
        unsafe {
            if !CVDisplayLinkIsRunning(link.link) {
                CVDisplayLinkStart(link.link);
            }
        }
    }
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

/// Stops and releases the display link, for shell teardown.
pub(super) fn release() {
    if let Ok(mut guard) = LINK.lock() {
        *guard = None;
    }
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
    fn output_time_sets_the_phase_when_it_differs_from_callback_time() {
        // CoreVideo calls about 1.5 frames before the output time it reports.
        let callback = ticks(1_000_000_000);
        let output = callback + ticks(PERIOD * 3 / 2);
        let anchor = vblank_anchor(callback, output);
        assert_eq!(anchor, output);
        let now = callback + ticks(PERIOD / 4);
        let (delay, _) = tick_delay(now, anchor, PERIOD, TIMEBASE).unwrap();
        // 1.25 periods ahead -> the boundary a quarter period from now; the
        // callback time would have given three quarters.
        let expected = (PERIOD * 3 / 2 - PERIOD / 4) % PERIOD;
        assert!(
            delay.as_nanos().abs_diff(u128::from(expected)) <= 100,
            "{delay:?}"
        );
        let (from_callback, _) = tick_delay(now, callback, PERIOD, TIMEBASE).unwrap();
        assert!(
            from_callback
                .as_nanos()
                .abs_diff(u128::from(PERIOD - PERIOD / 4))
                <= 100
        );
        assert_eq!(vblank_anchor(callback, 0), callback);
    }

    #[test]
    fn invalid_samples_use_the_timer_fallback() {
        for (now, vblank, period, timebase) in [
            (100, 0, PERIOD, TIMEBASE),
            (100, 100, PERIOD, (0, 3)),
            (100, 100, PERIOD, (125, 0)),
            (100, 100, 999_999, TIMEBASE),
            (100, 100, 1_000_000_001, TIMEBASE),
            (100, 100 + ticks(5 * PERIOD), PERIOD, TIMEBASE),
        ] {
            assert!(tick_delay(now, vblank, period, timebase).is_none());
        }
    }
}
