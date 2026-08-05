//! #1003 (follow-up to #984) — in-flight `CREATE INDEX` counter.
//!
//! [`degraded_index_count`](super::table_manager::TableManager::degraded_index_count)
//! counts every index whose `.state != IndexState::Ready` — but
//! `IndexState::Building` is set at the START of a `CREATE INDEX`, before the
//! backfill even begins (F-72, #899), and is only flipped to `Ready` once the
//! backfill fully completes. A large-table backfill can legitimately run for
//! minutes (`docs/guide-docs/KNOWN_LIMITATIONS.md` §3), so a completely
//! healthy, currently-in-progress create made the gauge read non-zero for the
//! ENTIRE build — a real false-positive that would page an operator to
//! "repair" an index that is not stuck at all.
//!
//! This module provides a tiny per-`TableManager` in-flight counter — one
//! `Arc<AtomicU64>`, bumped at the very start of each of the four index-create
//! families (`create_index`, `create_unique_index`,
//! `create_sorted_index_with_include`, `create_index_v2`) and decremented via
//! an RAII guard so a panic or early `?` error return still decrements
//! (mirroring [`WriterDrainBarrier`](super::writer_drain_barrier::WriterDrainBarrier)'s
//! `enter_writer`/`WriterDrainGuard` shape — the established pattern in this
//! module for exactly this "in flight, decrement on every exit path" shape).
//!
//! `degraded_index_count()` subtracts this counter's current value (
//! `saturating_sub`, floored at zero) from the raw non-`Ready` tally. This
//! correctly keeps reporting a GENUINELY stuck index — one left `Building` by
//! a crash in a PAST process — because this counter is process-local and
//! starts at 0 on every restart, so a crash-orphaned `Building` state from
//! before this boot is never masked; only a create that is ACTUALLY in
//! flight in THIS process is excluded.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Process-local, per-`TableManager` count of `CREATE INDEX` operations
/// currently in flight (across all four index families). Shared across
/// `TableManager` clones via `Arc`, like the sibling barrier/counter fields.
#[derive(Debug)]
pub struct InFlightCreateCounter {
    count: Arc<AtomicU64>,
}

impl InFlightCreateCounter {
    pub fn new() -> Self {
        Self {
            count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Enter: bump the in-flight counter, return an RAII guard that
    /// decrements on drop (panic-safe and early-`?`-return-safe — no
    /// hand-rolled decrement is needed at any exit site).
    ///
    /// Call this as the very FIRST step of a create-index family method,
    /// before any fallible work, so every return path (including the
    /// earliest `?`) is covered by the guard's `Drop`.
    #[must_use]
    pub fn enter(&self) -> InFlightCreateGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        InFlightCreateGuard {
            count: Arc::clone(&self.count),
        }
    }

    /// Current in-flight count. Read by `degraded_index_count()` to
    /// `saturating_sub` out currently-live creates from the raw
    /// non-`Ready` tally. Plain atomic load — no lock, no store I/O.
    pub fn current(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
}

impl Clone for InFlightCreateCounter {
    fn clone(&self) -> Self {
        Self {
            count: Arc::clone(&self.count),
        }
    }
}

impl Default for InFlightCreateCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by [`InFlightCreateCounter::enter`]. Decrements the
/// counter on drop — covers panics and every early `?` return, mirroring
/// [`WriterDrainGuard`](super::writer_drain_barrier::WriterDrainGuard).
#[derive(Debug)]
pub struct InFlightCreateGuard {
    count: Arc<AtomicU64>,
}

impl Drop for InFlightCreateGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}
