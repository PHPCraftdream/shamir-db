//! Fixtures shared by function-catalogue security regressions.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Security, Visibility};
use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::value::QueryValue;

use super::super::support::{setup, user};

pub(crate) const OWNER: u64 = 4101;
pub(crate) const WRITER: u64 = 4102;

pub(crate) fn options(security: Security, replace: bool) -> CreateFunctionOptions {
    CreateFunctionOptions {
        replace,
        visibility: Visibility::Public,
        security,
        ..CreateFunctionOptions::default()
    }
}

pub(crate) fn constant_wasm(value: QueryValue) -> Vec<u8> {
    let bytes = value.to_bytes().expect("encode constant WASM result");
    let data = bytes
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let len = bytes.len();
    let wat = format!(
        r#"(module
          (memory (export "memory") 2)
          (global $bump (mut i32) (i32.const 2048))
          (func (export "shamir_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          (func (export "shamir_call") (param i32 i32) (result i64)
            (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const {len})))
          (data (i32.const 1024) "{data}")
        )"#
    );
    wat::parse_str(wat).expect("parse constant-result WAT")
}

pub(crate) async fn create_wasm(
    db: &ShamirDb,
    name: &str,
    value: i64,
    actor: Actor,
    security: Security,
) {
    let wasm = constant_wasm(QueryValue::Int(value));
    db.create_function_with_opts_as(
        name,
        FunctionSource::Wasm(&wasm),
        options(security, false),
        actor,
    )
    .await
    .unwrap_or_else(|error| panic!("create {name}: {error}"));
}

pub(crate) async fn make_writeable(db: &ShamirDb, name: &str) {
    db.set_resource_meta(
        &shamir_types::access::ResourcePath::function(name),
        &ResourceMeta {
            owner: user(OWNER),
            group: None,
            mode: 0o707,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("grant other-write on {name}: {error}"));
}

pub(crate) async fn db_setup() -> ShamirDb {
    setup().await
}
