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
use shamir_query_builder::filter::{and, eq};
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{delete, doc, insert, update};
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

const MUTATION_PLAN: &str = r#"@entry procedure mutate(owner: i64, status: string) -> i64 {
    let changes: { profile: { active: bool, city: string }, tags: [string] } = { profile: { active: true, city: "Paris" }, tags: ["vip", "paid"] };
    let b = batch_update(batch(), "zeta", "profiles", { owner: owner, status: status }, changes);
    let b2 = batch_delete(b, "alpha", "sessions", { owner: owner });
    let b3 = batch_update(b2, "mid", "profiles", { id: 7 }, { status: "archived" });
    let resp = db_execute(request(b3));
    let affected: i64? = response_affected(resp, "zeta");
    if (is_null(affected)) { return -1; }
    return unwrap(affected);
}"#;

/// The guest builds a mixed update/delete plan whose native counterpart uses
/// only the public typed builders; this pins operation DTOs, predicate
/// conjunctions, nested values and source insertion order at the byte boundary.
#[tokio::test]
async fn batch_update_delete_match_native_builder_bytes_and_keep_order() {
    let function = compiled(MUTATION_PLAN).await;
    let db = gateway(affected_response_for("zeta", int(3)));
    let db_gateway: Arc<dyn DbGateway> = db.clone();
    let ctx = FnCtx::new().with_db(db_gateway, "main".to_string());
    let params: TMap<String, QueryValue> = [
        ("owner".to_string(), int(42)),
        ("status".to_string(), text("active")),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        function
            .call(&ctx, &FnBatch::new(), &Params::from_map(params))
            .await
            .expect("mutation plan executes"),
        int(3)
    );

    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let mut native = Batch::new();
    native
        .try_update(
            "zeta",
            update("profiles")
                .where_(and(vec![eq("owner", 42_i64), eq("status", "active")]))
                .set(
                    doc()
                        .set_value(
                            "profile",
                            map_of(&[("active", QueryValue::Bool(true)), ("city", text("Paris"))]),
                        )
                        .set_value("tags", QueryValue::List(vec![text("vip"), text("paid")])),
                ),
        )
        .expect("native update builds");
    native
        .try_delete("alpha", delete("sessions").where_(eq("owner", 42_i64)))
        .expect("native delete builds");
    native
        .try_update(
            "mid",
            update("profiles")
                .where_(eq("id", 7_i64))
                .set(doc().set("status", "archived").expect("native set builds")),
        )
        .expect("native update builds");

    let decoded = assert_matches_native_batch(&requests[0], &native);
    assert_eq!(query_aliases(&decoded), ["zeta", "alpha", "mid"]);
}

/// The mutation intrinsics reuse the existing single `db_execute` import;
/// `response_affected` is guest-side projection, not a new host import.
#[test]
fn batch_mutations_do_not_add_a_host_import() {
    let options = CompileOptions {
        stage: ShamirStage::Procedures,
        ..CompileOptions::default()
    };
    let module = compile(MUTATION_PLAN, &options).expect("mutation source compiles");
    // Derivation: `raise_error` plus the pre-existing `db_execute` import.
    assert_eq!(module.report.import_count, 2);
}

const AFFECTED_ONE: &str = r#"@entry procedure affected() -> i64 {
    let b = batch_update(batch(), "u", "users", { id: 7 }, { active: true });
    let resp = db_execute(request(b));
    let count: i64? = response_affected(resp, "u");
    if (is_null(count)) { return -1; }
    return unwrap(count);
}"#;

fn response_with_results(results: QueryValue) -> Vec<u8> {
    map_of(&[("results", results)])
        .to_bytes()
        .expect("response encodes")
        .to_vec()
}

fn response_with_entry(alias: &str, entry: QueryValue) -> Vec<u8> {
    response_with_results(map_of(&[(alias, entry)]))
}

