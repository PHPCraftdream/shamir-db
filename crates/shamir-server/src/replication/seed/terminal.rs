use std::sync::Arc;
use std::time::Duration;

use shamir_collections::TMap;
use shamir_db::query::admin::ReplStream;
use shamir_db::query::read::{QueryResult, ReadQuery};
use shamir_db::repl_binding::SubscriptionBinding;
use shamir_db::types::value::QueryValue;
use shamir_db::{DbResult, SeedRefusal, ShamirDb};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD;
use crate::replication::source::ReplSource;
use crate::replication::ReplError;

/// Round-6 item 2 currency check: read the CURRENT `system/subscriptions`
/// row for `name` (server-side read, same path as
/// `supervisor.rs::read_subscription_row`) and report whether it exists with
/// `binding_generation == expected`. Any read error counts as NOT current
/// (fail-safe: the retry aborts rather than mark a possibly-replaced
/// binding).
async fn binding_generation_is_current(local: &ShamirDb, name: &str, expected: u64) -> bool {
    let Ok(table) = local.system_store().subscriptions_table().await else {
        return false;
    };
    let Ok(records) = read_all(&table, "subscriptions").await else {
        return false;
    };
    records
        .into_iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name))
        .and_then(|r| r.get("binding_generation").and_then(QueryValue::as_u64))
        .is_some_and(|generation| generation == expected)
}

