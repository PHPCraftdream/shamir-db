//! Database calls from a WASM procedure (manual runs only, never part of the regular CI).
//!
//! Each `wasm/*` cell invokes a stored procedure that calls one `shamir_host` DB import
//! [`CALLS`] times (`db_get`, filtered and unfiltered `db_query`, `db_insert`, `db_execute`)
//! against an in-memory repository with [`ROWS`] indexed rows. The `direct/*` cells run the
//! same read through `ShamirDb::execute` with no guest, so
//! `wasm/* − direct/*` is what the guest boundary and the gateway add.
//!
//! The suite is sized to stay within a few seconds; counts are pinned in `bench-iters.txt`
//! (`-- --calibrate 0.05` regenerates them).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_collections::new_map;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_engine::function::Params;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Runtime;

const DB: &str = "benchdb";
const REPO: &str = "main";
const TABLE: &str = "people";
/// Rows seeded into the table.
const ROWS: usize = 200;
/// Host calls per invocation in the per-call cells.
const CALLS: u32 = 20;
/// Host calls per invocation of the unfiltered (all rows) query.
const CALLS_ALL: u32 = 2;
/// The row every point read targets.
const TARGET_ID: i64 = 5;

fn person(i: usize) -> QueryValue {
    let mut m = new_map();
    m.insert("id".into(), QueryValue::Int(i as i64));
    m.insert("name".into(), QueryValue::Str(format!("name{i}")));
    m.insert("age".into(), QueryValue::Int(18 + (i % 60) as i64));
    QueryValue::Map(m)
}

fn key_map(id: i64) -> QueryValue {
    let mut m = new_map();
    m.insert("id".into(), QueryValue::Int(id));
    QueryValue::Map(m)
}

async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db(DB).await.expect("create_db");
    let cfg = RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE));
    shamir.add_repo(DB, cfg).await.expect("add_repo");

    let rows: Vec<QueryValue> = (0..ROWS).map(person).collect();
    let mut seed = Batch::new();
    seed.id("seed")
        .return_flagged()
        .insert("ins", write::insert(TABLE).rows(rows));
    shamir.execute(DB, &seed.build()).await.expect("seed");

    let mut index = Batch::new();
    index
        .id("idx")
        .create_index("i", ddl::create_index("by_id", TABLE).field("id").build());
    shamir.execute(DB, &index.build()).await.expect("index");
    shamir
}

/// `\xx` WAT escapes for a data segment.
fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

/// Guest that calls one DB import `calls` times and returns the last answer. The table name,
/// the key/filter/doc and the batch request sit in data segments at fixed addresses.
fn guest(import: &str, call: &str, calls: u32, data: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let segments: String = data
        .iter()
        .map(|(addr, bytes)| format!("(data (i32.const {addr}) \"{}\")\n  ", wat_bytes(bytes)))
        .collect();
    let wat = format!(
        r#"(module
  {import}
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 4096))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  {segments}
  (func (export "shamir_call") (param $p i32) (param $l i32) (result i64)
    (local $i i32) (local $last i64)
    (block $done (loop $loop
      (br_if $done (i32.ge_u (local.get $i) (i32.const {calls})))
      (local.set $last {call})
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $loop)))
    (local.get $last)))"#
    );
    wat::parse_str(&wat).expect("valid guest wat")
}

const TABLE_ADDR: u32 = 1024;
const ARG_ADDR: u32 = 1100;

fn import4(name: &str) -> String {
    format!(r#"(import "shamir_host" "{name}" (func $f (param i32 i32 i32 i32) (result i64)))"#)
}

fn call4(arg_len: usize) -> String {
    format!(
        "(call $f (i32.const {TABLE_ADDR}) (i32.const {}) (i32.const {ARG_ADDR}) (i32.const {arg_len}))",
        TABLE.len()
    )
}

/// Runs `fut` on a worker thread of `rt` (large stack) and waits for it.
fn run<F>(rt: &Runtime, fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = rt.handle().clone();
    rt.block_on(async move { handle.spawn(fut).await.expect("task panicked") })
}

fn main() {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(8 * 1024 * 1024)
            .enable_all()
            .build()
            .expect("runtime"),
    );
    let shamir = run(&rt, fresh_db());

    let key = key_map(TARGET_ID).to_bytes().expect("key bytes").to_vec();
    let doc = person(ROWS + 1).to_bytes().expect("doc bytes").to_vec();
    let mut read = Batch::new();
    read.id("r")
        .query("r", Query::with_repo(REPO, TABLE).where_eq("id", TARGET_ID));
    let read_request = read.build();
    let execute_bytes = rmp_serde::to_vec_named(&read_request).expect("request bytes");

    let table = TABLE.as_bytes().to_vec();
    let functions: Vec<(String, Vec<u8>)> = vec![
        (
            "db_get".to_owned(),
            guest(
                &import4("db_get"),
                &call4(key.len()),
                CALLS,
                &[(TABLE_ADDR, table.clone()), (ARG_ADDR, key.clone())],
            ),
        ),
        (
            "db_query_filtered".to_owned(),
            guest(
                &import4("db_query"),
                &call4(key.len()),
                CALLS,
                &[(TABLE_ADDR, table.clone()), (ARG_ADDR, key.clone())],
            ),
        ),
        (
            "db_query_all".to_owned(),
            guest(
                &import4("db_query"),
                &call4(0),
                CALLS_ALL,
                &[(TABLE_ADDR, table.clone())],
            ),
        ),
        (
            "db_insert".to_owned(),
            guest(
                &import4("db_insert"),
                &call4(doc.len()),
                CALLS,
                &[(TABLE_ADDR, table.clone()), (ARG_ADDR, doc.clone())],
            ),
        ),
        (
            "db_execute".to_owned(),
            guest(
                r#"(import "shamir_host" "db_execute" (func $f (param i32 i32) (result i64)))"#,
                &format!(
                    "(call $f (i32.const {ARG_ADDR}) (i32.const {}))",
                    execute_bytes.len()
                ),
                CALLS,
                &[(ARG_ADDR, execute_bytes.clone())],
            ),
        ),
    ];
    for (name, wasm) in &functions {
        let shamir = shamir.clone();
        let name = name.clone();
        let wasm = wasm.clone();
        run(&rt, async move {
            shamir
                .create_function_from_wasm(&name, &wasm, false)
                .await
                .expect("create function");
            // One call up front: a guest that traps would otherwise only fail inside the timing loop.
            shamir
                .invoke_function_in_db(DB, REPO, &name, Params::new())
                .await
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        });
    }

    let mut h = Harness::new("wasm_db_calls", env!("CARGO_MANIFEST_DIR"));
    for (name, calls) in [
        ("db_get", CALLS),
        ("db_query_filtered", CALLS),
        ("db_query_all", CALLS_ALL),
        ("db_insert", CALLS),
        ("db_execute", CALLS),
    ] {
        let shamir = shamir.clone();
        let rt = rt.clone();
        h.bench(&format!("wasm/{name}_x{calls}"), move || {
            let shamir = shamir.clone();
            black_box(run(&rt, async move {
                shamir
                    .invoke_function_in_db(DB, REPO, name, Params::new())
                    .await
                    .expect("invoke")
            }));
        });
    }

    {
        let shamir = shamir.clone();
        let rt = rt.clone();
        h.bench(&format!("direct/execute_read_x{CALLS}"), move || {
            let shamir = shamir.clone();
            let request = read_request.clone();
            run(&rt, async move {
                for _ in 0..CALLS {
                    black_box(shamir.execute(DB, &request).await.expect("execute"));
                }
            });
        });
    }

    h.run();
}
