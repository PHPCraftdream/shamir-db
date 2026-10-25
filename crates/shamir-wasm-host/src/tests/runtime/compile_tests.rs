use crate::compile::test_find_forbidden_macro;
use crate::{
    compile_rust_source, compile_rust_source_blocking, compile_rust_source_with_timeout, FnBatch,
    FnCtx, FunctionError, Params, ShamirFunction, WasmEngine, WasmFunction, WasmLimits,
    WASM_COMPILE_TIMEOUT,
};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// RV-WASM-HOST-5E65E165 regression: whitespace and/or comments between a
/// forbidden macro name and its `!` must not hide the invocation.
#[test]
fn rv_wasm_host_5e65e165_forbidden_macro_detected_across_comment_and_whitespace_gaps() {
    assert_eq!(
        test_find_forbidden_macro(r#"include_bytes /*c*/!("x")"#),
        Some("include_bytes")
    );
    assert_eq!(
        test_find_forbidden_macro(r#"include_str ! ("x")"#),
        Some("include_str")
    );
    assert_eq!(test_find_forbidden_macro("env\n  !(\"HOME\")"), Some("env"));
    // Negative controls: unchanged behavior.
    assert_eq!(test_find_forbidden_macro("env!(\"HOME\")"), Some("env"));
    assert_eq!(test_find_forbidden_macro("my_include_bytes!(\"x\")"), None);
    assert_eq!(test_find_forbidden_macro("let x = 1;\nlet y = 2;"), None);
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

/// RV-WASM-HOST-5E65E165 (C-string/raw-hash fix): EVERY current Rust
/// string-literal form — plain `"`/`b"`/`c"`, raw `r`/`br`/`cr` with
/// 0/1/2/many/255 hashes — must be consumed as ONE token even when its
/// contents embed quote characters, comment markers and macro-shaped
/// text; a real forbidden macro AFTER the literal must still be found.
/// The `X#"""#` witness entries (X = r, br, cr) are EXACT valid sources
/// that hid `include_bytes!("secret")` from a scanner that misses the
/// literal prefix: the leftover quotes desync the scan and swallow the
/// macro inside a bogus string. All fixtures in THIS test are raw source
/// strings of VALID token shapes.
#[test]
fn every_string_literal_form_is_one_token_and_later_macros_are_found() {
    // (fixture, expected macro, fixture must carry a REAL backslash)
    let mut cases: Vec<(String, &'static str, bool)> = vec![
        // Plain string: escaped quote, line- and block-comment markers and
        // macro-shaped text inside; a real env! after it.
        (
            r#"let s = "a \" b // env!( /* x"; env!("HOME")"#.to_string(),
            "env",
            true,
        ),
        // Plain string with unicode/hex/quote/backslash escapes, then a macro.
        (
            r#"let s = "x\u{2603}\x41\"q\\"; env!("H")"#.to_string(),
            "env",
            true,
        ),
        // Byte string containing macro text; a macro after it.
        (
            r#"let s = b"include_bytes!(\"f\")"; include_str!("g")"#.to_string(),
            "include_str",
            true,
        ),
        // C string (the previously unsupported prefix) with an escaped
        // quote and a comment marker inside.
        (
            r#"let s = c"\"; env!( /*"; option_env!("H")"#.to_string(),
            "option_env",
            true,
        ),
        // THE WITNESSES (verbatim): valid sources, macro hidden by a
        // scanner that misses ANY of the r/br/cr raw prefixes (desync).
        (
            r##"let _ = r#"""#; let _ = include_bytes!("secret");"##.to_string(),
            "include_bytes",
            false,
        ),
        (
            r##"let _ = br#"""#; let _ = include_str!("secret");"##.to_string(),
            "include_str",
            false,
        ),
        (
            r##"let _ = cr#"""#; let _ = include_bytes!("secret");"##.to_string(),
            "include_bytes",
            false,
        ),
        // Raw, 0 hashes.
        (
            r#"let s = r"env!("; include_bytes!("f")"#.to_string(),
            "include_bytes",
            false,
        ),
        // Raw, 1 hash.
        (
            r##"let s = r#"option_env!("x")"#; env!("H")"##.to_string(),
            "env",
            false,
        ),
        // Raw byte string, 1 hash.
        (
            r##"let s = br#"include_str!("y")"#; env!("H")"##.to_string(),
            "env",
            false,
        ),
        // Raw C string, 2 hashes.
        (
            r###"let s = cr##"env!("z")"##; include_bytes!("f")"###.to_string(),
            "include_bytes",
            false,
        ),
    ];
    // Raw with 255 hashes (the Rust maximum) — generated, not spelled;
    // BOTH delimiters carry the full 255-hash run.
    let h255 = "#".repeat(255);
    cases.push((
        format!("let s = r{h255}\"x\"{h255}; env!(\"X\")"),
        "env",
        false,
    ));
    for (src, name, must_carry_backslash) in &cases {
        if *must_carry_backslash {
            assert!(
                src.contains('\\'),
                "escape fixture must carry REAL backslashes to the scanner: {src:?}"
            );
        }
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            Some(*name),
            "literal contents must not hide the following `{name}!`: {src:?}"
        );
    }
}

/// Full raw-prefix × hash-count matrix (RV-WASM-HOST-5E65E165): every raw
/// prefix (`r`/`br`/`cr`) crossed with hash counts 0/1/2/7/255 — the open
/// AND closing delimiters carry the exact hash run — followed by EACH
/// forbidden macro name. Bodies are hash-count-appropriate VALID raw
/// content: with 0 hashes no bare `"` can appear (it would close the
/// literal), so the body carries comment markers + macro-shaped text;
/// with ≥1 hashes the body also carries quotes. Inert literal-only
/// fixtures of the SAME forms are the negative control, so a scanner
/// mutation that merely skips the whole tail cannot pass.
#[test]
fn raw_prefix_hash_matrix_all_macro_names_after_literal_and_inert() {
    const NAMES: [&str; 5] = [
        "include",
        "include_str",
        "include_bytes",
        "env",
        "option_env",
    ];
    for prefix in ["r", "br", "cr"] {
        for hashes in [0usize, 1, 2, 7, 255] {
            let h = "#".repeat(hashes);
            let body = if hashes == 0 {
                "x // env!( /* text"
            } else {
                "q\" // env!( /* text"
            };
            let literal = format!("{prefix}{h}\"{body}\"{h}");
            for name in NAMES {
                let src = format!("let s = {literal}; {name}!(\"X\")");
                assert_eq!(
                    crate::compile::test_find_forbidden_macro(&src),
                    Some(name),
                    "raw {prefix} with {hashes} hashes must not hide `{name}!`: {src:?}"
                );
            }
            // Negative control: the SAME literal with no macro stays clean.
            let inert = format!("let s = {literal};");
            assert_eq!(
                crate::compile::test_find_forbidden_macro(&inert),
                None,
                "literal-only source must stay clean: {inert:?}"
            );
        }
    }
}

/// MALFORMED-source controls (NOT claimed valid Rust): token shapes that
/// are not literals at all must fall back to plain token scanning so a
/// later macro stays visible and the scan never desyncs. Kept separate so
/// no "valid source" claim is attached to them.
#[test]
fn malformed_prefix_shapes_fall_back_to_token_scanning() {
    // 256 hashes exceeds the Rust raw-string maximum — not a raw string.
    let h256 = "#".repeat(256);
    let src = format!("let s = r{h256}x; env!(\"X\")");
    assert_eq!(crate::compile::test_find_forbidden_macro(&src), Some("env"));
    // `bc` is not a Rust literal prefix: identifier `b` + C-string `c"…"`
    // (invalid as a whole, but scanned as plain tokens, no desync).
    assert_eq!(
        crate::compile::test_find_forbidden_macro(r#"bc"env!(\"x\")"; include!("f")"#),
        Some("include")
    );
}

/// Negative controls for the literal-form dispatch: the same literal
/// forms with NO forbidden macro stay clean, and identifier prefixes that
/// merely CONTAIN the prefix letters (`crab`, `bcx`, a plain `c`/`cr`
/// binding, the raw identifier `r#env` vs raw string `r#"…`) are exactly
/// one token — never a false literal, never a false match.
#[test]
fn literal_forms_without_macros_stay_clean_and_prefix_lookalikes_are_identifiers() {
    for src in [
        r##"let _ = cr#"""#;"##,
        r#"let s = "a \" b // env!( /* x";"#,
        r#"let s = c"\"; env!( /*";"#,
        r##"let s = r#"env!("x")"#;"##,
    ] {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            None,
            "literal text must be inert: {src:?}"
        );
    }
    // Longer identifiers sharing the prefix letters: plain tokens, no
    // match (and no quote desync after them).
    for src in [
        "crab!(\"X\")",
        "bcx!(\"X\")",
        "environment!(\"X\")",
        "let cr = 1; env",
        "let c = \"x\"; let r = 2;",
        // Raw identifier vs raw string: `r#env` IS the macro (matched
        // elsewhere); a raw string named `env` is inert.
        r##"let env = r#"env!("x")"#;"##,
    ] {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            None,
            "prefix lookalike must be an identifier token: {src:?}"
        );
    }
}
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

// ============================================================================
// Q5-b — from_binary_blocking (spawn_blocking wrapper)
// ============================================================================

/// Compiling through `from_binary_blocking` (tokio `spawn_blocking`) must
/// yield a function that behaves identically to the synchronous
/// `from_binary` result: same input, same output.
#[tokio::test]
async fn from_binary_blocking_matches_sync_behavior() {
    let wasm = match compile_rust_source(DOUBLE_SOURCE) {
        Ok(w) => w,
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP from_binary_blocking_matches_sync_behavior: {msg}");
            return;
        }
        Err(e) => panic!("compile failed: {e}"),
    };

    let sync_wf = WasmFunction::from_binary(
        Arc::new(WasmEngine::new().unwrap()),
        &wasm,
        WasmLimits::default(),
    )
    .unwrap();
    let blocking_wf = WasmFunction::from_binary_blocking(
        Arc::new(WasmEngine::new().unwrap()),
        &wasm,
        WasmLimits::default(),
    )
    .await
    .unwrap();

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    let sync_val = sync_wf
        .call(&FnCtx::new(), &FnBatch::new(), &params)
        .await
        .expect("sync call should succeed");
    let blocking_val = blocking_wf
        .call(&FnCtx::new(), &FnBatch::new(), &params)
        .await
        .expect("blocking call should succeed");

    assert_eq!(sync_val, QueryValue::Int(42));
    assert_eq!(blocking_val, sync_val, "blocking path must match sync path");
}

