//! Fixed-bucket atomic histogram with a torn-free seqlock snapshot
//! (design `engine-metrics.md` §3.1–3.4).

use crate::atomics::{fence, AtomicU64, Ordering};
use crate::bounds::MAX_BOUNDS;

/// Bounded reader retries before a best-effort `torn` copy (design §3.4).
const SNAPSHOT_RETRIES: u32 = 8;

/// Fixed static bucket bounds; inclusive `le` semantics with one implicit
/// overflow cell; integer `sum` in the bucket unit; no allocation in
/// `new` or `observe`.
pub struct Histogram {
    bounds: &'static [u64],
    /// Per-bound cells `[0..=n]`; index `n` is the overflow cell, the rest stay zero.
    cells: [AtomicU64; MAX_BOUNDS + 1],
    count: AtomicU64,
    sum: AtomicU64,
    /// Seqlock write counters: `started` is bumped before the data writes,
    /// `completed` after. Equal counters = no writer in flight.
    /// `pub(crate)` so the torn-path test can leave `completed` behind.
    pub(crate) started: AtomicU64,
    pub(crate) completed: AtomicU64,
}

/// Consistent point-in-time copy; `torn` marks a best-effort copy taken while
/// a writer stayed in flight for the whole retry budget (the exporter drops
/// such samples instead of blocking a scrape).
pub struct HistogramSnapshot {
    pub bounds: &'static [u64],
    /// `bounds.len() + 1` cells; the last is the overflow (`le = +Inf`) cell.
    pub buckets: Vec<u64>,
    pub count: u64,
    pub sum: u64,
    pub torn: bool,
}

#[cfg(not(loom))]
impl Histogram {
    /// Const constructor; panics (const assertion) when `bounds.len()` exceeds
    /// the fixed cell array.
    pub const fn new(bounds: &'static [u64]) -> Self {
        assert!(
            bounds.len() <= MAX_BOUNDS,
            "bounds exceed the fixed cell array"
        );
        Self {
            bounds,
            cells: [const { AtomicU64::new(0) }; MAX_BOUNDS + 1],
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
        }
    }
}

#[cfg(loom)]
impl Histogram {
    /// Loom atomics have no `const` constructor — non-const twin of the
    /// production constructor (precedent: `WriterDrainBarrier` loom gating).
    pub fn new(bounds: &'static [u64]) -> Self {
        assert!(
            bounds.len() <= MAX_BOUNDS,
            "bounds exceed the fixed cell array"
        );
        Self {
            bounds,
            cells: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
        }
    }
}

impl Histogram {
    /// Linear scan with early exit; bucket `i` counts `v <= bounds[i]`
    /// (inclusive `le`), values above the last bound land in the overflow
    /// cell. Three Relaxed `fetch_add`s inside the started/completed pair
    /// (four RMWs per `observe` with the counters).
    pub fn observe(&self, v: u64) {
        let idx = self
            .bounds
            .iter()
            .position(|&b| v <= b)
            .unwrap_or(self.bounds.len());
        self.started.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.cells[idx].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(v, Ordering::Relaxed);
        self.completed.fetch_add(1, Ordering::Release);
    }

    /// Two-counter seqlock snapshot (design §3.4).
    ///
    /// Soundness: equal counters at the first read mean no writer was in
    /// flight at that moment; an unchanged `started` after the copy means no
    /// writer began during it. A writer that did begin makes its data writes
    /// visible only after its Release fence, and the reader's Acquire fence
    /// before the final `started` read orders that visibility before the
    /// read — so any visible data write of a late writer forces
    /// `s2 != s1` and the attempt retries.
    ///
    /// After `SNAPSHOT_RETRIES` failures return a best-effort copy with
    /// `torn: true` instead of blocking the scrape.
    pub fn snapshot(&self) -> HistogramSnapshot {
        let n = self.bounds.len();
        let mut retry = 0;
        while retry < SNAPSHOT_RETRIES {
            let c1 = self.completed.load(Ordering::Acquire);
            let s1 = self.started.load(Ordering::Relaxed);
            if s1 != c1 {
                retry += 1;
                continue;
            }
            let buckets: Vec<u64> = self.cells[..=n]
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .collect();
            let count = self.count.load(Ordering::Relaxed);
            let sum = self.sum.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.started.load(Ordering::Relaxed) == s1 {
                return HistogramSnapshot {
                    bounds: self.bounds,
                    buckets,
                    count,
                    sum,
                    torn: false,
                };
            }
            retry += 1;
        }
        HistogramSnapshot {
            bounds: self.bounds,
            buckets: self.cells[..=n]
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .collect(),
            count: self.count.load(Ordering::Relaxed),
            sum: self.sum.load(Ordering::Relaxed),
            torn: true,
        }
    }
}
