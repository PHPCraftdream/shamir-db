use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use shamir_query_builder::filter;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};
use shamir_types::types::common::new_map;

// ============================================================================
// INSERT + UPDATE + DELETE pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_update_delete_pipeline() {
    let (table, repo) = setup_empty_table().await;
    let refs = new_map();

    // 1. Insert
    let insert_op = write::insert("users")
        .row(doc().set("name", "Alice").set("score", 100_i64))
        .row(doc().set("name", "Bob").set("score", 50_i64))
        .build();
    let r = insert_via_tx(&repo, &table, &insert_op, true)
        .await
        .unwrap();
    assert_eq!(r.affected, 2);

    // 2. Update: boost Bob's score
    let update_op = write::update("users")
        .where_(filter::eq("name", "Bob"))
        .set(doc().set("score", 75_i64))
        .returning(UpdateReturnMode::Changed)
        .build()
        .unwrap();
    let r = update_via_tx(&repo, &table, &update_op, &refs)
        .await
        .unwrap();
    assert_eq!(r.affected, 1);
    assert_eq!(
        r.records[0].get_value_owned("score"),
        Some(shamir_types::types::value::QueryValue::Int(75))
    );

    // 3. Delete: remove low scorers
    let delete_op = write::delete("users")
        .where_(filter::lt("score", 80_i64))
        .build()
        .unwrap();
    let r = delete_via_tx(&repo, &table, &delete_op, &refs)
        .await
        .unwrap();
    assert_eq!(r.affected, 1); // Bob(75) deleted

    assert_eq!(table.count().await.unwrap(), 1); // Only Alice remains
}
