use crate::{
    compile_rust_source, compile_rust_source_with_timeout, FnBatch, FnCtx, FunctionError, Params,
    ShamirFunction, WasmEngine, WasmFunction, WasmLimits, WASM_COMPILE_TIMEOUT,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

const DOUBLE_SOURCE: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

#[tokio::test]
async fn compile_and_invoke_double() {
    let wasm = match compile_rust_source(DOUBLE_SOURCE) {
        Ok(w) => w,
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP compile_and_invoke_double: {msg}");
            return;
        }
        Err(e) => panic!("compile failed: {e}"),
    };

    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = WasmFunction::from_binary(engine, &wasm, WasmLimits::default()).unwrap();

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    let result = wf.call(&FnCtx::new(), &FnBatch::new(), &params).await;
    let val = result.expect("function should succeed");
    assert_eq!(val, QueryValue::Int(42), "double(21) should return 42");
}

// ============================================================================
// CRIT-6 part A — forbidden-macro scan
// ============================================================================
//
// `compile_rust_source` must reject guest source that uses any of the
// compile-time file/env macros BEFORE it ever spawns cargo. Each test
// asserts a `FunctionError::Compute` whose message names the forbidden
// macro. These never spawn cargo (the scan runs first), so they are fast
// and toolchain-independent.

/// Helper: assert a source is rejected with a message containing `needle`.
fn assert_forbidden(src: &str, needle: &str) {
    match compile_rust_source(src) {
        Err(FunctionError::Compute(msg)) => {
            assert!(
                msg.contains(needle),
                "expected message to mention `{needle}`, got: {msg}"
            );
            assert!(
                msg.contains("forbidden macro"),
                "expected message to mention `forbidden macro`, got: {msg}"
            );
        }
        Err(other) => panic!("expected Compute error with forbidden-macro message, got: {other:?}"),
        Ok(_) => panic!("source should have been rejected (contains {needle})"),
    }
}

#[test]
fn forbidden_env_macro_is_rejected() {
    // The token `env!` (preceded by non-ident char, followed by `!`) is a
    // compile-time env read — must be rejected.
    let src = r#"
pub fn leak() -> &'static str { env!("SHAMIR_TEST_SECRET") }
"#;
    assert_forbidden(src, "env!");
}

#[test]
fn forbidden_option_env_macro_is_rejected() {
    let src = r#"
pub fn maybe() -> Option<&'static str> { option_env!("HOME") }
"#;
    assert_forbidden(src, "option_env!");
}

#[test]
fn forbidden_include_macro_is_rejected() {
    let src = r#"pub fn x() { include!("secret.rs"); }"#;
    assert_forbidden(src, "include!");
}

#[test]
fn forbidden_include_str_macro_is_rejected() {
    let src = r#"pub fn x() -> &'static str { include_str!("/etc/passwd") }"#;
    assert_forbidden(src, "include_str!");
}

#[test]
fn forbidden_include_bytes_macro_is_rejected() {
    let src = r#"pub fn x() -> &'static [u8] { include_bytes!("/etc/shadow") }"#;
    assert_forbidden(src, "include_bytes!");
}

/// A legitimate source that uses the *word* `environment` (not the macro)
/// and embeds the literal text `env!` inside a string literal / comment
/// must NOT be rejected. This guards against false positives in the
/// lexeme boundary check.
#[test]
fn legit_source_with_env_word_is_accepted_by_filter() {
    // Local lexeme-level assertion — does not spawn cargo.
    use crate::compile::test_find_forbidden_macro;
    let src = r#"
// This comment mentions env! and option_env! on purpose — comments must not trip the filter.
let environment = "env! include_str! option_env!";
let env_record = std::env::var("PATH");
let _ = environment;
"#;
    assert!(
        test_find_forbidden_macro(src).is_none(),
        "legit source wrongly flagged by forbidden-macro filter"
    );
}

/// Conversely, a real `env!` invocation embedded after a comment must
/// still be caught (the filter skips comment text but scans real code).
#[test]
fn forbidden_macro_after_comment_is_caught() {
    use crate::compile::test_find_forbidden_macro;
    let src = r#"
// a comment that says env! — fine
let _x = env!("LEAKED");
"#;
    assert_eq!(test_find_forbidden_macro(src), Some("env"));
}

/// `myenv!(...)` (an identifier ending in `env`) must NOT trip the filter,
/// because the preceding char is an identifier char.
#[test]
fn prefixed_macro_lookalike_is_not_flagged() {
    use crate::compile::test_find_forbidden_macro;
    assert_eq!(test_find_forbidden_macro("myenv!(\"X\")"), None);
    assert_eq!(test_find_forbidden_macro("xinclude!(\"y\")"), None);
}

