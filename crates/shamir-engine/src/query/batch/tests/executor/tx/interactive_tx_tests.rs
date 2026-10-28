//! Interactive (multi-call) transaction tests.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::filter::Filter;
use shamir_types::access::Actor;

use crate::query::batch::{
    commit_interactive_tx, execute_batch_unchecked as execute_batch,
    execute_in_open_tx_unchecked as execute_in_open_tx, open_interactive_tx, BatchError,
};

use super::super::common::TxTestResolver;

// ============================================================================
// Phase B — interactive (multi-call) transaction glue
// ============================================================================

#[tokio::test]
async fn interactive_tx_read_resolves_overlay_interned_field_names() {
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        crate::repo::repo_types::BoxRepoFactory::in_memory(),
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let mut insert_batch = Batch::new();
    insert_batch.insert(
        "insert-1",
        write::insert("users").row(
            doc()
                .set("name", "overlay-name-12")
                .unwrap()
                .set("qty", 12)
                .unwrap()
                .set("overlay_group", "shared")
                .unwrap(),
        ),
    );
    insert_batch.insert(
        "insert-2",
        write::insert("users").row(
            doc()
                .set("name", "overlay-name-37")
                .unwrap()
                .set("qty", 37)
                .unwrap()
                .set("overlay_group", "other")
                .unwrap(),
        ),
    );
    insert_batch.insert(
        "insert-3",
        write::insert("users").row(
            doc()
                .set("name", "overlay-name-25")
                .unwrap()
                .set("qty", 7)
                .unwrap()
                .set("overlay_group", "shared")
                .unwrap(),
        ),
    );
    execute_in_open_tx(
        &insert_batch.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();

    let mut read_batch = Batch::new();
    read_batch.query(
        "plain",
        Query::from("users").select(["name", "qty"]).build(),
    );
    let response = execute_in_open_tx(
        &read_batch.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    let plain = &response.results["plain"].records;
    assert_eq!(plain.len(), 3);
    let mut plain_rows = plain
        .iter()
        .map(|record| {
            (
                record.get_value_str("name").unwrap_or_default().to_owned(),
                record.get_value_i64("qty").unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    plain_rows.sort_unstable();
    assert_eq!(
        plain_rows,
        vec![
            ("overlay-name-12".to_string(), 12),
            ("overlay-name-25".to_string(), 7),
            ("overlay-name-37".to_string(), 37),
        ]
    );

    let mut ordered = Batch::new();
    ordered.query(
        "ordered",
        Query::from("users")
            .select(["name", "qty"])
            .order_by(shamir_query_types::read::OrderByItem::asc("qty"))
            .build(),
    );
    ordered.query(
        "descending",
        Query::from("users")
            .select(["name", "qty"])
            .order_by(shamir_query_types::read::OrderByItem::desc("qty"))
            .build(),
    );
    let conditional_false = shamir_query_builder::val::cond(
        shamir_query_builder::filter::eq("qty", 7),
        shamir_query_builder::val::lit(1),
        shamir_query_builder::val::lit(0),
    );
    ordered.query(
        "condition_false",
        Query::from("users")
            .select(["name", "qty"])
            .where_(shamir_query_builder::filter::value_eq(conditional_false, 0))
            .order_by(shamir_query_types::read::OrderByItem::asc("qty"))
            .build(),
    );
    let conditional_true = shamir_query_builder::val::cond(
        shamir_query_builder::filter::eq("qty", 7),
        shamir_query_builder::val::lit(1),
        shamir_query_builder::val::lit(0),
    );
    ordered.query(
        "condition_true",
        Query::from("users")
            .select(["name", "qty"])
            .where_(shamir_query_builder::filter::value_eq(conditional_true, 1))
            .order_by(shamir_query_types::read::OrderByItem::asc("qty"))
            .build(),
    );
    let response = execute_in_open_tx(
        &ordered.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    let rows = &response.results["ordered"].records;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].get_value_str("name"), Some("overlay-name-25"));
    assert_eq!(rows[0].get_value_i64("qty"), Some(7));
    assert_eq!(rows[1].get_value_str("name"), Some("overlay-name-12"));
    assert_eq!(rows[1].get_value_i64("qty"), Some(12));
    assert_eq!(rows[2].get_value_str("name"), Some("overlay-name-37"));
    assert_eq!(rows[2].get_value_i64("qty"), Some(37));
    let rows = &response.results["condition_false"].records;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_value_i64("qty"), Some(12));
    assert_eq!(rows[1].get_value_i64("qty"), Some(37));
    let rows = &response.results["condition_true"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value_i64("qty"), Some(7));
    let descending = &response.results["descending"].records;
    assert_eq!(descending.len(), 3);
    assert_eq!(descending[0].get_value_str("name"), Some("overlay-name-37"));
    assert_eq!(descending[0].get_value_i64("qty"), Some(37));
    assert_eq!(descending[1].get_value_str("name"), Some("overlay-name-12"));
    assert_eq!(descending[1].get_value_i64("qty"), Some(12));
    assert_eq!(descending[2].get_value_str("name"), Some("overlay-name-25"));
    assert_eq!(descending[2].get_value_i64("qty"), Some(7));

    let mut aggregates = Batch::new();
    aggregates.query(
        "grouped",
        Query::from("users")
            .select([
                shamir_query_builder::select::field("overlay_group"),
                shamir_query_builder::select::count_all("count_value"),
                shamir_query_builder::select::sum("qty", "sum_value"),
                shamir_query_builder::select::min("qty", "min_value"),
                shamir_query_builder::select::max("qty", "max_value"),
            ])
            .group_by("overlay_group")
            .build(),
    );
    let conditional = shamir_query_builder::val::cond(
        shamir_query_builder::filter::eq("qty", 12),
        shamir_query_builder::val::lit(1),
        shamir_query_builder::val::lit(0),
    );
    aggregates.query(
        "conditional_sum",
        Query::from("users")
            .select([shamir_query_builder::select::sum_of(
                conditional,
                "sum_value",
            )])
            .build(),
    );
    let response = execute_in_open_tx(
        &aggregates.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    let rows = &response.results["grouped"].records;
    assert_eq!(rows.len(), 2);
    let mut aggregate_rows = rows
        .iter()
        .map(|row| {
            (
                row.get_value_str("overlay_group").unwrap().to_owned(),
                row.get_value_i64("count_value").unwrap(),
                row.get_value_i64("sum_value").unwrap(),
                row.get_value_i64("min_value").unwrap(),
                row.get_value_i64("max_value").unwrap(),
            )
        })
        .collect::<Vec<_>>();
    aggregate_rows.sort_unstable();
    assert_eq!(
        aggregate_rows,
        vec![
            ("other".to_string(), 1, 37, 37, 37),
            ("shared".to_string(), 2, 19, 7, 12)
        ]
    );
    assert_eq!(
        response.results["conditional_sum"].records[0].get_value_i64("sum_value"),
        Some(1)
    );

    drop(tx);
    drop(guard);
}

#[tokio::test]
async fn interactive_tx_accumulates_writes_across_calls_then_commits() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // BEGIN — mint the interactive tx + its snapshot guard (the server would
    // park both in its registry; here the test holds them).
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // EXECUTE #1 — stage the first insert. The tx stays OPEN, so the response
    // carries no commit outcome.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "alice").unwrap()),
    );
    let call1 = b.build();
    let r1 = execute_in_open_tx(
        &call1,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(
        r1.transaction.is_none(),
        "tx is still open after EXECUTE #1 — no per-call commit outcome"
    );

    // A separate observer must NOT see the uncommitted staged write
    // (snapshot isolation — nothing is durable before COMMIT).
    let tbl = repo.get_table("users").await.unwrap();
    {
        let stream = tbl.list_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(b) = stream.next().await {
            count += b.unwrap().len();
        }
        assert_eq!(count, 0, "outside observer sees nothing before commit");
    }

    // EXECUTE #2 — stage a second insert into the SAME open tx.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "bob").unwrap()),
    );
    let call2 = b.build();
    let r2 = execute_in_open_tx(
        &call2,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(r2.transaction.is_none(), "tx still open after EXECUTE #2");

    // COMMIT — both calls' writes land together at one commit version.
    let outcome = commit_interactive_tx(&repo, tx).await.unwrap();
    assert!(outcome.commit_version > 0, "commit assigns a version");
    // The snapshot guard is released only AFTER commit returned.
    drop(guard);

    // Both records, staged across two SEPARATE EXECUTE calls, are visible.
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 2,
        "both writes staged across two EXECUTE calls must commit together"
    );
}

#[tokio::test]
async fn interactive_tx_rollback_discards_staged_writes() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "ghost").unwrap()),
    );
    let call = b.build();
    execute_in_open_tx(
        &call,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();

    // ROLLBACK = drop the parked tx (RAII rollback, no storage side effects),
    // then release the snapshot.
    drop(tx);
    drop(guard);

    // Nothing was committed — a fresh scan sees no records.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "a rolled-back interactive tx must leave nothing durable"
    );
}