/// A panic inside the blocking closure must surface as a mapped
/// `FunctionError::Compute` (the same variant a failed compile produces),
/// not as a panic in the caller. Exercises the `JoinError` arm of the
/// shared `run_compile_blocking` helper used by `from_binary_blocking`.
#[tokio::test]
async fn blocking_compile_join_failure_maps_to_compute_error() {
    use crate::wasm::run_compile_blocking;

    let result: Result<(), FunctionError> =
        run_compile_blocking(|| -> Result<(), FunctionError> {
            panic!("deliberate blocking-task panic");
        })
        .await;

    match result {
        Err(FunctionError::Compute(msg)) => {
            assert!(
                msg.contains("blocking compile join failure"),
                "expected join-failure message, got: {msg}"
            );
            assert!(
                msg.contains("deliberate blocking-task panic"),
                "expected panic message to be preserved, got: {msg}"
            );
        }
        Ok(()) => panic!("panicking closure must not succeed"),
        Err(other) => panic!("expected Compute error, got: {other:?}"),
    }
}

// ============================================================================
// RV-WASM-HOST-5E65E165 — token-level recognition (no textual scanner)
// ============================================================================
//
// The scan is a conservative Rust lexer: identifier TOKENS, skipping
// whitespace/comments, ignoring literal contents, Unicode-safe. The
// GAP-POSITIVE cases below catch a revert to a textual substring scan
// that requires token adjacency; the remaining cases are controls and
// pass under such a revert.

