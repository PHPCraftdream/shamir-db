//! PR6b-3 — stall visibility tests: a follower loop that swallows repeated
//! transient errors WITHOUT durable-bookmark progress must surface the stall
//! as a persistent `status_message` on the subscription row (never silent
//! again), while any bookmark progress resets the failure counter and can
//! never block normal convergence.
//!
//! Bound by construction: [`STALL_VISIBILITY_THRESHOLD`] is 10, strictly
//! greater than the engine's inline retry budget of 3, so the
//! `receipt_io_pause_retry_convergence` scenario (3 failures, then
//! convergence) can never trip it.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::principal64_from_username;
use shamir_db::access::Actor;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::query::filter::FilterContext;
use shamir_db::repl_binding::SubscriptionLease;
use shamir_db::types::common::new_map;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::query::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

async fn build_db(owner_label: &str) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username(owner_label));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    shamir
}

/// Create the `system/subscriptions` row the loop marks on a stall.
async fn create_subscription_row(follower: &ShamirDb, name: &str) {
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "cp",
        replication_profile("stall_profile").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "cs",
        subscription(name, "tcp://leader:9000", "pub", "stall_profile"),
    );
    follower
        .execute(
            "app",
            &batch
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .expect("create profile and subscription");
}

/// Read the single subscription row's `state` / `status_message`.
async fn read_subscription_row(follower: &ShamirDb) -> (Option<String>, Option<String>) {
    let table = follower.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = Query::from("subscriptions").build();
    let rows = table.read(&query, &ctx).await.unwrap();
    let row = rows.records[0].as_value();
    (
        row.get("state").and_then(|v| v.as_str()).map(String::from),
        row.get("status_message")
            .and_then(|v| v.as_str())
            .map(String::from),
    )
}

/// Round 13 G2: take the row's lease EXACTLY like the supervisor does —
/// under the control lock, for the row's REAL `binding_generation` — so a
/// supervised test loop writes with the row's actual generation (never 0:
/// the fenced status writer rejects 0 as a stale no-op).
async fn take_row_lease(follower: &ShamirDb, name: &str) -> Arc<SubscriptionLease> {
    let db = Arc::new(follower.clone());
    let generation = subscription_row_generation(follower).await;
    let control = db.lock_replication_control().await;
    let lease = db
        .take_subscription_lease(name, generation, &control)
        .await
        .expect("a fresh lease is issued for the current generation");
    drop(control);
    lease
}

/// Read the single subscription row as the COMPLETE `QueryValue` (round 13
/// G3b: the stale-report test compares the whole row, not a field tuple).
async fn read_whole_subscription_row(follower: &ShamirDb) -> QueryValue {
    let table = follower.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = Query::from("subscriptions").build();
    let rows = table.read(&query, &ctx).await.unwrap();
    rows.records[0].as_value().into_owned()
}

/// Round 13 G3b: write a REAL REPLACEMENT binding through the production
/// row-write path (implicit-tx SET on `system/subscriptions`, exactly like
/// the admin/catalogue writers): generation `gen + 1`, DIFFERENT
/// destinations.
async fn write_replacement_binding(follower: &ShamirDb, name: &str, old: &QueryValue, gen: u64) {
    let mut record = old.clone();
    if let QueryValue::Map(ref mut m) = record {
        m.insert(
            "binding_generation".to_string(),
            QueryValue::Int(gen as i64),
        );
        m.insert(
            "binding_destinations".to_string(),
            QueryValue::List(vec![QueryValue::Map({
                let mut dest = new_map();
                dest.insert("db".to_string(), QueryValue::Str("other".to_string()));
                dest.insert("repo".to_string(), QueryValue::Str("replica".to_string()));
                dest
            })]),
        );
    } else {
        panic!("subscription row is not a map");
    }
    let table = follower.system_store().subscriptions_table().await.unwrap();
    follower
        .replace_subscription_row_for_test(&table, name.to_string(), record)
        .await
        .expect("replacement binding row SET");
}

