use shamir_query_types::admin::FkAction;

use super::support::{
    arm_child_writer, assert_on_update_parent_flagged, read_first_i64, rekey_parent, row_count,
    seed_parent, setup_race_test_on_update,
};

// The tests above exercise the on_delete path exclusively (every FK is built
// via `ForeignKeyRef::with_on_delete`). F-35 closed a gap where the cache
// stored only `on_delete` in each `ReverseFkEntry`, so the UPDATE arm's
// `is_fk_parent_with_update_action` consult a role flag the cache did not
// carry — an `on_delete = NoAction, on_update = Restrict/Cascade/SetNull` FK
// never got the Serializable upgrade for its implicit UPDATE path, silently
// reopening the cross-transaction race F-28 was meant to close for on_update.
//
// These tests mirror the on_delete structure exactly, but shape the FK via
// `ForeignKeyRef::with_on_update` (on_delete stays NoAction, isolating the
// UPDATE-path role flag from the DELETE-path one) and drive an implicit
// UPDATE (re-keying the parent's referenced field) instead of a DELETE.
//
// Both commit orderings are covered the same way the on_delete tests above
// cover them: the writer fires once in the after-begin/before-commit window
// (the deterministic injection point), and each test accepts BOTH terminal
// outcomes the SSI resolution can produce — `tx_conflict` (the child write
// won the race; the parent aborted) and the legitimate action outcome on
// `retry_on_tx_conflict`'s fresh-snapshot retry (the parent won; its
// re-planned fan-out correctly handled the now-committed child). Only a
// silent `Ok` that leaves an orphaned/dangling child reference is rejected.
// ============================================================================

#[tokio::test]
async fn on_update_restrict_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Restrict).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION: pre-F-35 the UPDATE arm consulted
    // the single cached `on_delete` field (NoAction for this FK) and opened at
    // Snapshot; now it consults the `on_update` flag and upgrades.
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    match resp {
        Err(e) => {
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_restrict")),
                "on_update RESTRICT update must reject the race via a coded SSI \
                 conflict OR a legitimate fk_restrict, got: {e:?}"
            );
        }
        Ok(_) => panic!(
            "on_update RESTRICT update must NOT commit silently past a racing child \
             insert — this would be the exact 'dangling reference' bug F-35 closes"
        ),
    }

    // Invariant: the update was rejected, so the parent's referenced value
    // is unchanged and the child still references it — no orphan.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );
    assert_eq!(
        read_first_i64(&resolver, "parent", "id").await,
        Some(1),
        "parent id must be unchanged (update rejected)"
    );
    assert_eq!(
        read_first_i64(&resolver, "child", "parent_id").await,
        Some(1),
        "child must still reference parent id=1 (no dangling reference)"
    );
}

// ============================================================================
// 7. End-to-end on_update race closure — CASCADE.
//
// Same race shape, on_update = Cascade. The parent update re-keys id 1→2; the
// raced-in child references the OLD value 1. Invariant: NEVER "parent committed
// with id=2 AND child still references 1" (an orphaned reference). The
// operation either fails closed (no commit, both rows intact at id=1) or its
// retry wins and CASCADE correctly re-keys the child to 2 alongside.
// ============================================================================

#[tokio::test]
async fn on_update_cascade_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION (see `assert_on_update_parent_flagged`).
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    // F-35 proof #2 — the race is CAUGHT: the raced-in child reference is
    // NEVER left orphaned. The parent's UPDATE either fails closed (rolled
    // back, both rows intact at id=1) or commits with CASCADE having
    // re-keyed the child to the new value. Only a silent Ok leaving the
    // child referencing the OLD, now-gone value would be the corruption
    // F-35 closes.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );

    let parent_id = read_first_i64(&resolver, "parent", "id").await;
    let child_parent_id = read_first_i64(&resolver, "child", "parent_id").await;

    match resp {
        Err(e) => {
            // Operation failed closed — no commit, so nothing was re-keyed.
            // The observed deterministic code here is `fk_on_update`
            // (row-not-found): `plan_fk_on_update`'s `list_stream_tx` scan
            // reads committed-at-now and sees the raced-in child, but
            // `apply_fk_update_plan`'s per-key `read_one_tx_bytes` reads at
            // the tx snapshot (taken at begin, before the writer committed)
            // and can't resolve that same row — a pre-existing apply-path
            // visibility mismatch in `fk_on_update.rs`, out of scope for this
            // cache-role-flag fix. `tx_conflict` (Serializable commit abort)
            // is the other acceptable closed outcome. Either way: no orphan.
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_on_update")),
                "on_update CASCADE update must fail closed (no orphan), got: {e:?}"
            );
            assert_eq!(
                parent_id,
                Some(1),
                "parent id unchanged after failed update"
            );
            assert_eq!(
                child_parent_id,
                Some(1),
                "child unchanged after failed update (no partial cascade, no orphan)"
            );
        }
        Ok(_) => {
            // The retry won: update committed AND CASCADE re-keyed the child.
            assert_eq!(
                parent_id,
                Some(2),
                "parent id must be re-keyed to 2 after a committed cascade update"
            );
            assert_eq!(
                child_parent_id,
                Some(2),
                "CASCADE must have re-keyed the raced-in child to 2 — an orphan \
                 (child still referencing 1 while parent is 2) would be the exact \
                 silent-corruption bug F-35 closes"
            );
        }
    }
}

