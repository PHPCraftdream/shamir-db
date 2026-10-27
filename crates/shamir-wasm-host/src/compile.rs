//! Rust→WASM compile pipeline for user-defined functions.
//!
//! [`compile_rust_source`] scaffolds a temporary crate that depends on
//! `shamir-sdk` by absolute path, prepends the `use shamir_sdk as shamir;`
//! alias the macro expects, and builds it with `cargo build --target
//! wasm32-unknown-unknown --release`.
//!
//! ## Security posture (CRIT-6 / audit #440 part A)
//!
//! Guest Rust source is **untrusted code that compiles on the host**: the
//! compiler runs arbitrary native processes (build scripts, proc-macros,
//! `include_str!`, `env!`, …) with full filesystem and environment access.
//! Full seccomp/rlimit isolation is out of scope for this point fix (the
//! repo targets win32 and platform-specific sandboxing is a separate
//! effort). What this module *does* enforce is the minimum hardening the
//! audit calls out:
//!
//! 1. **Forbidden-macro scan** — the source is rejected if it contains
//!    `include!` / `include_str!` / `include_bytes!` / `env!` /
//!    `option_env!` as a macro invocation. This closes the only realistic
//!    compile-time host-execution channel available to guest source — see
//!    task #617's analysis below for why "smuggle it through a dependency"
//!    is not actually reachable here.
//! 2. **Environment allowlist** — the child `cargo build` inherits ONLY
//!    the handful of variables it actually needs (PATH, temp dirs, the
//!    cargo/rustup homes when the host has them). No host secrets
//!    (`*_KEY`, `*_SECRET`, `DATABASE_URL`, …) leak in.
//! 3. **Wall-clock timeout** — the build is killed after
//!    [`WASM_COMPILE_TIMEOUT`], so a malicious or pathological guest
//!    cannot wedge the host indefinitely.
//!
//! ## Defense-in-depth, layer 0: permission gate (task #607)
//!
//! Per the user's explicit direction (2026-07-14), the primary mitigation
//! for "untrusted host compilation" is NOT OS-level sandboxing (no
//! container/seccomp/rlimit) — it is a POSIX-style access-control gate,
//! applied *before* this module is ever reached. Only actors holding
//! `Action::Execute` on the `ResourcePath::WasmCompiler` singleton (default
//! mode `0o755`, mirroring `ResourcePath::Root`) may trigger
//! [`compile_rust_source`] at all; the check lives in
//! `shamir-db`'s `create_function_with_opts_as`
//! (`FunctionSource::Source` branch only — `FunctionSource::Wasm` uploads
//! bypass the host compiler entirely and are unaffected), NOT in this
//! module. This module stays policy-agnostic: it has no notion of actors,
//! ACLs, or grants. The forbidden-macro scan / env allowlist / timeout
//! above are the SECOND layer, defending the compilation process itself
//! once an authorized actor has already been let through.
//!
//! ## Task #617 — why a source-level fs/net/import scanner is not needed
//!
//! The original ask was: also reject `use`/path-qualified filesystem or
//! network access *inside the function body*, not just the compile-time
//! macros above. Analysis (2026-07-14) found this is already structurally
//! closed by two independent facts, confirmed by reading this module and
//! [`super::wasm_sanitizer`]:
//!
//! 1. **The guest crate cannot declare its own dependencies.** `Cargo.toml`
//!    below (see `write_cargo_toml`-equivalent code further down) is
//!    entirely host-generated from a fixed template — the only dependency
//!    is the first-party `shamir-sdk` (itself depending only on
//!    `serde`/`rmp-serde`/optionally `shamir-query-builder`). There is no
//!    `build.rs`, and guest source cannot smuggle in a malicious
//!    proc-macro or build-script crate — it never gets a `[dependencies]`
//!    section to edit. This makes the "smuggle file/env access through a
//!    proc-macro dependency" caveat that used to be in this doc comment
//!    factually unreachable, not just unlikely.
//! 2. **`std::fs`/`std::net`/`std::env` inside the function body cannot
//!    reach the host at runtime, on this target.** The compile target is
//!    `wasm32-unknown-unknown` (bare, not WASI) — these modules exist in
//!    libstd as stubs with no OS binding on this target; calling them
//!    returns an `io::Error`/panics, generating no host import at all.
//!    Even if one did, [`super::wasm_sanitizer`] structurally verifies the
//!    compiled module's import section against
//!    `SANCTIONED_HOST_IMPORTS` — "no WASI or any other import namespace
//!    is ever registered" — before the module is ever instantiated.
//!
//! Net effect: writing `std::fs::File::open(...)` in a guest function body
//! compiles (dead code, functionally inert) but cannot execute anything on
//! the host, at either compile time or run time. A dedicated source-level
//! scanner for this pattern would add complexity without closing a real
//! gap — the forbidden-macro scan above already covers the one channel
//! that *is* real (compile-time macro expansion on the host). Revisit if
//! the compile target or guest dependency model ever changes (e.g. a
//! future WASI target, or letting guests declare their own
//! `Cargo.toml`/dependencies).

