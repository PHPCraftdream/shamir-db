//! Shared fixtures for the function facade tests.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use shamir_engine::function::Params;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::access::ResourceMeta;
use crate::shamir_db::shamir_db::{ArtifactKind, KIND_FIELD};
use crate::shamir_db::{ShamirDb, SystemStoreConfig};

/// Contract echo module: memory export, bump allocator, `shamir_call`
/// returning ptr/len unchanged.
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

/// Bound for the clean-shutdown flush; the per-test timeout is the hang detector.
const FLUSH_BUDGET: Duration = Duration::from_secs(3600);

pub(super) fn echo_wasm() -> Vec<u8> {
    wat::parse_str(ECHO_WAT).expect("echo wat")
}

/// Contract-valid module whose bytes (and hash) differ from [`echo_wasm`].
pub(super) fn echo_variant_wasm() -> Vec<u8> {
    let text = ECHO_WAT.replace(
        r#"(memory (export "memory") 2)"#,
        r#"(memory (export "memory") 3)"#,
    );
    wat::parse_str(text).expect("variant wat")
}

/// Echo module plus an unused SIMD function, rejected under every policy.
pub(super) fn simd_wasm() -> Vec<u8> {
    let text = ECHO_WAT.replacen(
        "(global $bump",
        "(func $unused (drop (v128.const i64x2 0 0)))\n  (global $bump",
        1,
    );
    wat::parse_str(text).expect("simd wat")
}

fn push_leb128(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = u8::try_from(value & 0x7f).expect("7 bits");
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Appends a custom section (id 0, LEB128 size, name, payload) to a module.
pub(super) fn append_custom_section(bytes: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
    let mut section = Vec::new();
    push_leb128(&mut section, name.len());
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(payload);
    let mut out = bytes.to_vec();
    out.push(0);
    push_leb128(&mut out, section.len());
    out.extend_from_slice(&section);
    out
}

/// Echo module declaring ABI 1 and origin `external`.
pub(super) fn annotated_wasm() -> Vec<u8> {
    let with_abi = append_custom_section(&echo_wasm(), "shamir.abi", &1u32.to_le_bytes());
    append_custom_section(&with_abi, "shamir.origin", b"external")
}

pub(super) fn echo_params() -> Params {
    let mut params = Params::new();
    params.set("x", QueryValue::Int(7));
    params.set("name", QueryValue::Str("hi".to_string()));
    params
}

/// What the echo module returns for `p`.
pub(super) fn echo_result(p: &Params) -> QueryValue {
    QueryValue::Map(p.raw().clone())
}

/// Open a Fjall-backed db, retrying while a dropped instance still holds the lock.
pub(super) async fn open_fjall(path: &Path) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.to_path_buf())).await {
            Ok(db) => return db,
            Err(e) if e.to_string().contains("Locked") => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("open failed: {e}"),
        }
    }
    panic!("fjall lock was not released within the retry window");
}

/// Flush, drop `db` and reopen the same store.
pub(super) async fn restart(db: ShamirDb, path: &Path) -> ShamirDb {
    db.system_store()
        .flush_buffers_with_quiesce_timeout(FLUSH_BUDGET)
        .await
        .expect("flush before restart");
    drop(db);
    open_fjall(path).await
}

/// The pre-composite `wasm_hash` algorithm, computed independently.
pub(super) fn legacy_fx_hash(bytes: &[u8]) -> String {
    let v: Vec<u8> = bytes.to_vec();
    let mut h = rustc_hash::FxHasher::default();
    v.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Write a catalogue row directly, bypassing create.
pub(super) async fn put_row(
    db: &ShamirDb,
    name: &str,
    wasm: &[u8],
    extra: Vec<(&str, QueryValue)>,
) {
    let mut row = new_map();
    row.insert("name".into(), QueryValue::Str(name.into()));
    row.insert("wasm_b64".into(), QueryValue::Str(b64(wasm)));
    row.insert("lang".into(), QueryValue::Str("wasm".into()));
    row.insert("source".into(), QueryValue::Null);
    row.insert("version".into(), QueryValue::Int(1));
    row.insert(KIND_FIELD.into(), ArtifactKind::Wasm.as_query_value());
    for (key, value) in extra {
        row.insert(key.into(), value);
    }
    db.system_store()
        .save_function(name, &QueryValue::Map(row), &ResourceMeta::open())
        .await
        .expect("save row");
}

/// Replace the stored module bytes, leaving every other field (hash included).
pub(super) async fn tamper_row(db: &ShamirDb, name: &str, new_wasm: &[u8]) {
    let mut row = stored_row(db, name).await;
    if let QueryValue::Map(map) = &mut row {
        map.insert("wasm_b64".into(), QueryValue::Str(b64(new_wasm)));
    }
    let meta = ResourceMeta::from_record(&row);
    db.system_store()
        .save_function(name, &row, &meta)
        .await
        .expect("save tampered row");
}

/// The stored catalogue row of `name`.
pub(super) async fn stored_row(db: &ShamirDb, name: &str) -> QueryValue {
    db.system_store()
        .load_function(name)
        .await
        .expect("load row")
        .expect("row exists")
}

/// A copy of `row` without `key`.
pub(super) fn without_key(row: &QueryValue, key: &str) -> QueryValue {
    let mut copy = row.clone();
    if let QueryValue::Map(map) = &mut copy {
        map.shift_remove(key);
    }
    copy
}

/// Create `name` from the echo module as the system actor.
pub(super) async fn create_echo(db: &ShamirDb, name: &str) {
    db.create_function_from_wasm(name, &echo_wasm(), false)
        .await
        .expect("create echo");
}
