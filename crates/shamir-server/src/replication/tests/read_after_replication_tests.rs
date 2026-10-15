use std::sync::Arc;

use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::apply_replicated::ApplyRejectReason;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::create_index;
use shamir_query_builder::filter::eq;
use shamir_query_builder::query::Query;
use shamir_query_builder::response::BatchResponseExt;
use shamir_query_builder::select::count_all;
use shamir_query_builder::write::{delete, doc, insert, update};
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;

#[derive(Clone, Copy)]
enum Write {
    InsertOne,
    InsertMulti,
    Update,
    Delete,
    TwoTables,
}

#[derive(Clone, Copy)]
enum Read {
    PointGet,
    FullScan,
    Filter,
    Projection,
    Count,
}

fn owner() -> Actor {
    Actor::User(principal64_from_username("alice"))
}

async fn build_db(tables: &[&str], indexed: bool) -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("init memory database");
    db.create_db_as("app", owner()).await;
    let mut config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    for table in tables {
        let table = TableConfig::new(*table);
        config = config.add_table(if indexed { table.with_indexes() } else { table });
    }
    db.add_repo_as("app", config, owner())
        .await
        .expect("add repo");
    db
}

fn row(label: &str, value: i64) -> QueryValue {
    doc().set("label", label).set("v", value).build()
}

async fn execute(db: &ShamirDb, batch: &Batch) -> BatchResponse {
    let response = db
        .execute_as(owner(), "app", &batch.build())
        .await
        .expect("execute batch");
    assert!(!response.results.contains_key("__error"), "{response:?}");
    if let Some(transaction) = response.transaction() {
        assert_eq!(transaction.status, "committed", "{response:?}");
    }
    response
}

async fn insert_rows(db: &ShamirDb, table: &str, rows: Vec<QueryValue>) -> Vec<RecordId> {
    let expected_len = rows.len();
    let mut batch = Batch::new();
    batch.transactional();
    batch.insert("write", insert(table).rows(rows));
    let response = execute(db, &batch).await;
    let result = response.result("write").expect("insert result");
    assert_eq!(result.records.len(), expected_len);
    result
        .records
        .iter()
        .map(|record| match record {
            QueryRecord::Inserted(record) => record.id.expect("inserted record id"),
            other => panic!("expected inserted record, got {other:?}"),
        })
        .collect()
}

async fn journal_version(leader: &ShamirDb, events: usize) -> u64 {
    let target = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");
    for _ in 0..10_000 {
        let journal = leader
            .read_changelog_from_journal("app", "main", 0, 1000)
            .await
            .expect("leader journal");
        assert_eq!(journal.gap_at, None);
        if journal.events.len() == events
            && journal.events.last().map(|event| event.commit_version) == Some(target)
        {
            return target;
        }
        tokio::task::yield_now().await;
    }
    panic!("leader journal did not contain {events} events through version {target}");
}

async fn pull_once(leader: &ShamirDb, follower: &ShamirDb) {
    pull_once_result(leader, follower)
        .await
        .expect("bounded follower loop");
}

async fn pull_once_result(leader: &ShamirDb, follower: &ShamirDb) -> Result<(), ReplError> {
    let config = FollowerLoopConfig::new("matrix-follower", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(InProcessReplSource::new(leader.clone())),
        config,
        CancellationToken::new(),
    )
    .await
}

async fn bookmark(db: &ShamirDb) -> u64 {
    db.get_db("app")
        .expect("database")
        .get_repo("main")
        .expect("repo")
        .replication_bookmark()
        .await
        .expect("bookmark")
}

async fn read_query(db: &ShamirDb, query: Query) -> QueryResult {
    let mut batch = Batch::new();
    batch.query("read", query);
    let response = execute(db, &batch).await;
    let result = response.result("read").expect("read result");
    assert!(!result.skipped, "{result:?}");
    assert!(result.corrupt_records.is_empty(), "{result:?}");
    result.clone()
}

fn assert_rows(result: &QueryResult, expected: &[QueryValue]) {
    let mut actual: Vec<_> = result
        .records
        .iter()
        .map(|record| record.as_value().into_owned())
        .collect();
    assert_eq!(actual.len(), expected.len(), "{result:?}");
    for expected in expected {
        let position = actual
            .iter()
            .position(|actual| actual == expected)
            .unwrap_or_else(|| panic!("missing {expected:?} in {actual:?}"));
        actual.remove(position);
    }
    assert!(actual.is_empty());
}

