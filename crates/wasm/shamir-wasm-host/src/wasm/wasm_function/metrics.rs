//! Records call duration, consumed fuel, and depth high water per db/repo.
//! The hot path uses relaxed atomics and avoids allocation; the registry caps
//! named entries at 64 and sends overflow to the `(other)` aggregate.

use arc_swap::ArcSwap;
use shamir_metrics::{Histogram, HistogramSnapshot, LATENCY_NS_BOUNDS};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;

const MAX_NAMED_ENTRIES: usize = 64;

pub(crate) struct CallMetricsEntry {
    db: String,
    repo: String,
    duration: Histogram,
    fuel: AtomicU64,
    depth_high_water: AtomicU64,
}

impl CallMetricsEntry {
    fn new(db: String, repo: String) -> Self {
        Self {
            db,
            repo,
            duration: Histogram::new(LATENCY_NS_BOUNDS),
            fuel: AtomicU64::new(0),
            depth_high_water: AtomicU64::new(0),
        }
    }

    pub(crate) fn observe_duration_ns(&self, ns: u64) {
        self.duration.observe(ns);
    }

    pub(crate) fn add_fuel(&self, units: u64) {
        self.fuel.fetch_add(units, Ordering::Relaxed);
    }

    pub(crate) fn raise_depth(&self, depth: u32) {
        self.depth_high_water
            .fetch_max(u64::from(depth), Ordering::Relaxed);
    }

    pub(crate) fn db(&self) -> &str {
        &self.db
    }

    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    pub(crate) fn duration_snapshot(&self) -> HistogramSnapshot {
        self.duration.snapshot()
    }

    pub(crate) fn fuel_total(&self) -> u64 {
        self.fuel.load(Ordering::Relaxed)
    }

    pub(crate) fn depth_high_water(&self) -> u64 {
        self.depth_high_water.load(Ordering::Relaxed)
    }
}

struct CallMetricsRegistry {
    entries: ArcSwap<Vec<Arc<CallMetricsEntry>>>,
    aggregate: Arc<CallMetricsEntry>,
}

impl CallMetricsRegistry {
    fn new() -> Self {
        let aggregate = Arc::new(CallMetricsEntry::new(
            "(other)".to_string(),
            "(other)".to_string(),
        ));
        Self {
            entries: ArcSwap::from(Arc::new(vec![Arc::clone(&aggregate)])),
            aggregate,
        }
    }

    fn resolve(&self, db: Option<&str>, repo: &str) -> Arc<CallMetricsEntry> {
        let db = db.unwrap_or("");
        let entries = self.entries.load();
        if let Some(entry) = entries
            .iter()
            .find(|entry| !Arc::ptr_eq(entry, &self.aggregate) && entry.db == db && entry.repo == repo)
        {
            return Arc::clone(entry);
        }

        let named_count = entries
            .iter()
            .filter(|entry| !Arc::ptr_eq(entry, &self.aggregate))
            .count();
        if named_count >= MAX_NAMED_ENTRIES {
            return Arc::clone(&self.aggregate);
        }

        let candidate = Arc::new(CallMetricsEntry::new(
            db.to_string(),
            repo.to_string(),
        ));
        self.entries.rcu(|current| {
            if current.iter().any(|entry| {
                !Arc::ptr_eq(entry, &self.aggregate) && entry.db == db && entry.repo == repo
            }) || current
                .iter()
                .filter(|entry| !Arc::ptr_eq(entry, &self.aggregate))
                .count()
                >= MAX_NAMED_ENTRIES
            {
                return Arc::clone(current);
            }

            let mut updated = current.as_ref().clone();
            updated.push(Arc::clone(&candidate));
            Arc::new(updated)
        });

        let entries = self.entries.load();
        entries
            .iter()
            .find(|entry| !Arc::ptr_eq(entry, &self.aggregate) && entry.db == db && entry.repo == repo)
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::clone(&self.aggregate))
    }
}

static CALL_METRICS: LazyLock<CallMetricsRegistry> = LazyLock::new(CallMetricsRegistry::new);

pub struct CallMetricsSnapshot {
    pub db: String,
    pub repo: String,
    pub duration: HistogramSnapshot,
    pub fuel_total: u64,
    pub depth_high_water: u64,
}

pub fn call_metrics_snapshot() -> Vec<CallMetricsSnapshot> {
    CALL_METRICS
        .entries
        .load()
        .iter()
        .map(|entry| CallMetricsSnapshot {
            db: entry.db().to_string(),
            repo: entry.repo().to_string(),
            duration: entry.duration_snapshot(),
            fuel_total: entry.fuel_total(),
            depth_high_water: entry.depth_high_water(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn local_registry_capacity_totals() -> (usize, u64) {
    let registry = CallMetricsRegistry::new();
    for index in 0..70 {
        registry
            .resolve(Some("local-capacity-test"), &format!("repo-{index}"))
            .add_fuel(1);
    }
    let entries = registry.entries.load();
    let named = entries
        .iter()
        .filter(|entry| !Arc::ptr_eq(entry, &registry.aggregate))
        .count();
    (named, registry.aggregate.fuel_total())
}

pub(crate) fn resolve(db: Option<&str>, repo: &str) -> Arc<CallMetricsEntry> {
    CALL_METRICS.resolve(db, repo)
}