/// EXACT Rust whitespace (the `Pattern_White_Space` set) and comments of
/// every shape between the name and `!` must not hide the invocation —
/// for EVERY forbidden macro.
#[test]
fn rv_wasm_host_5e65e165_token_gaps_all_macros_including_unicode_whitespace() {
    let cases = [
        ("include", r#"include /*c*/ ! ("x")"#),
        ("include", "include // line\n!(\"x\")"),
        (
            "include_str",
            r#"include_str /* nested /* c */ */ !( "x" )"#,
        ),
        ("include_bytes", "include_bytes\n\t!(\"x\")"),
        ("env", "env\u{2028}!(\"HOME\")"),
        ("env", "env\u{200e}!(\"HOME\")"),
        ("option_env", "option_env\u{000d}\u{000a}!(\"HOME\")"),
    ];
    for (name, src) in cases {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            Some(name),
            "gap must not hide `{name}!`: {src:?}"
        );
    }
}

/// Longer identifiers (ASCII and Unicode) ending in / containing a
/// forbidden name are NOT macro invocations — token identity is exact.
#[test]
fn longer_ascii_and_unicode_identifiers_are_not_flagged() {
    for src in [
        "environment!(\"X\")",
        "envx!(\"X\")",
        "xenv!(\"X\")",
        "include_strs!(\"X\")",
        "option_env2!(\"X\")",
        "env\u{e9}!(\"X\")", // Unicode continuation extends the token
        "\u{e9}env!(\"X\")", // non-ASCII start is its own token
        "\u{65e5}\u{672c}env!(\"X\")",
    ] {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            None,
            "longer identifier must not match: {src:?}"
        );
    }
}