async fn assert_read(
    follower: &ShamirDb,
    table_name: &str,
    read: Read,
    ids: &[RecordId],
    expected: &[QueryValue],
    deleted: Option<RecordId>,
) {
    match read {
        Read::PointGet => {
            let repo = follower
                .get_db("app")
                .expect("database")
                .get_repo("main")
                .expect("repo");
            let table = repo.get_table(table_name).await.expect("table");
            assert_eq!(ids.len(), expected.len());
            for (id, expected) in ids.iter().zip(expected) {
                let value = table.get(*id).await.expect("replicated point get");
                let interner = table.interner_for_test().get().await.expect("follower interner");
                assert_eq!(
                    inner_value_to_query_value(&value, interner).expect("decode follower names"),
                    *expected,
                );
            }
            if let Some(id) = deleted {
                assert_eq!(table.get_bytes(id).await.expect("deleted point get"), None);
            }
            assert_eq!(
                table
                    .get_bytes(RecordId([0; 16]))
                    .await
                    .expect("missing point get"),
                None,
            );
        }
        Read::FullScan => {
            let result = read_query(follower, Query::with_repo("main", table_name)).await;
            assert_rows(&result, expected);
        }
        Read::Filter => {
            let result = read_query(
                follower,
                Query::with_repo("main", table_name).where_eq("v", 20_i64),
            )
            .await;
            let expected: Vec<_> = expected
                .iter()
                .filter(|row| row.get("v") == Some(&QueryValue::Int(20)))
                .cloned()
                .collect();
            assert_rows(&result, &expected);
            let missing = read_query(
                follower,
                Query::with_repo("main", table_name).where_eq("v", -1_i64),
            )
            .await;
            assert_rows(&missing, &[]);
        }
        Read::Projection => {
            let result =
                read_query(follower, Query::with_repo("main", table_name).select(["v"])).await;
            let expected: Vec<_> = expected
                .iter()
                .map(|row| {
                    doc()
                        .set_value("v", row.get("v").expect("expected value").clone())
                        .build()
                })
                .collect();
            assert_rows(&result, &expected);
        }
        Read::Count => {
            let result = read_query(
                follower,
                Query::with_repo("main", table_name).select([count_all("n")]),
            )
            .await;
            let count = i64::try_from(expected.len()).expect("small fixture");
            assert_rows(&result, &[doc().set("n", count).build()]);
            assert_eq!(
                result
                    .stats
                    .as_ref()
                    .expect("count stats")
                    .index_used
                    .as_deref(),
                Some("__record_counter__"),
            );
        }
    }
}

async fn matrix(write: Write, read: Read) {
    let leader = build_db(&["items", "other"], false).await;
    let follower = build_db(&["items", "other"], false).await;
    let (ids, expected, deleted, events) = match write {
        Write::InsertOne => {
            let expected = vec![row("one", 20)];
            (
                insert_rows(&leader, "items", expected.clone()).await,
                expected,
                None,
                1,
            )
        }
        Write::InsertMulti => {
            let expected = vec![row("one", 10), row("two", 20), row("three", 30)];
            (
                insert_rows(&leader, "items", expected.clone()).await,
                expected,
                None,
                1,
            )
        }
        Write::Update => {
            let ids = insert_rows(&leader, "items", vec![row("one", 10), row("two", 30)]).await;
            let mut batch = Batch::new();
            batch.transactional();
            batch
                .try_update(
                    "write",
                    update("items")
                        .where_(eq("label", "one"))
                        .set(doc().set("v", 20)),
                )
                .expect("build update");
            execute(&leader, &batch).await;
            (ids, vec![row("one", 20), row("two", 30)], None, 2)
        }
        Write::Delete => {
            let ids = insert_rows(&leader, "items", vec![row("one", 10), row("two", 20)]).await;
            let mut batch = Batch::new();
            batch.transactional();
            batch
                .try_delete("write", delete("items").where_(eq("label", "one")))
                .expect("build delete");
            execute(&leader, &batch).await;
            (vec![ids[1]], vec![row("two", 20)], Some(ids[0]), 2)
        }
        Write::TwoTables => {
            let mut batch = Batch::new();
            batch.transactional();
            batch.insert("items", insert("items").rows([row("one", 20)]));
            batch.insert("other", insert("other").rows([row("other", 20)]));
            let response = execute(&leader, &batch).await;
            let journal = journal_version(&leader, 1).await;
            assert!(journal > 0);
            let ids = response
                .result("items")
                .expect("items insert")
                .records
                .iter()
                .map(|record| match record {
                    QueryRecord::Inserted(record) => record.id.expect("id"),
                    other => panic!("expected inserted record, got {other:?}"),
                })
                .collect();
            (ids, vec![row("one", 20)], None, 1)
        }
    };
    assert_read(&leader, "items", Read::PointGet, &ids, &expected, deleted).await;
    assert_rows(
        &read_query(&leader, Query::with_repo("main", "items")).await,
        &expected,
    );
    let target = journal_version(&leader, events).await;
    pull_once(&leader, &follower).await;
    assert_eq!(bookmark(&follower).await, target);
    assert_read(&follower, "items", read, &ids, &expected, deleted).await;
    if matches!(read, Read::Filter) && matches!(write, Write::Update | Write::Delete) {
        let old_value = read_query(
            &follower,
            Query::with_repo("main", "items").where_eq("v", 10_i64),
        )
        .await;
        assert_rows(&old_value, &[]);
    }
    if matches!(write, Write::TwoTables) {
        let journal = leader
            .read_changelog_from_journal("app", "main", 0, 1000)
            .await
            .expect("journal");
        assert_eq!(journal.events.len(), 1);
        assert_eq!(journal.events[0].changes.len(), 2);
        let other_ids: Vec<_> = journal.events[0]
            .changes
            .iter()
            .filter(|change| change.table == "other")
            .map(|change| RecordId::try_from_bytes(&change.key).expect("record key"))
            .collect();
        assert_eq!(other_ids.len(), 1);
        assert_rows(
            &read_query(&leader, Query::with_repo("main", "other")).await,
            &[row("other", 20)],
        );
        assert_read(
            &follower,
            "other",
            read,
            &other_ids,
            &[row("other", 20)],
            None,
        )
        .await;
    }
}

