//! Engine behavior for the per-entry `abort_on_skip` transaction opt-in.

use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::filter;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::QueryRecord;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use crate::query::batch::{
    execute_batch_unchecked as execute_batch, execute_in_open_tx_unchecked as execute_in_open_tx,
    open_interactive_tx, BatchError,
};

use super::super::common::{setup_resolver, TestResolver, TxTestResolver};

const SCOPE_MESSAGE: &str = "abort_on_skip requires a single-call transactional batch";

fn false_when() -> Filter {
    Filter::IsNotNull {
        field: vec!["never_present".to_string()],
    }
}

fn true_when() -> Filter {
    Filter::IsNull {
        field: vec!["never_present".to_string()],
    }
}

fn assert_skip_abort(response: BatchResponse) {
    let info = response
        .transaction
        .as_ref()
        .expect("transactional response must include its outcome");
    assert_eq!(info.status, "aborted");
    assert_eq!(info.reason.as_deref(), Some("skip_aborted"));
    assert!(
        response.results.is_empty(),
        "aborted transactional execution must not expose partial results"
    );
}

fn assert_validation(err: BatchError) {
    match err {
        BatchError::QueryError { code, message, .. } => {
            assert_eq!(code.as_deref(), Some("validation"));
            assert_eq!(message, SCOPE_MESSAGE);
        }
        other => panic!("expected the abort_on_skip scope validation error, got {other:?}"),
    }
}