fn affected_response_for(alias: &str, count: QueryValue) -> Vec<u8> {
    response_with_entry(
        alias,
        map_of(&[("stats", map_of(&[("records_scanned", count)]))]),
    )
}

#[tokio::test]
async fn response_affected_projects_zero_and_i64_max() {
    let function = compiled(AFFECTED_ONE).await;
    for count in [0, i64::MAX] {
        let observed = call_with(&function, gateway(affected_response_for("u", int(count))))
            .await
            .expect("valid affected count projects");
        assert_eq!(observed, int(count));
    }
}

#[tokio::test]
async fn response_affected_missing_alias_is_null_only_with_results_map() {
    let function = compiled(
        r#"@entry procedure missing() -> i64 {
    let b = batch_delete(batch(), "d", "sessions", { owner: 7 });
    let resp = db_execute(request(b));
    let count: i64? = response_affected(resp, "absent");
    if (is_null(count)) { return -1; }
    return unwrap(count);
}"#,
    )
    .await;
    // A valid root results map with no `absent` entry represents no result;
    // the same absence at the root/results boundary is a kind-3 error below.
    let result = call_with(&function, gateway(response_with_results(map_of(&[]))))
        .await
        .expect("missing alias in a valid results map is nullable");
    assert_eq!(result, int(-1));

    let error = call_with(&function, gateway(response_root_missing_results()))
        .await
        .expect_err("missing results root must not become null");
    assert_codec(&error, "arg");
}

fn response_root_missing_results() -> Vec<u8> {
    map_of(&[])
        .to_bytes()
        .expect("malformed response fixture encodes")
        .to_vec()
}

/// This is deliberately raw response data: `QueryValue` cannot represent an
/// unsigned integer above `i64::MAX`, and query construction remains builder-only.
fn response_with_u64_max_count() -> Vec<u8> {
    vec![
        0x81, 0xa7, b'r', b'e', b's', b'u', b'l', b't', b's', // root results map
        0x81, 0xa1, b'u', // results alias
        0x81, 0xa5, b's', b't', b'a', b't', b's', // alias stats map
        0x81, 0xaf, b'r', b'e', b'c', b'o', b'r', b'd', b's', b'_', b's', b'c', b'a', b'n', b'n',
        b'e', b'd', // count key
        0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // u64::MAX
    ]
}

#[tokio::test]
async fn response_affected_rejects_malformed_and_invalid_projection_shapes_as_kind_three() {
    let function = compiled(AFFECTED_ONE).await;
    let malformed = [
        (
            "wrong response root type",
            QueryValue::List(Vec::new())
                .to_bytes()
                .expect("malformed response root encodes")
                .to_vec(),
        ),
        (
            "wrong results type",
            response_with_results(QueryValue::List(Vec::new())),
        ),
        ("missing stats", response_with_entry("u", map_of(&[]))),
        (
            "wrong stats type",
            response_with_entry("u", map_of(&[("stats", text("not a map"))])),
        ),
        (
            "missing count",
            response_with_entry("u", map_of(&[("stats", map_of(&[]))])),
        ),
        (
            "wrong count type",
            affected_response_for("u", text("not an integer")),
        ),
        ("negative count", affected_response_for("u", int(-1))),
        ("wrong alias entry type", response_with_entry("u", int(1))),
        (
            "unsigned count above i64::MAX",
            response_with_u64_max_count(),
        ),
    ];
    for (case, response) in malformed {
        let error = call_with(&function, gateway(response)).await.unwrap_err();
        assert_codec(
            &error,
            if case == "unsigned count above i64::MAX" {
                "u64"
            } else {
                "arg"
            },
        );
    }

    // Raw response bytes exercise the shared decoder's truncation rejection;
    // this is response decoding, not hand-built query/request construction.
    let truncated = vec![0x81, 0xa7, b'r', b'e', b's', b'u', b'l', b't', b's'];
    let error = call_with(&function, gateway(truncated))
        .await
        .expect_err("truncated response must fail decoding");
    assert_codec(&error, "trunc");
}