use crate::runtime::error::{FnResult, FunctionError};
use shamir_proc_tree::{supervise, ProcessTree, TeardownFaults};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Maximum wall-clock time a guest `cargo build` may take before it is
/// killed and reported as a compile timeout.
///
/// Compiling a small `cdylib` crate with `opt-level="z"` + LTO typically
/// lands in the low tens of seconds even on a cold cache; 120 s leaves a
/// generous margin while still bounding the worst case.
pub const WASM_COMPILE_TIMEOUT: Duration = Duration::from_secs(120);

/// Macros that read files or the host environment at *compile* time and
/// are therefore forbidden in untrusted guest source.
///
/// Each entry is matched as a Rust macro invocation found by TOKEN
/// recognition (RV-WASM-HOST-5E65E165): the exact identifier token
/// (whitespace and comments between the name and `!` are skipped; literal
/// contents never produce tokens; longer identifiers never match).
const FORBIDDEN_MACROS: &[&str] = &[
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
];

/// Scan untrusted Rust source for forbidden compile-time macros.
///
/// Returns the first forbidden macro name found, or `None` if the source
/// is clean. The scan is a conservative Rust LEXER (RV-WASM-HOST-5E65E165):
/// it recognizes identifier TOKENS — not textual substrings — so
///
/// - whitespace and comments (`// …`, nested `/* … */`) between the name
///   and the `!` do NOT hide an invocation,
/// - a longer identifier (`environment`, `xinclude`, a Unicode identifier
///   like `envé`) is never a match (token identity is exact),
/// - the contents of every string/byte-string/C-string literal (plain and
///   raw, `r`/`br`/`cr` with 0..=255 hashes) and char/byte literals never
///   contribute tokens, and a literal between a name and
///   its `!` breaks the invocation shape,
/// - raw identifiers (`r#env!`) are still matched (that IS the macro).
///
/// Unicode is handled on `char` boundaries throughout (no byte slicing on
/// non-boundary offsets), so multi-byte identifiers cannot panic.
fn find_forbidden_macro(source: &str) -> Option<&'static str> {
    let b = source.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    while i < n {
        match b[i] {
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                // Line comment — skip past the newline.
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                // Block comment — nesting-aware. Unterminated → clean (the
                // compiler, not this scanner, is the authority on errors).
                i = skip_block_comment(b, i)?;
            }
            b'"' | b'b' | b'c' | b'r' if is_string_literal_start(b, i) => {
                // String literal — plain `"…"` / `b"…"` / `c"…"` or
                // raw `r…` / `br…` / `cr…` (0..=255 hashes). Consumed as
                // ONE token; contents never produce tokens.
                i += try_consume_string_literal(b, i)
                    .expect("prefix checked by is_string_literal_start");
            }
            b'\'' => {
                // Char literal (`'a'`, `'\n'`, `'\u{…}'`) or a lifetime /
                // loop label (`'a`, `'static`). Only a literal is skipped
                // as one token; a lifetime continues below.
                if let Some(end) = find_char_literal_end(b, source, i) {
                    i = end + 1;
                } else {
                    // Lifetime: skip `'` + identifier token.
                    i = skip_identifier(b, source, i + 1);
                }
            }
            b'r' | b'b' | b'c' => {
                // Not a literal prefix (e.g. the identifiers `b`, `c`,
                // `br#tag`, `cr` not followed by a quote): plain token scan.
                let (name, end) = read_identifier_token(b, source, i);
                if let Some(name) = name {
                    // A forbidden identifier TOKEN followed (across Rust
                    // whitespace and comments only) by `!` is a macro
                    // invocation. A literal or any other token in between
                    // breaks the shape.
                    if FORBIDDEN_MACROS.contains(&name) && next_token_is_bang(b, source, end) {
                        return Some(name);
                    }
                }
                i = end;
            }
            _ => {
                // Any other byte: identifier-token scan (a no-op advance
                // when it does not start an identifier).
                let (name, end) = read_identifier_token(b, source, i);
                if let Some(name) = name {
                    if FORBIDDEN_MACROS.contains(&name) && next_token_is_bang(b, source, end) {
                        return Some(name);
                    }
                }
                i = end;
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn test_find_forbidden_macro(source: &str) -> Option<&'static str> {
    find_forbidden_macro(source)
}

/// Skip a (possibly nested) block comment starting at the `/*` at `i`.
/// Returns the index just past the matching `*/`, or `None` when the
/// comment is unterminated (the source cannot lex cleanly past it).
fn skip_block_comment(b: &[u8], mut i: usize) -> Option<usize> {
    let n = b.len();
    let mut depth = 1usize;
    i += 2;
    while i < n && depth > 0 {
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    (depth == 0).then_some(i)
}

/// Skip a double-quoted string literal starting at the opening `quote` at
/// `i`, honoring `\` escapes. Returns the index just past the closing
/// quote (or `n` at EOF).
fn skip_string_literal(b: &[u8], mut i: usize, quote: u8) -> usize {
    let n = b.len();
    i += 1;
    while i < n {
        if b[i] == b'\\' {
            i += 2; // skip the escape and the escaped byte
            continue;
        }
        if b[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    n
}

/// Whether a string literal starts at byte `i` — the unified
/// literal-prefix dispatch (RV-WASM-HOST-5E65E165, C-string fix): every
/// current Rust string-literal prefix,
///
/// * plain: `"` / `b"` / `c"`,
/// * raw: `r` / `br` / `cr` followed by 0..=255 `#` and a `"`,
///
/// used to disambiguate them from identifiers (`req`, `b`, `c`, the raw
/// identifier `r#tag`, and `cr…` identifiers not followed by a quote).
/// A `cr#name` sequence WITHOUT a closing quote shape falls through to the
/// identifier scanner (raw-identifier path), while `cr#"…"#` is the raw
/// C-string literal.
fn is_string_literal_start(b: &[u8], i: usize) -> bool {
    try_consume_string_literal(b, i).is_some()
}

/// If a string literal (see [`is_string_literal_start`] for the full
/// prefix set) starts at `i`, return its total byte length; else `None`.
/// Raw forms are matched by their exact `"` + hashes close pattern (same
/// matcher for every raw prefix); quoted forms honor `\` escapes so an
/// escaped quote cannot desync the scanner. Unterminated raw strings are
/// consumed to EOF; for quoted forms [`skip_string_literal`] owns that
/// fallback.
fn try_consume_string_literal(b: &[u8], i: usize) -> Option<usize> {
    let n = b.len();
    let mut j = i;
    // Optional prefix: exactly ONE of `b` or `c` (Rust has no `bc`
    // literal form — a `bc…` sequence is identifier + prefix, not one
    // literal, so accepting it here would mis-skip tokens).
    if j < n && (b[j] == b'b' || b[j] == b'c') {
        j += 1;
    }
    if j < n && b[j] == b'r' {
        // Raw form: r / br / cr + `#`* + `"`.
        j += 1;
        let mut hashes = 0usize;
        while j < n && b[j] == b'#' {
            hashes += 1;
            j += 1;
        }
        // Rust bounds raw hashes at 255; a longer `#` run is NOT a valid
        // raw-string start — fall through to the identifier scanner (the
        // conservative direction: real tokens stay scannable).
        if hashes > 255 || j >= n || b[j] != b'"' {
            return None;
        }
        j += 1;
        let mut close_pattern = Vec::with_capacity(hashes + 1);
        close_pattern.push(b'"');
        close_pattern.extend(std::iter::repeat_n(b'#', hashes));
        while j + close_pattern.len() <= n {
            if &b[j..j + close_pattern.len()] == close_pattern.as_slice() {
                return Some(j + close_pattern.len() - i);
            }
            j += 1;
        }
        // Unterminated raw string — consume to EOF.
        Some(n - i)
    } else if j < n && b[j] == b'"' {
        // Plain quoted form: `"` / `b"` / `c"` — escape-aware skip.
        Some(skip_string_literal(b, j, b'"') - i)
    } else {
        None
    }
}

/// End-index (inclusive) of a char literal starting at the `'` at index
/// `i`. Returns `None` if the bytes at `i` do not form a complete char
/// literal — in particular for a lifetime/label (`'a`, `'static`), which
/// the caller then treats as a lifetime, never as a token boundary to
/// skip across. Handles backslash escapes (RV-WASM-HOST-5E65E165 fix: the
/// escaped-literal branch keys on the BACKSLASH, not on the quote — the
/// old `b'\''` check sent every escape sequence (`'\''`, `'\n'`, `'\\'`,
/// `'\xNN'`) down the plain-char path and mis-parsed it as a lifetime,
/// letting a later `env!` hide behind the mis-skipped token), including
/// the multi-char `'\u{…}'` form and multi-byte literal chars (`'é'`): a
/// lifetime heuristic that mis-parses either would skip real code (or
/// worse, fail to advance and spin).
fn find_char_literal_end(b: &[u8], source: &str, i: usize) -> Option<usize> {
    let n = b.len();
    let mut j = i + 1;
    if j >= n {
        return None;
    }
    if b[j] == b'\\' {
        // Escape sequence: `\'`, `\"`, `\\`, `\n`, `\xNN`, `\u{…}`, ...
        j += 1; // past the backslash
        if j >= n {
            return None;
        }
        if b[j] == b'u' && j + 1 < n && b[j + 1] == b'{' {
            // `\u{…}` — skip to the closing brace.
            while j < n && b[j] != b'}' {
                j += 1;
            }
            if j >= n {
                return None;
            }
            j += 1;
        } else if b[j] == b'x' {
            // `\xNN` — two hex digits (not valid hex here is not valid
            // Rust; the scanner consumes the two bytes and lets the
            // compiler, the authority, reject the source).
            if j + 2 >= n {
                return None;
            }
            j += 3;
        } else {
            // Single escaped char (ASCII escapes; a multi-byte char after
            // the backslash is not valid Rust but is consumed safely).
            j += char_len_at(source, j);
        }
    } else {
        // Single (possibly multi-byte) literal char — skip one char.
        j += char_len_at(source, j);
    }
    if j < n && b[j] == b'\'' {
        Some(j)
    } else {
        None
    }
}

/// Read the identifier token starting at byte `i`.
///
/// Returns `(Some(matched_forbidden_name), end)` when the token text
/// equals one of the fixed [`FORBIDDEN_MACROS`] names (a `r#` raw-
/// identifier prefix is stripped first — `r#env!` IS the macro), and
/// `(None, end)` otherwise; `end` is always the index to resume scanning
/// from (one full char forward when `i` does not start an identifier).
/// Identifier-continue is `XID`-approximated by `char::is_alphanumeric`
/// plus `_` — a conservative superset of Rust's grammar, the safe
/// direction here: a longer token is never a false match.
fn read_identifier_token(b: &[u8], source: &str, i: usize) -> (Option<&'static str>, usize) {
    let end = skip_identifier(b, source, i);
    if end == i {
        // Not an identifier start: advance exactly one char (boundary-safe).
        return (None, i + char_len_at(source, i));
    }
    // Raw identifier `r#name` — compare the stripped name.
    let name_start = if b[i] == b'r' && i + 1 < b.len() && b[i + 1] == b'#' {
        i + 2
    } else {
        i
    };
    let matched = source
        .get(name_start..end)
        .and_then(|name| FORBIDDEN_MACROS.iter().copied().find(|m| *m == name));
    (matched, end)
}

/// Advance past an identifier token starting at `i` (or return `start`
/// unchanged if there is none). Handles the `r#` raw-identifier prefix.
fn skip_identifier(b: &[u8], source: &str, start: usize) -> usize {
    let n = b.len();
    let mut i = start;
    if i < n && b[i] == b'r' && i + 1 < n && b[i + 1] == b'#' {
        i += 2;
    }
    if i >= n || !is_ident_start(source, i) {
        return start;
    }
    i += char_len_at(source, i);
    while i < n && is_ident_continue(source, i) {
        i += char_len_at(source, i);
    }
    i
}

/// Rust identifier-start approximation: ASCII letter/`_`, or — the
/// CONSERVATIVE direction for this scanner — any other non-ASCII char
/// that is not whitespace or a control char. A superset of `XID_Start`
/// can only ever lengthen a candidate token, never split one, so a real
/// `env!` can never be hidden by it.
fn is_ident_start(source: &str, i: usize) -> bool {
    match source.get(i..).and_then(|s| s.chars().next()) {
        Some(c) => c == '_' || c.is_ascii_alphabetic() || is_conservative_non_ascii(c),
        None => false,
    }
}

/// Rust identifier-continue approximation: ASCII alnum/`_`, or any
/// non-ASCII char that is not Rust whitespace/control — a deliberate SUPERSET
/// of `XID_Continue` (it includes combining marks, e.g. `env\u{301}`,
/// which real Rust also folds into one identifier). The complementary
/// property is what matters: only EXACT Rust whitespace (the
/// [`is_rust_whitespace`] `Pattern_White_Space` set — a set that neither
/// contains nor is contained in `char::is_whitespace`) ends a token, so a
/// forbidden name separated from its `!` by Rust whitespace is still
/// found by `next_token_is_bang`. Note the approximation is COARSER than
/// real Rust: chars like U+00A0/U+3000 are invalid identifier-continue
/// characters in Rust (the lexer rejects them outright, not absorbs
/// them), while this scanner extends one non-matching token over them.
fn is_ident_continue(source: &str, i: usize) -> bool {
    match source.get(i..).and_then(|s| s.chars().next()) {
        Some(c) => c == '_' || c.is_ascii_alphanumeric() || is_conservative_non_ascii(c),
        None => false,
    }
}

/// Whether the char is in Rust's EXACT whitespace set — Unicode
/// `Pattern_White_Space` and nothing else: U+0009..U+000D, U+0020,
/// U+0085, U+200E, U+200F, U+2028, U+2029. This is a DIFFERENT set from
/// `char::is_whitespace` — neither contains the other: `is_whitespace`
/// adds NBSP U+00A0, IDEOGRAPHIC SPACE U+3000, …, but MISSES
/// U+200E/U+200F, which are Rust whitespace. Only this exact set
/// may be skipped as a token gap; every other char (including those
/// `char::is_whitespace` accepts) must break the name–`!` shape here.
fn is_rust_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            ..='\u{000D}'
                | '\u{0020}'
                | '\u{0085}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{2028}'
                | '\u{2029}'
    )
}

fn is_conservative_non_ascii(c: char) -> bool {
    !c.is_ascii() && !is_rust_whitespace(c) && !c.is_control()
}

/// Length in bytes of the char at boundary `i` (1 as the EOF/invalid
/// fallback — the scanner never calls this off-boundary).
fn char_len_at(source: &str, i: usize) -> usize {
    source
        .get(i..)
        .and_then(|s| s.chars().next())
        .map_or(1, char::len_utf8)
}

/// Whether the next TOKEN after `from` is `!` — skipping EXACT Rust
/// whitespace ([`is_rust_whitespace`], the `Pattern_White_Space` set —
/// the same set rustc's lexer skips; NOT `char::is_whitespace`, which is
/// a different set that both adds NBSP/U+3000-class chars and misses
/// U+200E/U+200F) and comments only. Any other token (including a
/// literal) between the name and the `!` breaks the invocation shape.
fn next_token_is_bang(b: &[u8], source: &str, mut i: usize) -> bool {
    let n = b.len();
    loop {
        while i < n {
            let Some(c) = source.get(i..).and_then(|s| s.chars().next()) else {
                return false;
            };
            if is_rust_whitespace(c) {
                i += c.len_utf8();
            } else {
                break;
            }
        }
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
            i = match skip_block_comment(b, i) {
                Some(next) => next,
                None => return false,
            };
            continue;
        }
        break;
    }
    i < n && b[i] == b'!'
}

/// Build the allowlisted environment for the guest `cargo build`.
///
/// The child receives ONLY the variables cargo/rustc genuinely need to
/// locate the toolchain, write temp artifacts, and (when the host already
/// has them populated) read the cargo/rustup caches so every compile does
/// not re-download the registry. **No host secret is forwarded** —
/// anything matching `*_KEY`/`*_SECRET`/`*_TOKEN`/`*_PASSWORD`/
/// `DATABASE_URL` is dropped by construction (it is simply not in the
/// allowlist).
fn scrubbed_env() -> Vec<(&'static str, std::ffi::OsString)> {
    let mut env: Vec<(&'static str, std::ffi::OsString)> = Vec::new();
    // Helper: forward a var only if the host has it set.
    fn fwd(env: &mut Vec<(&'static str, std::ffi::OsString)>, name: &'static str) {
        if let Some(v) = std::env::var_os(name) {
            env.push((name, v));
        }
    }
    // Toolchain discovery — `check_toolchain` relies on PATH to find
    // `cargo`/`rustc`/`rustup`, so PATH is mandatory.
    fwd(&mut env, "PATH");
    // Windows process/DLL loading requires these to be present for almost
    // any child process to start at all (rustc/link.exe included) — a
    // fully-cleared env without them fails the child near-instantly, not
    // as a "denied" security outcome but as a broken launch. They carry
    // no secret material.
    fwd(&mut env, "SystemRoot");
    fwd(&mut env, "SystemDrive");
    fwd(&mut env, "windir");
    fwd(&mut env, "ProgramData");
    fwd(&mut env, "ProgramFiles");
    fwd(&mut env, "ProgramFiles(x86)");
    fwd(&mut env, "LOCALAPPDATA");
    fwd(&mut env, "APPDATA");
    fwd(&mut env, "ComSpec");
    fwd(&mut env, "PATHEXT");
    fwd(&mut env, "NUMBER_OF_PROCESSORS");
    fwd(&mut env, "PROCESSOR_ARCHITECTURE");
    fwd(&mut env, "OS");
    // Compiler-cache wrapper (e.g. sccache, when configured as
    // `build.rustc-wrapper` on the host) reads its own env for cache
    // location/backend config — forwarding these does not leak any
    // secret, only cache placement.
    fwd(&mut env, "RUSTC_WRAPPER");
    fwd(&mut env, "SCCACHE_DIR");
    fwd(&mut env, "SCCACHE_CACHE_SIZE");
    // Temp dirs — linker/cargo write scratch files here on both Windows
    // and Unix.
    fwd(&mut env, "TEMP");
    fwd(&mut env, "TMP");
    fwd(&mut env, "TMPDIR");
    // Home — cargo locates its registry cache (`$CARGO_HOME/registry`)
    // and rustup locates the active toolchain (`$RUSTUP_HOME`) from
    // these. `shamir-sdk` pulls `serde`/`rmp-serde` from crates.io, so a
    // cold host without these would re-download on every compile.
    fwd(&mut env, "HOME");
    fwd(&mut env, "USERPROFILE");
    fwd(&mut env, "CARGO_HOME");
    fwd(&mut env, "RUSTUP_HOME");
    fwd(&mut env, "RUSTUP_TOOLCHAIN");
    env
}

/// Compile a Rust source string into a `.wasm` binary.
///
/// The source should contain a `#[shamir::function]`-annotated async function.
/// A `use shamir_sdk as shamir;` statement is prepended automatically.
///
/// Returns `FunctionError::ToolchainUnavailable` if `cargo` or the
/// `wasm32-unknown-unknown` target is missing, `FunctionError::Compute` on
/// compilation failure (including forbidden-macro rejection and compile
/// timeout — see the module-level security note), or the raw `.wasm` bytes
/// on success.
// NOTE: sync and blocking (up to 120 s on the caller's thread). Kept `pub`
// only because it is re-exported cross-crate; async callers MUST use
// [`compile_rust_source_blocking`] instead.
#[doc(hidden)]
pub fn compile_rust_source(source: &str) -> FnResult<Vec<u8>> {
    compile_rust_source_with_timeout(source, WASM_COMPILE_TIMEOUT)
}

/// Async counterpart of [`compile_rust_source`].
///
/// `compile_rust_source` spawns cargo and blocks in `wait_timeout`, so async
/// callers must not run it inline on a tokio worker. This moves the
/// (identical) synchronous pipeline onto `tokio::task::spawn_blocking` via
/// the shared [`crate::wasm::run_compile_blocking`] helper — behaviour and
/// error values are identical.
pub async fn compile_rust_source_blocking(source: &str) -> FnResult<Vec<u8>> {
    let source = source.to_string();
    crate::wasm::run_compile_blocking(move || compile_rust_source(&source)).await
}

/// Same as [`compile_rust_source`] but with an explicit compile timeout.
///
/// Exposed primarily for tests, which can pass a tiny timeout to exercise
/// the kill path without waiting minutes for a pathological build.
pub fn compile_rust_source_with_timeout(source: &str, timeout: Duration) -> FnResult<Vec<u8>> {
    // 1. Forbidden-macro scan — reject before we ever spawn cargo.
    if let Some(macro_name) = find_forbidden_macro(source) {
        return Err(FunctionError::Compute(format!(
            "forbidden macro `{macro_name}!` in function source — file/env \
             access at compile time is not permitted"
        )));
    }

    check_toolchain()?;

    let tmpdir =
        tempfile::TempDir::new().map_err(|e| FunctionError::Compute(format!("temp dir: {e}")))?;

    let crate_name = format!(
        "shamir_fn_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    // Compute absolute path to shamir-sdk from this crate's manifest dir.
    let sdk_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("shamir-sdk")
        .canonicalize()
        .map_err(|e| FunctionError::Compute(format!("resolving sdk path: {e}")))?;

    // On Windows, canonicalize() returns a UNC path (\\?\C:\...) that Cargo
    // cannot parse in a path dependency. Strip the prefix.
    let sdk_display = {
        let s = sdk_path.display().to_string();
        // Strip UNC prefix \\?\ if present.
        s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/")
    };

    // Write Cargo.toml — includes a size-optimised release profile so the
    // guest .wasm is as small as possible (P3 of WASM_SLIMMING.md).
    let cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
shamir-sdk = {{ path = "{sdk_display}" }}

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
"#,
    );
    fs::write(tmpdir.path().join("Cargo.toml"), cargo_toml)
        .map_err(|e| FunctionError::Compute(format!("write Cargo.toml: {e}")))?;

    // Write src/lib.rs — prepend the `use shamir_sdk as shamir;` alias.
    fs::create_dir_all(tmpdir.path().join("src"))
        .map_err(|e| FunctionError::Compute(format!("create src dir: {e}")))?;
    let lib_rs = format!("use shamir_sdk as shamir;\n{source}");
    fs::write(tmpdir.path().join("src").join("lib.rs"), lib_rs)
        .map_err(|e| FunctionError::Compute(format!("write lib.rs: {e}")))?;

    // Build. Explicit --target-dir ensures the WASM artifact lands inside
    // the temp directory even when a workspace-level CARGO_TARGET_DIR or
    // [build] target-dir is configured.
    let target_dir = tmpdir.path().join("target");
    let mut cmd = Command::new("cargo");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.args([
        "build",
        "--target",
        "wasm32-unknown-unknown",
        "--release",
        "--manifest-path",
        tmpdir.path().join("Cargo.toml").to_str().unwrap_or(""),
        "--target-dir",
        target_dir.to_str().unwrap_or(""),
    ])
    // CRIT-6 part A: scrub the environment. `env_clear` drops every
    // inherited variable; we then re-add only the allowlist built by
    // `scrubbed_env` (PATH, temp dirs, cargo/rustup homes). No host
    // secret can reach the guest compiler.
    .env_clear()
    .env_remove("CARGO_TARGET_DIR");
    for (k, v) in scrubbed_env() {
        cmd.env(k, v);
    }

    let (mut child, tree) = ProcessTree::spawn(&mut cmd)
        .map_err(|e| FunctionError::Compute(format!("cargo invocation: {e}")))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let output = supervise(child, tree, out, err, timeout, TeardownFaults::default())
        .map_err(FunctionError::Compute)?;
    let status = output.status;
    let stderr = output.stderr;

    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr);
        return Err(FunctionError::Compute(format!(
            "cargo build failed:\n{stderr_text}"
        )));
    }

    // Read the produced .wasm.
    let wasm_path = tmpdir
        .path()
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(&crate_name)
        .with_extension("wasm");

    let wasm_bytes = fs::read(&wasm_path)
        .map_err(|e| FunctionError::Compute(format!("read wasm output: {e}")))?;

    // P2 of WASM_SLIMMING.md — optional `wasm-opt -Oz` post-processing.
    // If binaryen is not installed the unoptimized artifact is returned as-is.
    let wasm_bytes = maybe_wasm_opt(&wasm_bytes, tmpdir.path())?;

    Ok(wasm_bytes)
}

/// Run `wasm-opt -Oz` on the compiled WASM if binaryen is available.
///
/// Returns the optimised bytes when `wasm-opt` is found and succeeds, or the
/// original `wasm_bytes` unchanged when the tool is absent or fails.  Never
/// returns an error from wasm-opt itself — graceful degradation only.
fn maybe_wasm_opt(wasm_bytes: &[u8], work_dir: &std::path::Path) -> FnResult<Vec<u8>> {
    // Quick PATH check — same style as `check_toolchain`.
    let probe = Command::new("wasm-opt").arg("--version").output();
    if probe.is_err() || !probe.as_ref().unwrap().status.success() {
        log::debug!(
            "wasm-opt not found on PATH — skipping post-optimisation (WASM returned as-is)"
        );
        return Ok(wasm_bytes.to_vec());
    }

    let input = work_dir.join("raw.wasm");
    let output = work_dir.join("opt.wasm");

    fs::write(&input, wasm_bytes)
        .map_err(|e| FunctionError::Compute(format!("write raw.wasm for wasm-opt: {e}")))?;

    let result = Command::new("wasm-opt")
        .args(["-Oz"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output();

    match result {
        Ok(o) if o.status.success() => {
            let opt_bytes = fs::read(&output)
                .map_err(|e| FunctionError::Compute(format!("read wasm-opt output: {e}")))?;
            log::debug!(
                "wasm-opt reduced WASM from {} to {} bytes",
                wasm_bytes.len(),
                opt_bytes.len(),
            );
            Ok(opt_bytes)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            log::warn!(
                "wasm-opt exited with {}: {stderr} — using unoptimised WASM",
                o.status
            );
            Ok(wasm_bytes.to_vec())
        }
        Err(e) => {
            log::warn!("wasm-opt invocation failed: {e} — using unoptimised WASM");
            Ok(wasm_bytes.to_vec())
        }
    }
}

/// Verify that `cargo` exists and the `wasm32-unknown-unknown` target is
/// installed.
fn check_toolchain() -> FnResult<()> {
    let cargo_output = Command::new("cargo")
        .arg("--version")
        .output()
        .map_err(|_| FunctionError::ToolchainUnavailable("cargo not found on PATH".into()))?;

    if !cargo_output.status.success() {
        return Err(FunctionError::ToolchainUnavailable(
            "cargo --version failed".into(),
        ));
    }

    let target_output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|_| {
            FunctionError::ToolchainUnavailable(
                "rustup not found — cannot verify wasm32-unknown-unknown target".into(),
            )
        })?;

    let stdout = String::from_utf8_lossy(&target_output.stdout);
    if !stdout.contains("wasm32-unknown-unknown") {
        return Err(FunctionError::ToolchainUnavailable(
            "wasm32-unknown-unknown target not installed".into(),
        ));
    }

    Ok(())
}
