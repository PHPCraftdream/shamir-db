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

/// The default `limits` record in serialization order (design section 4).
fn default_limits() -> QueryValue {
    map_of(&[
        ("max_queries", int(50)),
        ("max_dependency_depth", int(10)),
        ("max_execution_time_secs", int(30)),
        ("max_result_size", int(10_485_760)),
        ("max_nesting_depth", int(4)),
        ("max_iterations", int(1_000)),
        ("max_total_queries", int(10_000)),
        ("max_total_work", int(1_000_000)),
        ("max_page_size", int(100_000)),
        ("max_offset", int(1_000_000)),
    ])
}

fn select_all() -> QueryValue {
    map_of(&[
        (
            "items",
            QueryValue::List(vec![map_of(&[("type", text("all"))])]),
        ),
        ("distinct", QueryValue::Bool(false)),
    ])
}

fn plan(id: QueryValue, queries: QueryValue) -> QueryValue {
    map_of(&[
        ("id", id),
        ("transactional", QueryValue::Bool(false)),
        ("queries", queries),
        ("return_all", QueryValue::Bool(true)),
        ("limits", default_limits()),
        ("result_encoding", text("name")),
    ])
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
    // derivation: fixmap(6) then the first key `id` (fixstr 2) mapped to nil,
    // then `transactional` (fixstr 13): the descriptor keeps the DTO order.
    assert_eq!(
        &bytes[..5],
        &[0x86, 0xa2, b'i', b'd', 0xc0],
        "plan record header and id"
    );
    assert_eq!(&bytes[5..7], &[0xad, b't']);
    let decoded = QueryValue::from_bytes(bytes).expect("request decodes");
    // derivation: one equality on `role` (a single predicate field is a bare
    // `eq`, not an `and`); `from` is the bare table string (repo main).
    let expected = plan(
        QueryValue::Null,
        map_of(&[(
            "u",
            map_of(&[
                ("from", text("users")),
                ("select", select_all()),
                (
                    "where",
                    map_of(&[
                        ("op", text("eq")),
                        ("field", QueryValue::List(vec![text("role")])),
                        ("value", text("admin")),
                    ]),
                ),
                ("return_result", QueryValue::Bool(true)),
            ]),
        )]),
    );
    assert_eq!(decoded, expected);
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
    let decoded = QueryValue::from_bytes(&requests[0]).expect("request decodes");
    let read = |filter: Option<QueryValue>| {
        let mut fields = vec![("from", text("users")), ("select", select_all())];
        if let Some(filter) = filter {
            fields.push(("where", filter));
        }
        fields.push(("return_result", QueryValue::Bool(true)));
        map_of(&fields)
    };
    let eq = |field: &str, value: QueryValue| {
        map_of(&[
            ("op", text("eq")),
            ("field", QueryValue::List(vec![text(field)])),
            ("value", value),
        ])
    };
    // derivation: the record filter lists its fields sorted (name, role)
    // inside one `and`; a scalar filter is an `eq` on the default `id`; null
    // adds no `where`; the insert entry holds a one-element `values` list.
    let expected = plan(
        text("req-1"),
        map_of(&[
            (
                "a",
                map_of(&[
                    ("insert_into", text("users")),
                    (
                        "values",
                        QueryValue::List(vec![map_of(&[("name", text("Zed"))])]),
                    ),
                    ("return_result", QueryValue::Bool(true)),
                ]),
            ),
            (
                "b",
                read(Some(map_of(&[
                    ("op", text("and")),
                    (
                        "filters",
                        QueryValue::List(vec![eq("name", text("Zed")), eq("role", text("admin"))]),
                    ),
                ]))),
            ),
            ("c", read(Some(eq("id", int(7))))),
            ("d", read(None)),
        ]),
    );
    assert_eq!(decoded, expected);
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
    let decoded = QueryValue::from_bytes(&requests[0]).expect("request decodes");
    let QueryValue::Map(plan) = decoded else {
        panic!("the request is a map");
    };
    let Some(QueryValue::Map(queries)) = plan.get("queries") else {
        panic!("the request has a queries map");
    };
    let aliases: Vec<&str> = queries.keys().map(String::as_str).collect();
    assert_eq!(aliases, ["zeta", "alpha", "mid"]);
}
