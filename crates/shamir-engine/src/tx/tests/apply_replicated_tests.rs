//! R1-a — tests for [`apply_replicated`] (single-hop follower raw-apply).
//!
//! Each test constructs an in-memory follower repo with an `items` table,
//! builds a [`ChangelogEvent`] via the REAL `shamir_tx::project_event` path
//! (so the byte shape is identical to what a leader emits), and asserts the
//! apply contract: Put/Delete convergence, idempotency by leader-watermark,
//! watermark ordering, and downstream changefeed re-emission.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use crate::tx::{apply_replicated, ApplyOutcome};

/// Build an in-memory follower repo with one configured table `items`.
fn follower_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("items")],
    )
}

/// Fixed rid for tests — last byte varies so each rid is distinct.
fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn items_token() -> u64 {
    crate::table::table_manager::table_token_for("items")
}

/// Build a [`ChangelogEvent`] that projects a Put on `rid` of `value`,
/// using the REAL `shamir_tx::project_event` path so the byte shape matches
/// what a leader emits. The staging `TxContext` is built via
/// `ensure_table_staging` so the human-readable table name is recorded in
/// `table_tokens` (otherwise `project_event` falls back to `token:{n}` and
/// `apply_replicated`'s `table_token_for` would resolve the wrong token).
fn put_event(value: &str, record: RecordId, leader_version: u64) -> shamir_tx::ChangelogEvent {
    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(Actor::User(42));
    let data_store: Arc<dyn Store> =
        Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
    let body = InnerValue::Str(value.into()).to_bytes().unwrap();
    tx.ensure_table_staging(items_token(), "items", data_store)
        .set(record.to_bytes(), body);
    shamir_tx::project_event(&tx, "leader", leader_version).unwrap()
}

/// Build a [`ChangelogEvent`] that projects a Delete on `rid`, using the
/// REAL `shamir_tx::project_event` path. See `put_event`.
fn delete_event(record: RecordId, leader_version: u64) -> shamir_tx::ChangelogEvent {
    let mut tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(Actor::User(42));
    let data_store: Arc<dyn Store> =
        Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
    tx.ensure_table_staging(items_token(), "items", data_store)
        .remove(record.to_bytes());
    shamir_tx::project_event(&tx, "leader", leader_version).unwrap()
}

// ── Test 1: Put convergence ─────────────────────────────────────────

/// Apply a ChangelogEvent with a Put on `items` → the record reads back on
/// the follower with the same bytes.
#[tokio::test]
async fn put_convergence() {
    let follower = follower_repo();
    // Force the follower to instantiate its `items` TableManager + MvccStore.
    let follower_tbl = follower.get_table("items").await.unwrap();

    let event = put_event("hello", rid(1), 1);

    let outcome = apply_replicated(&follower, &event, 0).await.unwrap();
    let local_v = match outcome {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(local_v, 1, "first local version is 1");

    // Read back on the follower — same bytes as the leader staged.
    let got = follower_tbl.get(rid(1)).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "hello"),
        "Put should converge: got {got:?}"
    );
}

// ── Test 2: Delete convergence ──────────────────────────────────────

/// Apply a Put then a Delete → the record is absent on the follower.
#[tokio::test]
async fn delete_convergence() {
    let follower = follower_repo();
    let follower_tbl = follower.get_table("items").await.unwrap();

    let put_ev = put_event("tmp", rid(7), 1);
    let del_ev = delete_event(rid(7), 2);

    let _ = apply_replicated(&follower, &put_ev, 0).await.unwrap();
    // Confirm Put landed first.
    let got = follower_tbl.get(rid(7)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "tmp"));

    // Apply Delete — watermark is 1 (Put's leader version).
    let outcome = apply_replicated(&follower, &del_ev, 1).await.unwrap();
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));

    // Record is now absent.
    let err = follower_tbl.get(rid(7)).await.unwrap_err();
    assert!(
        matches!(err, shamir_storage::error::DbError::NotFound(_)),
        "Delete should remove the record: got {err:?}"
    );
}

// ── Test 3: Idempotency ─────────────────────────────────────────────

