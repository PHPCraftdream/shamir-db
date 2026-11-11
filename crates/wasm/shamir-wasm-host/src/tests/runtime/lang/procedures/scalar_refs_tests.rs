use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, Params, ShamirFunction, TierMode, WasmEngine, WasmFunction,
    WasmLimits,
};
use shamir_lang::{compile, CompileOptions, Stage};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::{and, value_eq, value_gte, value_lte};
use shamir_query_builder::query::Query;
use shamir_query_builder::val::{func, qref};
use shamir_query_builder::write::{doc, insert};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

struct RecordingDb {
    requests: Mutex<Vec<Vec<u8>>>,
}

fn canned_response() -> Vec<u8> {
    let mut results = new_map();
    for alias in ["source", "second", "target"] {
        results.insert(alias.to_string(), QueryValue::Map(new_map()));
    }
    let mut root = new_map();
    root.insert("results".to_string(), QueryValue::Map(results));
    QueryValue::Map(root)
        .to_bytes()
        .expect("reference response encodes")
        .to_vec()
}

#[async_trait::async_trait]
impl DbGateway for RecordingDb {
    async fn get(
        &self,
        _: &str,
        _: &str,
        _: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }

    async fn insert(&self, _: &str, _: &str, _: QueryValue) -> Result<QueryValue, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }

    async fn query(
        &self,
        _: &str,
        _: &str,
        _: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
    }

    async fn execute(&self, request: &[u8]) -> Result<Vec<u8>, DbGatewayError> {
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.to_vec());
        Ok(canned_response())
    }
}

const READ_TO_READ: &str = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    let b1 = batch_read(b0, "second", "people", null);
    const filter: { bool_value: bool, bytes_value: bytes, f64_value: f64, i32_value: i32, i64_value: i64, label: string, string_value: string } = {
        bool_value: query_ref_field(b1, "source", 0, "bool_value"),
        bytes_value: query_ref_field(b1, "second", 0, "bytes_value"),
        f64_value: query_ref_field(b1, "source", 0, "f64_value"),
        i32_value: query_ref_field(b1, "second", 0, "i32_value"),
        i64_value: query_ref_field(b1, "source", 0, "i64_value"),
        label: "literal",
        string_value: query_ref_field(b1, "second", 0, "string_value")
    };
    let b2 = batch_read(b1, "target", "people", filter);
    let response = db_execute(request(b2));
    return 1;
}"#;

const READ_TO_INSERT: &str = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    let b1 = batch_read(b0, "second", "people", null);
    const row: { bool_value: bool, bytes_value: bytes, f64_value: f64, i32_value: i32, i64_value: i64, label: string, string_value: string } = {
        bool_value: query_ref_field(b1, "source", 0, "bool_value"),
        bytes_value: query_ref_field(b1, "second", 0, "bytes_value"),
        f64_value: query_ref_field(b1, "source", 0, "f64_value"),
        i32_value: query_ref_field(b1, "second", 0, "i32_value"),
        i64_value: query_ref_field(b1, "source", 0, "i64_value"),
        label: "literal",
        string_value: query_ref_field(b1, "second", 0, "string_value")
    };
    let b2 = batch_insert(b1, "target", "sink", row);
    let response = db_execute(request(b2));
    return 1;
}"#;

fn options() -> CompileOptions {
    CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    }
}

async fn run_reference_source(source: &str) -> Arc<RecordingDb> {
    let module = compile(source, &options()).expect("production reference source compiles");
    let engine = Arc::new(WasmEngine::new().expect("engine constructed"));
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        &module.wasm,
        WasmLimits::default(),
        TierMode::Auto,
        AdmissionPolicy::Load,
    )
    .await
    .expect("reference module admitted and loaded");
    let db = Arc::new(RecordingDb {
        requests: Mutex::new(Vec::new()),
    });
    let db_gateway: Arc<dyn DbGateway> = db.clone();
    let ctx = FnCtx::new().with_db(db_gateway, "main".to_string());
    assert_eq!(
        function
            .call(&ctx, &FnBatch::new(), &Params::new())
            .await
            .expect("procedure executes"),
        QueryValue::Int(1)
    );
    db
}

fn typed_reference_guard() -> shamir_query_builder::shamir_query_types::filter::Filter {
    let type_is = |alias: &str, field: &str, kind: &str| {
        value_eq(
            func("value_nav/type_of", [qref(alias, format!("[0].{field}"))]),
            kind,
        )
    };
    and(vec![
        type_is("source", "bool_value", "bool"),
        type_is("second", "bytes_value", "bytes"),
        type_is("source", "f64_value", "f64"),
        type_is("second", "i32_value", "int"),
        value_gte(qref("second", "[0].i32_value"), i64::from(i32::MIN)),
        value_lte(qref("second", "[0].i32_value"), i64::from(i32::MAX)),
        type_is("source", "i64_value", "int"),
        type_is("second", "string_value", "string"),
    ])
}

