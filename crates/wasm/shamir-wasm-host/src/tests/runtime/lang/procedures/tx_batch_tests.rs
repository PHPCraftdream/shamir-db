use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, TierMode, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions, Stage};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::value_eq;
use shamir_query_builder::query::Query;
use shamir_query_builder::val::{func, qref};
use shamir_query_builder::write::{doc, insert};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

const SKIP_MESSAGE: &str = "batch reference did not resolve to the expected scalar";
const CONFLICT_MESSAGE: &str = "transactional batch aborted by a conflict";
const ABORT_MESSAGE: &str = "transactional batch aborted";
const INDETERMINATE_MESSAGE: &str = "transactional batch outcome is unknown";
const SECRET_REASON: &str = "private-table-value-must-not-leak";

const TX_REFERENCE_PLAN: &str = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch_tx(), "source", "people", null);
    let b_un = batch_insert(b0, "plain_insert", "sink", { id: 7 });
    let b1 = batch_read(b_un, "target_read", "people", null);
    let b2 = batch_read(b1, "target_read_again", "people", null);
    const target_filter: { id: i64 } = {
        id: query_ref_field(b2, "target_read", 0, "id")
    };
    let b3 = batch_read(b2, "target_filter", "people", target_filter);
    const target_row: { id: i64 } = {
        id: query_ref_field(b3, "target_filter", 0, "id")
    };
    let b4 = batch_insert(b3, "target_insert", "sink", target_row);
    let response = db_execute(request(b4));
    return 1;
}"#;

/// Records the guest's exact bytes and serves independent canned protocol
/// responses, including malformed transaction envelopes.
struct RecordingDb {
    requests: Mutex<Vec<Vec<u8>>>,
    responses: Mutex<Vec<Vec<u8>>>,
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
        let mut responses = self.responses.lock().expect("responses lock");
        if responses.is_empty() {
            return Err(DbGatewayError::Execution("no canned response".into()));
        }
        Ok(responses.remove(0))
    }
}

fn response_bytes(value: QueryValue) -> Vec<u8> {
    value.to_bytes().expect("canned response encodes").to_vec()
}

fn response_with_transaction(transaction: Option<QueryValue>) -> Vec<u8> {
    let mut root = new_map();
    root.insert("results".to_string(), QueryValue::Map(new_map()));
    if let Some(transaction) = transaction {
        root.insert("transaction".to_string(), transaction);
    }
    response_bytes(QueryValue::Map(root))
}

fn transaction(status: &str, reason: Option<QueryValue>, error: Option<&str>) -> QueryValue {
    let mut fields = new_map();
    fields.insert("status".to_string(), QueryValue::Str(status.to_string()));
    if let Some(reason) = reason {
        fields.insert("reason".to_string(), reason);
    }
    if let Some(error) = error {
        fields.insert("error".to_string(), QueryValue::Str(error.to_string()));
    }
    QueryValue::Map(fields)
}

fn gateway(responses: Vec<Vec<u8>>) -> Arc<RecordingDb> {
    Arc::new(RecordingDb {
        requests: Mutex::new(Vec::new()),
        responses: Mutex::new(responses),
    })
}

async fn compiled(source: &str) -> WasmFunction {
    let module = compile(
        source,
        &CompileOptions {
            stage: Stage::Procedures,
            ..CompileOptions::default()
        },
    )
    .expect("procedure source compiles");
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
    let db_gateway: Arc<dyn DbGateway> = db;
    let ctx = FnCtx::new().with_db(db_gateway, "main".to_string());
    let empty: TMap<String, QueryValue> = TMap::default();
    function
        .call(&ctx, &FnBatch::new(), &Params::from_map(empty))
        .await
}

fn reference_guard(alias: &str) -> shamir_query_builder::shamir_query_types::filter::Filter {
    value_eq(func("value_nav/type_of", [qref(alias, "[0].id")]), "int")
}

