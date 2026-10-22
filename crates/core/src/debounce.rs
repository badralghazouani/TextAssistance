//! Decides when typed text is stable enough to send for checking.
//!
//! Time is passed in rather than read, so the logic is deterministic in tests
//! and can be driven by any event loop.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Debouncer {
    idle: Duration,
    max_wait: Duration,
    first_edit: Option<Instant>,
    last_edit: Option<Instant>,
}

impl Debouncer {
    /// Fires once typing pauses for `idle`, or after `max_wait` of continuous typing.
    pub fn new(idle: Duration, max_wait: Duration) -> Debouncer {
        Debouncer {
            idle,
            max_wait,
            first_edit: None,
            last_edit: None,
        }
    }

    pub fn on_edit(&mut self, now: Instant) {
        self.first_edit.get_or_insert(now);
        self.last_edit = Some(now);
    }

    /// The moment the pending check should run, if one is pending.
    pub fn deadline(&self) -> Option<Instant> {
        let (first, last) = (self.first_edit?, self.last_edit?);
        Some((last + self.idle).min(first + self.max_wait))
    }

    /// Returns `true` (and resets) if a check is due at `now`.
    pub fn poll(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(deadline) if now >= deadline => {
                self.first_edit = None;
                self.last_edit = None;
                true
            }
            _ => false,
        }
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Debouncer::new(Duration::from_millis(400), Duration::from_secs(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn nothing_pending_without_edits() {
        let mut d = Debouncer::default();
        assert_eq!(d.deadline(), None);
        assert!(!d.poll(Instant::now()));
    }

    #[test]
    fn fires_after_idle_pause() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.on_edit(t0);
        assert!(!d.poll(t0 + 399 * MS));
        assert!(d.poll(t0 + 400 * MS));
        // Reset after firing.
        assert!(!d.poll(t0 + 1000 * MS));
    }

    #[test]
    fn continuous_typing_fires_at_max_wait() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        for i in 0..30 {
            d.on_edit(t0 + i * 100 * MS);
        }
        assert_eq!(d.deadline(), Some(t0 + 2000 * MS));
        assert!(d.poll(t0 + 2000 * MS));
    }
}