/// A source whose `pull` (and initial `hello`) always fails transiently —
/// the exact silent-retry shape that motivated PR6b-3.
struct AlwaysFailingSource;

#[async_trait]
impl ReplSource for AlwaysFailingSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport("leader unreachable".into()))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport("leader unreachable".into()))
    }
}

/// Fails the first `failures` pulls, then delegates to a healthy
/// [`InProcessReplSource`] so the bookmark advances (progress).
struct FlakyThenHealthySource {
    remaining_failures: Mutex<u32>,
    inner: InProcessReplSource,
}

#[async_trait]
impl ReplSource for FlakyThenHealthySource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let mut remaining = self.remaining_failures.lock().await;
        if *remaining > 0 {
            *remaining -= 1;
            return Err(ReplError::Transport("transient flake".into()));
        }
        drop(remaining);
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_failures_without_progress_surface_status_message() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // Enough iterations to cross the threshold and keep retrying past it.
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 2);
    // Round 13 G2: supervised loop ⇒ the row's REAL generation (0 would be
    // fenced out and the stall message would never land).
    let mut cfg = cfg;
    cfg.lease = Some(take_row_lease(&follower, "stall_sub").await);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(AlwaysFailingSource),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("swallowed transient errors must NOT terminate the loop");
    let (state, message) = read_subscription_row(&follower).await;
    // Least-destructive surfacing: the row stays active/retryable.
    assert_eq!(
        state.as_deref(),
        Some("active"),
        "stall must stay retryable"
    );
    let message = message.expect("stall must be visible via status_message");
    // R7-4: the assertion matches the production message in
    // follower_loop.rs::note_swallowed_failure EXACTLY — any wording drift
    // here must fail this test, not silently pass on a substring.
    assert_eq!(
        message,
        "follower stalled: 10 consecutive failures without progress; \
         last error: replication transport error: leader unreachable",
        "unexpected status_message: {message}"
    );
}

/// Fails the first 10 pulls ("streak one"), then delegates ONCE to the
/// healthy inner source (durable progress), then fails every further pull
/// ("streak two") — stall → recovery → fresh stall in ONE loop run.
///
/// Fully deterministic, no sleeps/yields as synchronization:
///
/// * Stage 1 — the test arms [`Self::stall_seen`] only AFTER it has read the
///   threshold-10 stall message from the follower's PERSISTED row; pull #11
///   (the healthy one) awaits that oneshot at entry, so the healthy pull
///   cannot run before the stall write is observed. Cannot deadlock: the
///   receiver is armed before the loop task is spawned.
/// * Stages 2/3 — the test polls the follower's OWN durable state (the
///   persisted row, then the repo bookmark) inside a
///   `tokio::time::timeout` budget; bounded timeout polling of real
///   persisted state, never an unbounded sleep.
struct StreakySource {
    inner: InProcessReplSource,
    pulls: Mutex<u32>,
    /// Armed by the test after the stage-1 stall message is persisted; taken
    /// by pull #11 before the healthy pull proceeds.
    stall_seen: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl ReplSource for StreakySource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let n = {
            let mut pulls = self.pulls.lock().await;
            *pulls += 1;
            *pulls
        };
        if n <= 10 {
            return Err(ReplError::Transport("streak one".into()));
        }
        if n == 11 {
            // Stage-1 handoff: the test resolves this receiver only after the
            // threshold-10 stall message was read back from the PERSISTED row.
            let rx = self
                .stall_seen
                .lock()
                .await
                .take()
                .expect("stage-1 gate must be armed before the loop is spawned");
            rx.await
                .expect("stage-1 sender dropped before the healthy pull");
            return self
                .inner
                .pull(db, repo, from_version, limit, wait_ms)
                .await;
        }
        Err(ReplError::Transport("streak two".into()))
    }
}