fn native_tx_reference_plan() -> Batch {
    let mut batch = Batch::new();
    batch.transactional();
    batch.query("source", Query::from("people"));
    batch.insert(
        "plain_insert",
        insert("sink").row(
            doc()
                .set("id", 7_i64)
                .expect("native unguarded insert document builds"),
        ),
    );
    batch.query("target_read", Query::from("people"));
    batch.query("target_read_again", Query::from("people"));
    let referenced_read = batch.query(
        "target_filter",
        Query::from("people").where_eq("id", qref("target_read", "[0].id")),
    );
    batch
        .when(&referenced_read, reference_guard("target_read"))
        .abort_on_skip(&referenced_read);
    let guarded_insert = batch.insert(
        "target_insert",
        insert("sink").row(
            doc()
                .set("id", qref("target_filter", "[0].id"))
                .expect("native reference insert document builds"),
        ),
    );
    batch
        .when(&guarded_insert, reference_guard("target_filter"))
        .abort_on_skip(&guarded_insert);
    batch
}

fn assert_kind_two(error: &FunctionError, message: &str) {
    assert!(
        matches!(error, FunctionError::Compute(observed)
            if observed == &format!("shamir_call: compute error: guest invalid argument: {message}")),
        "expected exact kind-2 message {message:?}, got {error}"
    );
}

fn assert_kind_three(error: &FunctionError) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message == "shamir_call: compute error: guest codec: arg"),
        "expected the existing kind-3 `arg` decode error, got {error}"
    );
}

#[tokio::test]
async fn transactional_reference_read_and_insert_match_native_builder_bytes() {
    let function = compiled(TX_REFERENCE_PLAN).await;
    let db = gateway(vec![response_with_transaction(Some(transaction(
        "committed",
        None,
        None,
    )))]);
    assert_eq!(
        call_with(&function, Arc::clone(&db))
            .await
            .expect("committed transactional reference plan succeeds"),
        QueryValue::Int(1)
    );

    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let expected = native_tx_reference_plan()
        .to_msgpack()
        .expect("native transactional batch encodes");
    assert_eq!(
        requests[0], expected,
        "guarded guest read and insert bytes match transactional Batch::when + abort_on_skip"
    );

    let decoded = QueryValue::from_bytes(&requests[0]).expect("guest request decodes");
    let QueryValue::Map(request) = decoded else {
        panic!("guest request is a map");
    };
    let Some(QueryValue::Map(queries)) = request.get("queries") else {
        panic!("guest request has a queries map");
    };
    let Some(QueryValue::Map(source)) = queries.get("source") else {
        panic!("source entry is a map");
    };
    assert!(
        !source.contains_key("abort_on_skip"),
        "unguarded reference-source read does not carry the abort flag"
    );
    let Some(QueryValue::Map(plain_insert)) = queries.get("plain_insert") else {
        panic!("unguarded insert entry is a map");
    };
    assert!(
        !plain_insert.contains_key("abort_on_skip"),
        "unguarded insert does not carry the abort flag"
    );
    let Some(QueryValue::Map(unguarded_read)) = queries.get("target_read") else {
        panic!("unguarded read entry is a map");
    };
    assert!(
        !unguarded_read.contains_key("abort_on_skip"),
        "unguarded read does not carry abort_on_skip"
    );
    let Some(QueryValue::Map(unguarded_read_again)) = queries.get("target_read_again") else {
        panic!("second unguarded read entry is a map");
    };
    assert!(
        !unguarded_read_again.contains_key("abort_on_skip"),
        "second unguarded read does not carry abort_on_skip"
    );
    for alias in ["target_filter", "target_insert"] {
        let Some(QueryValue::Map(entry)) = queries.get(alias) else {
            panic!("guarded reference operation {alias} is a map");
        };
        assert_eq!(
            entry.get("abort_on_skip"),
            Some(&QueryValue::Bool(true)),
            "transactional guarded operation {alias} carries abort_on_skip"
        );
    }
}

