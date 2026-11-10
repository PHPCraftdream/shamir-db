//! `lang = "shamir"` procedures against a real database: the direct
//! `db_insert` / `db_get` / `db_query` intrinsics and the typed builder
//! (`batch_insert`, `batch_read`, `db_execute`, `response_rows`,
//! `response_inserted`) run through the real gateway, so the server itself
//! accepts the guest-encoded `BatchRequest` DTO.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Params};
use shamir_engine::query::filter::FilterContext;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{delete, doc, insert, update};
use shamir_types::types::value::QueryValue;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");
    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["people"]),
    );
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();
    shamir
}

async fn create(shamir: &ShamirDb, name: &str, source: &str) {
    shamir
        .create_function_with_opts(
            name,
            FunctionSource::Shamir(source),
            CreateFunctionOptions::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("create {name}: {error}"));
}

async fn invoke(shamir: &ShamirDb, name: &str) -> QueryValue {
    shamir
        .invoke_function_in_db("testdb", "main", name, Params::new())
        .await
        .unwrap_or_else(|error| panic!("invoke {name}: {error}"))
}

#[tokio::test]
async fn direct_intrinsics_round_trip_through_the_real_gateway() {
    let shamir = setup().await;
    create(
        &shamir,
        "roundtrip",
        "@entry procedure roundtrip() -> i64 {\n\
         let stored: { id: i64, name: string } = db_insert(\"people\", { id: 1, name: \"neo\" });\n\
         let found: { id: i64, name: string }? = db_get(\"people\", 1);\n\
         let rows: [{ id: i64 }] = db_query(\"people\", null);\n\
         if (is_null(found)) { return -1; }\n\
         return unwrap(found).id + stored.id + to_i64(len(rows));\n\
         }\n",
    )
    .await;
    // derivation: the inserted row has id 1; `db_get` by key 1 finds it again
    // (id 1) and the table holds one row: 1 + 1 + 1 = 3.
    assert_eq!(invoke(&shamir, "roundtrip").await, QueryValue::Int(3));
}

#[tokio::test]
async fn typed_builder_batches_run_against_the_real_executor() {
    let shamir = setup().await;
    create(
        &shamir,
        "ins",
        "@entry procedure ins() -> i64 {\n\
         let b = batch_insert(batch(), \"ins\", \"people\", { id: 7, name: \"morpheus\" });\n\
         let resp = db_execute(request(b));\n\
         let rows: [{ id: i64, name: string }]? = response_inserted(resp, \"ins\");\n\
         if (is_null(rows)) { return -1; }\n\
         return unwrap(rows)[0].id;\n\
         }\n",
    )
    .await;
    create(
        &shamir,
        "count",
        "@entry procedure count() -> i64 {\n\
         let b = batch_read(batch(), \"all\", \"people\", null);\n\
         let resp = db_execute(request(b));\n\
         let rows: [{ id: i64, name: string }]? = response_rows(resp, \"all\");\n\
         if (is_null(rows)) { return -1; }\n\
         return to_i64(len(unwrap(rows)));\n\
         }\n",
    )
    .await;
    create(
        &shamir,
        "by_name",
        "@entry procedure by_name() -> i64 {\n\
         let b = batch_read(batch(), \"one\", \"people\", { name: \"morpheus\" });\n\
         let resp = db_execute(request(b));\n\
         let rows: [{ id: i64 }]? = response_rows(resp, \"one\");\n\
         if (is_null(rows)) { return -1; }\n\
         return to_i64(len(unwrap(rows)));\n\
         }\n",
    )
    .await;
    // derivation: the insert batch returns the stored row (id 7); afterwards
    // the table holds that single row, found by `all` and by the name filter.
    assert_eq!(invoke(&shamir, "ins").await, QueryValue::Int(7));
    assert_eq!(invoke(&shamir, "count").await, QueryValue::Int(1));
    assert_eq!(invoke(&shamir, "by_name").await, QueryValue::Int(1));
}

async fn assert_people(db: &ShamirDb, expected: &[(i64, &str, &str)]) {
    let mut read = Batch::new();
    read.id("verify_people");
    read.query("rows", Query::from("people"));
    let response = db
        .execute(
            "testdb",
            &read.to_request_via_msgpack().expect("people read request"),
        )
        .await
        .expect("native builder read of persisted people");
    let rows = &response.results["rows"].records;
    assert_eq!(rows.len(), expected.len(), "persisted row count");
    for (id, name, cohort) in expected {
        let row = rows
            .iter()
            .find(|row| row.get_value_i64("id") == Some(*id))
            .unwrap_or_else(|| panic!("native read did not find id={id}"));
        assert_eq!(row.get_value_str("name"), Some(*name), "id={id}");
        assert_eq!(row.get_value_str("cohort"), Some(*cohort), "id={id}");
    }
}