/// Bounded wait on the follower's persisted subscription row until
/// `status_message` equals `want` (stage 1 and stage 2). Panics with `what`
/// on timeout — that panic is the stage's counterfactual failure point.
///
/// The deadline is VIRTUAL time (`start_paused`) and must exceed the follower's
/// own retry backoff up to the stall threshold: the loop sleeps 0.2, 0.4, 0.8,
/// 1.6, 3.2, 6.4, 12.8, 25.6 and 30.0 (capped) seconds after failures 1..9
/// before the tenth failure writes the message (`follower_loop.rs:61-65`,
/// `:877-882`) = 80.8 s. 300 s leaves a wide margin and costs no real time.
async fn await_row_message(follower: &ShamirDb, want: &str, what: &'static str) {
    tokio::time::timeout(std::time::Duration::from_secs(300), async {
        loop {
            let (_, message) = read_subscription_row(follower).await;
            if message.as_deref() == Some(want) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: status_message never became {want:?}"));
}

/// R7-5: durable progress must OVERWRITE a reported stall message with the
/// progress note AND restart the failure counter, so a later fresh
/// 10-failure streak reports again with the fresh count.
///
/// COUNTERFACTUALS (first failing assertion per stage):
/// * Stage 1 — if `note_swallowed_failure`'s threshold stall write is
///   removed, the stage-1 `await_row_message` poll below never matches and
///   its `unwrap_or_else(panic!)` fires ("stage 1").
/// * Stage 2 — if `clear_stall`'s progress write is removed while the
///   counter reset stays, the stage-2 `await_row_message` poll never sees
///   the progress note and panics ("stage 2"); the bookmark assertion after
///   it would still pass.
/// * Stage 3 — if the counter were NOT restarted on progress, the
///   post-progress streak continues at 11 (counts 11..20 ≥ threshold 10 with
///   the latch cleared by `clear_stall`) and the message would read "…11
///   consecutive failures…" — the exact-match `assert_eq!` on
///   `10 consecutive failures` at the end of the test is the first failing
///   assertion.
#[tokio::test(start_paused = true)]
async fn progress_overwrites_stall_message_and_counter_restarts() {
    let leader = build_db("alice").await;
    let owner = Actor::User(principal64_from_username("alice"));
    use shamir_query_builder::batch::Batch as B;
    use shamir_query_builder::doc;
    use shamir_query_builder::write::insert;
    let mut batch = B::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "k0",
            "v" => 0_i64,
        }]),
    );
    leader
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("leader write");

    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // 10 failures, apply+idle, then 10 failures.
    const STALL_ONE: &str = "follower stalled: 10 consecutive failures without progress; \
         last error: replication transport error: streak one";
    const PROGRESS: &str = "replication progress restored; durable bookmark at 1";
    const STALL_TWO: &str = "follower stalled: 10 consecutive failures without progress; \
         last error: replication transport error: streak two";
    let (stall_seen_tx, stall_seen_rx) = tokio::sync::oneshot::channel();
    let source = StreakySource {
        inner: InProcessReplSource::new(leader),
        pulls: Mutex::new(0),
        stall_seen: Mutex::new(Some(stall_seen_rx)),
    };
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(25);
    // Round 13 G2: supervised loop ⇒ the row's REAL generation.
    let mut cfg = cfg;
    cfg.lease = Some(take_row_lease(&follower, "stall_sub").await);
    let run = tokio::spawn(run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(source),
        cfg,
        CancellationToken::new(),
    ));

    // Stage 1: the threshold-10 stall message is PERSISTED before the healthy
    // pull may run (pull #11 blocks on the oneshot we resolve here).
    await_row_message(&follower, STALL_ONE, "stage 1").await;
    stall_seen_tx
        .send(())
        .expect("stage-1 receiver still parked in pull #11");

    // Stage 2: observe the follower's OWN durable progress — the persisted
    // progress note (written by clear_stall after the apply) — never an
    // in-source signal that races the follower's write.
    await_row_message(&follower, PROGRESS, "stage 2").await;
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    assert_eq!(
        repo.replication_bookmark().await.unwrap(),
        1,
        "the healthy window durably applied the leader's event"
    );

    // Stage 3: the remaining pulls fail ("streak two"); the loop finishes its
    // iteration budget and the fresh message carries a RESTARTED count of 10
    // (not 11..20).
    run.await
        .expect("follower task")
        .expect("stall/recovery/stall must never terminate the loop");
    assert_eq!(
        read_subscription_row(&follower).await.1.as_deref(),
        Some(STALL_TWO),
        "the second streak must report with a RESTARTED counter"
    );
}

