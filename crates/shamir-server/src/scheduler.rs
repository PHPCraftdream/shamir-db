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
use tokio::sync::broadcast;
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
///
/// Shutdown is signalled via a `tokio::sync::broadcast::Sender<()>` rather
/// than `tokio::sync::Notify`. The latter has no "pending permit" semantics:
/// `notify_waiters()` only wakes tasks that are *already* parked on
/// `.notified()`. If `Scheduler::shutdown()` is called before every spawned
/// task has had a chance to enter its `select!`, the notify is silently
/// dropped and the task waits for the next `interval.tick()` — which can be
/// up to an hour for `identity_finalize`. broadcast retains the message for
/// any future `recv()`, so the race is closed.
pub struct Scheduler {
    handles: Vec<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
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
        // Capacity 1 is enough — we only ever send a single shutdown message.
        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        let mut handles = Vec::with_capacity(6);

        // Each subscriber MUST be created synchronously (in `spawn`, not
        // inside the async block) so the receiver exists before `Scheduler::
        // spawn` returns. Otherwise an immediate `shutdown()` could race
        // with the task starting up.

        // 1. Counter GC.
        {
            let store = inputs.counters.clone();
            let rx = shutdown_tx.subscribe();
            let period = config.counter_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("counter_gc", period, rx, move || {
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
            let rx = shutdown_tx.subscribe();
            let period = config.lockout_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("lockout_gc", period, rx, move || {
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
            let rx = shutdown_tx.subscribe();
            let period = config.rate_limit_gc_period;
            handles.push(tokio::spawn(async move {
                run_periodic("rate_limit_gc", period, rx, move || {
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
            let rx = shutdown_tx.subscribe();
            let period = config.session_gc_period;
            let max_age_ns = inputs.session_max_age_ns;
            let idle_ttl_ns = inputs.session_idle_ttl_ns;
            handles.push(tokio::spawn(async move {
                run_periodic("session_gc", period, rx, move || {
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
            let rx = shutdown_tx.subscribe();
            let period = config.audit_checkpoint_period;
            handles.push(tokio::spawn(async move {
                run_periodic("audit_checkpoint", period, rx, move || {
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
            let rx = shutdown_tx.subscribe();
            let period = config.identity_finalize_period;
            handles.push(tokio::spawn(async move {
                run_periodic("identity_finalize", period, rx, move || {
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
        Self { handles, shutdown_tx }
    }

    /// Signal shutdown and await all tasks.
    ///
    /// Calling `shutdown_tx.send(())` deposits the message in every
    /// subscriber's queue — even subscribers that haven't yet reached their
    /// `.recv()` will pick it up on the next poll. Once the sender is
    /// dropped (which happens when `self` is consumed at the bottom of this
    /// function via the implicit move-into-the-Vec drain), any remaining
    /// `recv()` calls also return `Err(Closed)` which the loop treats as
    /// shutdown.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        for h in self.handles {
            let _ = h.await;
        }
        tracing::info!("scheduler shutdown complete");
    }
}

/// Drive an interval + shutdown select loop. Each tick calls `tick_fn`.
///
/// Discards the first immediate tick (`interval.tick()` returns immediately on
/// the first call) so cold-start doesn't double-trigger. Shutdown breaks out
/// either via an explicit `()` broadcast OR via `Closed` (sender dropped) —
/// both treated identically.
async fn run_periodic<F>(
    name: &'static str,
    period: Duration,
    mut shutdown_rx: broadcast::Receiver<()>,
    tick_fn: F,
) where
    F: Fn() + Send + 'static,
{
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Burn the immediate first tick — `interval.tick()` always fires once
    // synchronously on first poll.
    interval.tick().await;
    loop {
        tokio::select! {
            biased;
            // Bias the shutdown branch so a notified shutdown wins over a
            // simultaneously-ready tick. `recv()` returns Ok(()) on send,
            // Err(Lagged) if we missed messages (impossible with capacity=1
            // and a single send), or Err(Closed) when sender is dropped.
            // All three are "exit now".
            res = shutdown_rx.recv() => {
                tracing::debug!(task = name, ?res, "scheduler task shutting down");
                break;
            }
            _ = interval.tick() => {
                tick_fn();
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
