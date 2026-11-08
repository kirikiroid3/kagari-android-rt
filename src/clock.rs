//! Engine time: milliseconds since the first frame, frozen while the app is in the background.
//!
//! The desktop clock never pauses; on Android the activity can sit stopped for hours, and the
//! game should continue where the player left it.

use std::time::{Duration, Instant};

/// A pausable millisecond clock.
#[derive(Debug, Default)]
pub struct Clock {
    /// Engine time 0; unset until [`Clock::start`].
    origin: Option<Instant>,
    /// When the current pause began.
    paused_at: Option<Instant>,
    /// The time spent in earlier pauses.
    paused_total: Duration,
}

impl Clock {
    pub fn new() -> Clock {
        Clock::default()
    }

    /// Starts the clock at `now`, unless it was started before.
    pub fn start(&mut self, now: Instant) {
        self.origin.get_or_insert(now);
    }

    /// The engine time at `now`: 0 before the start, and paused spans do not count.
    pub fn now_ms(&self, now: Instant) -> u64 {
        let Some(origin) = self.origin else {
            return 0;
        };
        let at = self.paused_at.unwrap_or(now);
        let elapsed = at
            .saturating_duration_since(origin)
            .saturating_sub(self.paused_total);
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }

    /// Freezes the clock at `now`; ignored before the start or while paused.
    pub fn pause(&mut self, now: Instant) {
        if self.origin.is_some() && self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    /// Lets the clock run again from where it was paused.
    pub fn resume(&mut self, now: Instant) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total = self
                .paused_total
                .saturating_add(now.saturating_duration_since(at));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn paused_interval_is_excluded() {
        let t0 = Instant::now();
        let mut clock = Clock::new();
        clock.start(t0);
        assert_eq!(clock.now_ms(t0 + ms(100)), 100);
        clock.pause(t0 + ms(100));
        assert_eq!(clock.now_ms(t0 + ms(5000)), 100);
        clock.resume(t0 + ms(5000));
        assert_eq!(clock.now_ms(t0 + ms(5250)), 350);
    }

    #[test]
    fn double_pause_is_idempotent() {
        let t0 = Instant::now();
        let mut clock = Clock::new();
        clock.start(t0);
        clock.pause(t0 + ms(100));
        clock.pause(t0 + ms(300));
        clock.resume(t0 + ms(1000));
        assert_eq!(clock.now_ms(t0 + ms(1000)), 100);
        // A resume without a pause changes nothing.
        clock.resume(t0 + ms(2000));
        assert_eq!(clock.now_ms(t0 + ms(2000)), 1100);
    }

    #[test]
    fn pause_before_start_is_ignored() {
        let t0 = Instant::now();
        let mut clock = Clock::new();
        assert_eq!(clock.now_ms(t0 + ms(50)), 0);
        clock.pause(t0);
        clock.start(t0 + ms(10));
        assert_eq!(clock.now_ms(t0 + ms(110)), 100);
        // A second start keeps the first origin.
        clock.start(t0 + ms(100));
        assert_eq!(clock.now_ms(t0 + ms(110)), 100);
    }
}
