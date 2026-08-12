//! #1098 round 2 — writer-side publish-order fix proof.
//!
//! A round-2 `@oh` review of #1098's reader-side reorder (see
//! `shamir-engine`'s `p1098_gen_read_order_tests.rs` for the reader-side
//! fix and its own pause-seam test) found the reader-side fix ALONE does
//! not close the race: `create_unique_index_from_records` used to publish
//! `bump_generation()` BEFORE `write_barrier_flags.set(UNIQUE_INDEX_EXISTS)`
//! — two independent atomics with no combined ordering guarantee. Even with
//! a reader capturing generation first, it could still observe the NEW
//! generation while the flag store hadn't landed, reopening the same hole
//! one level down.
//!
//! Fixed by publishing the flag BEFORE bumping generation. `bump_generation`'s
//! `fetch_add` is `AcqRel`, so a reader that observes the NEW generation via
//! an `Acquire` load is guaranteed — by that synchronizes-with edge plus
//! program order on the single writer thread — to also observe every write
//! made BEFORE the bump, including the (now earlier) flag store.
//!
//! This test proves that guarantee directly: it parks a `CREATE UNIQUE
//! INDEX` strictly AFTER the flag is set and BEFORE generation is bumped,
//! then reads generation-then-flag (the exact order the reader-side fix
//! uses) WHILE parked there — and asserts the flag read sees `true`, even
//! though the bump the test is parked in front of hasn't happened yet.

use super::helpers::{create_manager, create_test_value};
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_manager_unique::{
    PostFlagSetPreGenBumpHook, TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK,
};
use shamir_storage::types::RecordKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

#[tokio::test]
async fn p1098_writer_flag_before_gen_bump_is_visible_to_a_reader_parked_mid_create() {
    let (data_store, _info_store, manager) = create_manager();

    // Durable record A exists BEFORE any unique index — written directly
    // to `data_store` (the raw scan source `create_unique_index`'s
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

    // Arm the #1098 round-2 pause seam (one-shot; nextest isolates this
    // global per test process).
    let hook = Arc::new(PostFlagSetPreGenBumpHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // Spawn CREATE UNIQUE INDEX. It backfills A's posting (only record —
    // succeeds), sets the flag, then parks at the seam BEFORE bumping
    // generation.
    let mgr_for_create = manager.clone();
    let index_def = IndexDefinition::new(2001, vec![IndexInfoItem::new(vec![1])]);
    let create_task =
        tokio::spawn(async move { mgr_for_create.create_unique_index(index_def).await });

    // Rendezvous: wait until CREATE has actually parked at the seam
    // (busy-poll `reached`, no sleeps).
    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    // While parked (flag already true, generation not yet bumped),
    // simulate a concurrent tx's stage-time reads in the EXACT order the
    // #1098 reader-side fix uses: generation first, then the flag.
    let stage_gen = manager.generation();
    let flag_seen_while_parked = manager.has_unique_indexes();

    // Release the parked CREATE.
    hook.resume.notify_one();
    create_task
        .await
        .expect("task must not panic")
        .expect("CREATE UNIQUE INDEX must succeed — only A exists");

    // THE PROOF: the flag must already read `true` here, even though the
    // generation bump this reader is racing against hadn't landed yet at
    // the moment of the read. This is exactly the property that makes the
    // reader-side gen-then-checks order (captured first, still possibly
    // OLD) sufficient: whichever the reader sees regarding the flag, it is
    // never the case that the flag reads `false` while a LATER-in-time
    // generation read would already reflect the bump — because the writer
    // never lets the bump become visible before the flag does.
    assert!(
        flag_seen_while_parked,
        "has_unique_indexes() must read true while parked strictly after \
         the flag set and before the generation bump — proving the \
         writer's flag-then-gen publish order is what makes the reader's \
         gen-then-checks order actually safe"
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
