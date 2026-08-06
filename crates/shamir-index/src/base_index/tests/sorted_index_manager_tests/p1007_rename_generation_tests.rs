//! P0-2 (#1007) — `rename_definition` must bump `SortedIndexManager::generation`,
//! mirroring the existing `register`/`drop_index` bump sites.
//!
//! Before this fix, `rename_definition` performed the RCU definition swap +
//! epoch-carry + persist but never advanced `self.generation`, so
//! `pre_commit.rs`'s sorted rederive gate (`sorted_mgr.generation() ==
//! stage_gen`) could never detect that a rename happened between a tx's
//! stage time and its commit — the gate stayed closed and no re-derivation
//! ran. This test proves the fix in isolation, at the registry level, the
//! same way `IndexRegistry`'s `concurrent_inserts_advance_generation_by_exactly_n`
//! (`registry_tests.rs`) proves index2's `insert`/`remove_by_id` bump the
//! registry generation.

use super::helpers::fresh_mgr;
use crate::base_index::sorted_index_definition::SortedIndexDefinition;

/// `rename_definition` must advance `generation()` — the same contract
/// `register`/`drop_index` already uphold. Confirmed to FAIL (no advance)
/// against the pre-#1007 code (the `fetch_add` call is simply absent from
/// `rename_definition`).
#[tokio::test]
async fn rename_definition_bumps_generation() {
    let (_, mgr) = fresh_mgr().await;

    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let gen_after_register = mgr.generation();

    mgr.rename_definition(101, 102).await.unwrap();
    let gen_after_rename = mgr.generation();

    assert!(
        gen_after_rename > gen_after_register,
        "#1007: rename_definition must advance SortedIndexManager::generation() \
         (mirrors register/drop_index's existing fetch_add call) — before: {gen_after_register}, \
         after: {gen_after_rename}"
    );
}

/// Multiple renames must each independently bump `generation()` — not just
/// the first one (guards against an off-by-one / early-return regression).
#[tokio::test]
async fn multiple_renames_each_bump_generation() {
    let (_, mgr) = fresh_mgr().await;

    mgr.register(SortedIndexDefinition::new(201, vec![301]))
        .await
        .unwrap();
    let gen0 = mgr.generation();

    mgr.rename_definition(201, 202).await.unwrap();
    let gen1 = mgr.generation();
    assert!(gen1 > gen0, "first rename must bump generation");

    mgr.rename_definition(202, 203).await.unwrap();
    let gen2 = mgr.generation();
    assert!(gen2 > gen1, "second rename must ALSO bump generation");
}
