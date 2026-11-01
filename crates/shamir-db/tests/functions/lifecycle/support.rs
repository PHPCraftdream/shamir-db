//! Shared helpers for the function lifecycle end-to-end tests.

use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_engine::function::Params;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_storage::error::DbError;
use shamir_types::types::value::QueryValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Identity-echo WAT matching the slice-2 ABI.
///
/// Exports `memory` (2 pages), `shamir_alloc` (bump allocator), and
/// `shamir_call` which echoes `[ptr, len)` back as the packed result.
const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

pub(crate) fn echo_wasm() -> Vec<u8> {
    wat::parse_str(ECHO_WAT).unwrap()
}

pub(crate) fn make_echo_params() -> Params {
    let mut params = Params::new();
    params.set("x", QueryValue::Int(7));
    params.set("name", QueryValue::Str("hi".to_string()));
    params
}

/// Open a redb-backed `ShamirDb`, tolerating the brief window where a
/// just-dropped previous instance's redb file lock is still being released
/// (synchronous on Windows, can lag on Linux). Bounded retry (~5s).
pub(crate) async fn init_redb_retry(path: &std::path::Path) -> ShamirDb {
    for _ in 0..50 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.to_path_buf())).await {
            Ok(db) => return db,
            Err(e)
                if {
                    let m = e.to_string();
                    m.contains("Cannot acquire lock")
                        || m.contains("already open")
                        || m.contains("Locked")
                } =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("unexpected reopen error: {e}"),
        }
    }
    panic!("redb lock was not released within the retry window");
}

pub(crate) const DOUBLE_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

/// Helper: compile + register a source function, skipping on toolchain
/// unavailable.
pub(crate) async fn compile_or_skip(db: &ShamirDb, name: &str, source: &str) -> bool {
    match db.create_function_from_source(name, source, false).await {
        Ok(()) => true,
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP {name} — toolchain unavailable: {msg}");
            false
        }
        Err(e) => panic!("compile {name} failed: {e}"),
    }
}

/// Helper: set up an in-memory ShamirDb with database "testdb", repo "main",
/// and a table "people" ready for inserts/queries.
pub(crate) async fn setup_db_with_people_table() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");

    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["people"]),
    );
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();
    shamir
}

pub(crate) async fn exec_built(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

/// Start a hand-rolled mock HTTP server on `127.0.0.1:0`, return the
/// actual port assigned by the OS. The server reads one request (up to
/// the blank line) and writes a fixed response.
pub(crate) async fn start_mock_http_server(response_body: &[u8]) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server bind");
    let port = listener.local_addr().unwrap().port();
    let body = response_body.to_vec();

    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let mut received = 0;
            while received < buf.len() && !buf[..received].windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf[received..]).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => received += n,
                }
            }
            if !buf[..received].windows(4).any(|w| w == b"\r\n\r\n") {
                return;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.flush().await;
            // Give the client a moment to read before we drop.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    port
}

/// Check if `curl` is available on PATH.
pub(crate) fn curl_available() -> bool {
    std::process::Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
