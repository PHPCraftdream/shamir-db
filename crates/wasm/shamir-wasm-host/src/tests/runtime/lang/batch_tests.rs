//! Execution tests of the typed builder intrinsics (`batch`, `batch_read`,
//! `batch_insert`, `batch_id`, `request`, `db_execute`, `response_rows`,
//! `response_inserted`) against a recording gateway: the guest encodes the
//! plan as the exact `BatchRequest` DTO, calls the real `db_execute` host
//! import and projects the canned `BatchResponse` through the contextual
//! result type.
use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, ShamirStage, TierMode,
    WasmEngine, WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::ToWire;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

/// Records every request and answers with a canned response.
// Test fixture: one short critical section per call, no contention.
struct RecordingDb {
    requests: Mutex<Vec<Vec<u8>>>,
    response: Vec<u8>,
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
        Ok(self.response.clone())
    }
}

fn map_of(entries: &[(&str, QueryValue)]) -> QueryValue {
    let mut map = new_map();
    for (key, value) in entries {
        map.insert((*key).to_string(), value.clone());
    }
    QueryValue::Map(map)
}

fn int(value: i64) -> QueryValue {
    QueryValue::Int(value)
}

fn text(value: &str) -> QueryValue {
    QueryValue::Str(value.to_string())
}

/// `{ "records": [ { id, name } ] }` for the aliases `u` and `b`.
fn canned_response() -> Vec<u8> {
    let one_row = |id: i64, name: &str| {
        map_of(&[(
            "records",
            QueryValue::List(vec![map_of(&[("id", int(id)), ("name", text(name))])]),
        )])
    };
    map_of(&[
        ("id", QueryValue::Null),
        (
            "results",
            map_of(&[("u", one_row(1, "Ada")), ("b", one_row(2, "Zed"))]),
        ),
        ("execution_plan", QueryValue::List(Vec::new())),
        ("execution_time_us", int(0)),
    ])
    .to_bytes()
    .expect("response encodes")
    .to_vec()
}

fn gateway(response: Vec<u8>) -> Arc<RecordingDb> {
    Arc::new(RecordingDb {
        requests: Mutex::new(Vec::new()),
        response,
    })
}

async fn compiled(source: &str) -> WasmFunction {
    let options = CompileOptions {
        stage: ShamirStage::Procedures,
        ..CompileOptions::default()
    };
    let module = compile(source, &options).expect("procedure source compiles");
    let engine = Arc::new(WasmEngine::new().expect("engine constructed"));
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        &module.wasm,
        WasmLimits::default(),
        TierMode::Auto,
        AdmissionPolicy::Load,
    )
    .await
    .expect("procedure module admitted and loaded");
    function
}

async fn call_with(
    function: &WasmFunction,
    db: Arc<RecordingDb>,
) -> Result<QueryValue, FunctionError> {
    let ctx = FnCtx::new().with_db(db, "main".to_string());
    let empty: TMap<String, QueryValue> = TMap::default();
    function
        .call(&ctx, &FnBatch::new(), &Params::from_map(empty))
        .await
}

/// kind 2 marker helper (`FunctionError::Compute("guest invalid argument: ..")`).
fn assert_invalid_argument(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest invalid argument: {marker}"))),
        "expected invalid-argument marker {marker}, got {error}"
    );
}

/// kind 3 marker helper (`FunctionError::Compute("guest codec: ..")`).
fn assert_codec(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest codec: {marker}"))),
        "expected codec marker {marker}, got {error}"
    );
}