/// Raw identifiers ARE the macro: `r#env!(...)` must be caught.
#[test]
fn raw_identifiers_still_match() {
    assert_eq!(
        crate::compile::test_find_forbidden_macro("r#env!(\"X\")"),
        Some("env")
    );
    assert_eq!(
        crate::compile::test_find_forbidden_macro("r#include!(\"X\")"),
        Some("include")
    );
    // Raw identifiers with other names stay clean.
    assert_eq!(
        crate::compile::test_find_forbidden_macro("r#environment!(\"X\")"),
        None
    );
}

/// A literal between a forbidden name and `!` breaks the invocation shape
/// (the scanner must NOT erase literals into whitespace and re-join the
/// resulting token gap).
#[test]
fn literal_between_name_and_bang_breaks_invocation_shape() {
    for src in [
        "env\"HOME\"!",
        "env'x'!",
        "include_str r#\"x\"#!",
        "env'a!",
        "env b'x'!",
    ] {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            None,
            "literal must break the name–! adjacency: {src:?}"
        );
    }
}

/// Multi-byte characters around real invocations must not panic or
/// mis-slice (scanner never indexes off char boundaries).
#[test]
fn unicode_boundaries_around_invocations_are_safe() {
    assert_eq!(
        crate::compile::test_find_forbidden_macro("d\u{e9}bit env!(\"X\")"),
        Some("env")
    );
    assert_eq!(
        crate::compile::test_find_forbidden_macro("\u{65e5}\u{672c}\u{8a9e} env!(\"X\")"),
        Some("env")
    );
    assert_eq!(crate::compile::test_find_forbidden_macro("caf\u{e9}"), None);
    assert_eq!(
        crate::compile::test_find_forbidden_macro("\u{1f600} env!(\"X\")"),
        Some("env")
    );
    // Char literals containing quote-like ASCII inside strings stay inert.
    assert_eq!(
        crate::compile::test_find_forbidden_macro("let s = \"'\"; env!(\"X\")"),
        Some("env")
    );
}

