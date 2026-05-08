//! Integration tests for the background-task scheduler.
//!
//! Coverage:
//! - Smoke: spawn 6 tasks then `shutdown` joins them cleanly.
//! - Counter GC: a stub `ConsumedCounterStore` records calls; spawn with a
//!   short interval, observe `gc()` was invoked at least once.
//! - Lockout GC: same pattern with a stub `LockoutStore`.
//! - Audit checkpoint: a `MockAuditAppender` counts `checkpoint(...)` calls.

use shamir_connect::common::types::limits;
use shamir_connect::server::audit_chain::{AuditAppender, AuditChain, AuditEntry};
use shamir_connect::server::lockout::{
    FailureOutcome, InMemoryLockoutStore, LockoutStore, PairKey,
};
use shamir_connect::server::rate_limit::{InMemoryRateLimiter, RateDecision, RateLimiter};
use shamir_connect::server::resume::{ConsumedCounterStore, InMemoryConsumedCounters};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;
use shamir_server::scheduler::{Scheduler, SchedulerConfig, SchedulerInputs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

/// Counter store stub that records every `gc()` invocation.
#[derive(Default)]
struct CountingCounterStore {
    gc_calls: AtomicUsize,
}

impl ConsumedCounterStore for CountingCounterStore {
    fn try_advance(
        &self,
        _user_id: &[u8; 16],
        _family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        _new_counter: u64,
    ) -> bool {
        true
    }

    fn gc(&self, _now_ns: u64) {
        self.gc_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Lockout store stub that counts `gc()` calls.
#[derive(Default)]
struct CountingLockoutStore {
    gc_calls: AtomicUsize,
}

impl LockoutStore for CountingLockoutStore {
    fn register_failure(&self, _key: PairKey, _now_ns: u64) -> FailureOutcome {
        FailureOutcome::Backoff { delay_ms: 0 }
    }
    fn reset_on_success(&self, _key: PairKey) {}
    fn is_locked_out(&self, _key: PairKey, _now_ns: u64) -> bool {
        false
    }
    fn current_backoff_ms(&self, _key: PairKey, _now_ns: u64) -> u64 {
        0
    }
    fn admin_unlock_user(&self, _username_hash: [u8; 16]) {}
    fn gc(&self, _now_ns: u64) {
        self.gc_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Rate limiter stub that counts `gc()` calls.
#[derive(Default)]
struct CountingRateLimiter {
    gc_calls: AtomicUsize,
}

impl RateLimiter for CountingRateLimiter {
    fn check(&self, _subnet: shamir_connect::server::lockout::Subnet, _now_ns: u64) -> RateDecision {
        RateDecision::Allowed
    }
    fn gc(&self, _now_ns: u64) {
        self.gc_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Audit appender mock that counts `append_entry` and `checkpoint` calls.
#[derive(Default)]
struct MockAuditAppender {
    appends: AtomicUsize,
    checkpoints: AtomicUsize,
}

impl AuditAppender for MockAuditAppender {
    fn append_entry(&self, _entry: &AuditEntry) {
        self.appends.fetch_add(1, Ordering::SeqCst);
    }
    fn checkpoint(&self, _next_seq: u64, _prev_hmac: &[u8; 32]) {
        self.checkpoints.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fast_config() -> SchedulerConfig {
    SchedulerConfig {
        counter_gc_period: Duration::from_millis(50),
        lockout_gc_period: Duration::from_millis(50),
        rate_limit_gc_period: Duration::from_millis(50),
        session_gc_period: Duration::from_millis(50),
        audit_checkpoint_period: Duration::from_millis(50),
        identity_finalize_period: Duration::from_millis(50),
    }
}

fn build_inputs(
    counters: Arc<dyn ConsumedCounterStore>,
    lockout: Arc<dyn LockoutStore>,
    rate_limit: Arc<dyn RateLimiter>,
    audit_appender: Arc<dyn AuditAppender>,
) -> SchedulerInputs {
    SchedulerInputs {
        counters,
        lockout,
        rate_limit,
        session_store: Arc::new(SessionStore::new()),
        session_max_age_ns: 24 * 60 * 60 * 1_000_000_000,
        session_idle_ttl_ns: 30 * 60 * 1_000_000_000,
        audit_chain: Arc::new(AuditChain::new([0u8; 32])),
        audit_appender,
        identity: Arc::new(ServerIdentityState::fresh()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_creates_tasks_then_shutdown_joins_them() {
    let counters: Arc<dyn ConsumedCounterStore> = Arc::new(InMemoryConsumedCounters::new());
    let lockout: Arc<dyn LockoutStore> = Arc::new(InMemoryLockoutStore::new());
    let rate_limit: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(0));
    let appender: Arc<dyn AuditAppender> = Arc::new(MockAuditAppender::default());

    let inputs = build_inputs(counters, lockout, rate_limit, appender);
    let scheduler = Scheduler::spawn(inputs, SchedulerConfig::default_for_production());

    // Immediate shutdown — should not panic, all 6 handles must complete.
    scheduler.shutdown().await;
}

#[tokio::test]
async fn counter_gc_runs_at_interval() {
    let counters_concrete: Arc<CountingCounterStore> = Arc::new(CountingCounterStore::default());
    let counters: Arc<dyn ConsumedCounterStore> = counters_concrete.clone();
    let lockout: Arc<dyn LockoutStore> = Arc::new(InMemoryLockoutStore::new());
    let rate_limit: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(0));
    let appender: Arc<dyn AuditAppender> = Arc::new(MockAuditAppender::default());

    let inputs = build_inputs(counters, lockout, rate_limit, appender);
    let scheduler = Scheduler::spawn(inputs, fast_config());

    // 50ms period × ≥4 ticks = 200ms gives us at least 2-3 invocations.
    tokio::time::sleep(Duration::from_millis(250)).await;
    scheduler.shutdown().await;

    let calls = counters_concrete.gc_calls.load(Ordering::SeqCst);
    assert!(
        calls >= 1,
        "counter_gc should have been called at least once; got {} calls",
        calls
    );
}

#[tokio::test]
async fn lockout_gc_runs_at_interval() {
    let counters: Arc<dyn ConsumedCounterStore> = Arc::new(InMemoryConsumedCounters::new());
    let lockout_concrete: Arc<CountingLockoutStore> = Arc::new(CountingLockoutStore::default());
    let lockout: Arc<dyn LockoutStore> = lockout_concrete.clone();
    let rate_limit: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(0));
    let appender: Arc<dyn AuditAppender> = Arc::new(MockAuditAppender::default());

    let inputs = build_inputs(counters, lockout, rate_limit, appender);
    let scheduler = Scheduler::spawn(inputs, fast_config());

    tokio::time::sleep(Duration::from_millis(250)).await;
    scheduler.shutdown().await;

    let calls = lockout_concrete.gc_calls.load(Ordering::SeqCst);
    assert!(
        calls >= 1,
        "lockout_gc should have been called at least once; got {}",
        calls
    );
}

#[tokio::test]
async fn audit_checkpoint_runs_at_interval() {
    let counters: Arc<dyn ConsumedCounterStore> = Arc::new(InMemoryConsumedCounters::new());
    let lockout: Arc<dyn LockoutStore> = Arc::new(InMemoryLockoutStore::new());
    let rate_limit: Arc<dyn RateLimiter> = Arc::new(InMemoryRateLimiter::new(0));
    let appender_concrete: Arc<MockAuditAppender> = Arc::new(MockAuditAppender::default());
    let appender: Arc<dyn AuditAppender> = appender_concrete.clone();

    let inputs = build_inputs(counters, lockout, rate_limit, appender);
    let scheduler = Scheduler::spawn(inputs, fast_config());

    tokio::time::sleep(Duration::from_millis(250)).await;
    scheduler.shutdown().await;

    let checkpoints = appender_concrete.checkpoints.load(Ordering::SeqCst);
    assert!(
        checkpoints >= 1,
        "audit_checkpoint should have been called at least once; got {}",
        checkpoints
    );
}

#[tokio::test]
async fn rate_limit_gc_runs_at_interval() {
    let counters: Arc<dyn ConsumedCounterStore> = Arc::new(InMemoryConsumedCounters::new());
    let lockout: Arc<dyn LockoutStore> = Arc::new(InMemoryLockoutStore::new());
    let rate_concrete: Arc<CountingRateLimiter> = Arc::new(CountingRateLimiter::default());
    let rate_limit: Arc<dyn RateLimiter> = rate_concrete.clone();
    let appender: Arc<dyn AuditAppender> = Arc::new(MockAuditAppender::default());

    let inputs = build_inputs(counters, lockout, rate_limit, appender);
    let scheduler = Scheduler::spawn(inputs, fast_config());

    tokio::time::sleep(Duration::from_millis(250)).await;
    scheduler.shutdown().await;

    let calls = rate_concrete.gc_calls.load(Ordering::SeqCst);
    assert!(
        calls >= 1,
        "rate_limit_gc should have been called at least once; got {}",
        calls
    );
}
