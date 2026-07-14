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
//!    `option_env!` as a macro invocation. This is NOT a complete defense
//!    (a determined attacker can smuggle file/env access through a
//!    proc-macro dependency or `concat_idents!`-style tricks), but it
//!    closes the cheapest, most obvious exfiltration paths.
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

use super::error::{FnResult, FunctionError};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
// CRIT-6 part A: `ChildExt::wait_timeout` powers the wall-clock compile
// timeout below.
use wait_timeout::ChildExt;

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
/// Each entry is matched as a Rust macro invocation: a preceding
/// non-identifier character (or start of input) followed by the name and
/// a literal `!`. The scan skips string/byte-string/raw-string literals
/// and `//` + `/* */` comments so that the *text* of a string or comment
/// cannot trigger a false positive.
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
/// is clean. The scan walks the source while skipping:
///
/// - line comments (`// …`) and block comments (`/* … */`, nested),
/// - string literals (`"…"`), byte-string literals (`b"…"`),
///   raw strings (`r"…"`, `r#"…"#`) and raw byte strings (`br"…"`),
/// - char and byte literals (`'…'` / `b'…'`).
///
/// so a forbidden name appearing *inside* a string or comment does NOT
/// trigger a match. A real invocation like `env!("HOME")` (preceded by a
/// non-identifier char or start-of-input, followed by `!`) does.
///
/// As noted in the module docs this is a defense-in-depth check, not a
/// sandbox: it does not stop a proc-macro dependency from reading the
/// environment, nor does it parse the full Rust grammar (e.g. attribute
/// tokens). It exists to close the cheapest exfiltration paths.
fn find_forbidden_macro(source: &str) -> Option<&'static str> {
    let cleaned = strip_strings_and_comments(source);
    find_forbidden_macro_in_clean(&cleaned)
}

#[cfg(test)]
pub(crate) fn test_find_forbidden_macro(source: &str) -> Option<&'static str> {
    find_forbidden_macro(source)
}