// ============================================================================
// Phase B Stage 9 — SSI read-set ACCUMULATES across multiple TxExecute calls.
// ============================================================================
#[tokio::test]
async fn interactive_ssi_write_skew_across_calls_one_aborts() {
    use shamir_types::core::interner::TouchInd;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Seed two named rows so an UPDATE can target one by field.
    let tbl = repo.get_table("users").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();
    let name_id = match interner.touch_ind("name").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let val_id = match interner.touch_ind("val").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    tbl.interner().persist().await.unwrap();
    let mk_row = |name: &str| {
        let mut m = shamir_types::types::common::new_map_wc(2);
        m.insert(
            shamir_types::core::interner::InternerKey::new(name_id),
            shamir_types::types::value::InnerValue::Str(name.into()),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(val_id),
            shamir_types::types::value::InnerValue::Str("initial".into()),
        );
        shamir_types::types::value::InnerValue::Map(m)
    };
    tbl.insert(&mk_row("alice")).await.unwrap();
    tbl.insert(&mk_row("bob")).await.unwrap();

    // BEGIN two interactive Serializable txs.
    let (mut tx_a, guard_a) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    let (mut tx_b, guard_b) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();

    // Call #1 on EACH tx: SELECT every users row -> recorded into that tx's
    // read_set via the tx-aware read path.
    let mut b = Batch::new();
    b.id(1);
    b.query("r", Query::from("users"));
    let select_req = b.build();

    let ra1 = execute_in_open_tx(
        &select_req,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    assert!(ra1.transaction.is_none(), "tx_a still open after call #1");
    assert_eq!(
        ra1.results["r"].records.len(),
        2,
        "tx_a SELECT sees both seeded rows"
    );

    let rb1 = execute_in_open_tx(
        &select_req,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();
    assert!(rb1.transaction.is_none(), "tx_b still open after call #1");
    assert_eq!(
        rb1.results["r"].records.len(),
        2,
        "tx_b SELECT sees both seeded rows"
    );

    // Call #2 on EACH tx: UPDATE the row the OTHER tx also read.
    let mut b = Batch::new();
    b.id(2);
    b.try_update(
        "w",
        write::update("users")
            .where_(shamir_query_builder::filter::eq("name", "alice"))
            .set(doc().set("val", "a2").unwrap()),
    )
    .unwrap();
    let update_alice = b.build();

    let mut b = Batch::new();
    b.id(2);
    b.try_update(
        "w",
        write::update("users")
            .where_(shamir_query_builder::filter::eq("name", "bob"))
            .set(doc().set("val", "b2").unwrap()),
    )
    .unwrap();
    let update_bob = b.build();

    let _ra2 = execute_in_open_tx(
        &update_alice,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    let _rb2 = execute_in_open_tx(
        &update_bob,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();

    // COMMIT tx_a first — succeeds.
    let oa = commit_interactive_tx(&repo, tx_a).await;
    assert!(
        oa.is_ok(),
        "tx_a must commit cleanly; got {:?}",
        oa.as_ref().err()
    );
    drop(guard_a);

    // COMMIT tx_b — MUST abort with SsiConflict.
    let ob = commit_interactive_tx(&repo, tx_b).await;
    match ob {
        Err(crate::tx::CommitError::SsiConflict { .. }) => {}
        other => panic!(
            "tx_b MUST abort with SsiConflict — recorded read from call #1 \
             must outlive that call inside the parked TxContext and be \
             validated at commit. Got {:?}",
            other
                .as_ref()
                .map(|_| "Ok(committed)")
                .map_err(|e| format!("Err({:?})", e)),
        ),
    }
    drop(guard_b);
}

// ============================================================================
// Phase B Stage 10 — concurrency + recovery tests for interactive tx
// ============================================================================

/// (a) Two interactive SI transactions race — each accumulates writes across
/// TWO `execute_in_open_tx` calls (the load-bearing Phase-B property), then
/// both commit.
#[tokio::test]
async fn two_interactive_si_txs_race_last_commit_wins() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Seed a baseline row OUTSIDE any tx.
    let tbl = repo.get_table("users").await.unwrap();
    let mut sb = Batch::new();
    sb.id(0);
    sb.insert(
        "seed",
        write::insert("users").row(doc().set("name", "baseline").unwrap()),
    );
    let seed_req = sb.build();
    let seed_resp = execute_batch(&seed_req, &resolver, None, None, Actor::System, "test").await;
    assert!(
        seed_resp.is_ok(),
        "seeding baseline row failed: {:?}",
        seed_resp.err()
    );

    // BEGIN two interactive SI txs.
    let (mut tx_a, guard_a) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    let (mut tx_b, guard_b) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Each tx does TWO execute calls (proves state accumulates across calls).
    let mk_ins = |id: i32, name: &str| -> crate::query::batch::BatchRequest {
        let mut b = Batch::new();
        b.id(id);
        b.insert(
            "ins",
            write::insert("users").row(doc().set("name", name).unwrap()),
        );
        b.build()
    };
    let ins_a1 = mk_ins(1, "a1");
    let ins_a2 = mk_ins(2, "a2");
    let ins_b1 = mk_ins(3, "b1");
    let ins_b2 = mk_ins(4, "b2");

    // Interleave the calls.
    execute_in_open_tx(
        &ins_a1,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    execute_in_open_tx(
        &ins_b1,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();
    execute_in_open_tx(
        &ins_a2,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    execute_in_open_tx(
        &ins_b2,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();

    // tx_a commits first → version V_a. tx_b commits second → version V_b > V_a.
    let o_a = commit_interactive_tx(&repo, tx_a).await.unwrap();
    drop(guard_a);
    let o_b = commit_interactive_tx(&repo, tx_b).await.unwrap();
    drop(guard_b);
    assert!(
        o_b.commit_version > o_a.commit_version,
        "second-committing interactive SI tx assigns a higher version \
         (last-commit-wins ordering)"
    );

    // Both txs' writes survive (SI permits both): 1 baseline + 4 inserts = 5.
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 5,
        "both interactive SI txs committed (1 seed + 2 + 2)"
    );
}

/// (b) Crash mid-interactive-tx leaves NOTHING durable.
#[tokio::test]
async fn crash_mid_interactive_tx_leaves_no_durable_footprint() {
    use std::sync::Arc;

    use futures::StreamExt;
    use shamir_storage::storage_in_memory::InMemoryRepo;

    let underlying = Arc::new(InMemoryRepo::new());

    // === ORIGINAL PROCESS ===
    {
        let repo = crate::repo::RepoInstance::new(
            "r".into(),
            crate::repo::BoxRepo::InMemory(Arc::clone(&underlying)),
            vec![crate::table::TableConfig::new("users")],
        );
        let resolver = TxTestResolver { repo: repo.clone() };

        let (mut tx, _guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();

        // Stage writes across TWO execute calls.
        let mut b1 = Batch::new();
        b1.id(1);
        b1.insert(
            "ins",
            write::insert("users").row(doc().set("name", "alpha").unwrap()),
        );
        let c1 = b1.build();

        let mut b2 = Batch::new();
        b2.id(2);
        b2.insert(
            "ins",
            write::insert("users").row(doc().set("name", "beta").unwrap()),
        );
        let c2 = b2.build();

        execute_in_open_tx(&c1, &resolver, None, None, &Actor::System, "test", &mut tx)
            .await
            .unwrap();
        execute_in_open_tx(&c2, &resolver, None, None, &Actor::System, "test", &mut tx)
            .await
            .unwrap();

        // Sanity: while tx is open, BEFORE commit, the WAL has no entry
        // (the WAL append runs only in commit Phase 4).
        let wal = repo.repo_wal().await.unwrap();
        assert!(
            wal.recover().await.unwrap().is_empty(),
            "no WAL entry exists pre-commit — the append runs only in Phase 4"
        );

        // === CRASH === drop tx + guard + repo WITHOUT calling
        // commit_interactive_tx.
        drop(tx);
        drop(_guard);
        drop(resolver);
        drop(repo);
    }

    // === RESTART === fresh RepoInstance over the SAME underlying storage.
    let repo = crate::repo::RepoInstance::new(
        "r".into(),
        crate::repo::BoxRepo::InMemory(Arc::clone(&underlying)),
        vec![crate::table::TableConfig::new("users")],
    );

    // (1) No WAL entry survives — none was ever written.
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.recover().await.unwrap().is_empty(),
        "crash before any commit leaves no WAL entry"
    );

    // (2) Recovery is a no-op.
    let replayed = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(
        replayed, 0,
        "recovery has nothing to replay — interactive tx never reached the WAL"
    );

    // (3) Nothing materialized — the table is empty.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "a crash mid-interactive-tx must leave NOTHING durable \
         (no wal.begin → clean abort)"
    );
}

// ============================================================================
// #670 — `execute_in_open_tx` must enforce the filter-nesting-depth DoS
// guard, exactly like `execute_batch_impl`/`run_nested_body_in_outer_tx`
// already do. Before the fix, `execute_in_open_tx` called `BatchPlanner::plan`
// + `validate_tables` but never `validate_filter_depth`, letting an
// interactive-tx client submit an arbitrarily deeply nested filter.
// ============================================================================

/// Build a `Filter::Not` chain `depth` levels deep, wrapping a trivial
/// `IsNotNull` leaf — the cheapest way to exceed `MAX_FILTER_DEPTH` (64)
/// without needing real field data.
fn deeply_nested_not_filter(depth: usize) -> Filter {
    let mut f = Filter::IsNotNull {
        field: vec!["x".to_string()],
    };
    for _ in 0..depth {
        f = Filter::Not {
            filter: Box::new(f),
        };
    }
    f
}

#[tokio::test]
async fn execute_in_open_tx_rejects_deeply_nested_filter() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    let mut read_q = Query::from("users").build();
    read_q.r#where = Some(deeply_nested_not_filter(100));
    b.query("r", read_q);
    let req = b.build();

    let err = execute_in_open_tx(&req, &resolver, None, None, &Actor::System, "test", &mut tx)
        .await
        .expect_err(
            "a filter nesting 100 levels deep (> MAX_FILTER_DEPTH=64) must be \
             rejected by execute_in_open_tx, exactly like execute_batch already \
             rejects it",
        );
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected a QueryError naming the nesting-depth violation, got {:?}",
        err
    );

    drop(tx);
    drop(guard);
}

/// Regression pairing: the SAME shape over the plain `execute_batch` path
/// must ALSO reject — proving both call sites now agree (before #670 they
/// silently diverged: `execute_batch` rejected, `execute_in_open_tx` did not).
#[tokio::test]
async fn execute_batch_rejects_deeply_nested_filter_parity_check() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo };

    let mut b = Batch::new();
    b.id(1);
    let mut read_q = Query::from("users").build();
    read_q.r#where = Some(deeply_nested_not_filter(100));
    b.query("r", read_q);
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("execute_batch must reject the same over-deep filter");
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected a QueryError naming the nesting-depth violation, got {:?}",
        err
    );
}
