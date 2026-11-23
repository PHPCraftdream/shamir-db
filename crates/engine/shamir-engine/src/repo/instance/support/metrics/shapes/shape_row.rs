use std::sync::atomic::{AtomicU64, Ordering};

use super::shape_stats_snapshot::ShapeCounters;

/// Lock-free counters for one canonical shape.
pub struct ShapeRow {
    executions: AtomicU64,
    total_nanos: AtomicU64,
    errors: AtomicU64,
    last_tick: AtomicU64,
}

impl Default for ShapeRow {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeRow {
    pub fn new() -> Self {
        Self {
            executions: AtomicU64::new(0),
            total_nanos: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            last_tick: AtomicU64::new(0),
        }
    }

    /// Record one execution. `last_tick` uses `fetch_max` so recency never
    /// regresses when two executions race.
    pub fn observe(&self, elapsed_ns: u64, failed: bool, tick: u64) {
        self.add(elapsed_ns, failed, tick);
    }

    /// Fold one execution using already-observed values, for a lost insert race.
    pub fn add(&self, elapsed_ns: u64, failed: bool, tick: u64) {
        self.executions.fetch_add(1, Ordering::Relaxed);
        self.total_nanos.fetch_add(elapsed_ns, Ordering::Relaxed);
        if failed {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.last_tick.fetch_max(tick, Ordering::Relaxed);
    }

    /// Fold another row's raw counters into this one.
    pub fn merge(&self, other: &ShapeRow) {
        self.executions
            .fetch_add(other.executions.load(Ordering::Relaxed), Ordering::Relaxed);
        self.total_nanos
            .fetch_add(other.total_nanos.load(Ordering::Relaxed), Ordering::Relaxed);
        self.errors
            .fetch_add(other.errors.load(Ordering::Relaxed), Ordering::Relaxed);
        self.last_tick
            .fetch_max(other.last_tick.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    pub fn counters(&self) -> ShapeCounters {
        ShapeCounters {
            executions: self.executions.load(Ordering::Relaxed),
            total_nanos: self.total_nanos.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }

    pub fn tick(&self) -> u64 {
        self.last_tick.load(Ordering::Relaxed)
    }
}
