//! Interactive transaction overlay-name regression for HAVING and reads.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::{
    execute_in_open_tx_unchecked as execute_in_open_tx, open_interactive_tx,
};

use super::super::common::TxTestResolver;

#[tokio::test]
async fn interactive_overlay_having_then_projection_filter_and_totals() {
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

    let mut having = Batch::new();
    having.query(
        "having_kept",
        Query::from("users")
            .select([
                shamir_query_builder::select::field("overlay_group"),
                shamir_query_builder::select::sum("qty", "sum_value"),
            ])
            .group_by("overlay_group")
            .having(shamir_query_builder::filter::eq("overlay_group", "shared"))
            .build(),
    );
    let response = execute_in_open_tx(
        &having.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    let kept = &response.results["having_kept"].records;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].get_value_str("overlay_group"), Some("shared"));
    assert_eq!(kept[0].get_value_i64("sum_value"), Some(19));

    let mut reads = Batch::new();
    reads.query(
        "projected",
        Query::from("users")
            .select(["name", "qty", "overlay_group"])
            .build(),
    );
    reads.query(
        "filtered",
        Query::from("users")
            .select(["name", "qty"])
            .where_(shamir_query_builder::filter::eq("overlay_group", "shared"))
            .build(),
    );
    reads.query(
        "totals",
        Query::from("users")
            .select([
                shamir_query_builder::select::field("overlay_group"),
                shamir_query_builder::select::count_all("count_value"),
                shamir_query_builder::select::sum("qty", "sum_value"),
            ])
            .group_by("overlay_group")
            .build(),
    );
    let response = execute_in_open_tx(
        &reads.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    let projected = &response.results["projected"].records;
    assert_eq!(projected.len(), 3);
    let mut projected_rows = projected
        .iter()
        .map(|r| {
            (
                r.get_value_str("name").unwrap().to_owned(),
                r.get_value_i64("qty").unwrap(),
                r.get_value_str("overlay_group").unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    projected_rows.sort_unstable();
    assert_eq!(
        projected_rows,
        vec![
            ("overlay-name-12".to_owned(), 12, "shared".to_owned()),
            ("overlay-name-25".to_owned(), 7, "shared".to_owned()),
            ("overlay-name-37".to_owned(), 37, "other".to_owned()),
        ]
    );
    let filtered = &response.results["filtered"].records;
    assert_eq!(filtered.len(), 2);
    let mut filtered_rows = filtered
        .iter()
        .map(|r| {
            (
                r.get_value_str("name").unwrap().to_owned(),
                r.get_value_i64("qty").unwrap(),
            )
        })
        .collect::<Vec<_>>();
    filtered_rows.sort_unstable();
    assert_eq!(
        filtered_rows,
        vec![
            ("overlay-name-12".to_owned(), 12),
            ("overlay-name-25".to_owned(), 7),
        ]
    );
    let mut total_rows = response.results["totals"]
        .records
        .iter()
        .map(|r| {
            (
                r.get_value_str("overlay_group").unwrap().to_owned(),
                r.get_value_i64("count_value").unwrap(),
                r.get_value_i64("sum_value").unwrap(),
            )
        })
        .collect::<Vec<_>>();
    total_rows.sort_unstable();
    assert_eq!(
        total_rows,
        vec![("other".to_owned(), 1, 37), ("shared".to_owned(), 2, 19)]
    );
    drop(tx);
    drop(guard);
}