async fn read_names(resolver: &TestResolver) -> Vec<String> {
    let mut batch = Batch::new();
    batch.query("rows", Query::from("users"));
    let response = execute_batch(&batch.build(), resolver, None, None, Actor::System, "test")
        .await
        .expect("observer read should execute");
    let mut names = response.results["rows"]
        .records
        .iter()
        .filter_map(|record| record.get_value_str("name").map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[tokio::test]
async fn false_when_aborts_transaction_with_stable_token_and_rolls_back_visibility() {
    let resolver = setup_resolver().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.transactional();
    let first = batch.insert(
        "first",
        write::insert("users").row(doc().set("name", "must_roll_back").unwrap()),
    );
    let guarded = batch.insert(
        "guarded",
        write::insert("users").row(doc().set("name", "must_not_insert").unwrap()),
    );
    batch.after(&guarded, &first);
    batch.when(&guarded, false_when());
    batch.abort_on_skip(&guarded);

    let response = execute_batch(&batch.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("an execution abort is returned as a transaction outcome");
    assert_skip_abort(response);
    assert!(
        read_names(&resolver).await.is_empty(),
        "a fresh observer must see neither the staged insert nor the skipped insert"
    );
}

#[tokio::test]
async fn cascaded_read_dependency_skip_aborts_transaction() {
    let resolver = setup_resolver().await;
    let mut batch = Batch::new();
    batch.id(2);
    batch.transactional();
    let seed = batch.insert(
        "seed",
        write::insert("users").row(doc().set("name", "cascade_seed").unwrap()),
    );
    let source = batch.query("source", Query::from("users"));
    batch.after(&source, &seed);
    let dependent = batch.query(
        "dependent",
        Query::from("users").where_eq("name", source.first().field("name")),
    );
    batch.when(&source, false_when());
    batch.abort_on_skip(&dependent);

    let response = execute_batch(&batch.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("a dependency skip abort should be reported through transaction info");
    assert_skip_abort(response);
    assert!(
        read_names(&resolver).await.is_empty(),
        "the staged seed write must roll back when the data-flow dependent cascades to skip"
    );
}

#[tokio::test]
async fn nested_batch_and_foreach_abort_preserve_the_exact_token() {
    let resolver = setup_resolver().await;

    let mut inner = Batch::new();
    let inner_read = inner.query("inner_skip", Query::from("users"));
    inner.when(&inner_read, false_when());
    inner.abort_on_skip(&inner_read);
    let mut outer = Batch::new();
    outer.id(3);
    outer.transactional();
    let seed = outer.insert(
        "seed",
        write::insert("users").row(doc().set("name", "nested_batch_seed").unwrap()),
    );
    let nested = outer.sub_batch_no_bind("nested", inner.build());
    outer.after(&nested, &seed);
    let response = execute_batch(&outer.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("nested skip abort should be represented in the outer response");
    assert_skip_abort(response);
    assert!(read_names(&resolver).await.is_empty());

    let mut body = Batch::new();
    let body_read = body.query("loop_skip", Query::from("users"));
    body.when(&body_read, false_when());
    body.abort_on_skip(&body_read);
    let mut outer = Batch::new();
    outer.id(4);
    outer.transactional();
    let seed = outer.insert(
        "seed",
        write::insert("users").row(doc().set("name", "foreach_seed").unwrap()),
    );
    let loop_entry = outer.for_each(
        "loop",
        FilterValue::Array(vec![FilterValue::Int(1)]),
        "item",
        body.build(),
    );
    outer.after(&loop_entry, &seed);
    let response = execute_batch(&outer.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("ForEach skip abort should be represented in the outer response");
    assert_skip_abort(response);
    assert!(read_names(&resolver).await.is_empty());
}

#[tokio::test]
async fn non_transactional_root_and_nested_flags_validate_before_any_insert() {
    let resolver = setup_resolver().await;
    let mut root = Batch::new();
    let root_insert = root.insert(
        "insert",
        write::insert("users").row(doc().set("name", "root_must_not_run").unwrap()),
    );
    root.when(&root_insert, true_when());
    root.abort_on_skip(&root_insert);
    assert_validation(
        execute_batch(&root.build(), &resolver, None, None, Actor::System, "test")
            .await
            .expect_err("a flag under a non-transactional root must be refused"),
    );
    assert!(read_names(&resolver).await.is_empty());

    let mut inner = Batch::new();
    let inner_insert = inner.insert(
        "inner_insert",
        write::insert("users").row(doc().set("name", "nested_must_not_run").unwrap()),
    );
    inner.when(&inner_insert, true_when());
    inner.abort_on_skip(&inner_insert);
    let mut root = Batch::new();
    let outer_insert = root.insert(
        "insert",
        write::insert("users").row(doc().set("name", "outer_must_not_run").unwrap()),
    );
    let nested = root.sub_batch_no_bind("nested", inner.build());
    root.after(&nested, &outer_insert);
    assert_validation(
        execute_batch(&root.build(), &resolver, None, None, Actor::System, "test")
            .await
            .expect_err("a nested flag must be refused before the root starts"),
    );
    assert!(read_names(&resolver).await.is_empty());
}

#[tokio::test]
async fn interactive_transaction_refuses_abort_on_skip_before_staging_writes() {
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

    let mut batch = Batch::new();
    let first = batch.insert(
        "first",
        write::insert("users").row(doc().set("name", "interactive_must_not_stage").unwrap()),
    );
    let guarded = batch.insert(
        "guarded",
        write::insert("users").row(doc().set("name", "interactive_skip").unwrap()),
    );
    batch.after(&guarded, &first);
    batch.when(&guarded, false_when());
    batch.abort_on_skip(&guarded);
    let result = execute_in_open_tx(
        &batch.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;
    assert_validation(result.expect_err("interactive TxExecute must reject the flag"));

    let mut read = Batch::new();
    read.query("rows", Query::from("users"));
    let response = execute_in_open_tx(
        &read.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .expect("the open transaction remains usable after validation refusal");
    assert!(response.results["rows"].records.is_empty());
    drop(tx);
    drop(guard);
}

async fn run_true_when_case(flag_abort: bool) -> BatchResponse {
    let resolver = setup_resolver().await;
    let mut batch = Batch::new();
    batch.id(if flag_abort { 5 } else { 6 });
    batch.transactional();
    let inserted = batch.insert(
        "inserted",
        write::insert("users").row(doc().set("name", "committed_row").unwrap()),
    );
    batch.when(&inserted, true_when());
    if flag_abort {
        batch.abort_on_skip(&inserted);
    }
    let read = batch.query("visible", Query::from("users"));
    batch.after(&read, &inserted);
    let mut response = execute_batch(&batch.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("true when condition must execute and commit");
    assert!(!response.results["inserted"].skipped);
    assert_eq!(
        response
            .transaction
            .as_ref()
            .map(|info| info.status.as_str()),
        Some("committed")
    );

    // Compare the complete deterministic response, excluding per-run timings,
    // generated record/request/transaction IDs, MVCC versions, and receipts.
    response.id = QueryValue::Null;
    response.execution_time_us = 0;
    if let Some(info) = &mut response.transaction {
        info.tx_id = 0;
        info.snapshot_version = None;
        info.commit_version = None;
    }
    for result in response.results.values_mut() {
        result.stats = None;
        result.versions = None;
        if let Some(receipt) = &mut result.receipt {
            receipt.tx_id = 0;
            receipt.snapshot_version = None;
            receipt.commit_version = None;
        }
        for record in &mut result.records {
            match record {
                QueryRecord::Direct(QueryValue::Map(fields)) => {
                    fields.shift_remove("_id");
                }
                QueryRecord::Inserted(inserted) => {
                    inserted.id = None;
                    if let QueryValue::Map(fields) = &mut inserted.fields {
                        fields.shift_remove("_id");
                    }
                }
                QueryRecord::Direct(_) | QueryRecord::IdBytes(_) => {}
            }
        }
    }
    response
}

#[tokio::test]
async fn true_condition_commits_with_unflagged_result_parity() {
    let flagged = run_true_when_case(true).await;
    let unflagged = run_true_when_case(false).await;
    assert_eq!(flagged, unflagged);
    assert_eq!(flagged.results["inserted"].records.len(), 1);
    assert_eq!(
        flagged.results["inserted"].records[0].get_value_str("name"),
        Some("committed_row")
    );
    assert_eq!(flagged.results["visible"].records.len(), 1);
    assert_eq!(
        flagged.results["visible"].records[0].get_value_str("name"),
        Some("committed_row")
    );
}

#[tokio::test]
async fn unflagged_skip_still_commits_other_transactional_writes() {
    let resolver = setup_resolver().await;
    let mut batch = Batch::new();
    batch.id(7);
    batch.transactional();
    let first = batch.insert(
        "first",
        write::insert("users").row(doc().set("name", "still_committed").unwrap()),
    );
    let skipped = batch.insert(
        "skipped",
        write::insert("users").row(doc().set("name", "not_inserted").unwrap()),
    );
    batch.after(&skipped, &first);
    batch.when(&skipped, false_when());
    let response = execute_batch(&batch.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("an ordinary conditional skip is not a transaction failure");
    assert_eq!(
        response
            .transaction
            .as_ref()
            .map(|info| info.status.as_str()),
        Some("committed")
    );
    assert!(response.results["skipped"].skipped);
    assert_eq!(read_names(&resolver).await, vec!["still_committed"]);
}

#[tokio::test]
async fn pessimistic_abort_releases_locks_for_a_followup_transaction() {
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        crate::repo::repo_types::BoxRepoFactory::in_memory(),
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        write::insert("users").row(
            doc()
                .set("key", 1_i64)
                .unwrap()
                .set("value", 0_i64)
                .unwrap(),
        ),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("seed insert should commit");

    let mut aborting = Batch::new();
    aborting.id(8);
    aborting.transactional();
    aborting.isolation(Isolation::Pessimistic);
    let update = aborting.update(
        "locked_update",
        write::update("users")
            .where_(filter::eq("key", 1_i64))
            .set(doc().set("value", 1_i64).unwrap())
            .build()
            .unwrap(),
    );
    let skipped = aborting.query("skip", Query::from("users"));
    aborting.after(&skipped, &update);
    aborting.when(&skipped, false_when());
    aborting.abort_on_skip(&skipped);
    let response = execute_batch(
        &aborting.build(),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect("skip abort should return a transaction outcome");
    assert_skip_abort(response);

    let mut followup = Batch::new();
    followup.id(9);
    followup.transactional();
    followup.isolation(Isolation::Pessimistic);
    followup.update(
        "same_key",
        write::update("users")
            .where_(filter::eq("key", 1_i64))
            .set(doc().set("value", 2_i64).unwrap())
            .build()
            .unwrap(),
    );
    let response = execute_batch(
        &followup.build(),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect("a new pessimistic transaction must reacquire the aborted transaction's row lock");
    assert_eq!(
        response
            .transaction
            .as_ref()
            .map(|info| info.status.as_str()),
        Some("committed")
    );

    let mut read = Batch::new();
    read.query("row", Query::from("users").where_eq("key", 1_i64));
    let response = execute_batch(&read.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("observer should read the committed follow-up update");
    assert_eq!(response.results["row"].records.len(), 1);
    assert_eq!(
        response.results["row"].records[0].get_value_i64("value"),
        Some(2)
    );
}
