//! DWM's QPC clock supplies the phase; the existing event-loop timer wakes us.

use super::VsyncTiming;
use std::sync::Once;
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo},
    System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
};

/// Convert a native sample into the next strictly future tick. Advancing by
/// modulo avoids accumulating timer lateness or iterating over stale samples.
fn tick_delay(now: u64, vblank: u64, period: u64, frequency: u64) -> Option<(Duration, Duration)> {
    if frequency == 0 || period == 0 || vblank == 0 {
        return None;
    }
    let nanos = |ticks: u64| -> Option<u64> {
        u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(frequency)).ok()
    };
    let interval = nanos(period)?;
    if !(1_000_000..=1_000_000_000).contains(&interval) {
        return None;
    }
    let remaining = if now >= vblank {
        period - (now - vblank) % period
    } else {
        let ahead = vblank - now;
        if ahead > period {
            return None;
        }
        ahead
    };
    Some((
        Duration::from_nanos(nanos(remaining)?.max(1)),
        Duration::from_nanos(interval),
    ))
}

pub(super) fn next_vsync() -> Option<VsyncTiming> {
    // DWM_TIMING_INFO is packed by the Windows SDK; use the generated binding
    // rather than maintaining a second native layout here. Windows 8.1+ requires
    // a null HWND, so this is a composition-wide clock, not a per-monitor API.
    let mut timing: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    timing.cbSize = std::mem::size_of::<DWM_TIMING_INFO>() as u32;
    let mut frequency = 0;
    let mut counter = 0;
    // SAFETY: all outputs are initialized, correctly sized native structures.
    let valid = unsafe {
        DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut timing) == 0
            && QueryPerformanceFrequency(&mut frequency) != 0
            && QueryPerformanceCounter(&mut counter) != 0
    };
    let sampled_at = Instant::now();
    let result = if valid && frequency > 0 && counter >= 0 {
        tick_delay(
            counter as u64,
            timing.qpcVBlank,
            timing.qpcRefreshPeriod,
            frequency as u64,
        )
        .and_then(|(delay, interval)| {
            Some(VsyncTiming {
                deadline: sampled_at.checked_add(delay)?,
                interval,
            })
        })
    } else {
        None
    };
    static DWM_REPORTED: Once = Once::new();
    static FALLBACK_REPORTED: Once = Once::new();
    if let Some(timing) = &result {
        DWM_REPORTED.call_once(|| {
            log::info!(
                "Windows DWM vsync enabled: interval_ns={}",
                timing.interval.as_nanos()
            );
        });
    } else {
        FALLBACK_REPORTED.call_once(|| {
            log::warn!("Windows DWM timing unavailable or invalid; using timer vsync fallback");
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ticks_preserve_phase_without_accumulating_lateness() {
        let sample = |now| tick_delay(now, 100, 10, 1_000).unwrap();
        assert_eq!(
            sample(100),
            (Duration::from_millis(10), Duration::from_millis(10))
        );
        assert_eq!(sample(103).0, Duration::from_millis(7));
        assert_eq!(sample(119).0, Duration::from_millis(1));
        assert_eq!(sample(1_000_103).0, Duration::from_millis(7));
        assert_eq!(sample(97).0, Duration::from_millis(3));
    }

    #[test]
    fn invalid_native_samples_use_the_timer_fallback() {
        for (now, vblank, period, frequency) in [
            (100, 100, 0, 1_000),
            (100, 100, 10, 0),
            (100, 0, 10, 1_000),
            (100, 100, 1, 10_000),
            (100, 100, 2_000, 1_000),
            (100, 111, 10, 1_000),
            (u64::MAX, 100, u64::MAX, 1),
        ] {
            assert!(tick_delay(now, vblank, period, frequency).is_none());
        }
    }

    #[test]
    fn native_tick_conversion_handles_fractional_refresh_and_large_counters() {
        let (delay, interval) = tick_delay(u64::MAX - 1, 1, 166_833, 10_000_000).unwrap();
        assert_eq!(interval, Duration::from_nanos(16_683_300));
        assert!(!delay.is_zero() && delay <= interval);
    }
}