#[tokio::test]
async fn transaction_response_table_maps_commits_and_aborts_without_reason_leakage() {
    let function = compiled(TX_REFERENCE_PLAN).await;
    let cases = [
        (
            "committed without reason",
            transaction("committed", None, None),
            None,
        ),
        (
            "committed with WAL durability warning",
            transaction("committed", None, Some("wal_durability")),
            None,
        ),
        (
            "skip-aborted reference",
            transaction(
                "aborted",
                Some(QueryValue::Str("skip_aborted".to_string())),
                None,
            ),
            Some(SKIP_MESSAGE),
        ),
        (
            "transaction conflict",
            transaction(
                "aborted",
                Some(QueryValue::Str("tx_conflict".to_string())),
                None,
            ),
            Some(CONFLICT_MESSAGE),
        ),
        (
            "arbitrary secret abort reason",
            transaction(
                "aborted",
                Some(QueryValue::Str(SECRET_REASON.to_string())),
                None,
            ),
            Some(ABORT_MESSAGE),
        ),
        (
            "indeterminate outcome",
            transaction("indeterminate", None, Some("wal_outcome_unknown")),
            Some(INDETERMINATE_MESSAGE),
        ),
    ];

    for (case, info, expected_error) in cases {
        let db = gateway(vec![response_with_transaction(Some(info))]);
        match expected_error {
            None => assert_eq!(
                call_with(&function, db)
                    .await
                    .unwrap_or_else(|error| panic!("{case} must continue: {error}")),
                QueryValue::Int(1),
                "{case} continues to the procedure result"
            ),
            Some(message) => {
                let error = call_with(&function, db)
                    .await
                    .expect_err("non-committed transaction must fail the guest call");
                assert_kind_two(&error, message);
                if case == "arbitrary secret abort reason" {
                    assert!(
                        !error.to_string().contains(SECRET_REASON),
                        "transaction reason must never appear in guest-visible errors"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn malformed_transaction_and_invalid_reason_shapes_are_kind_three() {
    let function = compiled(TX_REFERENCE_PLAN).await;
    let malformed = [
        ("missing transaction", response_with_transaction(None)),
        (
            "non-map transaction",
            response_with_transaction(Some(QueryValue::Int(7))),
        ),
        (
            "missing status",
            response_with_transaction(Some(QueryValue::Map(new_map()))),
        ),
        (
            "non-string status",
            response_with_transaction(Some({
                let mut fields = new_map();
                fields.insert("status".to_string(), QueryValue::Bool(true));
                QueryValue::Map(fields)
            })),
        ),
        (
            "unknown status",
            response_with_transaction(Some(transaction("pending", None, None))),
        ),
        (
            "aborted reason missing",
            response_with_transaction(Some(transaction("aborted", None, None))),
        ),
        (
            "aborted reason non-string",
            response_with_transaction(Some(transaction("aborted", Some(QueryValue::Int(9)), None))),
        ),
        (
            "committed present non-string reason",
            response_with_transaction(Some(transaction(
                "committed",
                Some(QueryValue::Int(9)),
                None,
            ))),
        ),
        (
            "indeterminate present non-string reason",
            response_with_transaction(Some(transaction(
                "indeterminate",
                Some(QueryValue::Int(9)),
                Some("wal_outcome_unknown"),
            ))),
        ),
    ];

    for (case, response) in malformed {
        let error = call_with(&function, gateway(vec![response]))
            .await
            .unwrap_err();
        assert_kind_three(&error);
        assert!(
            !error.to_string().contains(SECRET_REASON),
            "malformed transaction fixture {case} does not expose reason data"
        );
    }
}

#[tokio::test]
async fn nontransactional_execute_in_tx_using_module_accepts_missing_transaction() {
    let source = r#"@entry procedure p() -> i64 {
    let tx_plan = batch_read(batch_tx(), "tx_read", "people", null);
    let tx_response = db_execute(request(tx_plan));
    let ordinary = batch_read(batch(), "plain_read", "people", null);
    let ordinary_response = db_execute(request(ordinary));
    return 1;
}"#;
    let function = compiled(source).await;
    let db = gateway(vec![
        response_with_transaction(Some(transaction("committed", None, None))),
        response_with_transaction(None),
    ]);
    assert_eq!(
        call_with(&function, Arc::clone(&db))
            .await
            .expect("non-transactional call accepts response without transaction metadata"),
        QueryValue::Int(1)
    );

    let requests = db.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    let mut native_tx = Batch::new();
    native_tx.transactional();
    native_tx.query("tx_read", Query::from("people"));
    let mut native_plain = Batch::new();
    native_plain.query("plain_read", Query::from("people"));
    assert_eq!(
        requests[0],
        native_tx.to_msgpack().expect("native tx batch encodes")
    );
    assert_eq!(
        requests[1],
        native_plain
            .to_msgpack()
            .expect("native ordinary batch encodes")
    );
}
