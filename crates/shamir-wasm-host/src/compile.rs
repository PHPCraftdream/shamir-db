//! Rust→WASM compile pipeline for user-defined functions.
//!
//! [`compile_rust_source`] scaffolds a temporary crate that depends on
//! `shamir-sdk` by absolute path, prepends the `use shamir_sdk as shamir;`
//! alias the macro expects, and builds it with `cargo build --target
//! wasm32-unknown-unknown --release`.

use super::error::{FnResult, FunctionError};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Compile a Rust source string into a `.wasm` binary.
///
/// The source should contain a `#[shamir::function]`-annotated async function.
/// A `use shamir_sdk as shamir;` statement is prepended automatically.
///
/// Returns `FunctionError::ToolchainUnavailable` if `cargo` or the
/// `wasm32-unknown-unknown` target is missing, `FunctionError::Compute` on
/// compilation failure, or the raw `.wasm` bytes on success.
pub fn compile_rust_source(source: &str) -> FnResult<Vec<u8>> {
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
    let output = Command::new("cargo")
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "--manifest-path",
            tmpdir.path().join("Cargo.toml").to_str().unwrap_or(""),
            "--target-dir",
            target_dir.to_str().unwrap_or(""),
        ])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .map_err(|e| FunctionError::Compute(format!("cargo invocation: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FunctionError::Compute(format!(
            "cargo build failed:\n{stderr}"
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
