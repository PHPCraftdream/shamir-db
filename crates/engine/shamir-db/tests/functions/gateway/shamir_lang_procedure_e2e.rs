//! `lang = "shamir"` procedures against a real database: the direct
//! `db_insert` / `db_get` / `db_query` intrinsics and the typed builder
//! (`batch_insert`, `batch_read`, `db_execute`, `response_rows`,
//! `response_inserted`) run through the real gateway, so the server itself
//! accepts the guest-encoded `BatchRequest` DTO.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Params};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
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
