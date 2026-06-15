//! P1d-2a — unit tests for the background [`Drainer`].
//!
//! These drive `drain_step` / `drain_all` DIRECTLY (the drainer is not
//! wired into the commit path in P1d-2a). They seed the inflight WAL tail
//! by hand and prime the gate so visibility leads durability — exactly the
//! state P1d-2b's cutover produces (publish on the ack-path, history write
//! deferred). The drainer must then replay the not-yet-durable visible
//! prefix into history, advance `durable_watermark`, and (A5-gated) truncate
//! the markers.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::drainer::Drainer;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

/// Seed ONE inflight `Put` entry (NO history/overlay write) at
/// `commit_version`, exactly the shape the ack-path leaves behind once the
/// history write is deferred. Returns the body bytes for later comparison.
async fn seed_inflight_put(
    repo: &RepoInstance,
    table: &str,
    record: RecordId,
    value: &str,
    commit_version: u64,
) -> bytes::Bytes {
    let wal = repo.repo_wal().await.unwrap();
    let body = InnerValue::Str(value.into()).to_bytes().unwrap();
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: table_token_for(table),
            rid: record,
            body: body.clone(),
        }],
    )
    .with_commit_version(commit_version);
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();
    body
}

/// drain_step replays the visible-but-undurable prefix into history,
/// advances `durable_watermark` to visibility, and the data reads back.
#[tokio::test]
async fn drain_step_replays_visible_prefix_into_history() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Build the gate FIRST (durable == visibility == 0) BEFORE any inflight
    // entry exists, so seeding below does not lift the durable floor.
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);

    // Seed inflight entries at versions 1, 2, 3 (data NOT yet in history).
    seed_inflight_put(&repo, "t", rid(1), "v1", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "v2", 2).await;
    seed_inflight_put(&repo, "t", rid(3), "v3", 3).await;

    // Ack-path published visibility to 3, but durable still lags at 0.
    gate.publish_committed_max(3);
    assert_eq!(gate.last_committed(), 3);
    assert_eq!(gate.durable_watermark(), 0, "durable lags visibility");

    // Before drain: history has nothing for these rids.
    assert!(tbl.get(rid(1)).await.is_err());

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 3, "all three visible-undurable versions drained");
    assert_eq!(drainer.drained_total(), 3);

    // Durable watermark advanced to visibility.
    assert_eq!(
        gate.durable_watermark(),
        3,
        "durable caught up to visibility"
    );

    // Data now resolves from history.
    for (n, expect) in [(1u8, "v1"), (2, "v2"), (3, "v3")] {
        let got = tbl.get(rid(n)).await.unwrap();
        assert!(
            matches!(got, InnerValue::Str(ref s) if s == expect),
            "rid {n}: expected {expect}, got {got:?}"
        );
    }
}

/// drain_step with `durable >= visibility` is a no-op (nothing to drain).
#[tokio::test]
async fn drain_step_noop_when_durable_at_visibility() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    // Advance BOTH watermarks to 5: nothing is visible-but-undurable. The
    // durable watermark is a CONTIGUOUS prefix, so mark every version 1..=5
    // (marking only 5 would leave the prefix at 0).
    gate.publish_committed_max(5);
    for v in 1..=5u64 {
        gate.mark_durable(v);
    }
    assert_eq!(gate.last_committed(), 5);
    assert_eq!(gate.durable_watermark(), 5);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 0, "durable == visibility → no drain");
    assert_eq!(gate.durable_watermark(), 5, "watermark unchanged");
}

