use super::support::{
    create, create_as, invoke, invoke_as, native_exists, open_acl_ancestors,
    restrict_table_to_owner, restricted, seed_text, setup,
};
use shamir_engine::function::Security;
use shamir_types::types::value::QueryValue;

const ORDERED_FLOW: &str = r#"@entry procedure ordered() -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: 1 });
    const row: { id: i64, value: string } = {
        id: 20,
        value: query_ref_field(b0, "source", 0, "value")
    };
    let b1 = batch_insert(b0, "dependent", "sink_rows", row);
    let b2 = batch_read(b1, "observer", "sink_rows", { id: 20 });
    let response = db_execute(request(b2));
    let inserted: [{ id: i64, value: string }]? = response_inserted(response, "dependent");
    let rows: [{ id: i64, value: string }]? = response_rows(response, "observer");
    if (is_null(inserted)) { return -2; }
    if (is_null(rows)) { return -1; }
    return to_i64(len(unwrap(rows)));
}"#;

const ACL_FLOW: &str = r#"@entry procedure copy() -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: 1 });
    const row: { id: i64, value: string } = {
        id: 30,
        value: query_ref_field(b0, "source", 0, "value")
    };
    let b1 = batch_insert(b0, "dependent", "sink_rows", row);
    let b2 = batch_read(b1, "observer", "sink_rows", { id: 30 });
    let response = db_execute(request(b2));
    let inserted: [{ id: i64, value: string }]? = response_inserted(response, "dependent");
    let rows: [{ id: i64, value: string }]? = response_rows(response, "observer");
    if (is_null(inserted)) { return -2; }
    if (is_null(rows)) { return -1; }
    return to_i64(len(unwrap(rows)));
}"#;

#[tokio::test]
async fn independent_read_appended_after_dependent_insert_runs_before_it() {
    let db = setup(&["source_rows", "sink_rows"]).await;
    seed_text(&db, "source_rows", 1, "ordered-value").await;
    create(&db, "ordered_scalar_refs", ORDERED_FLOW).await;

    assert_eq!(
        invoke(&db, "ordered_scalar_refs")
            .await
            .expect("ordered reference batch"),
        QueryValue::Int(0)
    );
    assert!(
        native_exists(&db, "sink_rows", 20).await,
        "dependent insert must persist after observer ran"
    );
}

#[tokio::test]
async fn reference_reads_obey_invoker_acl_and_definer_owner_rights() {
    let db = setup(&["source_rows", "sink_rows"]).await;
    open_acl_ancestors(&db).await;
    restrict_table_to_owner(&db, "source_rows").await;
    // The Definer run inserts as the owner, so the sink must be the owner's too.
    restrict_table_to_owner(&db, "sink_rows").await;
    seed_text(&db, "source_rows", 1, "acl-value").await;
    create_as(&db, "scalar_refs_invoker", ACL_FLOW, Security::Invoker).await;
    create_as(&db, "scalar_refs_definer", ACL_FLOW, Security::Definer).await;

    let invocation = invoke_as(&db, "scalar_refs_invoker", restricted()).await;
    super::support::assert_gateway_read_denial(invocation);
    assert!(
        !native_exists(&db, "sink_rows", 30).await,
        "denied Invoker read must not insert dependent row"
    );

    assert_eq!(
        invoke_as(&db, "scalar_refs_definer", restricted())
            .await
            .expect("Definer uses owner READ rights"),
        QueryValue::Int(0)
    );
    assert_eq!(
        super::support::native_text(&db, "sink_rows", 30)
            .await
            .as_deref(),
        Some("acl-value")
    );
}