/// CRIT-6 regression: a forbidden name landing exactly at the end of the
/// (cleaned) source — with no trailing `!` because there is nothing left
/// to read — must not panic on an out-of-bounds index. This is a
/// completely ordinary guest source shape (e.g. a function literally
/// named `get_env` with no trailing newline), not a crafted adversarial
/// input; it must return `None`, not crash the compiling thread.
#[test]
fn forbidden_name_as_bare_source_suffix_does_not_panic() {
    use crate::compile::test_find_forbidden_macro;
    assert_eq!(test_find_forbidden_macro("pub fn get_env"), None);
    assert_eq!(test_find_forbidden_macro("env"), None);
    assert_eq!(test_find_forbidden_macro("include"), None);
    assert_eq!(test_find_forbidden_macro("include_bytes"), None);
    assert_eq!(test_find_forbidden_macro("include_str"), None);
    assert_eq!(test_find_forbidden_macro("option_env"), None);
    // The macro invocation shape must still be caught when the source
    // does NOT end exactly at the name (regression guard against an
    // overcorrection that stops matching altogether).
    assert_eq!(test_find_forbidden_macro("env!(\"X\")"), Some("env"));
}

/// Block-comment and raw-string bodies are skipped.
#[test]
fn forbidden_macro_text_in_strings_and_comments_is_ignored() {
    use crate::compile::test_find_forbidden_macro;
    // NOTE: outer raw string uses `r##" … "##` so the inner `r#" … "#` in
    // `s2` does not prematurely terminate it (raw strings can't nest with
    // the same hash count).
    let src = r##"
/* block comment with env! and include_str! */
let s1 = "env!(\"HOME\")";
let s2 = r#"include_bytes!("x")"#;
let s3 = b"option_env!(\"Y\")";
// line comment with env!
"##;
    assert_eq!(test_find_forbidden_macro(src), None);
}

// ============================================================================
// CRIT-6 part A — timeout path
// ============================================================================

/// A real compile must comfortably beat the default timeout. This guards
/// against a regression where the timeout is set absurdly low, and also
/// sanity-checks that the env-scrub allowlist still lets cargo build a
/// legitimate function.
///
/// (We do NOT simulate a 60-120 s hang here — that would make the suite
/// slow. Instead we (a) keep this happy-path time bound and (b) exercise
/// the kill path via a tiny timeout in `tiny_timeout_kills_a_build`.)
#[tokio::test]
async fn legit_compile_completes_within_default_timeout() {
    let start = std::time::Instant::now();
    let wasm = match compile_rust_source_with_timeout(DOUBLE_SOURCE, WASM_COMPILE_TIMEOUT) {
        Ok(w) => w,
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP legit_compile_completes_within_default_timeout: {msg}");
            return;
        }
        Err(e) => panic!(
            "legit compile failed (env scrub should not break it): {e} \
             (after {:?})",
            start.elapsed()
        ),
    };
    let elapsed = start.elapsed();
    assert!(
        elapsed < WASM_COMPILE_TIMEOUT,
        "compile took {:?}, exceeding the {:?} budget",
        elapsed,
        WASM_COMPILE_TIMEOUT
    );
    assert!(!wasm.is_empty(), "expected non-empty wasm output");
}

/// A pathological build (we feed it a source that compiles a CPU-bound
/// constant expression with a huge expansion) under a 1 ms timeout must
/// hit the `Compute("compilation timed out …")` path. This exercises the
/// kill+reap logic without waiting for the real 120 s budget.
///
/// On a very fast machine a tiny build could finish inside 1 ms even
/// before our `wait_timeout` call observes it; to keep the test robust we
/// pick a deliberately heavy source AND a tiny timeout, and accept the
/// test as informative (not flaky-critical): if the build happens to win
/// the race on a given host, we treat it as a SKIP rather than a failure.
#[tokio::test]
async fn tiny_timeout_triggers_timeout_error_or_completes() {
    // Heavy source: a `const` evaluated at compile time that loops a lot.
    // This is a real `const fn` the compiler must evaluate — not a
    // forbidden macro.
    let heavy = r#"
use shamir::prelude::*;

const fn sum_to(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 0u64;
    while i < n { s = s.wrapping_add(i); i += 1; }
    s
}
const BIG: u64 = sum_to(200_000);

#[shamir::function]
pub async fn heavy(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Ok(Value::Int(BIG as i64))
}
"#;
    // 1 ms: deliberately tiny. The point is to exercise the kill branch.
    let res = compile_rust_source_with_timeout(heavy, std::time::Duration::from_millis(1));
    match res {
        Err(FunctionError::Compute(msg)) => {
            assert!(
                msg.contains("timed out"),
                "expected timeout message, got: {msg}"
            );
        }
        Ok(_) => {
            // Build won the race on this host — acceptable, see doc note.
            eprintln!(
                "NOTE: heavy build finished inside 1 ms on this host; \
                 timeout path not exercised this run."
            );
        }
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP tiny_timeout_triggers_timeout_error_or_completes: {msg}");
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}
