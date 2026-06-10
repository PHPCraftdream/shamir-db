//! Shared helpers for ddl_wire_e2e test modules.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// WAT helpers — build WASM modules that return baked msgpack bytes
// ═══════════════════════════════════════════════════════════════════════

/// WAT module that ignores input and returns msgpack `null` (0xC0) = valid.
const ACCEPT_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (data (i32.const 512) "\c0")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const 1)
    )
  )
)
"#;

pub(super) fn accept_wasm() -> Vec<u8> {
    wat::parse_str(ACCEPT_WAT).expect("WAT parse failed")
}

/// Build a WAT module whose `shamir_call` returns the given `QueryValue`
/// serialised as msgpack.
pub(super) fn make_wat_returning(value: &QueryValue) -> Vec<u8> {
    let bytes = rmp_serde::to_vec(value).expect("msgpack encode");
    let hex_data: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
    let len = bytes.len();

    let wat = format!(
        r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (data (i32.const 512) "{hex_data}")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const {len})
    )
  )
)
"#
    );

    wat::parse_str(&wat).expect("generated WAT parse failed")
}

/// Build a `QueryValue` for a single-error rejection.
pub(super) fn rejection_single_error() -> QueryValue {
    let mut error_item = new_map();
    error_item.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("age".to_owned())]),
    );
    error_item.insert("code".to_owned(), QueryValue::Str("too_young".to_owned()));

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(false));
    QueryValue::Map(root)
}

pub(super) fn wasm_b64(wasm: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(wasm)
}

// ═══════════════════════════════════════════════════════════════════════
// Setup helper
// ═══════════════════════════════════════════════════════════════════════

pub(super) async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}
