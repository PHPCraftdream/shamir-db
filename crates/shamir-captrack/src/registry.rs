// Capacity-telemetry global registry — only compiled with the
// `capacity-telemetry` feature.
//
// Stores two counters per named call-site:
//   • peak_capacity  — maximum observed capacity across all instances (AtomicUsize).
//   • creation_count — total number of instances created (AtomicU64).
//
// The registry is a lock-free `scc::HashMap` with `ShamirHasher` (FxHasher),
// initialised once via `OnceLock`.  Keys are `&'static str` (macro literals)
// so they live for the whole program.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::ShamirHasher;

/// Per-name capacity statistics accumulated over the lifetime of the process.
pub struct CapStats {
    pub peak_capacity: AtomicUsize,
    pub creation_count: AtomicU64,
}

impl CapStats {
    fn new() -> Self {
        Self {
            peak_capacity: AtomicUsize::new(0),
            creation_count: AtomicU64::new(0),
        }
    }
}

type Registry = scc::HashMap<&'static str, CapStats, ShamirHasher>;

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Return a reference to the process-global registry, initialising it on
/// first call.
pub(crate) fn registry() -> &'static Registry {
    // `with_hasher` is not banned by the workspace disallowed-methods list.
    // Only `scc::HashMap::new` and `scc::HashMap::with_capacity` are banned.
    REGISTRY.get_or_init(|| scc::HashMap::with_hasher(ShamirHasher::default()))
}

/// Record one new creation for `name`.  Called from every `with_capacity_named`
/// constructor.  Thread-safe: the scc map is lock-free; the counter update is
/// `Relaxed` (ordering against the Drop `record_peak` is not required — both
/// are monotone operations on separate fields).
pub fn record_creation(name: &'static str) {
    let reg = registry();
    // Fast-path: entry already present.
    if let Some(entry) = reg.get(&name) {
        entry.creation_count.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Slow-path: first time we see this name.  Two threads may race here;
    // scc::HashMap::insert returns Err if the key is already present (the
    // other thread won the race) — both outcomes are correct.
    let _ = reg.insert(name, CapStats::new());
    // After insert (whether ours or the other thread's) the entry exists.
    if let Some(entry) = reg.get(&name) {
        entry.creation_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Update the peak capacity for `name`.  Called from every `Drop` impl.
/// `fetch_max` is atomic so concurrent drops are safe.
pub fn record_peak(name: &'static str, cap: usize) {
    if let Some(entry) = registry().get(&name) {
        entry.peak_capacity.fetch_max(cap, Ordering::Relaxed);
    }
    // If the entry is somehow absent (can only happen if record_creation was
    // never called for this name, which violates our contract) we silently
    // ignore — telemetry is best-effort.
}