/// Bounded, cancellation-aware retry for terminal catalogue marking (round 5,
/// B3; round 6 item 2). Up to 5 attempts; backoff 100 ms doubling, capped at
/// 1600 ms. Each wait is a `tokio::select!` against `tok`'s cancellation, and
/// the retry aborts early when `tok` is cancelled, the CURRENT subscription
/// row no longer carries `expected_generation` (a replaced or dropped binding
/// — the mark itself would be a logged no-op inside `ShamirDb`), or the mark
/// succeeded.
///
/// Round-6 item 2 — WHY the guard is generation CURRENCY, not `lease.valid`:
/// the terminal mark itself quiesces/invalidates the subscription's lease
/// first (`admin_replication.rs::mark_subscription_terminal_generation` →
/// `quiesce_subscription_lease_locked`), so after the FIRST persistence
/// failure a `lease.valid` guard sees an already-invalid lease and every
/// later retry exits immediately — the mark never lands. The row's
/// `binding_generation`, by contrast, is untouched by the mark's own quiesce
/// and flips only when the binding is genuinely replaced/dropped.
///
/// Every failed attempt logs `warn!` with the attempt number and error;
/// exhaustion logs "terminal marking retries exhausted".
///
/// LOCK CONSTRAINT: the retry runs AFTER the follower loop returned (its
/// apply-gate read guard is dropped), so taking the replication control lock
/// inside `mark_*` cannot overlap an in-flight apply of THIS loop. NEVER call
/// this while any apply-gate read guard is held.
pub(crate) async fn mark_with_retry<F, Fut>(
    local: &ShamirDb,
    name: &str,
    op: &str,
    tok: &CancellationToken,
    expected_generation: u64,
    mut mark: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = DbResult<()>>,
{
    const MAX_ATTEMPTS: u32 = 5;
    let mut delay_ms = 100u64;
    for attempt in 1..=MAX_ATTEMPTS {
        if tok.is_cancelled()
            || !binding_generation_is_current(local, name, expected_generation).await
        {
            return;
        }
        match mark().await {
            Ok(()) => return,
            Err(error) => {
                if attempt == MAX_ATTEMPTS {
                    tracing::warn!(
                        subscription = %name,
                        op = %op,
                        error = %error,
                        "terminal marking retries exhausted"
                    );
                    return;
                }
                tracing::warn!(
                    subscription = %name,
                    op = %op,
                    attempt,
                    error = %error,
                    "terminal marking failed; retrying"
                );
                tokio::select! {
                    _ = tok.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                }
                delay_ms = (delay_ms * 2).min(1600);
            }
        }
    }
}

/// [`persist_terminal`] with the round-5 B3 bounded retry (see
/// [`mark_with_retry`] for the retry shape, the generation-currency guard and
/// the lock constraint).
pub(crate) async fn persist_terminal_with_retry(
    home: &ShamirDb,
    name: &str,
    error: &ReplError,
    generation: u64,
    tok: &CancellationToken,
) {
    let (state, message) = match error {
        ReplError::Seed(reason) => (reason.code(), error.to_string()),
        ReplError::IdentityMismatch { code, .. } => (code.code(), error.to_string()),
        _ => return,
    };
    mark_with_retry(home, name, "persist_terminal", tok, generation, || {
        let home = home.clone();
        let name = name.to_string();
        let state = state.to_string();
        let message = message.clone();
        async move {
            home.mark_subscription_terminal_generation(&name, &state, &message, Some(generation))
                .await
        }
    })
    .await;
}

/// Preserve the typed seed refusal code instead of leaving an identity/schema
/// failure as a log line only. The S5c-X PR7 universal identity fence
/// (`ReplError::IdentityMismatch`) is persisted the same way, so the recorded
/// and observed identities remain available in per-repository ReplicationStatus.
/// Other terminal errors keep their prior handling. The mark is
/// generation-aware (`mark_subscription_terminal_generation(..., Some(generation))`):
/// a terminal callback from a task started under an older binding generation
/// is a logged no-op once the row was replaced under a new generation.
// Test-only (loop_tests.rs, `#![cfg(unix)]`): unused on non-Unix test builds.
#[cfg(all(test, unix))]
pub(crate) async fn persist_terminal(
    home: &ShamirDb,
    name: &str,
    error: &ReplError,
    generation: u64,
) {
    let (state, message) = match error {
        ReplError::Seed(reason) => (reason.code(), error.to_string()),
        ReplError::IdentityMismatch { code, .. } => (code.code(), error.to_string()),
        _ => return,
    };
    if let Err(mark_error) = home
        .mark_subscription_terminal_generation(name, state, &message, Some(generation))
        .await
    {
        tracing::warn!(error = %mark_error, "failed to persist terminal replication reason");
    }
}

pub(crate) async fn validate_target(
    home: &ShamirDb,
    streams: &[shamir_query_types::admin::ReplStream],
    db: &str,
    repo: &str,
) -> Result<(), ReplError> {
    if let Some(receipt) = super::fence::load_receipt(home, db, repo).await? {
        if streams.len() != 1
            || streams[0].direction != shamir_query_types::admin::ReplDirection::Pull
            || streams[0].mode != shamir_query_types::admin::ReplMode::ReadOnly
            || streams[0].scope.db != receipt.source_db
            || streams[0].scope.repo.as_deref() != Some(receipt.source_repo.as_str())
            || streams[0].scope.table.is_some()
        {
            return Err(SeedRefusal::WrongScope.into());
        }
    }
    Ok(())
}

/// PR6b-3 stall visibility: once [`STALL_VISIBILITY_THRESHOLD`] consecutive
/// swallowed failures accumulate without durable-bookmark progress, persist
/// a `status_message` on the subscription row so operators (and tests) can
/// OBSERVE the stalled loop via the existing status/list admin surface.
///
/// Mechanism choice: [`ShamirDb::mark_subscription_terminal`] with
/// `state = "active"` is the lightest EXISTING persistent write (there is
/// no status-only API in `shamir-db` and production changes outside
/// `shamir-server` are out of scope). It writes `status_message` +
/// `last_terminal_reason` but — because the state is not
/// `resync_required` — installs NO quarantine, leaves the row `active` so
/// `reconcile()` keeps (re)starting the loop, and the loop itself keeps
/// retrying: the stall stays visible, not terminal.
///
/// Fires once per stall streak (`stall_reported` guard); cleared by
/// [`clear_stall`] on the next durable progress.
pub(crate) async fn note_swallowed_failure(
    db: &ShamirDb,
    node_id: &str,
    subscription: Option<&str>,
    failures: usize,
    last_error: &str,
    stall_reported: &mut bool,
) {
    if failures < STALL_VISIBILITY_THRESHOLD || *stall_reported {
        return;
    }
    let message = format!(
        "follower stalled: {failures} consecutive failures without progress; \
         last error: {last_error}"
    );
    let Some(name) = subscription else {
        warn!(
            node_id = %node_id,
            failures,
            "follower_loop: stalled without progress (no subscription row to mark)"
        );
        return;
    };
    // R7-5: the latch is set ONLY after a SUCCESSFUL persistence — on Err it
    // stays clear so the NEXT failure retries the write (a failed write must
    // never silence the stall permanently).
    match db
        .mark_subscription_terminal(name, "active", &message)
        .await
    {
        Ok(()) => {
            *stall_reported = true;
            warn!(
                node_id = %node_id,
                subscription = %name,
                failures,
                "follower_loop: stall persisted to subscription status_message"
            );
        }
        Err(e) => warn!(
            node_id = %node_id,
            subscription = %name,
            error = %e,
            "follower_loop: failed to persist stall status_message (will retry on next failure)"
        ),
    }
}

/// PR6b-3: the bookmark advanced after a reported stall — overwrite the
/// stall `status_message` with a progress note so a RECOVERED loop is as
/// visible as a stalled one was.
pub(crate) async fn clear_stall(
    db: &ShamirDb,
    node_id: &str,
    subscription: Option<&str>,
    bookmark: u64,
    stall_reported: &mut bool,
) {
    *stall_reported = false;
    let Some(name) = subscription else {
        return;
    };
    let message = format!("replication progress restored; durable bookmark at {bookmark}");
    if let Err(e) = db
        .mark_subscription_terminal(name, "active", &message)
        .await
    {
        warn!(
            node_id = %node_id,
            subscription = %name,
            error = %e,
            "follower_loop: failed to persist progress status_message"
        );
    } else {
        info!(
            node_id = %node_id,
            subscription = %name,
            bookmark,
            "follower_loop: stall cleared (bookmark advanced)"
        );
    }
}

/// A subscription resolved from `system/subscriptions` (386-a schema).
///
/// Passed to the [`ReplSourceFactory`] so it can build the transport
/// (`WireReplSource` in prod, `InProcessReplSource` in tests) with the
/// subscription's `upstream` endpoint and credentials.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Subscription name (registry key).
    pub name: String,
    /// Upstream leader endpoint string (e.g. `tcp://leader:9000`). For R1 the
    /// factory owns credential resolution (see the module TODO on creds).
    pub upstream: String,
    /// Publication name advertised by the upstream.
    pub publication: String,
    /// Bound replication-profile name (resolved to `pull` streams).
    pub profile: String,
    /// `active` → loop should run; `paused` (or anything else) → stopped.
    pub state: String,
    /// S5c-X PR8b-2: the row's durable binding generation. The DURABLE
    /// authority for loop identity on reconcile — a loop started under an
    /// older generation must never survive to mark or clear a replacement
    /// binding.
    pub binding_generation: u64,
    /// S5c-X PR8b-2: the row's durable destination set — the ENFORCED write
    /// boundary for follower targets and the coverage authority for
    /// quarantine. The replication profile is NOT the authority.
    pub binding_destinations: Vec<(String, String)>,
}

