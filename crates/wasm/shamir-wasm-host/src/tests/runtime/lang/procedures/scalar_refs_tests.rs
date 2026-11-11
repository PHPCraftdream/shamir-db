use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, Params, ShamirFunction, TierMode, WasmEngine, WasmFunction,
    WasmLimits,
};
use shamir_lang::testsupport::compile_with_references;
use shamir_lang::{compile, CompileError, CompileOptions, Stage};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::val::qref;
use shamir_query_builder::write::{doc, insert};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

struct RecordingDb {
    requests: Mutex<Vec<Vec<u8>>>,
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
        QueryValue::Map(new_map())
            .to_bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|error| DbGatewayError::Execution(error.to_string()))
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
    let module = compile_with_references(source, &options()).expect("reference source compiles");
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

fn assert_production_gate(source: &str) {
    let error = compile(source, &options()).expect_err("production compilation remains gated");
    let CompileError::Diagnostics(diagnostics) = error else {
        panic!("expected T010 gated diagnostic, got {error}");
    };
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "T010"
            && diagnostic.message
                == "query_ref_field is gated: scalar reference validation is not implemented"
    }));
}

fn read_to_read_batch() -> Batch {
    let mut batch = Batch::new();
    batch.query("source", Query::from("people"));
    batch.query("second", Query::from("people"));
    batch.query(
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
    batch
}

fn read_to_insert_batch() -> Batch {
    let mut batch = Batch::new();
    batch.query("source", Query::from("people"));
    batch.query("second", Query::from("people"));
    batch.insert(
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
    batch
}

fn assert_request_matches_native(requests: &Mutex<Vec<Vec<u8>>>, native: &Batch) {
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0],
        native.to_msgpack().expect("native batch encodes"),
        "guest request bytes match the native Batch/qref oracle"
    );
}

#[tokio::test]
async fn read_references_match_native_query_bytes_and_production_stays_gated() {
    assert_production_gate(READ_TO_READ);
    let db = run_reference_source(READ_TO_READ).await;
    assert_request_matches_native(&db.requests, &read_to_read_batch());
}

#[tokio::test]
async fn insert_references_match_native_insert_bytes_and_production_stays_gated() {
    assert_production_gate(READ_TO_INSERT);
    let db = run_reference_source(READ_TO_INSERT).await;
    assert_request_matches_native(&db.requests, &read_to_insert_batch());
}
