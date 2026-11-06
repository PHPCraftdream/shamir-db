use crate::{
    admit_module, compile_rust_source_blocking, AdmissionLimits, AdmissionPolicy, FunctionError,
};

/// Compiles `source`, then asserts the admission gate set accepts the output.
/// Skips (prints `SKIP`) when the toolchain or target is unavailable.
async fn assert_corpus_entry_admitted(name: &str, source: &str) {
    let bytes = match compile_rust_source_blocking(source).await {
        Ok(bytes) => bytes,
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP {name}: {msg}");
            return;
        }
        Err(e) => panic!("{name}: compile failed: {e}"),
    };
    let report = admit_module(
        &bytes,
        &AdmissionPolicy::Admission(AdmissionLimits::default()),
    )
    .unwrap_or_else(|e| panic!("{name}: admission rejected compiled module: {e}"));
    assert_eq!(report.byte_size, bytes.len(), "{name}: byte_size mismatch");
}

const TRAIT_OBJECT_SOURCE: &str = r#"
use shamir::prelude::*;

trait Op {
    fn apply(&self, x: i64) -> i64;
}

struct Add(i64);
struct Mul(i64);

impl Op for Add {
    fn apply(&self, x: i64) -> i64 {
        x + self.0
    }
}

impl Op for Mul {
    fn apply(&self, x: i64) -> i64 {
        x * self.0
    }
}

#[shamir::function]
pub async fn trait_object(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    let ops: Vec<Box<dyn Op>> = vec![Box::new(Add(n)), Box::new(Mul(n))];
    let total: i64 = ops.iter().map(|op| op.apply(n)).sum();
    Ok(Value::Int(total))
}
"#;

const FN_POINTER_TABLE_SOURCE: &str = r#"
use shamir::prelude::*;

fn inc(x: i64) -> i64 {
    x + 1
}

fn dbl(x: i64) -> i64 {
    x * 2
}

fn neg(x: i64) -> i64 {
    -x
}

#[shamir::function]
pub async fn fn_pointer_table(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    let table: [fn(i64) -> i64; 3] = [inc, dbl, neg];
    let idx = (n.unsigned_abs() % 3) as usize;
    Ok(Value::Int(table[idx](n)))
}
"#;

const VEC_STRING_SOURCE: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn vec_string(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    let mut items: Vec<String> = Vec::new();
    for i in 0..n.clamp(0, 16) {
        items.push(format!("item-{i}"));
    }
    let joined: String = items.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",");
    let mut out = String::from("[");
    out.push_str(&joined);
    out.push(']');
    Ok(Value::Int(out.len() as i64))
}
"#;

const PANIC_PATH_SOURCE: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn panic_path(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    if n < 0 {
        panic!("neg");
    }
    Ok(Value::Int(n))
}
"#;

#[tokio::test]
async fn rust_corpus_trait_object_is_admitted() {
    assert_corpus_entry_admitted("rust_corpus_trait_object_is_admitted", TRAIT_OBJECT_SOURCE).await;
}

#[tokio::test]
async fn rust_corpus_fn_pointer_table_is_admitted() {
    assert_corpus_entry_admitted(
        "rust_corpus_fn_pointer_table_is_admitted",
        FN_POINTER_TABLE_SOURCE,
    )
    .await;
}

#[tokio::test]
async fn rust_corpus_vec_and_string_use_is_admitted() {
    assert_corpus_entry_admitted(
        "rust_corpus_vec_and_string_use_is_admitted",
        VEC_STRING_SOURCE,
    )
    .await;
}

#[tokio::test]
async fn rust_corpus_reachable_panic_path_is_admitted() {
    assert_corpus_entry_admitted(
        "rust_corpus_reachable_panic_path_is_admitted",
        PANIC_PATH_SOURCE,
    )
    .await;
}