/// Assert the guest request against the native builder at the decoded-value,
/// query-order, typed-request, and raw-byte levels.
fn assert_matches_native_batch(guest_bytes: &[u8], batch: &Batch) -> QueryValue {
    let native_bytes = batch.to_msgpack().expect("native batch encodes");
    let native_request = batch
        .to_request_via_msgpack()
        .expect("native batch decodes through msgpack");
    let native_value = QueryValue::from_bytes(&native_bytes).expect("native request decodes");
    let typed_request_aliases: Vec<&str> =
        native_request.queries.keys().map(String::as_str).collect();
    let typed_request_value = native_request
        .to_query_value()
        .expect("typed native request converts to QueryValue");
    let guest_value = QueryValue::from_bytes(guest_bytes).expect("guest request decodes");

    assert_eq!(
        native_value, typed_request_value,
        "native bytes match typed DTO"
    );
    assert_eq!(guest_value, native_value, "guest and native DTO values");
    assert_eq!(
        query_aliases(&native_value),
        typed_request_aliases,
        "native serialized query order matches typed BatchRequest"
    );
    assert_eq!(
        query_aliases(&guest_value),
        typed_request_aliases,
        "guest query insertion order matches native Batch order"
    );

    // rmp-serde's QueryValue::Int calls write_sint and the guest DTO encoder
    // chooses shortest-width MessagePack integers. Confirmed containers use
    // write_map_len (fixmap < 16, map16 through u16::MAX, then map32); flattened
    // QueryEntry serialization buffers its fields and writes that counted map
    // header at end. These operations and all fixture maps are below 16, so
    // raw byte equality is meaningful and intentionally asserted here.
    assert_eq!(
        guest_bytes, native_bytes,
        "guest and native MessagePack bytes"
    );
    guest_value
}

fn query_aliases(value: &QueryValue) -> Vec<&str> {
    let QueryValue::Map(fields) = value else {
        panic!("batch request is a map");
    };
    let Some(QueryValue::Map(queries)) = fields.get("queries") else {
        panic!("batch request contains a queries map");
    };
    queries.keys().map(String::as_str).collect()
}

const READ_ONE: &str = "@entry procedure users() -> i64 {\n\
     let b = batch_read(batch(), \"u\", \"users\", { role: \"admin\" });\n\
     let resp = db_execute(request(b));\n\
     let rows: [{ id: i64, name: string }]? = response_rows(resp, \"u\");\n\
     if (is_null(rows)) { return -1; }\n\
     return to_i64(len(unwrap(rows)));\n\
     }\n";

#[tokio::test]
async fn batch_read_sends_the_exact_dto_and_projects_the_rows() {
    let function = compiled(READ_ONE).await;
    let db = gateway(canned_response());
    // derivation: the canned answer holds one row for alias `u`.
    assert_eq!(
        call_with(&function, Arc::clone(&db))
            .await
            .expect("read succeeds"),
        int(1)
    );
    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let bytes = &requests[0];
    // The decoded oracle below establishes the complete DTO; retain this small
    // header pin for the guest's field order at the transport boundary.
    assert_eq!(
        &bytes[..5],
        &[0x86, 0xa2, b'i', b'd', 0xc0],
        "plan record header and id"
    );
    assert_eq!(&bytes[5..7], &[0xad, b't']);

    let mut native = Batch::new();
    native.query("u", Query::from("users").where_eq("role", "admin"));
    assert_matches_native_batch(bytes, &native);
}

#[tokio::test]
async fn batch_with_id_insert_and_two_filters_encodes_every_shape() {
    let function = compiled(
        "@entry procedure mixed() -> i64 {\n\
         let b = batch_id(batch(), \"req-1\");\n\
         let b2 = batch_insert(b, \"a\", \"users\", { name: \"Zed\" });\n\
         let b3 = batch_read(b2, \"b\", \"users\", { name: \"Zed\", role: \"admin\" });\n\
         let b4 = batch_read(b3, \"c\", \"users\", 7);\n\
         let b5 = batch_read(b4, \"d\", \"users\", null);\n\
         let resp = db_execute(request(b5));\n\
         let rows: [{ id: i64, name: string }]? = response_inserted(resp, \"b\");\n\
         if (is_null(rows)) { return -1; }\n\
         return to_i64(len(unwrap(rows)));\n\
         }\n",
    )
    .await;
    let db = gateway(canned_response());
    assert_eq!(
        call_with(&function, Arc::clone(&db))
            .await
            .expect("batch succeeds"),
        int(1)
    );
    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let mut native = Batch::new();
    native.id("req-1");
    native.insert(
        "a",
        insert("users").row(
            doc()
                .set("name", "Zed")
                .expect("native insert document builds"),
        ),
    );
    native.query(
        "b",
        Query::from("users")
            .where_eq("name", "Zed")
            .where_eq("role", "admin"),
    );
    native.query("c", Query::from("users").where_eq("id", 7));
    native.query("d", Query::from("users"));
    assert_matches_native_batch(&requests[0], &native);
}

