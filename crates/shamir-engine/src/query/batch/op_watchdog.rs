//! OS-thread-based watchdog for stuck batch ops (#1094).
//!
//! This module provides a live diagnostic watchdog that logs "batch op 'alias'
//! still running after Ns" WHILE a single op is stuck inside its own `.await`.
//! This closes the observability gap documented in `execution_deadline.rs`'s
//! "Deliberate non-goal" section: `ExecutionDeadline`'s cooperative checkpoints
//! and the existing post-completion `warn_if_op_slow` both only notice/log an
//! overrun AFTER the stalled op returns — an op that never returns produces NO
//! signal at all today.
//!
//! The watchdog is **strictly additive/observational** — it NEVER cancels or
//! otherwise affects the op's execution. It runs on its own OS thread
//! (`std::thread::spawn`, not `tokio::spawn`) to avoid the stack-overflow
//! hazards that in-process techniques (`tokio::select!` / `tokio::spawn`)
//! demonstrated in the #1085 investigation (see `execution_deadline.rs` for the
//! full analysis).
//!
//! # Design overview
//!
//! 1. **Registry**: a lock-free `scc::HashMap<u64, (String, Instant, bool), THasher>`
//!    mapping op-id → (alias, started_at, already_warned). The registry is static
//!    (`OnceLock`) to avoid threading it through signatures.
//! 2. **Registration**: immediately before each `.await` on `execute_single_impl`,
//!    we insert `(alias.to_string(), Instant::now(), false)` under a fresh
//!    `AtomicU64` id; immediately after the `.await` resolves (success OR error),
//!    we remove that entry. This is a couple of lock-free map ops on the hot path —
//!    negligible cost, and all state is heap-allocated (not stack-resident).
//! 3. **Watchdog thread**: ONE `std::thread::spawn` started lazily on first use.
//!    The thread loops: sleep 1s, scan the registry, and `log::warn!` for any
//!    entry whose `elapsed()` exceeds `SLOW_OP_WARN_THRESHOLD`. Each op triggers
//!    at most ONE warning (tracked via the `already_warned` field) to avoid spam.
//! 4. **No cancellation/interruption**: the watchdog thread ONLY reads the
//!    registry and logs — it never touches the op's execution, never sends a
//!    cancel signal, never panics the op's task.
//! 5. **Shutdown**: the watchdog thread is a daemon-style thread that dies with
//!    the process. This is acceptable for a diagnostic-only thread with no owned
//!    resources needing flush/cleanup — it holds no file handles, no channels
//!    requiring graceful shutdown, no state that must be persisted. The registry
//!    is `Arc`-wrapped and lives for the process lifetime, so there's no
//!    double-drop concern.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use scc::HashMap;

use shamir_collections::THasher;

/// Same threshold as `SLOW_OP_WARN_THRESHOLD` in `batch_execute.rs` for
/// consistency — an op taking longer than this triggers a "still running"
/// warning from the watchdog.
const WATCHDOG_WARN_THRESHOLD: Duration = Duration::from_secs(1);

/// Watchdog poll interval — scan the registry every 1 second.
const WATCHDOG_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Registry entry: (alias, started_at, already_warned).
type RegistryEntry = (String, Instant, bool);

/// The global registry of in-flight ops. `OnceLock` for lazy initialization.
pub(crate) static REGISTRY: OnceLock<Arc<HashMap<u64, RegistryEntry, THasher>>> = OnceLock::new();

/// Next unique op-id. `AtomicU64` for lock-free allocation.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Test-only counter to verify only one watchdog thread is spawned.
#[cfg(test)]
pub(crate) static THREAD_SPAWN_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Watchdog thread handle. `OnceLock` ensures we spawn exactly one thread.
static WATCHDOG_THREAD: OnceLock<std::thread::JoinHandle<()>> = OnceLock::new();

/// Guard that removes an op from the registry when dropped.
pub struct OpGuard {
    id: u64,
}

impl OpGuard {
    fn new(id: u64) -> Self {
        Self { id }
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        if let Some(registry) = REGISTRY.get() {
            // Note: we ignore the result if the entry was already removed
            // (e.g., by another thread).
            let _ = registry.remove_sync(&self.id);
        }
    }
}

/// Initialize the watchdog thread (lazy, called on first registration).
fn init_watchdog() {
    WATCHDOG_THREAD.get_or_init(|| {
        // Test-only: increment spawn counter to verify thread creation.
        #[cfg(test)]
        THREAD_SPAWN_COUNT.fetch_add(1, Ordering::SeqCst);

        let registry = REGISTRY.get_or_init(|| Arc::new(HashMap::with_hasher(THasher::default())));
        let registry_for_thread = Arc::clone(registry);

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(WATCHDOG_POLL_INTERVAL);

                let now = Instant::now();

                // Scan the registry and log warnings for stuck ops.
                // Use a Vec to collect IDs to update, then do a second pass.
                let mut ids_to_update: Vec<u64> = Vec::new();

                registry_for_thread
                    .iter_sync(|id, (alias, started_at, already_warned)| {
                        let elapsed = now.duration_since(*started_at);

                        if elapsed > WATCHDOG_WARN_THRESHOLD && !*already_warned {
                            log::warn!(
                                "batch op '{alias}' still running after {elapsed:?} (exceeds the {WATCHDOG_WARN_THRESHOLD:?} slow-op watchdog threshold)"
                            );
                            ids_to_update.push(*id);
                        }

                        true // Continue iteration
                    });

                // Update the already_warned flag for ops we warned about.
                for id in ids_to_update {
                    // Use read_sync to check if the entry still exists, then update if needed.
                    let mut update = false;
                    let _ = registry_for_thread.read_sync(&id, |_, entry| {
                        update = !entry.2;
                    });

                    if update {
                        // Try to update the entry with already_warned = true.
                        // This is a best-effort CAS-like update.
                        let _ = registry_for_thread.update_sync(&id, |_, entry| {
                            entry.2 = true;
                            true
                        });
                    }
                }
            }
        })
    });
}

/// Register an op as starting, returning a guard that removes it on drop.
///
/// Call this immediately before awaiting `execute_single_impl`:
///
/// ```ignore
/// let guard = register_op_watchdog(alias);
/// let result = execute_single_impl(...).await?;
/// // guard is dropped here, removing the entry from the registry
/// ```
pub fn register_op_watchdog(alias: &str) -> OpGuard {
    // Initialize the watchdog thread on first call.
    init_watchdog();

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    if let Some(registry) = REGISTRY.get() {
        let _ = registry.insert_sync(id, (alias.to_string(), Instant::now(), false));
    }

    OpGuard::new(id)
}