async fn seed_update_delete_rows(db: &ShamirDb) {
    let mut seed = Batch::new();
    seed.id("seed_update_delete");
    seed.insert(
        "seed_people",
        insert("people").rows([
            doc! { "id" => 1, "name" => "old", "cohort" => "selected" },
            doc! { "id" => 2, "name" => "updated", "cohort" => "selected" },
            doc! { "id" => 3, "name" => "native-old", "cohort" => "native_selected" },
            doc! { "id" => 4, "name" => "updated", "cohort" => "native_selected" },
            doc! { "id" => 5, "name" => "engine-old", "cohort" => "engine_selected" },
            doc! { "id" => 6, "name" => "updated", "cohort" => "engine_selected" },
        ]),
    );
    db.execute(
        "testdb",
        &seed.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed update/delete rows");
}

const UPDATE_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_update(batch(), "update", "people", { cohort: "selected" }, { name: "updated" });
    let resp = db_execute(request(b));
    let affected: i64? = response_affected(resp, "update");
    if (is_null(affected)) { return -1; }
    return unwrap(affected);
}"#;

const DELETE_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_delete(batch(), "delete", "people", { cohort: "selected" });
    let resp = db_execute(request(b));
    let affected: i64? = response_affected(resp, "delete");
    if (is_null(affected)) { return -1; }
    return unwrap(affected);
}"#;

async fn engine_update_affected(db: &ShamirDb, cohort: &str) -> u64 {
    let instance = db.get_db("testdb").expect("test database");
    let repo = instance.get_repo("main").expect("main repository");
    let table = db
        .get_table("testdb", "main", "people")
        .await
        .expect("people table");
    let interner_manager = repo.repo_interner().await.expect("repository interner");
    let op = update("people")
        .where_(eq("cohort", cohort))
        .set(doc().set("name", "updated").expect("update document"))
        .build()
        .expect("engine update operation");
    let resolved_refs = shamir_types::types::common::new_map();
    repo.run_implicit_batch_tx(
        shamir_types::access::Actor::System,
        "engine_update",
        move |tx| {
            let table = table.clone();
            let interner_manager = interner_manager.clone();
            let op = op.clone();
            let resolved_refs = resolved_refs.clone();
            Box::pin(async move {
                let interner = interner_manager.get().await?;
                let context = FilterContext::new(interner, &resolved_refs);
                table
                    .execute_update_tx(
                        &op,
                        &context,
                        tx,
                        None,
                        &shamir_types::access::Actor::System,
                    )
                    .await
            })
        },
    )
    .await
    .expect("direct engine update")
    .affected
}

async fn engine_delete_affected(db: &ShamirDb, cohort: &str) -> u64 {
    let instance = db.get_db("testdb").expect("test database");
    let repo = instance.get_repo("main").expect("main repository");
    let table = db
        .get_table("testdb", "main", "people")
        .await
        .expect("people table");
    let interner_manager = repo.repo_interner().await.expect("repository interner");
    let op = delete("people")
        .where_(eq("cohort", cohort))
        .build()
        .expect("engine delete operation");
    let resolved_refs = shamir_types::types::common::new_map();
    repo.run_implicit_batch_tx(
        shamir_types::access::Actor::System,
        "engine_delete",
        move |tx| {
            let table = table.clone();
            let interner_manager = interner_manager.clone();
            let op = op.clone();
            let resolved_refs = resolved_refs.clone();
            Box::pin(async move {
                let interner = interner_manager.get().await?;
                let context = FilterContext::new(interner, &resolved_refs);
                table
                    .execute_delete_tx(
                        &op,
                        &context,
                        tx,
                        None,
                        &shamir_types::access::Actor::System,
                    )
                    .await
            })
        },
    )
    .await
    .expect("direct engine delete")
    .affected
}