fn read_to_read_batch() -> Batch {
    let mut batch = Batch::new();
    batch.query("source", Query::from("people"));
    batch.query("second", Query::from("people"));
    let target = batch.query(
        "target",
        Query::from("people")
            .where_eq("bool_value", qref("source", "[0].bool_value"))
            .where_eq("bytes_value", qref("second", "[0].bytes_value"))
            .where_eq("f64_value", qref("source", "[0].f64_value"))
            .where_eq("i32_value", qref("second", "[0].i32_value"))
            .where_eq("i64_value", qref("source", "[0].i64_value"))
            .where_eq("label", "literal")
            .where_eq("string_value", qref("second", "[0].string_value")),
    );
    batch.when(&target, typed_reference_guard());
    batch
}

fn read_to_insert_batch() -> Batch {
    let mut batch = Batch::new();
    batch.query("source", Query::from("people"));
    batch.query("second", Query::from("people"));
    let target = batch.insert(
        "target",
        insert("sink").row(
            doc()
                .set("bool_value", qref("source", "[0].bool_value"))
                .expect("bool reference field")
                .set("bytes_value", qref("second", "[0].bytes_value"))
                .expect("bytes reference field")
                .set("f64_value", qref("source", "[0].f64_value"))
                .expect("f64 reference field")
                .set("i32_value", qref("second", "[0].i32_value"))
                .expect("i32 reference field")
                .set("i64_value", qref("source", "[0].i64_value"))
                .expect("i64 reference field")
                .set("label", "literal")
                .expect("literal field")
                .set("string_value", qref("second", "[0].string_value"))
                .expect("string reference field"),
        ),
    );
    batch.when(&target, typed_reference_guard());
    batch
}

fn assert_request_matches_native(requests: &Mutex<Vec<Vec<u8>>>, native: &Batch) {
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0],
        native.to_msgpack().expect("native batch encodes"),
        "guest request including `when` guard bytes matches the native Batch/qref oracle"
    );
}

#[tokio::test]
async fn read_references_match_native_query_and_when_bytes_under_production_compile() {
    let db = run_reference_source(READ_TO_READ).await;
    assert_request_matches_native(&db.requests, &read_to_read_batch());
}

#[tokio::test]
async fn insert_references_match_native_insert_and_when_bytes_under_production_compile() {
    let db = run_reference_source(READ_TO_INSERT).await;
    assert_request_matches_native(&db.requests, &read_to_insert_batch());
}

#[tokio::test]
async fn string_reference_insert_matches_native_single_atom_when_bytes() {
    let source = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    const row: { value: string } = { value: query_ref_field(b0, "source", 0, "value") };
    let b1 = batch_insert(b0, "target", "sink", row);
    let response = db_execute(request(b1));
    return 1;
}"#;
    let db = run_reference_source(source).await;

    let mut native = Batch::new();
    native.query("source", Query::from("people"));
    let target = native.insert(
        "target",
        insert("sink").row(
            doc()
                .set("value", qref("source", "[0].value"))
                .expect("reference field builds"),
        ),
    );
    native.when(
        &target,
        value_eq(
            func("value_nav/type_of", [qref("source", "[0].value")]),
            "string",
        ),
    );
    assert_request_matches_native(&db.requests, &native);
}

#[tokio::test]
async fn i32_reference_insert_matches_native_three_atom_and_when_bytes() {
    let source = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    const row: { value: i32 } = { value: query_ref_field(b0, "source", 0, "value") };
    let b1 = batch_insert(b0, "target", "sink", row);
    let response = db_execute(request(b1));
    return 1;
}"#;
    let db = run_reference_source(source).await;

    let mut native = Batch::new();
    native.query("source", Query::from("people"));
    let target = native.insert(
        "target",
        insert("sink").row(
            doc()
                .set("value", qref("source", "[0].value"))
                .expect("reference field builds"),
        ),
    );
    native.when(
        &target,
        and(vec![
            value_eq(
                func("value_nav/type_of", [qref("source", "[0].value")]),
                "int",
            ),
            value_gte(qref("source", "[0].value"), i64::from(i32::MIN)),
            value_lte(qref("source", "[0].value"), i64::from(i32::MAX)),
        ]),
    );
    assert_request_matches_native(&db.requests, &native);
}
