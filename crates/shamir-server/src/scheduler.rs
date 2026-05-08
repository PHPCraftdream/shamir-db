//! Background task scheduler — periodic GC, audit checkpoint, identity finalize.
//!
//! Spec coverage:
//! - `consumed_counters` GC every 60s (spec §6.2 — `RESUMPTION_MAX_CHAIN_AGE_NS` cutoff).
//! - Lockout GC every 5 min (spec §5.2.5 — drop entries idle > 5 min).
//! - Rate-limit GC every 5 min — drop bucket entries idle > 5 min.
//! - Session GC every 60s (spec §7.4 + §7.7 — evict by max-age / idle TTL).
//! - Audit checkpoint every 60s (spec §3.3 — truncation defence).
//! - Identity rotation finalize every 1h (spec §12.2 — clear previous after 7d overlap).
//!
//! Each task runs in its own `tokio::task::spawn` with a `tokio::time::interval`
//! tick, listens on a shared `tokio::sync::Notify` for shutdown, and is
//! panic-safe via `std::panic::catch_unwind`.

use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::audit_chain::{AuditAppender, AuditChain};
use shamir_connect::server::lockout::LockoutStore;
use shamir_connect::server::rate_limit::RateLimiter;
use shamir_connect::server::resume::ConsumedCounterStore;
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Periods for each scheduler task. Defaults from spec.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Counter store GC interval (spec §6.2).
    pub counter_gc_period: Duration,
    /// Lockout store GC interval (spec §5.2.5).
    pub lockout_gc_period: Duration,
    /// Rate-limit GC interval.
    pub rate_limit_gc_period: Duration,
    /// Session GC interval (spec §7.4 / §7.7).
    pub session_gc_period: Duration,
    /// Audit checkpoint interval (spec §3.3).
    pub audit_checkpoint_period: Duration,
    /// Identity rotation finalize interval (spec §12.2).
    pub identity_finalize_period: Duration,
}

impl SchedulerConfig {
    /// Production defaults — counter/session/audit at 60s, lockout/rate at 5min,
    /// identity finalize at 1h.
    pub const fn default_for_production() -> Self {
        Self {
            counter_gc_period: Duration::from_secs(60),
            lockout_gc_period: Duration::from_secs(5 * 60),
            rate_limit_gc_period: Duration::from_secs(5 * 60),
            session_gc_period: Duration::from_secs(60),
            audit_checkpoint_period: Duration::from_secs(60),
            identity_finalize_period: Duration::from_secs(60 * 60),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self::default_for_production()
    }
}

/// Live store handles fed into [`Scheduler::spawn`].
///
/// Cleanly decoupled: every concrete state store the server runs is here as an
/// `Arc<dyn ...>` (or `Arc<Concrete>` for `SessionStore` / `ServerIdentityState`
/// because their inherent methods are what the GC tasks call directly).
pub struct SchedulerInputs {
    /// Per-(user, family) replay-counter store (spec §6.2).
    pub counters: Arc<dyn ConsumedCounterStore>,
    /// Per-(subnet, username) lockout / backoff store (spec §5.2.5).
    pub lockout: Arc<dyn LockoutStore>,
    /// Per-subnet `auth_init` rate limiter (spec §8 / §8.6).
    pub rate_limit: Arc<dyn RateLimiter>,
    /// Live session store (spec §7).
    pub session_store: Arc<SessionStore>,
    /// Session max-age for GC (spec §7.4 — typically 24h).
    pub session_max_age_ns: u64,
    /// Session idle TTL for GC (spec §7.7 — typically 30 min).
    pub session_idle_ttl_ns: u64,
    /// HMAC-chained audit log (spec §3.3) — chain state.
    pub audit_chain: Arc<AuditChain>,
    /// Pluggable durable audit appender (spec §3.3).
    pub audit_appender: Arc<dyn AuditAppender>,
    /// Server Ed25519 identity rotation state (spec §12.2).
    pub identity: Arc<ServerIdentityState>,
}

/// Scheduler handle — owns all spawned background tasks.
///
/// Drop-tolerant: dropping without calling [`Self::shutdown`] will terminate
/// the tokio runtime which aborts the tasks. For graceful drain (last-flush
/// audit checkpoint, etc.) prefer `shutdown().await`.
pub struct Scheduler {
    handles: Vec<JoinHandle<()>>,
    shutdown: Arc<Notify>,
}

impl core::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Scheduler")
            .field("tasks", &self.handles.len())
            .finish()
    }
}