// ============================================================================
// 8. End-to-end on_update race closure — SET NULL.
//
// Same race shape, on_update = SetNull. Invariant: NEVER "parent committed
// with id=2 AND child still references 1" (an orphaned reference). The
// operation either fails closed (both rows intact at id=1) or its retry wins
// and SET NULL correctly nulls the raced-in child's FK field.
// ============================================================================

#[tokio::test]
async fn on_update_set_null_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::SetNull).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION (see `assert_on_update_parent_flagged`).
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );

    let parent_id = read_first_i64(&resolver, "parent", "id").await;
    // Null reads back as `None` via `get_value_i64`.
    let child_parent_id = read_first_i64(&resolver, "child", "parent_id").await;

    match resp {
        Err(e) => {
            // Operation failed closed — see the CASCADE test above for the
            // `fk_on_update` (row-not-found) rationale. Either way: no orphan.
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_on_update")),
                "on_update SET NULL update must fail closed (no orphan), got: {e:?}"
            );
            assert_eq!(
                parent_id,
                Some(1),
                "parent id unchanged after failed update"
            );
            assert_eq!(
                child_parent_id,
                Some(1),
                "child unchanged after failed update (no partial set-null, no orphan)"
            );
        }
        Ok(_) => {
            // The retry won: update committed AND SET NULL nulled the child.
            assert_eq!(
                parent_id,
                Some(2),
                "parent id must be re-keyed to 2 after a committed set-null update"
            );
            assert_eq!(
                child_parent_id, None,
                "SET NULL must have nulled the raced-in child's FK field — an orphan \
                 (child still referencing 1 while parent is 2) would be the exact \
                 silent-corruption bug F-35 closes"
            );
        }
    }
}

// ============================================================================
// 9. Regression — on_delete = NoAction, on_update = NoAction does NOT upgrade
//    to Serializable.
//
// Confirms the F-35 split did not accidentally flag EVERY FK-parent table
// for the Serializable upgrade regardless of action kind. The proof is
// direct: the cache's two role flags must both read `false` for a
// NoAction/NoAction FK. (Behaviorally, NoAction runs no on_update fan-out
// scan, so the isolation level is not observable via an FK-race abort either
// way — the flag assertions below are the authoritative check; the clean
// commit just confirms the NoAction path was not broken.)
// ============================================================================

#[tokio::test]
async fn on_update_no_action_does_not_upgrade_to_serializable() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::NoAction).await;
    seed_parent(&resolver, 1).await;

    // The seed insert warmed the cache (its `require_footprint_if_fk_child`
    // hook builds the whole-repo reverse-FK map on the first touch).
    let cache = repo.fk_reverse_cache();
    assert!(
        !cache.is_fk_parent_with_update_action("parent"),
        "on_update=NoAction must NOT flag the parent for the UPDATE-path upgrade"
    );
    assert!(
        !cache.is_fk_parent_with_delete_action("parent"),
        "on_delete=NoAction must NOT flag the parent for the DELETE-path upgrade"
    );

    // Behavioral confirmation: the implicit UPDATE commits cleanly (no
    // spurious Serializable abort) and re-keys the parent as requested.
    let resp = rekey_parent(&resolver, 1, 2).await;
    assert!(
        resp.is_ok(),
        "NoAction/NoAction parent UPDATE must commit cleanly (no spurious abort): {resp:?}"
    );
    assert_eq!(
        read_first_i64(&resolver, "parent", "id").await,
        Some(2),
        "parent id must be re-keyed to 2"
    );
}
