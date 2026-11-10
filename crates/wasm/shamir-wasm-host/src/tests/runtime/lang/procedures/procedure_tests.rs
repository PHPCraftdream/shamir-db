//! Execution tests of compiled `@entry procedure` modules against an
//! in-memory database gateway: the direct `db_get` / `db_insert` /
//! `db_query` intrinsics encode their arguments with the guest codec, call
//! the real host imports and decode the answer into the contextual result
//! type (kind-3 failures on a shape mismatch).
use crate::gateway::db_gateway::{DbGateway, DbGatewayError};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, ShamirStage, TierMode,
    WasmEngine, WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Mutex};

/// Rows live in memory; `get` matches a scalar key against `id` or a record
/// key against every listed field, `query` filters the same way, `insert`
/// stores the document with a generated `id` (rows.len() + 1).
// Test fixture: one short critical section per call, no contention.
struct MemoryDb {
    rows: Mutex<Vec<QueryValue>>,
}

fn field<'a>(row: &'a QueryValue, name: &str) -> Option<&'a QueryValue> {
    match row {
        QueryValue::Map(map) => map.get(name),
        _ => None,
    }
}

fn matches(row: &QueryValue, filter: &QueryValue) -> bool {
    match filter {
        QueryValue::Map(wanted) => wanted
            .iter()
            .all(|(name, value)| field(row, name) == Some(value)),
        scalar => field(row, "id") == Some(scalar),
    }
}

#[async_trait::async_trait]
impl DbGateway for MemoryDb {
    async fn get(
        &self,
        _repo: &str,
        _table: &str,
        key: QueryValue,
    ) -> Result<Option<QueryValue>, DbGatewayError> {
        let rows = self.rows.lock().expect("rows lock");
        Ok(rows.iter().find(|row| matches(row, &key)).cloned())
    }

    async fn insert(
        &self,
        _repo: &str,
        _table: &str,
        doc: QueryValue,
    ) -> Result<QueryValue, DbGatewayError> {
        let QueryValue::Map(mut map) = doc else {
            return Err(DbGatewayError::Execution("document must be a map".into()));
        };
        let mut rows = self.rows.lock().expect("rows lock");
        let id = i64::try_from(rows.len()).unwrap_or(0) + 1;
        map.insert("id".to_string(), QueryValue::Int(id));
        let stored = QueryValue::Map(map);
        rows.push(stored.clone());
        Ok(stored)
    }

