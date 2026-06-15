//! P1d-1 — tx-path coverage for the durable_watermark machinery.
//!
//! Under the current inline-materialize path the durable watermark must
//! catch up to `last_committed()` after every successful (`Complete`) tx
//! commit, and `durable_watermark() <= last_committed()` must hold at every
//! observation point during the sequence. Non-tx coverage lives in
//! `shamir-tx::tests::durable_watermark_tests`; the tests here drive the
//! full `commit_tx` pipeline through `RepoInstance`/`commit_tx_lockfree`
//! (the default path) so the engine-side `mark_durable` call after
//! `post_publish_cleanup` is exercised.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::tx::commit_tx;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Build a `TxContext` that stages one `(k, v)` write — minimum needed to
/// cross the commit point (an empty tx fast-paths and does not assign a
/// version, see `commit_empty_tx_succeeds`).
fn staged_tx(tx_id: u64, table_token: u64, k: &'static [u8], v: &'static [u8]) -> TxContext {
    let mut tx = TxContext::new(TxId::new(tx_id), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    staging.set(Bytes::from_static(k), Bytes::from_static(v));
    tx.write_set.insert(table_token, staging);
    tx
}

/// One tx commit advances both watermarks; durable catches up to visibility.
#[tokio::test]
async fn single_tx_commit_durable_equals_visibility() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.durable_watermark(), 0);
    assert_eq!(gate.last_committed(), 0);

    let outcome = commit_tx(staged_tx(1, 100, b"k", b"v"), &repo)
        .await
        .unwrap();
    assert!(outcome.materialized(), "InMemoryRepo commit is Complete");

    let dur = gate.durable_watermark();
    let vis = gate.last_committed();
    assert!(dur <= vis, "durable={dur} must not lead visibility={vis}");
    assert_eq!(
        dur, vis,
        "Complete commit under inline materialize → durable == visibility"
    );
    assert_eq!(vis, outcome.commit_version);
}

/// Several sequential tx commits: at every observation `durable <= visibility`,
/// and after each commit the two are equal (inline materialize). Exercises the
/// `commit_tx_lockfree` path's `mark_durable` call after each
/// `post_publish_cleanup`.
#[tokio::test]
async fn many_tx_commits_durable_tracks_visibility() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();

    for i in 0..6u64 {
        let outcome = commit_tx(staged_tx(10 + i, 200, b"k", b"v"), &repo)
            .await
            .unwrap();
        assert!(outcome.materialized());
        let dur = gate.durable_watermark();
        let vis = gate.last_committed();
        assert!(
            dur <= vis,
            "step {i}: durable={dur} must not lead vis={vis}"
        );
        assert_eq!(
            dur, vis,
            "step {i}: inline materialize → durable == visibility"
        );
        assert_eq!(vis, outcome.commit_version);
    }
}

/// Mixed tx + non-tx commits through the same gate. Non-tx writes route
/// through `MvccStore::set_versioned` (when one is attached). To exercise the
/// non-tx mark_durable site we drive the gate directly via the
/// `assign_next_version_guarded` + `mark_durable` pair the way `MvccStore`
/// does — this mirrors the production ordering (`guard.commit()` first,
/// `mark_durable` second) without spinning up an `MvccStore` we then never
/// read from. End state: durable == visibility; never durable > visibility.
#[tokio::test]
async fn mixed_tx_and_nontx_durable_equals_visibility_at_end() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();

    // tx commit #1
    let o1 = commit_tx(staged_tx(1, 300, b"a", b"1"), &repo)
        .await
        .unwrap();
    assert!(o1.materialized());
    assert!(gate.durable_watermark() <= gate.last_committed());
    assert_eq!(gate.durable_watermark(), gate.last_committed());

    // non-tx style burst: assign + commit + mark_durable (matches MvccStore
    // ordering: guard.commit() first, mark_durable second).
    for _ in 0..3 {
        let g = gate.assign_next_version_guarded();
        let v = g.version();
        g.commit();
        // Mid-window observation: between commit() and mark_durable, durable
        // can lag — must not lead.
        assert!(gate.durable_watermark() <= gate.last_committed());
        gate.mark_durable(v);
        assert!(gate.durable_watermark() <= gate.last_committed());
        assert_eq!(gate.durable_watermark(), gate.last_committed());
    }

    // tx commit #2
    let o2 = commit_tx(staged_tx(2, 300, b"b", b"2"), &repo)
        .await
        .unwrap();
    assert!(o2.materialized());
    assert!(o2.commit_version > o1.commit_version);

    let dur = gate.durable_watermark();
    let vis = gate.last_committed();
    assert!(dur <= vis);
    assert_eq!(
        dur, vis,
        "final: inline materialize → durable == visibility"
    );
    assert_eq!(vis, o2.commit_version);
}

/// Observe between operations in a sequence: at NO point may durable exceed
/// visibility (the by-construction invariant). Runs a longer sequence with
/// frequent sampling to make any future ordering regression in the
/// `mark_durable` call sites loud.
#[tokio::test]
async fn durable_never_exceeds_visibility_under_load() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();

    for i in 0..20u64 {
        let before_dur = gate.durable_watermark();
        let before_vis = gate.last_committed();
        assert!(before_dur <= before_vis, "pre-commit step {i}");

        let outcome = commit_tx(staged_tx(100 + i, 400, b"k", b"v"), &repo)
            .await
            .unwrap();
        assert!(outcome.materialized());

        let after_dur = gate.durable_watermark();
        let after_vis = gate.last_committed();
        assert!(after_dur <= after_vis, "post-commit step {i}");
        assert!(after_vis >= before_vis, "visibility monotonic step {i}");
        assert!(after_dur >= before_dur, "durable monotonic step {i}");
    }
}