/// Strip Rust string/char/byte literals and comments, replacing their
/// bodies with spaces (preserving length & newline positions so byte
/// offsets stay meaningful). The result contains only "code" characters.
fn strip_strings_and_comments(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = b.to_vec();
    let mut i = 0usize;
    while i < n {
        match b[i] {
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                // Line comment — blank until newline (keep the newline).
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                while i < n && b[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                // Block comment — supports nesting. Blank until matching
                // close. Unterminated → blank to EOF.
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                let mut depth = 1usize;
                while i < n && depth > 0 {
                    if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
                        depth += 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
                        depth -= 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else {
                        if b[i] == b'\n' {
                            // preserve newlines for offset accuracy
                        } else {
                            out[i] = b' ';
                        }
                        i += 1;
                    }
                }
            }
            // Raw string / raw byte string: optional `b`, `r`, then `#`*
            // then `"`.
            ch @ (b'r' | b'b') => {
                let (consumed, is_string) = match try_consume_raw_string(b, i) {
                    Some(len) => (len, true),
                    None => (0, false),
                };
                if !is_string {
                    // Fall through to normal handling (could be a regular
                    // identifier/byte literal).
                    if ch == b'b' && i + 1 < n && b[i + 1] == b'\'' {
                        // byte literal `b'…'`
                        out[i] = b' ';
                        i += 1;
                        i = blank_until_quote(b, &mut out, i, b'\'');
                    } else {
                        i += 1;
                    }
                } else {
                    // Blank the whole raw-string span (preserving newlines).
                    for k in out.iter_mut().skip(i).take(consumed) {
                        if *k != b'\n' {
                            *k = b' ';
                        }
                    }
                    i += consumed;
                }
            }
            b'"' => {
                out[i] = b' ';
                i += 1;
                i = blank_until_quote(b, &mut out, i, b'"');
            }
            b'\'' => {
                // Could be a lifetime/label (`'a`, `'static`) — those have
                // an identifier char after the quote and are NOT string
                // literals. Heuristic: only treat as a char literal when
                // the closing `'` is found on the same "short" span.
                if let Some(end) = find_char_literal_end(b, i) {
                    for k in out.iter_mut().take(end + 1).skip(i) {
                        if *k != b'\n' {
                            *k = b' ';
                        }
                    }
                    i = end + 1;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    // SAFETY: we only replaced ASCII bytes with ASCII spaces/newlines,
    // preserving UTF-8 well-formedness of the unchanged bytes.
    String::from_utf8(out).unwrap_or_default()
}

/// Blank bytes from `i` until the closing `quote`, honouring `\` escapes.
/// Returns the index just past the closing quote (or `n` if EOF).
fn blank_until_quote(b: &[u8], out: &mut [u8], mut i: usize, quote: u8) -> usize {
    let n = b.len();
    while i < n {
        let c = b[i];
        if c == b'\\' {
            if c != b'\n' {
                out[i] = b' ';
            }
            i += 1;
            if i < n && b[i] != b'\n' {
                out[i] = b' ';
            }
            i += 1;
            continue;
        }
        if c == quote {
            out[i] = b' ';
            return i + 1;
        }
        if c != b'\n' {
            out[i] = b' ';
        }
        i += 1;
    }
    i
}

/// If a raw string (`r"…"`, `r#"…"#`) or raw byte string (`br"…"`) starts
/// at `i`, return its total byte length; otherwise `None`.
fn try_consume_raw_string(b: &[u8], i: usize) -> Option<usize> {
    let n = b.len();
    let mut j = i;
    if j >= n {
        return None;
    }
    if b[j] == b'b' {
        j += 1;
    }
    if j >= n || b[j] != b'r' {
        return None;
    }
    j += 1;
    // Count opening `#`s.
    let mut hashes = 0usize;
    while j < n && b[j] == b'#' {
        hashes += 1;
        j += 1;
    }
    if j >= n || b[j] != b'"' {
        return None;
    }
    j += 1;
    let close_pattern: Vec<u8> = {
        let mut p = vec![b'"'];
        p.extend(std::iter::repeat_n(b'#', hashes));
        p
    };
    while j + close_pattern.len() <= n {
        if &b[j..j + close_pattern.len()] == close_pattern.as_slice() {
            return Some(j + close_pattern.len() - i);
        }
        j += 1;
    }
    // Unterminated raw string — consume to EOF.
    Some(n - i)
}

/// Heuristic end-index (inclusive) of a char literal starting at the `'`
/// at index `i`. Returns `None` if this looks like a lifetime/label
/// (`'a`) rather than a literal.
fn find_char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let n = b.len();
    let mut j = i + 1;
    if j >= n {
        return None;
    }
    // `'\`… — escaped char literal, e.g. `'\n`, `'\\`, `'\u{...}`.
    if b[j] == b'\\' {
        j += 1;
        while j < n && b[j] != b'\'' {
            j += 1;
        }
        return if j < n { Some(j) } else { None };
    }
    // Single ASCII char then `'`.
    if j + 1 < n && b[j + 1] == b'\'' {
        return Some(j + 1);
    }
    None
}

/// Run the forbidden-macro lexeme search over source that has already had
/// strings and comments blanked.
fn find_forbidden_macro_in_clean(clean: &str) -> Option<&'static str> {
    let bytes = clean.as_bytes();
    for name in FORBIDDEN_MACROS {
        let needle = name.as_bytes();
        let mut from = 0usize;
        while from < bytes.len() {
            let found = match clean[from..].find(name) {
                Some(idx) => from + idx,
                None => break,
            };
            let after = found + needle.len();
            // Must be immediately followed by `!` to be a macro invocation.
            // `after == bytes.len()` (needle is a buffer suffix) is a valid,
            // in-bounds "no `!` follows" case — bounds-check before
            // indexing rather than relying on the loop guard to exclude it.
            if after < bytes.len() && bytes[after] == b'!' {
                let preceded_by_ident = found > 0 && is_ident_continue(bytes[found - 1]);
                if !preceded_by_ident {
                    return Some(name);
                }
            }
            from = found + needle.len();
        }
    }
    None
}

/// Rust identifier-continuation byte predicate (ascii alnum + `_`).
fn is_ident_continue(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
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
pub fn compile_rust_source(source: &str) -> FnResult<Vec<u8>> {
    compile_rust_source_with_timeout(source, WASM_COMPILE_TIMEOUT)
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

    // Pipe stdout/stderr so a build failure can still report cargo's
    // diagnostic output (a plain `.spawn()` with inherited stdio would
    // print to the host's own console but lose the text for the caller).
    // Drain both pipes on dedicated threads WHILE waiting for the child —
    // reading only after `wait_timeout` returns would risk a classic
    // pipe-buffer deadlock if cargo's output exceeds the OS pipe capacity
    // before the process exits.
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| FunctionError::Compute(format!("cargo invocation: {e}")))?;

    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = child_stdout.take() {
            use std::io::Read;
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = child_stderr.take() {
            use std::io::Read;
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });

    // 2. Wall-clock timeout — wait up to `timeout`, then kill. Some status
    // (exit / signal) is guaranteed available once `wait_timeout` returns
    // `Ok(Some(_))`; the `None` arm is the timed-out path where we kill
    // and reap.
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            // Timed out: kill and reap to avoid orphaned cargo/rustc.
            // Killing the child closes its stdout/stderr pipes, which
            // unblocks the reader threads' `read_to_end`.
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(FunctionError::Compute(format!(
                "compilation timed out after {}s",
                timeout.as_secs()
            )));
        }
        Err(e) => {
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(FunctionError::Compute(format!("cargo wait failed: {e}")));
        }
    };

    let stderr = stderr_reader.join().unwrap_or_default();
    let _stdout = stdout_reader.join().unwrap_or_default();

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
