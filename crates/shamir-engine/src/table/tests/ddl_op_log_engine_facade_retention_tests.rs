//! #1068 regression (cross-crate audit, shamir-engine group 26, defect 2).
//!
//! `crate::table::ddl_op_log` is a `pub use` re-export of
//! `shamir_index::base_index::ddl_op_log` (see
//! `crates/shamir-engine/src/table/mod.rs`) — every engine-level call site
//! (`TableManager::create`'s open-time catch-up sweep,
//! `table_manager_index_mgmt.rs`'s DDL status writes, `shamir-db`'s admin DDL
//! handlers) reaches the DDL op-log exclusively through this re-export.
//!
//! A STALE, orphaned copy of this module used to also live at
//! `crates/shamir-engine/src/table/ddl_op_log.rs`, with a permanently
//! `Ok(())` stub `maybe_evict_terminal_records` and a doc comment describing
//! a FIFO cap that was never actually enforced. That file was never wired
//! into `crate::table` via a `mod` declaration (confirmed: zero
//! `mod ddl_op_log;` anywhere in this crate's source — `table/mod.rs` only
//! has the `pub use` above), so it was 100% dead code, unreachable from any
//! call site, and has been deleted.
//!
//! The REAL implementation every caller actually reaches (the re-exported
//! one) already enforces the FIFO cap correctly. This test proves that from
//! the engine crate's own call surface, exercising BOTH documented trigger
//! points: the throttled post-terminal-write sweep (every 100th terminal
//! write, internal to `write_op_status`) and the explicit open-time
//! catch-up sweep (`TableManager::create` calls `maybe_evict_terminal_records`
//! directly — see `table_manager.rs`'s "#1068" comment).

use crate::table::ddl_op_log::{maybe_evict_terminal_records, read_op_status, write_op_status};
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::{Repo, Store};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

/// Mirrors `shamir_index::base_index::ddl_op_log::DDL_OP_LOG_CAP`, which is
/// private to that module (and therefore unreachable from this crate). Both
/// modules' doc comments describe it as a fixed cap for the first slice; if
/// it ever changes, this test's op counts (chosen relative to it) need to
/// move in lockstep.
const CAP: u32 = 10_000;

/// Builds `count` distinct, strictly-ascending-by-seq op ids up front —
/// `RecordId::from_ts_seq` fills a 4-byte RANDOM tail on every call, so the
/// SAME `(seq)` must map to a `RecordId` computed exactly ONCE and reused
/// for both the write and the later read, rather than recomputed from
/// `seq` a second time (a second call would silently produce a DIFFERENT
/// key, since only the timestamp+seq prefix is deterministic).
fn build_op_ids(count: u32) -> Vec<RecordId> {
    (0..count)
        .map(|seq| RecordId::from_ts_seq(1_700_000_000_000_000, seq))
        .collect()
}

fn terminal_status(op_id: RecordId, seq: u32) -> DdlOpStatus {
    DdlOpStatus {
        op_id,
        kind: DdlOpKind::Other {
            description: format!("test op {seq}"),
        },
        state: DdlOpState::Succeeded {
            completed_at: seq as u64,
        },
    }
}

async fn fresh_info_store() -> Arc<dyn Store> {
    let repo = InMemoryRepo::new();
    repo.store_get("ddl_op_log_retention_test").await.unwrap()
}

#[tokio::test]
async fn maybe_evict_terminal_records_fires_at_both_documented_trigger_points() {
    let store = fresh_info_store().await;

    // `op_ids[seq]` is fixed for the whole test — both trigger phases and
    // both write/read sides reuse it. Ascending `seq` ⇒ ascending byte
    // order ⇒ FIFO oldest-first (per the eviction sweep's own doc).
    let op_ids = build_op_ids(CAP + 105);

    // ── Phase 1: post-terminal-write (throttled) trigger ───────────────
    // Write CAP + 100 terminal records via the real production entry
    // point. `write_op_status`'s internal throttle checks every 100th
    // terminal write; the checkpoint at CAP+100 is the first one that
    // actually has excess to evict (every earlier checkpoint is <= CAP).
    for seq in 0..(CAP + 100) {
        write_op_status(&store, &terminal_status(op_ids[seq as usize], seq))
            .await
            .unwrap();
    }

    // THE PROOF (post-terminal-write trigger): no explicit eviction call
    // was made above, yet the log must already be back at the cap — the
    // throttled sweep inside `write_op_status` did it on its own.
    let mut remaining = 0usize;
    for op_id in &op_ids[0..(CAP + 100) as usize] {
        if read_op_status(&store, op_id).await.unwrap().is_some() {
            remaining += 1;
        }
    }
    assert_eq!(
        remaining, CAP as usize,
        "the throttled post-terminal-write sweep must bring the log back to the cap"
    );

    // FIFO: the 100 OLDEST (lowest seq) must be exactly the ones evicted.
    for op_id in &op_ids[0..100] {
        assert!(
            read_op_status(&store, op_id).await.unwrap().is_none(),
            "the oldest 100 op ids must have been evicted"
        );
    }
    for op_id in &op_ids[100..(CAP + 100) as usize] {
        assert!(
            read_op_status(&store, op_id).await.unwrap().is_some(),
            "op ids within the retained window must still be present"
        );
    }

    // ── Phase 2: open-time (explicit) trigger ───────────────────────────
    // Write 5 more terminal records — NOT a multiple of 100 past the last
    // checkpoint, so the internal throttle does not fire again. The log is
    // now CAP + 5, unswept.
    for seq in (CAP + 100)..(CAP + 105) {
        write_op_status(&store, &terminal_status(op_ids[seq as usize], seq))
            .await
            .unwrap();
    }

    // THE PROOF (open-time trigger): call `maybe_evict_terminal_records`
    // directly, exactly as `TableManager::create`'s open path does.
    maybe_evict_terminal_records(&store).await.unwrap();

    let mut remaining_after_open = 0usize;
    for op_id in &op_ids[0..(CAP + 105) as usize] {
        if read_op_status(&store, op_id).await.unwrap().is_some() {
            remaining_after_open += 1;
        }
    }
    assert_eq!(
        remaining_after_open, CAP as usize,
        "the explicit open-time sweep must bring the log back to the cap"
    );
    // The next-oldest batch (seq 100..105, the new front of the FIFO
    // queue after phase 1's eviction) must now be gone too.
    for op_id in &op_ids[100..105] {
        assert!(
            read_op_status(&store, op_id).await.unwrap().is_none(),
            "the next-oldest batch must have been evicted by the open-time sweep"
        );
    }
}