/// A source whose `pull` SUCCEEDS but returns an EMPTY batch while its
/// `current_version` advertises a backlog past the follower's bookmark —
/// the leader says "I have version 5" and hands over nothing, forever.
struct EmptyBacklogSource;

#[async_trait]
impl ReplSource for EmptyBacklogSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        // Transient hello: the loop seeds epoch 0 and proceeds to pulls.
        Err(ReplError::Transport("no hello".into()))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        Ok(ReplResponse::Pull {
            leader_epoch: 1,
            events: rmp_serde::to_vec_named(&Vec::<ReplicatedEvent>::new()).unwrap(),
            gap_at: None,
            current_version: 5,
            repo_lineage_id: Some([2; 16]),
            history_epoch: Some(0),
            gap_first: None,
            gap_through: None,
            schema_digest: None,
        })
    }
}

/// R7-5: successful empty pulls where the leader ADVERTISES a backlog
/// (`current_version` > durable bookmark) must feed the stall measure.
///
/// COUNTERFACTUAL: before this change a successful pull NEVER fed the stall
/// measure, so this loop ran forever with `state = active` and
/// `status_message = null` — the exact silent-green stall round 6 exposed in
/// CI. The first failing assertion is `message.expect(...)` below.
#[tokio::test(start_paused = true)]
async fn empty_pull_with_advertised_backlog_feeds_stall_measure() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 2);
    // Round 13 G2: supervised loop ⇒ the row's REAL generation (0 would be
    // fenced out and the backlog stall message would never land).
    let mut cfg = cfg;
    cfg.lease = Some(take_row_lease(&follower, "stall_sub").await);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(EmptyBacklogSource),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("successful-but-empty pulls must NOT terminate the loop");
    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(state.as_deref(), Some("active"), "stall stays retryable");
    let message = message.expect("backlog stall must be visible via status_message");
    assert_eq!(
        message,
        "follower stalled: 10 consecutive failures without progress; \
         last error: leader advertises backlog through version 5 but \
         returned no events from 1",
        "unexpected status_message: {message}"
    );
}

#[tokio::test(start_paused = true)]
async fn progress_resets_counter_and_normal_convergence_is_never_blocked() {
    let leader = build_db("alice").await;
    let owner = Actor::User(principal64_from_username("alice"));
    use shamir_query_builder::batch::Batch as B;
    use shamir_query_builder::doc;
    use shamir_query_builder::write::insert;
    let mut batch = B::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "k0",
            "v" => 0_i64,
        }]),
    );
    leader
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("leader write");
    // Wait for the journal to land.
    for _ in 0..200 {
        if leader
            .read_changelog_from_journal("app", "main", 0, 10)
            .await
            .ok()
            .flatten()
            .is_some_and(|jr| !jr.events.is_empty())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // 3 failures (the engine's inline retry budget) then healthy progress —
    // the exact receipt_io_pause_retry_convergence shape. Must NOT trip.
    let source = FlakyThenHealthySource {
        remaining_failures: Mutex::new(3),
        inner: InProcessReplSource::new(leader),
    };
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(10);
    // Round 13 G2: supervised loop ⇒ the row's REAL generation — the three
    // swallowed failures and the progress reset now run through the REAL
    // fenced writer (previously generation 0 silently no-op'd every write).
    let mut cfg = cfg;
    cfg.lease = Some(take_row_lease(&follower, "stall_sub").await);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(source),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("3 failures + convergence must converge cleanly");
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 1, "converged");
    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(state.as_deref(), Some("active"));
    assert!(
        message.is_none(),
        "progress must reset the counter — no stall message may exist: {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Round 12 — F1/F2 regression tests (lease/status-writer interaction)
// ---------------------------------------------------------------------------

/// The single subscription row's durable `binding_generation`.
async fn subscription_row_generation(follower: &ShamirDb) -> u64 {
    let table = follower.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = Query::from("subscriptions").build();
    let rows = table.read(&query, &ctx).await.unwrap();
    rows.records[0]
        .as_value()
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .expect("row carries binding_generation")
}

/// Round 12 F2: a LEASED follower that reaches the stall threshold must
/// persist the stall `status_message` AND keep running — the status-only
/// writer must never quiesce/invalidate the loop's own lease.
///
/// COUNTERFACTUAL: the pre-fix mechanism (`mark_subscription_terminal(name,
/// "active", ...)`) quiesced the lease inside every stall report, so the
/// loop exited at its next top-of-iteration validity check with the lease
/// `valid == false` — the first failing assertion is the lease-liveness
/// check below (the message itself was persisted either way).
#[tokio::test(start_paused = true)]
async fn leased_follower_stall_persists_message_and_keeps_lease_valid() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    let db = Arc::new(follower.clone());
    let lease = take_row_lease(&follower, "stall_sub").await;
    assert!(lease.valid.load(Ordering::Acquire));

    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 2);
    let mut cfg = cfg;
    cfg.lease = Some(lease.clone());
    run_follower_loop(
        db,
        Arc::new(AlwaysFailingSource),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("swallowed transient errors must NOT terminate a LEASED loop");

    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(
        state.as_deref(),
        Some("active"),
        "stall must stay retryable"
    );
    assert!(
        message.is_some(),
        "the stall must still be visible via status_message"
    );
    assert!(
        lease.valid.load(Ordering::Acquire),
        "round 12 F2: the stall report must NOT quiesce/invalidate the lease"
    );
}