    async fn query(
        &self,
        _repo: &str,
        _table: &str,
        filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, DbGatewayError> {
        let rows = self.rows.lock().expect("rows lock");
        Ok(rows
            .iter()
            .filter(|row| filter.as_ref().is_none_or(|wanted| matches(row, wanted)))
            .cloned()
            .collect())
    }

    async fn execute(&self, _request: &[u8]) -> Result<Vec<u8>, DbGatewayError> {
        Err(DbGatewayError::Execution("unused".into()))
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

/// Users: id 1 Ada (admin), id 2 Bob (user), id 3 Eve (admin).
fn users() -> Arc<MemoryDb> {
    Arc::new(MemoryDb {
        rows: Mutex::new(vec![
            map_of(&[
                ("id", int(1)),
                ("name", text("Ada")),
                ("role", text("admin")),
            ]),
            map_of(&[
                ("id", int(2)),
                ("name", text("Bob")),
                ("role", text("user")),
            ]),
            map_of(&[
                ("id", int(3)),
                ("name", text("Eve")),
                ("role", text("admin")),
            ]),
        ]),
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

fn params_from(entries: &[(&str, QueryValue)]) -> Params {
    let map: TMap<String, QueryValue> = entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    Params::from_map(map)
}

async fn call_with(
    function: &WasmFunction,
    db: Arc<MemoryDb>,
    params: &Params,
) -> Result<QueryValue, FunctionError> {
    let ctx = FnCtx::new().with_db(db, "main".to_string());
    function.call(&ctx, &FnBatch::new(), params).await
}

/// kind 3 marker helper; marker format derived from `raise.rs`:
/// `FunctionError::Compute(format!("guest codec: {message}"))`.
fn assert_codec(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest codec: {marker}"))),
        "expected codec marker {marker}, got {error}"
    );
}

const FIND: &str = "@entry procedure find(key: i64) -> i64 {\n\
     let user: { id: i64, name: string }? = db_get(\"users\", key);\n\
     if (is_null(user)) { return -1; }\n\
     return unwrap(user).id;\n\
     }\n";

#[tokio::test]
async fn db_get_returns_the_row_or_null() {
    let function = compiled(FIND).await;
    // derivation: key 2 matches the row with id 2, whose id field is 2; the
    // scalar key travels as a msgpack integer and `get` matches it with `id`.
    assert_eq!(
        call_with(&function, users(), &params_from(&[("key", int(2))]))
            .await
            .expect("present row"),
        int(2)
    );
    // derivation: key 9 matches nothing, the host answers zero (absent), the
    // helper returns the null cell and `is_null` selects the -1 branch.
    assert_eq!(
        call_with(&function, users(), &params_from(&[("key", int(9))]))
            .await
            .expect("absent row"),
        int(-1)
    );
}

#[tokio::test]
async fn db_get_with_a_record_key_matches_every_listed_field() {
    let function = compiled(
        "@entry procedure by_role(role: string) -> i64 {\n\
         let user: { id: i64 }? = db_get(\"users\", { role: role });\n\
         if (is_null(user)) { return -1; }\n\
         return unwrap(user).id;\n\
         }\n",
    )
    .await;
    // derivation: the predicate record is encoded as a msgpack map, `get`
    // returns the FIRST row with role admin (id 1); undeclared row fields
    // (name, role) are ignored by the projection.
    assert_eq!(
        call_with(&function, users(), &params_from(&[("role", text("admin"))]))
            .await
            .expect("first admin"),
        int(1)
    );
}

#[tokio::test]
async fn db_insert_returns_the_stored_record() {
    let function = compiled(
        "@entry procedure add(name: string) -> { id: i64, name: string } {\n\
         let stored: { id: i64, name: string } = db_insert(\"users\", { name: name });\n\
         return stored;\n\
         }\n",
    )
    .await;
    let db = users();
    // derivation: three rows exist, so the generated id is 4; the stored
    // record echoes the document plus `id`.
    assert_eq!(
        call_with(
            &function,
            Arc::clone(&db),
            &params_from(&[("name", text("Zed"))])
        )
        .await
        .expect("insert succeeds"),
        map_of(&[("id", int(4)), ("name", text("Zed"))])
    );
    assert_eq!(db.rows.lock().expect("rows lock").len(), 4);
}

#[tokio::test]
async fn db_query_filters_and_counts() {
    let by_role = compiled(
        "@entry procedure count(role: string) -> i64 {\n\
         let rows: [{ id: i64 }] = db_query(\"users\", { role: role });\n\
         return to_i64(len(rows));\n\
         }\n",
    )
    .await;
    // derivation: two rows have role admin (ids 1 and 3), one has role user.
    assert_eq!(
        call_with(&by_role, users(), &params_from(&[("role", text("admin"))]))
            .await
            .expect("admins"),
        int(2)
    );
    assert_eq!(
        call_with(&by_role, users(), &params_from(&[("role", text("user"))]))
            .await
            .expect("users"),
        int(1)
    );
    let all = compiled(
        "@entry procedure all() -> i64 {\n\
         let rows: [{ id: i64 }] = db_query(\"users\", null);\n\
         return to_i64(len(rows));\n\
         }\n",
    )
    .await;
    // derivation: a null filter sends zero bytes, which means every row.
    assert_eq!(
        call_with(&all, users(), &params_from(&[]))
            .await
            .expect("all rows"),
        int(3)
    );
}

#[tokio::test]
async fn several_calls_in_one_execution_reuse_the_runtime_context() {
    let function = compiled(
        "@entry procedure two(a: i64, b: i64) -> i64 {\n\
         let x: { id: i64 }? = db_get(\"users\", a);\n\
         let y: { id: i64 }? = db_get(\"users\", b);\n\
         let rows: [{ id: i64 }] = db_query(\"users\", null);\n\
         if (is_null(x) || is_null(y)) { return -1; }\n\
         return unwrap(x).id + unwrap(y).id + to_i64(len(rows));\n\
         }\n",
    )
    .await;
    // derivation: ids 1 and 3 plus three rows: 1 + 3 + 3 = 7.
    assert_eq!(
        call_with(
            &function,
            users(),
            &params_from(&[("a", int(1)), ("b", int(3))])
        )
        .await
        .expect("three calls"),
        int(7)
    );
}

#[tokio::test]
async fn helpers_may_call_the_database_under_a_procedure_entry() {
    let function = compiled(
        "fn name_len(key: i64) -> i32 {\n\
         let user: { name: string }? = db_get(\"users\", key);\n\
         if (is_null(user)) { return 0; }\n\
         return len(unwrap(user).name);\n\
         }\n\
         @entry procedure total(a: i64, b: i64) -> i64 {\n\
         return to_i64(name_len(a) + name_len(b));\n\
         }\n",
    )
    .await;
    // derivation: Ada and Bob are three characters each.
    assert_eq!(
        call_with(
            &function,
            users(),
            &params_from(&[("a", int(1)), ("b", int(2))])
        )
        .await
        .expect("helper db calls"),
        int(6)
    );
}

#[tokio::test]
async fn a_missing_required_field_in_the_answer_is_a_codec_error() {
    let function = compiled(
        "@entry procedure missing(key: i64) -> i64 {\n\
         let user: { id: i64, age: i64 }? = db_get(\"users\", key);\n\
         if (is_null(user)) { return -1; }\n\
         return unwrap(user).age;\n\
         }\n",
    )
    .await;
    // derivation: the rows carry no `age`; the decode-class projection
    // raises kind 3 with the `field` marker (not the kind-2 argument class).
    let error = call_with(&function, users(), &params_from(&[("key", int(1))]))
        .await
        .expect_err("missing field must fail");
    assert_codec(&error, "field");
}

#[tokio::test]
async fn a_wrongly_typed_field_in_the_answer_is_a_codec_error() {
    let function = compiled(
        "@entry procedure typed(key: i64) -> i64 {\n\
         let user: { name: i64 }? = db_get(\"users\", key);\n\
         if (is_null(user)) { return -1; }\n\
         return unwrap(user).name;\n\
         }\n",
    )
    .await;
    // derivation: `name` is a string in the row but declared i64.
    let error = call_with(&function, users(), &params_from(&[("key", int(1))]))
        .await
        .expect_err("wrong type must fail");
    assert_codec(&error, "arg");
}