#[tokio::test]
async fn matrix_insert_one_point_get() {
    matrix(Write::InsertOne, Read::PointGet).await;
}

#[tokio::test]
async fn matrix_insert_one_full_scan() {
    matrix(Write::InsertOne, Read::FullScan).await;
}

#[tokio::test]
async fn matrix_insert_one_filter() {
    matrix(Write::InsertOne, Read::Filter).await;
}

#[tokio::test]
async fn matrix_insert_one_projection() {
    matrix(Write::InsertOne, Read::Projection).await;
}

#[tokio::test]
async fn matrix_insert_one_count() {
    matrix(Write::InsertOne, Read::Count).await;
}

#[tokio::test]
async fn matrix_insert_multi_point_get() {
    matrix(Write::InsertMulti, Read::PointGet).await;
}

#[tokio::test]
async fn matrix_insert_multi_full_scan() {
    matrix(Write::InsertMulti, Read::FullScan).await;
}

#[tokio::test]
async fn matrix_insert_multi_filter() {
    matrix(Write::InsertMulti, Read::Filter).await;
}

#[tokio::test]
async fn matrix_insert_multi_projection() {
    matrix(Write::InsertMulti, Read::Projection).await;
}

#[tokio::test]
async fn matrix_insert_multi_count() {
    matrix(Write::InsertMulti, Read::Count).await;
}

#[tokio::test]
async fn matrix_update_point_get() {
    matrix(Write::Update, Read::PointGet).await;
}

#[tokio::test]
async fn matrix_update_full_scan() {
    matrix(Write::Update, Read::FullScan).await;
}

#[tokio::test]
async fn matrix_update_filter() {
    matrix(Write::Update, Read::Filter).await;
}

#[tokio::test]
async fn matrix_update_projection() {
    matrix(Write::Update, Read::Projection).await;
}

#[tokio::test]
async fn matrix_update_count() {
    matrix(Write::Update, Read::Count).await;
}

#[tokio::test]
async fn matrix_delete_point_get() {
    matrix(Write::Delete, Read::PointGet).await;
}

#[tokio::test]
async fn matrix_delete_full_scan() {
    matrix(Write::Delete, Read::FullScan).await;
}

#[tokio::test]
async fn matrix_delete_filter() {
    matrix(Write::Delete, Read::Filter).await;
}

#[tokio::test]
async fn matrix_delete_projection() {
    matrix(Write::Delete, Read::Projection).await;
}

#[tokio::test]
async fn matrix_delete_count() {
    matrix(Write::Delete, Read::Count).await;
}

#[tokio::test]
async fn matrix_two_tables_point_get() {
    matrix(Write::TwoTables, Read::PointGet).await;
}

#[tokio::test]
async fn matrix_two_tables_full_scan() {
    matrix(Write::TwoTables, Read::FullScan).await;
}

#[tokio::test]
async fn matrix_two_tables_filter() {
    matrix(Write::TwoTables, Read::Filter).await;
}

#[tokio::test]
async fn matrix_two_tables_projection() {
    matrix(Write::TwoTables, Read::Projection).await;
}

#[tokio::test]
async fn matrix_two_tables_count() {
    matrix(Write::TwoTables, Read::Count).await;
}