#[tokio::test]
async fn a_missing_alias_reads_as_null() {
    let function = compiled(
        "@entry procedure missing() -> i64 {\n\
         let b = batch_read(batch(), \"u\", \"users\", null);\n\
         let resp = db_execute(request(b));\n\
         let rows: [{ id: i64 }]? = response_rows(resp, \"nope\");\n\
         if (is_null(rows)) { return -1; }\n\
         return 0;\n\
         }\n",
    )
    .await;
    // derivation: the answer has no `nope` alias, so the projection sees null.
    assert_eq!(
        call_with(&function, gateway(canned_response()))
            .await
            .expect("missing alias is null"),
        int(-1)
    );
}

#[tokio::test]
async fn an_answer_without_results_is_a_codec_error() {
    let function = compiled(READ_ONE).await;
    let broken = map_of(&[("id", QueryValue::Null)])
        .to_bytes()
        .expect("encodes")
        .to_vec();
    // derivation: the walk `results -> alias -> records` fails at `results`
    // with the decode-class `arg` marker, never as null.
    let error = call_with(&function, gateway(broken))
        .await
        .expect_err("missing results must fail");
    assert_codec(&error, "arg");
}

#[tokio::test]
async fn invalid_and_duplicate_aliases_are_rejected() {
    let duplicate = compiled(
        "@entry procedure dup() -> i64 {\n\
         let b = batch_read(batch(), \"a\", \"users\", null);\n\
         let c = batch_read(b, \"a\", \"users\", null);\n\
         return 0;\n\
         }\n",
    )
    .await;
    let error = call_with(&duplicate, gateway(canned_response()))
        .await
        .expect_err("duplicate alias must fail");
    // derivation: the second append finds `a` among the existing keys.
    assert_invalid_argument(&error, "alias");
    for bad in ["", "1abc", "a-b", "a b"] {
        let source = format!(
            "@entry procedure bad() -> i64 {{\n\
             let b = batch_read(batch(), \"{bad}\", \"users\", null);\n\
             return 0;\n\
             }}\n"
        );
        let function = compiled(&source).await;
        let error = call_with(&function, gateway(canned_response()))
            .await
            .expect_err("invalid alias must fail");
        // derivation: empty, a leading digit and non-identifier bytes fail
        // the 1..=64 identifier check.
        assert_invalid_argument(&error, "alias");
    }
}

#[tokio::test]
async fn queries_keep_the_insertion_order_of_the_builder_calls() {
    // derivation: the server runs independent queries in the order of the
    // encoded `queries` map, so the guest must not sort the aliases: the
    // insert named `zeta` is added first and has to stay first.
    let function = compiled(
        "@entry procedure ordered() -> i64 {\n\
         let b = batch_insert(batch(), \"zeta\", \"users\", { name: \"Zed\" });\n\
         let b2 = batch_read(b, \"alpha\", \"users\", null);\n\
         let b3 = batch_read(b2, \"mid\", \"users\", null);\n\
         let resp = db_execute(request(b3));\n\
         let rows: [{ id: i64, name: string }]? = response_rows(resp, \"alpha\");\n\
         if (is_null(rows)) { return -1; }\n\
         return 0;\n\
         }\n",
    )
    .await;
    let db = gateway(canned_response());
    call_with(&function, Arc::clone(&db))
        .await
        .expect("batch succeeds");
    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let mut native = Batch::new();
    native.insert(
        "zeta",
        insert("users").row(
            doc()
                .set("name", "Zed")
                .expect("native insert document builds"),
        ),
    );
    native.query("alpha", Query::from("users"));
    native.query("mid", Query::from("users"));
    let decoded = assert_matches_native_batch(&requests[0], &native);
    assert_eq!(query_aliases(&decoded), ["zeta", "alpha", "mid"]);
}
