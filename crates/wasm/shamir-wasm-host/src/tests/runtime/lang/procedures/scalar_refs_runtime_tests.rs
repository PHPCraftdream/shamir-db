use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, TierMode, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions, Stage};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

const SKIP_MESSAGE: &str = "batch reference did not resolve to the expected scalar";

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

fn map_of(entries: &[(&str, QueryValue)]) -> QueryValue {
    let mut map = new_map();
    for (key, value) in entries {
        map.insert((*key).to_string(), value.clone());
    }
    QueryValue::Map(map)
}

fn response_bytes(value: QueryValue) -> Vec<u8> {
    value.to_bytes().expect("canned response encodes").to_vec()
}

fn result_entry(entries: &[(&str, QueryValue)]) -> QueryValue {
    map_of(entries)
}

fn response_with_results(results: QueryValue) -> Vec<u8> {
    response_bytes(map_of(&[("results", results)]))
}

fn response_with_entries(entries: &[(&str, QueryValue)]) -> Vec<u8> {
    response_with_results(map_of(entries))
}

fn skipped(value: QueryValue) -> QueryValue {
    result_entry(&[("skipped", value)])
}

fn standard_response(target: QueryValue) -> Vec<u8> {
    response_with_entries(&[("source", result_entry(&[])), ("target", target)])
}

fn gateway(responses: Vec<Vec<u8>>) -> Arc<RecordingDb> {
    Arc::new(RecordingDb {
        requests: Mutex::new(Vec::new()),
        responses: Mutex::new(responses),
    })
}

async fn compiled(source: &str) -> WasmFunction {
    let options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    let module =
        compile(source, &options).expect("procedure source compiles with production compile");
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

const DIRECT_REFERENCE: &str = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    const value: { id: i64 } = { id: query_ref_field(b0, "source", 0, "id") };
    let b1 = batch_insert(b0, "target", "sink", value);
    let response = db_execute(request(b1));
    return 1;
}"#;

const HELPER_REFERENCE: &str = r#"fn build_plan() -> Batch {
    let b0 = batch_read(batch(), "source", "people", null);
    const value: { id: i64 } = { id: query_ref_field(b0, "source", 0, "id") };
    return batch_insert(b0, "target", "sink", value);
}
fn execute_plan(b: Batch) -> i64 {
    let response = db_execute(request(b));
    return 1;
}
@entry procedure p() -> i64 {
    return execute_plan(build_plan());
}"#;

fn assert_reference_failure(error: &FunctionError) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message == &format!("shamir_call: compute error: guest invalid argument: {SKIP_MESSAGE}")),
        "expected exact kind-2 reference failure, got {error}"
    );
}

fn assert_codec_failure(error: &FunctionError) {
    assert!(
        matches!(error, FunctionError::Compute(message) if message.starts_with("shamir_call: compute error: guest codec: ")),
        "expected kind-3 codec failure, got {error}"
    );
}

#[tokio::test]
async fn skipped_reference_operation_and_independent_result_fail_with_exact_kind_two() {
    let function = compiled(DIRECT_REFERENCE).await;
    let cases = [
        (
            "guarded target skipped",
            response_with_entries(&[("target", skipped(QueryValue::Bool(true)))]),
        ),
        (
            "independent alias skipped",
            response_with_entries(&[("independent", skipped(QueryValue::Bool(true)))]),
        ),
    ];
    for (case, response) in cases {
        let error = call_with(&function, gateway(vec![response]))
            .await
            .expect_err(case);
        assert_reference_failure(&error);
    }
}

#[tokio::test]
async fn skipped_check_rejects_malformed_response_shapes_as_kind_three() {
    let function = compiled(DIRECT_REFERENCE).await;
    let malformed = [
        ("missing results", response_bytes(map_of(&[]))),
        ("non-map root", response_bytes(QueryValue::List(Vec::new()))),
        (
            "non-map results",
            response_with_results(QueryValue::List(Vec::new())),
        ),
        (
            "non-map result entry",
            response_with_entries(&[("other", QueryValue::Int(1))]),
        ),
        (
            "non-bool skipped flag",
            response_with_entries(&[("other", skipped(QueryValue::Int(1)))]),
        ),
    ];
    for (case, response) in malformed {
        let error = call_with(&function, gateway(vec![response]))
            .await
            .expect_err(case);
        assert_codec_failure(&error);
    }
}

#[tokio::test]
async fn omitted_and_false_skipped_flags_pass_and_a_fresh_call_succeeds_after_failure() {
    let function = compiled(DIRECT_REFERENCE).await;
    for target in [result_entry(&[]), skipped(QueryValue::Bool(false))] {
        assert_eq!(
            call_with(&function, gateway(vec![standard_response(target)]))
                .await
                .expect("omitted or false skipped flag passes"),
            QueryValue::Int(1)
        );
    }

    let db = gateway(vec![
        standard_response(skipped(QueryValue::Bool(true))),
        standard_response(skipped(QueryValue::Bool(false))),
    ]);
    let error = call_with(&function, Arc::clone(&db))
        .await
        .expect_err("first execution has a skipped guarded op");
    assert_reference_failure(&error);
    assert_eq!(
        call_with(&function, Arc::clone(&db))
            .await
            .expect("a fresh invocation succeeds after the failure"),
        QueryValue::Int(1)
    );
    assert_eq!(db.requests.lock().expect("requests lock").len(), 2);
}

#[tokio::test]
async fn module_wide_reference_flag_checks_execute_helper_response() {
    let function = compiled(HELPER_REFERENCE).await;
    let error = call_with(
        &function,
        gateway(vec![response_with_entries(&[(
            "unrelated",
            skipped(QueryValue::Bool(true)),
        )])]),
    )
    .await
    .expect_err("execute helper must apply module-wide skipped guard");
    assert_reference_failure(&error);
}

#[test]
fn no_reference_module_keeps_guard_helpers_out_and_import_count_unchanged() {
    let source = r#"@entry procedure p() -> i64 {
    let b = batch_read(batch(), "source", "people", null);
    let response = db_execute(request(b));
    return 1;
}"#;
    let module = compile(
        source,
        &CompileOptions {
            stage: Stage::Procedures,
            ..CompileOptions::default()
        },
    )
    .expect("no-reference procedure compiles");
    assert_eq!(module.report.import_count, 2);
    assert!(
        !module
            .wasm
            .windows(SKIP_MESSAGE.len())
            .any(|window| window == SKIP_MESSAGE.as_bytes()),
        "a module without references must not contain the skipped-check message"
    );
}