/// Round 12 F2/F3: the status writer is generation-fenced — a stale task's
/// report on a replacement binding's row is a logged no-op (Ok), and the
/// row is untouched; only the CURRENT generation's report lands. It also
/// never touches `state` or `last_terminal_reason`.
#[tokio::test(start_paused = true)]
async fn stale_generation_status_report_is_a_no_op_on_the_row() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    let generation = subscription_row_generation(&follower).await;
    let before = read_whole_subscription_row(&follower).await;

    // Round 13 G3b: make the row a REAL replacement binding (generation + 1,
    // different destinations) through the production write path, so the
    // stale report below targets a row that genuinely moved on.
    write_replacement_binding(&follower, "stall_sub", &before, generation + 1).await;
    let replaced = read_whole_subscription_row(&follower).await;
    assert_ne!(
        replaced, before,
        "the replacement binding must differ from the original row"
    );

    follower
        .set_subscription_status_message("stall_sub", "stale report", generation)
        .await
        .expect("a stale generation is a logged no-op, not an error");
    assert_eq!(
        read_whole_subscription_row(&follower).await,
        replaced,
        "the stale report must not touch ANY field of the replacement row"
    );

    // The FRESH generation's report on the replacement row still lands.
    follower
        .set_subscription_status_message("stall_sub", "fresh report", generation + 1)
        .await
        .expect("the current generation's report persists");
    let after = read_whole_subscription_row(&follower).await;
    assert_eq!(
        after.get("status_message").and_then(|v| v.as_str()),
        Some("fresh report"),
        "the fresh generation's report landed on the replacement row"
    );
    let mut expected = replaced.clone();
    if let QueryValue::Map(ref mut m) = expected {
        m.insert(
            "status_message".to_string(),
            QueryValue::Str("fresh report".to_string()),
        );
    }
    assert_eq!(
        after, expected,
        "the status writer must touch ONLY status_message"
    );
}

