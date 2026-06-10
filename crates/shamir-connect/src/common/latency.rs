//! Latency padding for auth-flow responses (spec §8.5 NORMATIVE).
//!
//! Per AUTH §8.5 / diagram 01: the server MUST pad the wall-clock time
//! between receiving `client_proof` (or any auth message) and emitting the
//! response so the real-vs-fake user paths and the success-vs-failure paths
//! are wall-clock indistinguishable. Branch-equivalent code alone is not
//! enough — see SECURITY_MODEL §9.2.
//!
//! ```text
//! target_constant_time_ms = FIXED_FLOOR_MS + uniform[0, JITTER_MAX_MS]
//! pad_ms = max(0, target_constant_time_ms - elapsed_ms)
//! sleep(pad_ms)
//! ```
//!
//! `FIXED_FLOOR_MS = 50`, `JITTER_MAX_MS = 25` per spec §8.5 + diagram 01
//! step 14 ("50ms floor + uniform[0,25] jitter"). The result is therefore in
//! `[50, 75]` ms — the floor defeats LAN/loopback nano-timing distinguishers
//! and the jitter adds statistical noise to the rest.
//!
//! This module provides:
//! - [`padding_for`] — pure-logic helper that returns the `Duration` to
//!   sleep based on observed elapsed time. No I/O, no time source — fully
//!   testable + portable across `std` / `tokio` / `async-std`.
//! - [`LatencyPadGuard`] — RAII helper that captures `Instant::now()` on
//!   creation and computes the pad at finish time (caller responsible for
//!   the actual sleep — we deliberately don't pick a runtime).

use std::time::Duration;
use std::time::Instant;

use rand::Rng;

/// Floor below which the auth response MUST NOT be released (spec §8.5).
pub const FIXED_FLOOR_MS: u64 = 50;
/// Maximum jitter sampled uniform\[0, JITTER_MAX_MS\] added on top of the
/// floor (spec §8.5 + diagram 01).
pub const JITTER_MAX_MS: u64 = 25;

/// Compute the per-response `target_constant_time_ms` per spec §8.5 +
/// diagram 01 step 14.
///
/// Result range: `[FIXED_FLOOR_MS, FIXED_FLOOR_MS + JITTER_MAX_MS]` — i.e.
/// `[50, 75]` ms with the current constants.
///
/// Implementation note: the spec text writes `max(jitter_ms, fixed_floor_ms)`
/// AND `jitter_ms = uniform[0, JITTER_MAX_MS]` separately. Diagram 01 step
/// 14 spells out the intent explicitly as `floor + uniform[0, jitter]`,
/// which is what this function computes. The previous implementation used
/// `floor.max(j) + j` which only happened to produce the correct result
/// because `FLOOR > JITTER_MAX` (so `max(50, j) == 50` always). The current
/// form is unambiguous: caller-visible behaviour is byte-equivalent.
pub fn target_constant_time_ms() -> u64 {
    let jitter: u64 = rand::thread_rng().gen_range(0..=JITTER_MAX_MS);
    FIXED_FLOOR_MS + jitter
}

/// Compute padding to sleep given `elapsed` and a sampled `target_ms`.
///
/// Pure function — no I/O, no time. Returns `Duration::ZERO` if the elapsed
/// time already exceeds the target.
pub fn padding_for(elapsed: Duration, target_ms: u64) -> Duration {
    let target = Duration::from_millis(target_ms);
    target.checked_sub(elapsed).unwrap_or(Duration::ZERO)
}

/// RAII handle: captures `Instant::now()` at creation; on `finish()` returns
/// the [`Duration`] the caller MUST sleep BEFORE writing the response to the
/// wire.
///
/// ```rust,ignore
/// let guard = LatencyPadGuard::start();
/// // ... run all auth crypto including failure path ...
/// let pad = guard.finish_with_target(target_constant_time_ms());
/// tokio::time::sleep(pad).await;
/// // now write response
/// ```
pub struct LatencyPadGuard {
    started_at: Instant,
}

impl LatencyPadGuard {
    /// Capture `Instant::now()`.
    pub fn start() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    /// Compute the pad duration against an externally-sampled target. The
    /// target is a parameter (rather than internally sampled) so callers can
    /// pass a deterministic value in tests.
    pub fn finish_with_target(&self, target_ms: u64) -> Duration {
        padding_for(self.started_at.elapsed(), target_ms)
    }

    /// Sample the target via [`target_constant_time_ms`] and compute the pad
    /// in one call.
    pub fn finish(&self) -> Duration {
        self.finish_with_target(target_constant_time_ms())
    }
}
