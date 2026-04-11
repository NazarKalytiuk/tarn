//! Debounce bookkeeping for `textDocument/didChange` diagnostics.
//!
//! `didChange` arrives on every keystroke. We do not want to re-parse the
//! buffer that aggressively — parsing is cheap but the notification traffic
//! is noisy and real editors already debounce incremental formatting feedback
//! at ~200–300ms. We pick 300ms to match what rust-analyzer and vscode-yaml
//! use by default.
//!
//! # Strategy
//!
//! This module has **zero threads and zero runtimes**. The implementation is
//! a plain `HashMap<Url, Instant>` that records a per-document "fire at"
//! deadline whenever `didChange` is observed. The LSP main loop consults the
//! map before every blocking `receiver.recv()` and, if any deadline has
//! already passed, drains the affected URLs. Otherwise it computes the
//! earliest upcoming deadline and uses `recv_timeout` to wake when it is
//! reached.
//!
//! That approach is deliberately simple:
//!
//!   * No background thread, no tokio, no async.
//!   * A burst of rapid didChange notifications collapses to a single
//!     `validate_and_publish` call for the final content.
//!   * An idle server (empty map) blocks forever in `recv()`.
//!   * `didOpen` and `didSave` bypass the map entirely — they flush
//!     immediately, matching the acceptance criteria.
//!
//! The entry points that actually touch the LSP connection live in
//! `server.rs`. Everything here is pure and deterministic.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lsp_types::Url;

/// The debounce window for `didChange` notifications. Also exported as a
/// constant so tests can reference it instead of hard-coding 300ms.
pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);

/// Per-URL pending-fire state.
#[derive(Debug, Default)]
pub struct DebounceTracker {
    deadlines: HashMap<Url, Instant>,
}

impl DebounceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `uri` as having a change at `now`; the flush deadline becomes
    /// `now + DEBOUNCE_WINDOW`. Call from the `didChange` handler.
    pub fn record_change(&mut self, uri: Url, now: Instant) {
        self.deadlines.insert(uri, now + DEBOUNCE_WINDOW);
    }

    /// Cancel any pending fire for `uri`. Call from `didClose` so a closed
    /// buffer never wakes the main loop.
    pub fn forget(&mut self, uri: &Url) {
        self.deadlines.remove(uri);
    }

    /// Return the `Instant` the main loop must wake at to service the
    /// earliest pending debounce, or `None` if nothing is pending.
    pub fn earliest_deadline(&self) -> Option<Instant> {
        self.deadlines.values().min().copied()
    }

    /// Drain every URL whose debounce deadline has already passed at `now`.
    /// The returned vector is empty when nothing is due.
    pub fn drain_due(&mut self, now: Instant) -> Vec<Url> {
        let due: Vec<Url> = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(uri, _)| uri.clone())
            .collect();
        for uri in &due {
            self.deadlines.remove(uri);
        }
        due
    }

    /// True when no debounces are pending. Lets the main loop fall back to
    /// the simple blocking `recv()` path.
    pub fn is_idle(&self) -> bool {
        self.deadlines.is_empty()
    }
}

/// Given `last_change` and the current wall-clock, return the moment the
/// loop should wake. Pure helper so the branch logic is unit-testable
/// without touching the channel.
///
/// Returns:
///   * `Some(now)` when `last_change + DEBOUNCE_WINDOW` has already passed —
///     the caller should flush immediately.
///   * `Some(deadline)` when the deadline is still in the future.
pub fn next_fire_deadline(last_change: Instant, now: Instant) -> Instant {
    let deadline = last_change + DEBOUNCE_WINDOW;
    if deadline <= now {
        now
    } else {
        deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_url(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    #[test]
    fn new_tracker_is_idle_and_has_no_deadline() {
        let tracker = DebounceTracker::new();
        assert!(tracker.is_idle());
        assert!(tracker.earliest_deadline().is_none());
    }

    #[test]
    fn record_change_sets_a_future_deadline() {
        let mut tracker = DebounceTracker::new();
        let uri = mk_url("/tmp/a.tarn.yaml");
        let now = Instant::now();
        tracker.record_change(uri.clone(), now);
        let deadline = tracker
            .earliest_deadline()
            .expect("deadline must be present");
        assert!(deadline >= now + DEBOUNCE_WINDOW - Duration::from_millis(1));
        assert!(!tracker.is_idle());
    }

    #[test]
    fn rapid_changes_collapse_to_the_last_one() {
        // Three changes in quick succession on the same URL should yield a
        // single deadline, aligned with the final change. This is the
        // defining property of debounce.
        let mut tracker = DebounceTracker::new();
        let uri = mk_url("/tmp/a.tarn.yaml");
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(50);
        let t2 = t0 + Duration::from_millis(120);

        tracker.record_change(uri.clone(), t0);
        tracker.record_change(uri.clone(), t1);
        tracker.record_change(uri.clone(), t2);

        let deadline = tracker.earliest_deadline().unwrap();
        assert_eq!(deadline, t2 + DEBOUNCE_WINDOW);

        // One URL → draining when due should return exactly one entry.
        let due = tracker.drain_due(t2 + DEBOUNCE_WINDOW + Duration::from_millis(1));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0], uri);
        assert!(tracker.is_idle());
    }

    #[test]
    fn changes_on_different_urls_are_tracked_independently() {
        let mut tracker = DebounceTracker::new();
        let a = mk_url("/tmp/a.tarn.yaml");
        let b = mk_url("/tmp/b.tarn.yaml");
        let now = Instant::now();
        tracker.record_change(a.clone(), now);
        tracker.record_change(b.clone(), now + Duration::from_millis(50));

        // Earliest deadline is `a`'s, not `b`'s.
        let earliest = tracker.earliest_deadline().unwrap();
        assert_eq!(earliest, now + DEBOUNCE_WINDOW);

        // Draining at `a`'s deadline but before `b`'s leaves `b` pending.
        let due = tracker.drain_due(now + DEBOUNCE_WINDOW);
        assert_eq!(due, vec![a]);
        assert!(!tracker.is_idle());

        // Draining past `b`'s deadline yields `b`.
        let due = tracker.drain_due(now + Duration::from_millis(50) + DEBOUNCE_WINDOW);
        assert_eq!(due, vec![b]);
        assert!(tracker.is_idle());
    }

    #[test]
    fn drain_due_returns_nothing_before_deadline() {
        let mut tracker = DebounceTracker::new();
        let uri = mk_url("/tmp/a.tarn.yaml");
        let now = Instant::now();
        tracker.record_change(uri, now);
        let due = tracker.drain_due(now + Duration::from_millis(100));
        assert!(due.is_empty());
        assert!(!tracker.is_idle());
    }

    #[test]
    fn forget_cancels_a_pending_fire() {
        let mut tracker = DebounceTracker::new();
        let uri = mk_url("/tmp/a.tarn.yaml");
        tracker.record_change(uri.clone(), Instant::now());
        assert!(!tracker.is_idle());
        tracker.forget(&uri);
        assert!(tracker.is_idle());
        assert!(tracker.earliest_deadline().is_none());
    }

    #[test]
    fn next_fire_deadline_future_returns_deadline() {
        let now = Instant::now();
        let last = now; // just changed
        let d = next_fire_deadline(last, now);
        assert_eq!(d, now + DEBOUNCE_WINDOW);
    }

    #[test]
    fn next_fire_deadline_past_returns_now() {
        let now = Instant::now();
        let last = now - Duration::from_millis(500); // already past the window
        let d = next_fire_deadline(last, now);
        assert_eq!(d, now);
    }
}