/// Round 12 F1 — deterministic lock-order test: while the loop holds its
/// apply-gate READ guard (parked inside the apply), a lifecycle mutation
/// (control lock → apply_gate.write()) waits for the guard; the loop's own
/// guard-held stall-clear persistence must NOT await the control lock until
/// AFTER the guard is dropped, so both complete. NO sleeps, NO timeouts:
/// synchronisation is the park's Notify pair plus plain `.await`s (a
/// deadlock fails via the test harness, exactly like CI would).
///
/// COUNTERFACTUAL: the pre-fix loop called `clear_stall` (which takes the
/// replication control lock) while still holding the read guard; with the
/// mutation task holding the control lock and waiting on the write gate,
/// that is a textbook lock-order inversion — the loop hangs forever inside
/// the guarded clear.
#[tokio::test(start_paused = true)]
async fn guarded_stall_persistence_does_not_deadlock_against_write_gate() {
    // Leader with one durable event.
    let leader = build_db("alice").await;
    let owner = Actor::User(principal64_from_username("alice"));
    use shamir_query_builder::batch::Batch as B;
    use shamir_query_builder::doc;
    use shamir_query_builder::write::insert;
    let mut batch = B::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "k0",
            "v" => 0_i64,
        }]),
    );
    leader
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("leader write");
    for _ in 0..200 {
        if leader
            .read_changelog_from_journal("app", "main", 0, 10)
            .await
            .ok()
            .flatten()
            .is_some_and(|jr| !jr.events.is_empty())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    let generation = subscription_row_generation(&follower).await;
    let db = Arc::new(follower.clone());
    let control = db.lock_replication_control().await;
    let lease = db
        .take_subscription_lease("stall_sub", generation, &control)
        .await
        .expect("a fresh lease is issued for the current generation");
    drop(control);

    // Park the loop INSIDE the apply (hook phase 1 = before apply, i.e.
    // with the read guard held), after the 10-failure stall streak.
    let arrived = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let hook = Arc::new(crate::replication::follower_loop::ApplyParkHook {
        repo: Some("main".into()),
        after_apply: false,
        arrived: Arc::clone(&arrived),
        release: Arc::clone(&release),
        exited: Arc::new(tokio::sync::Notify::new()),
    });
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 3);
    let mut cfg = cfg;
    cfg.lease = Some(lease.clone());
    cfg.apply_park = Some(hook);
    let loop_task = tokio::spawn(run_follower_loop(
        db.clone(),
        Arc::new(FlakyThenHealthySource {
            remaining_failures: Mutex::new(
                crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD as u32,
            ),
            inner: InProcessReplSource::new(leader),
        }),
        cfg,
        CancellationToken::new(),
    ));

    // Wait until the loop is parked holding the read guard (the healthy
    // pull #11 returned an event and the guard was taken).
    arrived.notified().await;

    // Concurrent lifecycle mutation: control lock → apply_gate.write() —
    // it must wait for the guard, never for the control lock held by the
    // loop (the loop must not hold it under the guard).
    let mark = {
        let db = db.clone();
        tokio::spawn(async move {
            db.mark_subscription_terminal_generation(
                "stall_sub",
                "resync_required",
                "round 12 lock-order probe",
                Some(generation),
            )
            .await
        })
    };
    // Round 13 G3a: deterministic ACKNOWLEDGEMENT, not an assumption. The
    // parked loop's READ guard alone never blocks another reader, but a
    // QUEUED writer does (tokio's RwLock is write-preferring), so a failing
    // `try_read()` proves the mutation owns the control lock and is parked on
    // `apply_gate.write()` BEFORE the follower is released. The mutation does
    // real I/O first, so the wait polls with a virtual-time sleep (the
    // suite's paused-clock idiom), not a count of scheduler yields.
    let mut parked = false;
    for _ in 0..20_000 {
        if lease.apply_gate.try_read().is_err() {
            parked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(
        parked,
        "the lifecycle mutation must be queued on the apply_gate write lock          before the follower is released"
    );
    release.notify_one();

    // Both complete WITHOUT a deadlock (an inversion hangs this test).
    mark.await
        .expect("mark task")
        .expect("the generation-fenced mark lands");
    loop_task
        .await
        .expect("follower task")
        .expect("the loop finishes its budget cleanly");

    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "the lifecycle mutation landed"
    );
    assert_eq!(
        message.as_deref(),
        Some("replication progress restored; durable bookmark at 1"),
        "the DEFERRED guard-held stall-clear landed after the guard dropped"
    );
}
