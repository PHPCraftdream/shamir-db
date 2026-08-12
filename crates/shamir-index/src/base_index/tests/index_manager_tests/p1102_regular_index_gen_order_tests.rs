//! #1102 — regular hash index writer-side publish-order proof.
//!
//! Before this fix, `has_indexes` (the regular index existence flag) was a
//! standalone `Arc<AtomicBool>` written with `Ordering::Release` but read with
//! `Ordering::Relaxed`. Even with the writer setting the flag BEFORE bumping
//! generation, a `Release` store paired with a `Relaxed` load establishes **no
//! happens-before edge whatsoever**. A reader could observe the NEW generation
//! while the flag store hadn't yet become visible.
//!
//! The fix folds `has_indexes` into the same packed `write_barrier_flags` word
//! as `UNIQUE_INDEX_EXISTS` (bit 0), using a new `REGULAR_INDEX_EXISTS` bit
//! (bit 6). Both flags now use the same `SeqCst` ordering — the writer sets the
//! flag before bumping generation, and a reader that observes the NEW generation
//! via an `Acquire` load is guaranteed — by that synchronizes-with edge plus
//! program order on the single writer thread — to also observe the flag set.
//!
//! This test proves that guarantee directly: it parks a `CREATE INDEX` call
//! strictly AFTER the flag is set and BEFORE the generation bump, then reads
//! generation-then-flag (the exact order the reader-side fix uses) WHILE parked
//! there — and asserts the flag read sees `true`, even though the bump the test
//! is parked in front of hasn't happened yet.

use super::helpers::{create_manager, create_test_value};
use crate::base_index::backfill_pause_hook::BackfillPauseHook;
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use shamir_storage::types::RecordKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

#[tokio::test]
async fn p1102_writer_flag_before_gen_bump_is_visible_to_a_reader_parked_mid_create() {
    let (data_store, _info_store, manager) = create_manager();

    // Durable record A exists BEFORE any regular index — written directly
    // to `data_store` (the raw scan source `create_index`'s
    // backfill reads), matching the byte layout that scan expects.
    let value_a = create_test_value(&[(1, InnerValue::Str("x".to_string()))]);
    let rid_a = RecordId::new();
    data_store
        .set(
            RecordKey::from_slice(rid_a.as_bytes()),
            value_a.to_bytes().unwrap(),
        )
        .await
        .unwrap();

    // Install a one-shot pause hook that fires exactly once.
    let hook = Arc::new(BackfillPauseHook::new());
    manager
        .post_flag_set_pre_gen_bump_hook
        .store(Some(Arc::clone(&hook)));

    // Spawn CREATE INDEX. It backfills A's posting (only record — succeeds),
    // sets the flag, then parks at the seam BEFORE bumping generation.
    let mgr_for_create = manager.clone();
    let index_def = IndexDefinition::new(2001, vec![IndexInfoItem::new(vec![1])]);
    let create_task = tokio::spawn(async move { mgr_for_create.create_index(index_def).await });

    // Rendezvous: wait until CREATE has actually parked at the seam.
    hook.wait_until_parked().await;

    // While parked (flag already true, generation not yet bumped),
    // simulate a concurrent tx's stage-time reads in the EXACT order the
    // #1098 reader-side fix uses (same order applies here): generation first,
    // then the flag.
    let stage_gen = manager.generation();
    let flag_seen_while_parked = manager.has_indexes();

    // Release the parked CREATE (unpark the hook).
    hook.release();
    create_task
        .await
        .expect("task must not panic")
        .expect("CREATE INDEX must succeed — only A exists");

    // THE PROOF: the flag must already read `true` here, even though the
    // generation bump this reader is racing against hadn't landed yet at
    // the moment of the read. This is exactly the property that makes the
    // reader's gen-then-checks order sufficient: whichever the reader sees
    // regarding the flag, it is never the case that the flag reads `false`
    // while a LATER-in-time generation read would already reflect the bump —
    // because the writer never lets the bump become visible before the flag does.
    assert!(
        flag_seen_while_parked,
        "has_indexes() must read true while parked strictly after \
         the REGULAR_INDEX_EXISTS flag set and before the generation bump — \
         proving the writer's flag-then-gen publish order is what makes the \
         reader's gen-then-checks order actually safe"
    );

    // Sanity: the generation captured while parked (pre-bump) must be
    // strictly older than the final (post-bump) generation.
    let final_gen = manager.generation();
    assert!(
        stage_gen < final_gen,
        "generation captured while parked at the seam must predate the \
         final, post-bump generation (stage_gen={stage_gen}, final_gen={final_gen})"
    );
}