/// Apply one event twice with watermark = event.commit_version after the
/// first → the second is a skip (no-op) and state is unchanged.
#[tokio::test]
async fn idempotent_reapply_is_skip() {
    let follower = follower_repo();
    let follower_tbl = follower.get_table("items").await.unwrap();

    let event = put_event("v3", rid(3), 5);

    // First apply at watermark 0 → Applied.
    let first = apply_replicated(&follower, &event, 0).await.unwrap();
    assert!(matches!(first, ApplyOutcome::Applied { .. }));

    // Capture the follower's gate floor after the first apply.
    let gate = follower.tx_gate().await.unwrap();
    let floor_after_first = gate.last_committed();

    // Second apply with watermark = event.commit_version (5) → Skipped, no
    // version consumed, no state change.
    let second = apply_replicated(&follower, &event, 5).await.unwrap();
    assert_eq!(second, ApplyOutcome::Skipped);

    // No new version allocated.
    assert_eq!(
        gate.last_committed(),
        floor_after_first,
        "skip must NOT advance the gate floor"
    );

    // State unchanged — same bytes.
    let got = follower_tbl.get(rid(3)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "v3"));
}

// ── Test 4: watermark ordering ──────────────────────────────────────

/// v=5 with watermark=3 applies; v=5 with watermark=5 skips.
#[tokio::test]
async fn watermark_ordering() {
    let event = put_event("ord", rid(9), 5);

    // watermark=3 < 5 → Applied.
    {
        let follower = follower_repo();
        let follower_tbl = follower.get_table("items").await.unwrap();
        let outcome = apply_replicated(&follower, &event, 3).await.unwrap();
        assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
        let got = follower_tbl.get(rid(9)).await.unwrap();
        assert!(matches!(got, InnerValue::Str(ref s) if s == "ord"));
    }

    // watermark=5 (== 5) → Skipped, in isolation on a fresh follower.
    {
        let follower = follower_repo();
        let _ = follower.get_table("items").await.unwrap();
        let outcome = apply_replicated(&follower, &event, 5).await.unwrap();
        assert_eq!(outcome, ApplyOutcome::Skipped);
    }

    // watermark=4 (< 5) → Applied on a fresh follower.
    {
        let follower = follower_repo();
        let follower_tbl = follower.get_table("items").await.unwrap();
        let outcome = apply_replicated(&follower, &event, 4).await.unwrap();
        assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
        let got = follower_tbl.get(rid(9)).await.unwrap();
        assert!(matches!(got, InnerValue::Str(ref s) if s == "ord"));
    }
}

// ── Test 5: finalize-tail — downstream changefeed re-emit ───────────

/// After apply_replicated, the follower's OWN changefeed carries a
/// re-emitted event at the follower-local version (downstream chain
/// replication works).
#[tokio::test]
async fn downstream_changefeed_reemit() {
    let follower = follower_repo();
    let _ = follower.get_table("items").await.unwrap();

    // Subscribe to the follower's changefeed BEFORE applying so the live
    // broadcast track fires (the journal-only fallback path skips the
    // broadcast when subscriber_count == 0).
    let mut rx = follower.subscribe_changelog().await.unwrap();

    let event = put_event("chain", rid(11), 7);

    let outcome = apply_replicated(&follower, &event, 0).await.unwrap();
    let local_v = match outcome {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(local_v, 1, "first local version is 1");

    // The follower should have re-emitted the event on its own changefeed.
    let rebroadcast = rx
        .recv()
        .await
        .expect("follower changefeed re-emitted the event");
    assert_eq!(
        rebroadcast.commit_version, local_v,
        "downstream event keyed on follower-local version, not leader version"
    );
    assert_eq!(
        rebroadcast.repo, "follower",
        "downstream repo name is the follower's"
    );
    assert_eq!(
        rebroadcast.changes.len(),
        event.changes.len(),
        "downstream carries the same record changes"
    );
    // The leader version 7 is NOT the downstream key.
    assert_ne!(rebroadcast.commit_version, event.commit_version);
}

// ── Test 6: apply before any read is MVCC-visible (MVCC-attach ordering) ──

/// Regression for the MVCC-attach ordering risk surfaced by R1-d: a
/// follower that applies a replicated event BEFORE ever serving a read had
/// no per-table `MvccStore` attached, so `apply_replicated` took the
/// base-store fallback and the write was invisible to a later MVCC read.
/// `apply_replicated` now forces the attach (via `get_table`) before
/// writing, so the record is visible even when apply is the very first
/// touch of the table — NO prior `get_table` here on purpose.
#[tokio::test]
async fn apply_before_any_read_is_mvcc_visible() {
    let follower = follower_repo();

    // Apply FIRST — the follower has never read `items`, so its MvccStore
    // is not yet attached at the top of apply_replicated.
    let event = put_event("fresh", rid(3), 1);
    let outcome = apply_replicated(&follower, &event, 0).await.unwrap();
    assert!(
        matches!(outcome, ApplyOutcome::Applied { .. }),
        "apply on a fresh follower should succeed"
    );

    // Now read back through the MVCC path — the value must be visible
    // (before the fix this returned NotFound because the write went to the
    // base store while the read routes through `history`).
    let follower_tbl = follower.get_table("items").await.unwrap();
    let got = follower_tbl.get(rid(3)).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "fresh"),
        "apply-before-read must be MVCC-visible: got {got:?}"
    );
}