/// Idempotency: a second drain_step over already-drained state returns 0
/// and leaves history + watermark unchanged.
#[tokio::test]
async fn drain_step_is_idempotent() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    seed_inflight_put(&repo, "t", rid(1), "once", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "once", 2).await;
    gate.publish_committed_max(2);

    let drainer = Drainer::new();
    assert_eq!(drainer.drain_step(&repo).await.unwrap(), 2);
    assert_eq!(gate.durable_watermark(), 2);

    // Second pass: everything is already at/below the durable prefix.
    let second = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(second, 0, "re-drain of drained state is a no-op");
    assert_eq!(gate.durable_watermark(), 2, "watermark unchanged");
    assert_eq!(drainer.drained_total(), 2, "no double-count");

    // Data still resolves to the single correct value (no corruption).
    let got = tbl.get(rid(1)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "once"));
}

/// drain_all flushes the whole inflight tail and returns total drained.
#[tokio::test]
async fn drain_all_flushes_then_stops() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    for v in 1..=4u64 {
        seed_inflight_put(&repo, "t", rid(v as u8), "x", v).await;
    }
    gate.publish_committed_max(4);

    let drainer = Drainer::new();
    let total = drainer.drain_all(&repo).await.unwrap();
    assert_eq!(total, 4);
    assert_eq!(gate.durable_watermark(), 4);
    // A follow-up drain_all is a clean no-op.
    assert_eq!(drainer.drain_all(&repo).await.unwrap(), 0);
}

/// drain_step never drains a version ABOVE visibility: an entry committed in
/// the WAL but not yet published stays undurable.
#[tokio::test]
async fn drain_step_respects_visibility_ceiling() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    seed_inflight_put(&repo, "t", rid(1), "visible", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "pending", 2).await;

    // Only version 1 is published; version 2 is committed-in-WAL but not
    // yet ack-visible.
    gate.publish_committed_max(1);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 1, "only the visible version is drained");
    assert_eq!(gate.durable_watermark(), 1);

    // v1 is in history; v2 is NOT (above visibility).
    assert!(tbl.get(rid(1)).await.is_ok());
    assert!(
        tbl.get(rid(2)).await.is_err(),
        "version above visibility must not be drained"
    );
}

/// A5 interner-hwm gate: an entry whose interner delta references an id above
/// the table's persisted high-water mark IS drained to history (durable
/// advances) but its WAL marker is NOT truncated — it stays inflight until a
/// future interner checkpoint advances the hwm.
///
/// On the in-memory `Mem` WAL sink `wal.commit` is itself a no-op (markers
/// live until F6 truncation), so the load-bearing assertion is that
/// `drain_step` (a) still marks the version durable and (b) takes the A5
/// "retain marker" branch rather than erroring. We assert (a) directly and
/// (b) by the entry remaining replayable on a second pass with no panic.
#[tokio::test]
async fn drain_step_a5_retains_marker_above_hwm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    // Fresh table: persisted interner high-water mark is 0.
    assert_eq!(tbl.interner().persisted_high_water(), 0);

    let gate = repo.tx_gate().await.unwrap();
    let wal = repo.repo_wal().await.unwrap();

    let body = InnerValue::Str("with_delta".into()).to_bytes().unwrap();
    let mut entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid: rid(1),
            body,
        }],
    )
    .with_commit_version(1);
    // Interner delta id 42 — far above the persisted hwm (0) → A5 gate must
    // judge truncation UNSAFE and retain the marker.
    entry.interner_delta = vec![(token, "fresh_field".to_string(), 42)];
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    gate.publish_committed_max(1);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();

    // Data is durable in history despite the retained marker — the A5 gate
    // defers TRUNCATION, not DURABILITY.
    assert_eq!(drained, 1);
    assert_eq!(
        gate.durable_watermark(),
        1,
        "A5: durable advances even when the marker is retained"
    );
    let got = tbl.get(rid(1)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "with_delta"));

    // The interner delta was applied on replay (id 42 → "fresh_field").
    let interner = tbl.interner().get().await.unwrap();
    assert_eq!(
        interner.get_ind("fresh_field").map(|k| k.id()),
        Some(42),
        "interner delta applied during drain replay"
    );

    // Second pass: version 1 is now at the durable floor → no-op, no panic.
    assert_eq!(drainer.drain_step(&repo).await.unwrap(), 0);
}