#[tokio::test(start_paused = true)]
async fn matrix_unknown_table_bookmark() {
    let leader = build_db(&["items", "unknown"], false).await;
    let follower = build_db(&["items"], false).await;
    insert_rows(&leader, "unknown", vec![row("unknown", 20)]).await;
    let rejected_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("unknown table commit version");
    insert_rows(&leader, "items", vec![row("later", 20)]).await;
    journal_version(&leader, 2).await;
    let error = pull_once_result(&leader, &follower)
        .await
        .expect_err("unknown table must stop replication");
    assert!(
        matches!(
            error,
            ReplError::ApplyRejected {
                leader_version,
                reason: ApplyRejectReason::UnknownTable,
            } if leader_version == rejected_version
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(bookmark(&follower).await, 0);
    let repo = follower
        .get_db("app")
        .expect("database")
        .get_repo("main")
        .expect("repo");
    assert!(!repo.has_table("unknown"));
    let result = read_query(&follower, Query::with_repo("main", "items")).await;
    assert_rows(&result, &[]);
    let journal = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert!(journal.events.is_empty());
}

#[tokio::test(start_paused = true)]
async fn matrix_secondary_index_lookup() {
    let leader = build_db(&["items"], true).await;
    let follower = build_db(&["items"], true).await;
    let mut name_ids = Vec::new();
    for db in [&leader, &follower] {
        let repo = db
            .get_db("app")
            .expect("database")
            .get_repo("main")
            .expect("repo");
        let table = repo.get_table("items").await.expect("table");
        let interner = table.interner_for_test().get().await.expect("interner");
        for field in ["label", "v", "by_v"] {
            interner
                .touch_ind(field)
                .expect("intern index fixture field");
        }
        let mut batch = Batch::new();
        batch.create_index(
            "index",
            create_index("by_v", "items").repo("main").field("v"),
        );
        execute(db, &batch).await;
        let interner = table.interner_for_test().get().await.expect("post-DDL interner");
        let pairs: Vec<_> = ["label", "v", "by_v"]
            .into_iter()
            .map(|name| {
                let id = interner.get_ind(name).expect("post-DDL name id").id();
                (name, id)
            })
            .collect();
        name_ids.push(pairs);
    }
    assert_eq!(name_ids.len(), 2);
    assert_eq!(name_ids[0], name_ids[1], "post-DDL dictionaries must align");
    let expected = row("indexed", 20);
    let ids = insert_rows(&leader, "items", vec![expected.clone()]).await;
    let target = journal_version(&leader, 1).await;
    let error = pull_once_result(&leader, &follower)
        .await
        .expect_err("indexed table must stop replication");
    assert!(
        matches!(
            error,
            ReplError::ApplyRejected {
                leader_version,
                reason: ApplyRejectReason::IndexedTable { .. },
            } if leader_version == target
        ),
        "unexpected error: {error:?}"
    );
    let applied = bookmark(&follower).await;
    assert_eq!(applied, 0);
    let repo = follower
        .get_db("app")
        .expect("database")
        .get_repo("main")
        .expect("repo");
    let table = repo.get_table("items").await.expect("table");
    assert_eq!(table.get_bytes(ids[0]).await.expect("refused row"), None);
    assert_rows(
        &read_query(&follower, Query::with_repo("main", "items")).await,
        &[],
    );
}

#[tokio::test]
async fn matrix_preinterned_field_leader_names() {
    let leader = build_db(&["items"], false).await;
    let follower = build_db(&["items"], false).await;
    let repo = follower
        .get_db("app")
        .expect("database")
        .get_repo("main")
        .expect("repo");
    let table = repo.get_table("items").await.expect("table");
    let interner = table.interner_for_test().get().await.expect("interner");
    let collision = interner
        .touch_ind("follower_only")
        .expect("preintern field")
        .key()
        .id();
    assert_eq!(collision, 1);
    let expected = row("leader", 20);
    let ids = insert_rows(&leader, "items", vec![expected.clone()]).await;
    let leader_repo = leader
        .get_db("app")
        .expect("database")
        .get_repo("main")
        .expect("repo");
    let leader_table = leader_repo.get_table("items").await.expect("table");
    let leader_interner = leader_table
        .interner_for_test()
        .get()
        .await
        .expect("leader interner");
    assert!(
        ["label", "v"].iter().any(|field| {
            let id = leader_interner.get_ind(field).expect("leader field id");
            id.id() == collision
        }),
        "preinterned follower field must collide with a leader field",
    );
    let target = journal_version(&leader, 1).await;
    pull_once(&leader, &follower).await;
    assert_eq!(bookmark(&follower).await, target);
    assert_read(
        &follower,
        "items",
        Read::PointGet,
        &ids,
        std::slice::from_ref(&expected),
        None,
    )
    .await;
    let result = read_query(&follower, Query::with_repo("main", "items")).await;
    assert_rows(&result, &[expected]);
    assert_eq!(result.records[0].as_value().get("follower_only"), None);
}