// ── A12 regression — completion-tracker terminal marking ───────────
//
// apply_replicated used to allocate its local version via the BARE
// `assign_next_version()` (which only bumps the version_counter atomic) and
// never marked that version in the gate's `completion` tracker on the success
// path — only `mark_durable_aborted` ran on the failure path, and that touches
// the SEPARATE `durable_completion` tracker, not the visibility one. A version
// allocated but never marked in `completion` leaves a permanent hole: the
// tracker's `try_advance` stops at the first version with no terminal state,
// so a later local commit at M > N cannot push the contiguous watermark past
// N. The fix routes both paths through a `VersionGuard` (success → `commit()`
// marks `Materialized`, failure → `Drop` marks `Aborted`), matching the
// compiler-enforced RAII pattern the rest of the commit pipeline uses.

/// Reproduce the stuck-watermark scenario: apply_replicated succeeds (version
/// N allocated), then a LOCAL tx commits at M > N. The completion tracker's
/// contiguous watermark MUST advance through both N and M — before the fix it
/// wedged at N-1 forever because N was never terminally marked in
/// `completion`.
#[tokio::test]
async fn a12_success_path_completion_watermark_advances_past_replicated_version() {
    let follower = follower_repo();
    // Force-attach the `items` MvccStore BEFORE applying so the write hits the
    // MVCC path (matches the production follower that has served at least one
    // read).
    let _ = follower.get_table("items").await.unwrap();
    let gate = follower.tx_gate().await.unwrap();
    let completion_before = gate.completion().watermark();
    assert_eq!(completion_before, 0, "fresh gate: watermark at 0");

    // Step 1 — apply_replicated allocates local_version = 1 (N) and (post-fix)
    // marks it `Materialized` in `completion` via the VersionGuard's `commit()`.
    let event = put_event("a12-ok", rid(21), 1);
    let outcome = apply_replicated(&follower, &event, 0).await.unwrap();
    let n = match outcome {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(n, 1, "first allocated version is 1 (N)");

    // The completion tracker MUST now reflect N as terminally marked. Before
    // the fix the bare `assign_next_version` left NO entry for N here, so the
    // watermark stayed at 0 (N was neither Pending, Materialized, nor Aborted
    // in the tracker — `try_advance` breaks on a missing entry).
    assert_eq!(
        gate.completion().watermark(),
        n,
        "completion watermark must advance to N after a successful apply_replicated"
    );

    // Step 2 — a LOCAL tx commit at M = N + 1 through the normal commit path.
    // Its `VersionGuard::commit()` marks M `Materialized`; the tracker's
    // `try_advance` then needs BOTH N and N+1 terminally marked to advance
    // past N+1. Before the fix N was unmarked, so the watermark was stuck at
    // N-1 = 0 even though M was correctly marked.
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{StagingStore, TxContext};
    let mut local_tx = TxContext::new(TxId::new(99), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    staging.set(
        Bytes::from_static(b"local-k"),
        Bytes::from_static(b"local-v"),
    );
    // Use a DIFFERENT table token than `items` so this commit is independent
    // of the replicated table's state. Any non-zero token works for an
    // InMemory commit (see durable_watermark_tests::staged_tx).
    local_tx.write_set.insert(7777u64, staging);

    let outcome = crate::tx::commit_tx(local_tx, &follower).await.unwrap();
    let m = outcome.commit_version;
    assert!(m > n, "local commit M ({m}) must be > N ({n})");
    assert!(outcome.materialized(), "InMemory local commit is Complete");

    // The HARD assertion: completion watermark advances to M. Before the fix
    // this was `0` (stuck at N-1's gap), proving the clog.
    assert_eq!(
        gate.completion().watermark(),
        m,
        "completion watermark must advance through M after both N (replicated) \
         and M (local) are terminally marked — was stuck at N-1 before the fix"
    );
    // Visibility floor tracks the watermark (both run through the same
    // `last_committed_version` atomic via `advance_last_committed`).
    assert_eq!(gate.last_committed(), m);
}

/// Failure path: when `apply_replicated` errors AFTER allocating a version,
/// the VersionGuard's `Drop` MUST mark the version `Aborted` in the
/// completion tracker so the watermark advances past it. Before the fix the
/// bare `mark_durable_aborted` touched ONLY the durable tracker — the
/// visibility tracker's watermark was permanently clogged at N-1.
#[tokio::test]
async fn a12_failure_path_version_marked_aborted_in_completion_tracker() {
    let follower = follower_repo();
    let _ = follower.get_table("items").await.unwrap();
    let gate = follower.tx_gate().await.unwrap();
    assert_eq!(gate.completion().watermark(), 0);

    // Build a Put event whose RecordChange carries NO value bytes — this
    // trips the early `Err(DbError::Internal(...))` return inside
    // `apply_replicated`'s grouping loop (apply_replicated.rs:154-158) AFTER
    // the version has been allocated but BEFORE any physical write. Clean,
    // deterministic failure-path trigger that needs no custom failing store.
    let bad_event = shamir_tx::ChangelogEvent {
        repo: "leader".into(),
        commit_version: 1,
        tx_id: 1,
        actor: Actor::User(42),
        timestamp_ns: 0,
        changes: vec![shamir_tx::RecordChange {
            table: "items".into(),
            key: rid(31).to_bytes(),
            op: shamir_tx::ChangeOp::Put,
            value: None, // ← triggers the Internal error
        }],
    };

    let err = apply_replicated(&follower, &bad_event, 0).await;
    assert!(err.is_err(), "Put with no value must fail; got {err:?}");

    // The burned version (N = 1) MUST be terminally marked `Aborted` in the
    // completion tracker — the guard's Drop does this automatically. Before
    // the fix `mark_durable_aborted` marked only the durable tracker, leaving
    // the visibility tracker with a hole at N that would clog a later local
    // commit's watermark advance.
    assert_eq!(
        gate.completion().watermark(),
        1,
        "completion watermark must advance past the aborted version N=1"
    );

    // Prove the un-clog end-to-end: a subsequent LOCAL commit at M > N must
    // be able to push the visibility watermark through the (now-Aborted) N.
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{StagingStore, TxContext};
    let mut local_tx = TxContext::new(TxId::new(99), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    staging.set(Bytes::from_static(b"fk"), Bytes::from_static(b"fv"));
    local_tx.write_set.insert(7777u64, staging);
    let outcome = crate::tx::commit_tx(local_tx, &follower).await.unwrap();
    let m = outcome.commit_version;
    assert!(m > 1, "local commit M ({m}) must be > aborted N=1");
    assert_eq!(
        gate.completion().watermark(),
        m,
        "completion watermark must advance through M after the aborted N"
    );
}

/// Regression: the A12 fix MUST NOT change apply_replicated's externally
/// observable success-path contract — the record still lands, the downstream
/// changefeed still re-emits at the follower-local version, and the durable
/// watermark still tracks the visibility watermark (inline-materialize
/// invariant `durable_watermark() <= last_committed()` preserved).
#[tokio::test]
async fn a12_regression_success_path_preserves_data_changefeed_and_durable() {
    let follower = follower_repo();
    let _ = follower.get_table("items").await.unwrap();
    let gate = follower.tx_gate().await.unwrap();
    let mut rx = follower.subscribe_changelog().await.unwrap();

    let event = put_event("regress", rid(41), 3);
    let outcome = apply_replicated(&follower, &event, 0).await.unwrap();
    let n = match outcome {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(n, 1);

    // Data converged.
    let follower_tbl = follower.get_table("items").await.unwrap();
    let got = follower_tbl.get(rid(41)).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "regress"),
        "regression: data must still converge: got {got:?}"
    );

    // Downstream changefeed re-emitted at the follower-local version.
    let rebroadcast = rx.recv().await.expect("downstream event re-emitted");
    assert_eq!(rebroadcast.commit_version, n);
    assert_eq!(rebroadcast.repo, "follower");

    // Durable watermark tracks visibility (the fix keeps `mark_durable(n)`
    // alongside the new `guard.commit()` — both are needed: the guard covers
    // the visibility tracker, `mark_durable` covers the durable tracker +
    // `durable_progress.notify_waiters()`).
    assert!(
        gate.durable_watermark() <= gate.last_committed(),
        "inline-materialize invariant: durable <= visibility"
    );
    assert_eq!(gate.durable_watermark(), n);
    assert_eq!(gate.last_committed(), n);
}