#[tokio::test]
async fn update_delete_procedures_report_real_changed_and_removed_rows() {
    let shamir = setup().await;
    seed_update_delete_rows(&shamir).await;
    create(&shamir, "update_people", UPDATE_SOURCE).await;
    create(&shamir, "delete_people", DELETE_SOURCE).await;

    let engine_update_result = engine_update_affected(&shamir, "engine_selected").await;
    assert_eq!(engine_update_result, 1);
    assert_eq!(
        invoke(&shamir, "update_people").await,
        QueryValue::Int(engine_update_result as i64)
    );
    assert_people(
        &shamir,
        &[
            (1, "updated", "selected"),
            (2, "updated", "selected"),
            (3, "native-old", "native_selected"),
            (4, "updated", "native_selected"),
            (5, "updated", "engine_selected"),
            (6, "updated", "engine_selected"),
        ],
    )
    .await;
    let engine_update_repeat_result = engine_update_affected(&shamir, "engine_selected").await;
    assert_eq!(engine_update_repeat_result, 0);
    assert_eq!(
        invoke(&shamir, "update_people").await,
        QueryValue::Int(engine_update_repeat_result as i64)
    );

    let mut native_update = Batch::new();
    native_update.id("native_update_accounting");
    native_update
        .try_update(
            "native_update",
            update("people")
                .where_(eq("cohort", "native_selected"))
                .set(doc().set("name", "updated").expect("update document")),
        )
        .expect("native update builder");
    let native_update_response = shamir
        .execute(
            "testdb",
            &native_update
                .to_request_via_msgpack()
                .expect("native update request"),
        )
        .await
        .expect("native update execution");
    let native_update_result = &native_update_response.results["native_update"];
    assert!(native_update_result.records.is_empty());
    assert_eq!(
        native_update_result
            .stats
            .as_ref()
            .expect("native update statistics")
            .records_scanned,
        engine_update_result,
        "stats.records_scanned must equal direct TableManager WriteResult.affected"
    );
    assert_people(
        &shamir,
        &[
            (1, "updated", "selected"),
            (2, "updated", "selected"),
            (3, "updated", "native_selected"),
            (4, "updated", "native_selected"),
            (5, "updated", "engine_selected"),
            (6, "updated", "engine_selected"),
        ],
    )
    .await;

    let engine_delete_result = engine_delete_affected(&shamir, "engine_selected").await;
    assert_eq!(engine_delete_result, 2);
    assert_eq!(
        invoke(&shamir, "delete_people").await,
        QueryValue::Int(engine_delete_result as i64)
    );
    let engine_delete_repeat_result = engine_delete_affected(&shamir, "engine_selected").await;
    assert_eq!(engine_delete_repeat_result, 0);
    assert_eq!(
        invoke(&shamir, "delete_people").await,
        QueryValue::Int(engine_delete_repeat_result as i64)
    );

    let mut native_delete = Batch::new();
    native_delete.id("native_delete_accounting");
    native_delete
        .try_delete(
            "native_delete",
            delete("people").where_(eq("cohort", "native_selected")),
        )
        .expect("native delete builder");
    let native_delete_response = shamir
        .execute(
            "testdb",
            &native_delete
                .to_request_via_msgpack()
                .expect("native delete request"),
        )
        .await
        .expect("native delete execution");
    let native_delete_result = &native_delete_response.results["native_delete"];
    assert!(native_delete_result.records.is_empty());
    assert_eq!(
        native_delete_result
            .stats
            .as_ref()
            .expect("native delete statistics")
            .records_scanned,
        engine_delete_result,
        "stats.records_scanned must equal direct TableManager WriteResult.affected"
    );
    assert_people(&shamir, &[]).await;
}

#[tokio::test]
async fn independent_queries_run_in_the_order_the_procedure_added_them() {
    let shamir = setup().await;
    create(
        &shamir,
        "ordered",
        "@entry procedure ordered() -> i64 {\n\
         let b = batch_insert(batch(), \"zeta\", \"people\", { id: 3, name: \"trinity\" });\n\
         let b2 = batch_read(b, \"alpha\", \"people\", null);\n\
         let resp = db_execute(request(b2));\n\
         let rows: [{ id: i64, name: string }]? = response_rows(resp, \"alpha\");\n\
         if (is_null(rows)) { return -1; }\n\
         return to_i64(len(unwrap(rows)));\n\
         }\n",
    )
    .await;
    // derivation: the server runs independent queries in the order of the
    // encoded `queries` map; `zeta` (the insert) was added first, so the
    // read `alpha` sees the row. Sorted aliases would run `alpha` first and
    // return 0.
    assert_eq!(invoke(&shamir, "ordered").await, QueryValue::Int(1));
}