impl Subscription {
    /// `true` when the row's `state` field is exactly `active`.
    pub(crate) fn is_active(&self) -> bool {
        self.state == "active"
    }
}

/// Factory that builds a [`ReplSource`] for a subscription.
///
/// This is the seam that makes the supervisor unit-testable: production wires
/// a `WireReplSource` (a TLS+SCRAM `shamir_client::Client` connected to
/// `sub.upstream` under a `replicator` account), tests wire an
/// `InProcessReplSource` over a leader `Arc<ShamirDb>`. The supervisor never
/// looks at credentials — the factory owns them.
///
/// TODO(386-c): upstream credentials. Today the factory carries whatever creds
/// it needs (prod: a config/env `replicator` account keyed by `upstream`).
/// A dedicated per-subscription credential store on the catalogue row is
/// future work — `Subscription.upstream` is the only endpoint hint available.
pub type ReplSourceFactory =
    Arc<dyn Fn(&Subscription) -> Arc<dyn ReplSource> + Send + Sync + 'static>;

impl crate::replication::supervisor::SubscriptionSupervisor {
    /// Read the CURRENT catalogue row for one subscription by name,
    /// mirroring [`Self::read_subscriptions`]' public read path.
    pub(crate) async fn read_subscription_row(
        &self,
        name: &str,
    ) -> Result<Option<QueryValue>, String> {
        let table = self
            .local
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| e.to_string())?;
        let records = read_all(&table, "subscriptions")
            .await
            .map_err(|e| e.to_string())?;
        Ok(records
            .into_iter()
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name)))
    }

    /// Resolve a profile name → its `streams` by reading
    /// `system/replication_profiles` (386-a schema: `{name, streams}`).
    pub(crate) async fn resolve_profile_streams(
        &self,
        profile: &str,
    ) -> Result<Vec<ReplStream>, String> {
        let table = self
            .local
            .system_store()
            .replication_profiles_table()
            .await
            .map_err(|e| e.to_string())?;
        let records = read_all(&table, "replication_profiles")
            .await
            .map_err(|e| e.to_string())?;
        let row = records
            .into_iter()
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(profile))
            .ok_or_else(|| format!("replication profile '{profile}' not found"))?;
        let streams_val = row
            .get("streams")
            .cloned()
            .ok_or_else(|| format!("profile '{profile}' has no streams field"))?;
        // `streams` was persisted via `to_qv(&Vec<ReplStream>)` (a msgpack
        // round-trip); reverse it the same way.
        decode_streams(&streams_val)
    }

    /// Read `system/subscriptions` into typed [`Subscription`]s.
    pub(crate) async fn read_subscriptions(&self) -> Result<Vec<Subscription>, String> {
        let table = self
            .local
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| e.to_string())?;
        let records = read_all(&table, "subscriptions")
            .await
            .map_err(|e| e.to_string())?;
        records
            .into_iter()
            .map(|r| {
                // S5c-X PR8b-2: the binding is strictly validated; a row
                // that fails is a typed catalogue error naming the
                // subscription. Reconcile logs a warning and leaves running
                // loops alone — fail-safe, never fail-open.
                let binding = SubscriptionBinding::from_row(&r).map_err(|e| e.to_string())?;
                let (binding_generation, binding_destinations) =
                    (binding.generation(), binding.destinations().to_vec());
                Ok(Subscription {
                    name: r
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "subscription row is missing its 'name' string".to_string())?
                        .to_string(),
                    upstream: r
                        .get("upstream")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    publication: r
                        .get("publication")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    profile: r
                        .get("profile")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    state: r
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    binding_generation,
                    binding_destinations,
                })
            })
            .collect()
    }
}