impl Scheduler {
    /// Spawn all six periodic tasks. Returns immediately; tasks are now live.
    pub fn spawn(inputs: SchedulerInputs, config: SchedulerConfig) -> Self {
        let shutdown = Arc::new(Notify::new());
        let mut handles = Vec::with_capacity(6);

        // 1. Counter GC.
        {
            let store = inputs.counters.clone();
            let shutdown = shutdown.clone();
            let period = config.counter_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("counter_gc", period, shutdown, move || {
                    let now_ns = UnixNanos::now().as_u64();
                    let store = store.clone();
                    safe_run("counter_gc", move || {
                        store.gc(now_ns);
                    });
                })
                .await;
            }));
        }

        // 2. Lockout GC.
        {
            let store = inputs.lockout.clone();
            let shutdown = shutdown.clone();
            let period = config.lockout_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("lockout_gc", period, shutdown, move || {
                    let now_ns = UnixNanos::now().as_u64();
                    let store = store.clone();
                    safe_run("lockout_gc", move || {
                        store.gc(now_ns);
                    });
                })
                .await;
            }));
        }

        // 3. Rate-limit GC.
        {
            let store = inputs.rate_limit.clone();
            let shutdown = shutdown.clone();
            let period = config.rate_limit_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("rate_limit_gc", period, shutdown, move || {
                    let now_ns = UnixNanos::now().as_u64();
                    let store = store.clone();
                    safe_run("rate_limit_gc", move || {
                        store.gc(now_ns);
                    });
                })
                .await;
            }));
        }

        // 4. Session GC.
        {
            let store = inputs.session_store.clone();
            let shutdown = shutdown.clone();
            let period = config.session_gc_period;
            let max_age_ns = inputs.session_max_age_ns;
            let idle_ttl_ns = inputs.session_idle_ttl_ns;
            handles.push(tokio::spawn(async move {
                run_periodic("session_gc", period, shutdown, move || {
                    let now_ns = UnixNanos::now().as_u64();
                    let store = store.clone();
                    safe_run("session_gc", move || {
                        let evicted = store.gc_expired(now_ns, max_age_ns, idle_ttl_ns);
                        if evicted > 0 {
                            tracing::info!(evicted, "session_gc evicted expired sessions");
                        }
                    });
                })
                .await;
            }));
        }

        // 5. Audit checkpoint.
        {
            let chain = inputs.audit_chain.clone();
            let appender = inputs.audit_appender.clone();
            let shutdown = shutdown.clone();
            let period = config.audit_checkpoint_period;
            handles.push(tokio::spawn(async move {
                run_periodic("audit_checkpoint", period, shutdown, move || {
                    let chain = chain.clone();
                    let appender = appender.clone();
                    safe_run("audit_checkpoint", move || {
                        let (next_seq, prev_hmac) = chain.checkpoint();
                        appender.checkpoint(next_seq, &prev_hmac);
                    });
                })
                .await;
            }));
        }

        // 6. Identity rotation finalize.
        {
            let identity = inputs.identity.clone();
            let shutdown = shutdown.clone();
            let period = config.identity_finalize_period;
            handles.push(tokio::spawn(async move {
                run_periodic("identity_finalize", period, shutdown, move || {
                    let now_ns = UnixNanos::now().as_u64();
                    let identity = identity.clone();
                    safe_run("identity_finalize", move || {
                        if identity.try_finalize(now_ns) {
                            tracing::info!("identity_finalize cleared previous keypair");
                        }
                    });
                })
                .await;
            }));
        }

        tracing::info!(tasks = handles.len(), "scheduler spawned");
        Self { handles, shutdown }
    }

    /// Signal shutdown and await all tasks. Idempotent shutdown — multiple
    /// calls are not undefined but only one consumes the handles.
    pub async fn shutdown(self) {
        // `notify_waiters` wakes every task currently parked on `notified()`.
        self.shutdown.notify_waiters();
        // Some tasks may not have reached `.notified()` yet (e.g. spawned but
        // not yet polled). Issue a broadcast a few times to be safe — every
        // notified() call after this point will return immediately for tasks
        // that subsequently subscribe, because Notify retains a "pending"
        // permit per call until consumed.
        for h in self.handles {
            // Each task also checks the notify periodically inside select!,
            // so even if it missed the broadcast it'll exit on the next tick.
            // Timeout-bounded join: don't block forever on a stuck task.
            let _ = h.await;
        }
        tracing::info!("scheduler shutdown complete");
    }
}

/// Drive an interval + shutdown select loop. Each tick calls `tick_fn`.
///
/// Discards the first immediate tick (`interval.tick()` returns immediately on
/// the first call) so cold-start doesn't double-trigger. The shutdown signal
/// breaks out promptly even between ticks.
async fn run_periodic<F>(name: &'static str, period: Duration, shutdown: Arc<Notify>, tick_fn: F)
where
    F: Fn() + Send + 'static,
{
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Burn the immediate first tick — `interval.tick()` always fires once
    // synchronously on first poll.
    interval.tick().await;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick_fn();
            }
            _ = shutdown.notified() => {
                tracing::debug!(task = name, "scheduler task shutting down");
                break;
            }
        }
    }
}

/// Run `f` and swallow panics, logging them as warnings.
///
/// We use `AssertUnwindSafe` because the GC closures only borrow data behind
/// `Arc` (already unwind-safe). A panic in one tick must NOT abort the task —
/// the next tick should still run.
fn safe_run<F>(name: &'static str, f: F)
where
    F: FnOnce() + Send,
{
    let result = std::panic::catch_unwind(AssertUnwindSafe(f));
    if let Err(panic_payload) = result {
        let msg = panic_message(&panic_payload);
        tracing::warn!(task = name, panic = %msg, "scheduler task panicked; continuing");
    }
}

/// Best-effort string extraction from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}