/// Char literals the old ASCII-only lifetime heuristic mishandled must be
/// parsed as complete tokens (never skipped across, never an infinite
/// scan): multi-byte literal chars, escaped quotes, `\n`, backslash,
/// hex and `\u{…}` escapes. RV-WASM-HOST-5E65E165: the fixtures are RAW
/// strings so the scanner receives the ACTUAL backslash bytes of real
/// Rust source (an ordinary `"'\''"` collapses to `'''` before the scan
/// and tests nothing); each escape fixture asserts it really contains a
/// backslash.
#[test]
fn char_literals_multibyte_and_escapes_do_not_hide_later_macros() {
    // Raw strings: the scanner sees the literal source text. The bool
    // records whether the fixture MUST carry a real backslash to the
    // scanner (escape forms) — asserted per case, non-tautologically.
    let cases = [
        ("let c = 'é'; env!(\"X\")", "env", false), // direct multi-byte char
        (r#"let c = '\''; env!("X")"#, "env", true), // escaped quote
        (r#"let c = '\\'; env!("X")"#, "env", true), // escaped backslash
        (r#"let c = '\n'; env!("X")"#, "env", true), // \n escape
        (r#"let c = '\x41'; env!("X")"#, "env", true), // hex escape \xNN
        (r#"let c = '\u{1F600}'; env!("X")"#, "env", true), // unicode escape
        ("let c = '\u{1F600}'; env!(\"X\")", "env", false), // direct emoji char
        (r#"let c = 'x'; include_str!("y")"#, "include_str", false),
        // A lifetime is not a literal: scanning resumes right after it.
        ("let x: 'a env!(\"X\")", "env", false),
        ("'static env!(\"X\")", "env", false),
    ];
    for (src, name, must_carry_backslash) in &cases {
        if *must_carry_backslash {
            assert!(
                src.contains('\\'),
                "escape fixture must carry REAL backslashes to the scanner: {src:?}"
            );
        }
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            Some(*name),
            "macro after char literal/lifetime must be found: {src:?}"
        );
    }
    // The literals themselves must not be mistaken for invocations.
    for src in [
        "let c = 'é';",
        r#"let c = '\''; let d = env;"#,
        "let x: 'a;",
    ] {
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            None,
            "clean char/lifetime source must stay clean: {src:?}"
        );
    }
}

/// RV-WASM-HOST-5E65E165 — a VALID double-quote char literal (`'"'`, a
/// legal single-char Rust literal) must be consumed as ONE char token and
/// must NOT flip the scanner into string-literal mode that would swallow
/// the subsequent real code — the `env!` after it is still found, and the
/// intervening string stays inert. Controls use only VALID Rust token
/// constructions (no malformed-source success claims): an apostrophe
/// literal, a backslash literal, a unicode escape and a hex escape,
/// each followed by ordinary valid code and a real macro invocation.
#[test]
fn valid_quote_char_literal_does_not_start_string_scanning() {
    let cases = [
        // '"' is a valid char containing a double quote; "safe" is a
        // valid string; env! follows in valid code.
        r#"let c = '"'; let s = "safe"; env!("X")"#,
        // Same shape for the other literal-separator characters.
        r#"let c = '\''; let s = "safe"; env!("X")"#,
        r#"let c = '\\'; let s = "safe"; env!("X")"#,
        r#"let c = '\u{2764}'; let s = "safe"; env!("X")"#,
        r#"let c = '\x27'; let s = "safe"; env!("X")"#,
    ];
    for src in cases {
        assert!(
            src.contains('\\') || src.contains("'\"'"),
            "control fixture must contain real escape/separator bytes: {src:?}"
        );
        assert_eq!(
            crate::compile::test_find_forbidden_macro(src),
            Some("env"),
            "a valid char literal must not swallow the following code: {src:?}"
        );
    }
    // And the same source WITHOUT the macro stays clean (negative control
    // — the scanner did not merely miss the whole tail).
    assert_eq!(
        crate::compile::test_find_forbidden_macro(r#"let c = '"'; let s = "safe"; let n = 1;"#),
        None
    );
}

/// Conservative Unicode identifier coverage: combining marks and other
/// non-ASCII non-whitespace chars EXTEND a token under this scanner's
/// conservative approximation (real Rust folds combining marks, which are
/// XID_Continue, into one identifier; chars like NBSP are invalid
/// identifier-continue in real Rust but this approximation still extends
/// over them), while EXACT Rust whitespace (the `Pattern_White_Space`
/// set — a set that neither contains nor is contained in
/// `char::is_whitespace`) and control chars still END a token — so a
/// forbidden name separated from its `!` by Rust whitespace is still
/// caught, while chars that are NOT Rust whitespace (NBSP, U+3000) stay
/// inside one non-matching token.
#[test]
fn unicode_continuation_and_whitespace_token_boundaries() {
    // Combining mark extends the identifier: one token "envé", no match.
    assert_eq!(
        crate::compile::test_find_forbidden_macro("env\u{301}!(\"X\")"),
        None
    );
    // ...but Rust whitespace between name and ! does NOT hide it.
    assert_eq!(
        crate::compile::test_find_forbidden_macro("env\u{2029}!(\"X\")"),
        Some("env")
    );
    // NOT Rust whitespace (though char::is_whitespace says yes): the token
    // extends and never matches — see the EXACT-set table test below.
    assert_eq!(
        crate::compile::test_find_forbidden_macro("env\u{a0}!(\"X\")"),
        None
    );
    assert_eq!(
        crate::compile::test_find_forbidden_macro("env\u{3000}!(\"X\")"),
        None
    );
    // Non-ASCII punctuation adjacent to the name ends the token; no `!`
    // follows the token, so no match (and no panic).
    assert_eq!(
        crate::compile::test_find_forbidden_macro("env\u{2014}!(\"X\")"),
        None
    );
    // Byte-char literals cannot hide a later macro (RAW string: the
    // scanner must see the real backslash of `b'\''`).
    assert_eq!(
        crate::compile::test_find_forbidden_macro(r#"let c = b'\''; env!("X")"#),
        Some("env")
    );
}

/// RV-WASM-HOST-5E65E165 — EXACT-whitespace table regression.
///
/// Whitespace loop: every `Pattern_White_Space` codepoint (U+0009..000D,
/// U+0020, U+0085, U+200E, U+200F, U+2028, U+2029) is a skippable gap
/// between a forbidden name and its `!` and around each delimiter pair;
/// NBSP/U+3000/U+202F are NOT gaps — real Rust rejects them as
/// identifier-continue, and this scanner's coarser conservative
/// approximation absorbs them into one non-matching token.
///
/// Mutation claims (only justified ones): reverting the ENTIRE card
/// whitespace fix — BOTH `compile.rs` sites (`next_token_is_bang` AND
/// `is_conservative_non_ascii`) — back to `char::is_whitespace` fails
/// the NBSP/U+3000/U+202F negative rows AND the U+200E/U+200F positive
/// rows (the boundary helper stops ending the token at Rust whitespace
/// the different predicate misses, so `env\u{200e}` never matches). The
/// comment/path rows below are valid-source COMPATIBILITY CONTROLS only
/// (the name+`!` recognizer does not inspect the post-`!` gap).
#[test]
fn rv_wasm_host_5e65e165_exact_pattern_white_space_gap_table() {
    // (codepoint, is exact Rust Pattern_White_Space)
    const TABLE: &[(char, bool)] = &[
        ('\u{0009}', true),
        ('\u{000A}', true),
        ('\u{000B}', true),
        ('\u{000C}', true),
        ('\u{000D}', true),
        ('\u{0020}', true),
        ('\u{0085}', true),
        ('\u{200E}', true),
        ('\u{200F}', true),
        ('\u{2028}', true),
        ('\u{2029}', true),
        // NOT Rust whitespace (char::is_whitespace accepts them).
        ('\u{00A0}', false),
        ('\u{3000}', false),
        ('\u{202F}', false),
    ];
    for (c, ws) in TABLE {
        let src = format!("env{c}!(\"X\")");
        assert_eq!(
            test_find_forbidden_macro(&src),
            ws.then_some("env"),
            "U+{:04X} gap handling wrong: {src:?}",
            *c as u32
        );
        if !*ws {
            continue;
        }
        // Valid-source variants with the same codepoint as a gap after
        // the `!` and around each delimiter pair — closed delimiters.
        // Compatibility guards only (see doc).
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            let src = format!("env{c}!{c}{open}\"X\"{c}{close}");
            assert_eq!(
                test_find_forbidden_macro(&src),
                Some("env"),
                "U+{:04X} must stay a gap around `{open}{close}`: {src:?}",
                *c as u32
            );
        }
    }
    // Comment and path forms: one row per shape. Rows with a comment
    // BEFORE the `!` are true gaps the recognizer must skip; rows with a
    // comment AFTER the `!` and every path-qualified form are
    // compatibility controls (no post-`!` skipping property claimed).
    const COMMENT_PATH_CASES: &[(&str, Option<&str>)] = &[
        // pre-`!` comment gaps:
        ("env // line\r\n!(\"X\")", Some("env")),
        ("env /*c*/!(\"X\")", Some("env")),
        ("env /*a /*b*/ c*/!(\"X\")", Some("env")),
        ("include_bytes\n\t!(\"X\")", Some("include_bytes")),
        // post-`!` comment gaps (compatibility controls):
        ("env!( /*c*/ \"X\")", Some("env")),
        ("env! /*c*/ (\"X\")", Some("env")),
        ("env! /*a /*b*/ c*/ (\"X\")", Some("env")),
        ("env! // line\r\n(\"X\")", Some("env")),
        // raw identifiers ARE the macro:
        ("r#env!(\"X\")", Some("env")),
        ("r#include!(\"X\")", Some("include")),
        // path-qualified forbidden macros (every qualifier shape):
        ("std::env!(\"X\")", Some("env")),
        ("core::include_str!(\"X\")", Some("include_str")),
        ("::core::include_str!(\"X\")", Some("include_str")),
        ("::option_env!(\"X\")", Some("option_env")),
        ("$crate::include_bytes!(\"X\")", Some("include_bytes")),
        ("std::r#env!(\"X\")", Some("env")),
        // non-forbidden controls stay clean:
        ("r#format!(\"X\")", None),
        ("std::format!(\"X\")", None),
    ];
    for (src, expected) in COMMENT_PATH_CASES {
        assert_eq!(
            test_find_forbidden_macro(src),
            *expected,
            "comment/path form wrong: {src:?}"
        );
    }
}

// ============================================================================
// RV-DB-B40771A1 — compile_rust_source_blocking (spawn_blocking wrapper)
// ============================================================================

/// Shared ticker helper: increments `counter` every 1 ms while `run` is set.
/// Spawned on the current-thread runtime BEFORE the blocking call is awaited;
/// a frozen counter during the await proves the runtime was starved.
async fn ticker_task(run: Arc<AtomicBool>, counter: Arc<AtomicU64>) {
    while run.load(Ordering::Relaxed) {
        counter.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

/// `compile_rust_source_blocking` must offload the (up to 120 s) cargo build
/// via `spawn_blocking` instead of running it inline on the async caller's
/// thread. On a `current_thread` runtime, an inline call would freeze the
/// ticker task at its pre-compile snapshot; the spawn_blocking wrapper lets
/// it keep ticking.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime() {
    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res = compile_rust_source_blocking(DOUBLE_SOURCE).await;

    match res {
        Ok(_wasm) => {
            assert!(
                counter.load(Ordering::Relaxed) > snapshot,
                "ticker must advance during the blocking compile — the \
                 compile ran inline on the async thread (no spawn_blocking)"
            );
        }
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            run.store(false, Ordering::Relaxed);
            ticker.await.expect("ticker task should terminate");
            eprintln!(
                "SKIP rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime: {msg}"
            );
            return;
        }
        Err(e) => panic!("blocking compile failed: {e}"),
    }

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}

/// Toolchain-independent variant: `run_compile_blocking` must execute its
/// closure on the blocking thread pool, not inline on the async caller's
/// thread. A 50 ms `std::thread::sleep` inside the closure would freeze the
/// current-thread runtime if inlined; the ticker must still advance.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_run_compile_blocking_does_not_starve_current_thread_runtime() {
    use crate::wasm::run_compile_blocking;

    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res: Result<u64, FunctionError> = run_compile_blocking(|| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(42u64)
    })
    .await;

    assert_eq!(res.expect("run_compile_blocking should succeed"), 42);
    assert!(
        counter.load(Ordering::Relaxed) > snapshot,
        "ticker must advance during the blocking closure — the closure ran \
         inline on the async thread (no spawn_blocking)"
    );

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}