/// Reverse `to_qv(&Vec<ReplStream>)`: re-encode the `QueryValue` to msgpack
/// and deserialise the typed streams back out.
pub(crate) fn decode_streams(v: &QueryValue) -> Result<Vec<ReplStream>, String> {
    let bytes = rmp_serde::to_vec_named(v).map_err(|e| format!("encode streams: {e}"))?;
    rmp_serde::from_slice::<Vec<ReplStream>>(&bytes).map_err(|e| format!("decode streams: {e}"))
}

/// Read every record of a system-store table as owned `QueryValue`s.
///
/// Mirrors `admin_replication.rs::read_all` — the system catalogue read path.
/// `pub(crate)` (round-6 item 2): `seed/terminal.rs`'s generation-currency
/// check reuses this reader instead of duplicating it.
pub(crate) async fn read_all(
    table: &shamir_db::engine::table::TableManager,
    table_name: &str,
) -> shamir_db::DbResult<Vec<QueryValue>> {
    let refs: TMap<String, QueryResult> = TMap::default();
    // S5c-X F1 (Round 6): the FilterContext is built engine-side; no
    // `&Interner` crosses the boundary.
    let ctx = table.info_store_read_view().filter_context(&refs).await?;
    let query = ReadQuery::new(table_name);
    let result = table.read(&query, &ctx).await?;
    Ok(result
        .records
        .into_iter()
        .map(|r| r.as_value().into_owned())
        .collect())
}
