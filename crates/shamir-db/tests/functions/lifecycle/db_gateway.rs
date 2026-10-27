//! WASM functions reading/writing the database through the DbGateway
//! host imports.

use shamir_engine::function::Params;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;
use shamir_types::types::value::QueryValue;

use super::support::{compile_or_skip, exec_built, setup_db_with_people_table};

// ── Slice 8b: DB read/write from WASM functions ──────────────────────

/// Source for a function that inserts a doc into a table, queries it back,
/// and returns the row count. Proves both insert and query work from WASM.
const DB_INSERT_QUERY_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn save_and_count(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    let doc = Value::Map(vec![
        ("id".to_string(), Value::Int(1)),
        ("name".to_string(), Value::Str("neo".to_string())),
    ]);
    ctx.db().table("people").insert(doc)?;
    let rows = ctx.db().table("people").query(None)?;
    Ok(Value::Int(rows.len() as i64))
}
"#;

/// Source for a function that inserts a doc then reads it back by key.
const DB_GET_BY_KEY_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn insert_then_get(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    let doc = Value::Map(vec![
        ("id".to_string(), Value::Int(42)),
        ("name".to_string(), Value::Str("trinity".to_string())),
    ]);
    ctx.db().table("people").insert(doc)?;
    let key = Value::Map(vec![
        ("id".to_string(), Value::Int(42)),
    ]);
    match ctx.db().table("people").get(key) {
        Some(Value::Map(entries)) => {
            let name = entries.iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Null);
            Ok(name)
        }
        _ => Ok(Value::Null),
    }
}
"#;

/// A WASM function inserts a document and queries it back through the
/// `shamir_host` db_get/db_insert/db_query async host imports.
/// Then an independent `ShamirDb::execute` Read proves the write persisted.
#[tokio::test]
async fn wasm_function_inserts_and_queries() {
    let shamir = setup_db_with_people_table().await;

    if !compile_or_skip(&shamir, "save_and_count", DB_INSERT_QUERY_SRC).await {
        return;
    }

    // Invoke the function with db gateway wired up.
    let result = shamir
        .invoke_function_in_db("testdb", "main", "save_and_count", Params::new())
        .await
        .unwrap();

    assert_eq!(
        result,
        QueryValue::Int(1),
        "function should have seen exactly 1 row after insert + query"
    );

    // Independent verification: read the table directly via execute to prove
    // the write persisted (not just an in-memory echo inside the function).
    let mut b = Batch::new();
    b.id("verify");
    b.query("all", Query::from("people"));
    let resp = exec_built(
        &shamir,
        b.to_request_via_msgpack().expect("batch request encoding"),
    )
    .await;
    let records = &resp.results["all"].records;
    assert_eq!(
        records.len(),
        1,
        "independent read should find exactly 1 persisted row"
    );
    assert_eq!(records[0].get_value_i64("id"), Some(1));
    assert_eq!(records[0].get_value_str("name"), Some("neo"));
}

/// A WASM function inserts a doc then reads it back by key via `db_get`.
#[tokio::test]
async fn wasm_function_get_by_key() {
    let shamir = setup_db_with_people_table().await;

    if !compile_or_skip(&shamir, "insert_then_get", DB_GET_BY_KEY_SRC).await {
        return;
    }

    let result = shamir
        .invoke_function_in_db("testdb", "main", "insert_then_get", Params::new())
        .await
        .unwrap();

    assert_eq!(
        result,
        QueryValue::Str("trinity".to_string()),
        "get-by-key should return the name from the inserted record"
    );

    // Independent persistence check.
    let mut b = Batch::new();
    b.id("verify");
    b.query("all", Query::from("people"));
    let resp = exec_built(
        &shamir,
        b.to_request_via_msgpack().expect("batch request encoding"),
    )
    .await;
    let records = &resp.results["all"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_i64("id"), Some(42));
    assert_eq!(records[0].get_value_str("name"), Some("trinity"));
}
