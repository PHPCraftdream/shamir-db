//! Pins the standalone `shamir-server backup-verify SNAPSHOT` subcommand
//! (design `admin-shell.md` §4.7): exit `0` for a verified snapshot, `1`
//! for a usage error, and `5` for every snapshot that does not verify. The
//! failure path prints only one of the bounded `error_class` labels
//! (`manifest_missing`, `manifest_invalid`, `checksum_mismatch`,
//! `unmanifested_file`, `unsafe_manifest_path`, ...) — never the library's
//! message text. The command is a pure offline directory check: no
//! `--config`, no running server, no data directory and no network, so it
//! runs before `Cli::parse()` and before any config/logging/runtime work.
//! The pre-existing dispatch is unchanged: every other subcommand
//! (`doctor`, `backup`, ...) still requires `--config`, so this early path
//! did not weaken the normal CLI contract.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::shell_tests::harness::SERVER_BIN;

/// Write a minimal valid v1 snapshot (one `data.bin` plus its manifest) and
/// return the snapshot directory path as a `String` for argv.
fn snapshot(dir: &Path, content: &[u8]) -> String {
    fs::write(dir.join("data.bin"), content).expect("write snapshot payload");
    let sha256 = hex::encode(Sha256::digest(content));
    let manifest = json!({
        "format_version": 1,
        "created_at_unix_ns": 1_700_000_000_000_000_000u64,
        "files": [{
            "path": "data.bin",
            "sha256": sha256,
            "size_bytes": content.len(),
        }],
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("encode manifest"),
    )
    .expect("write manifest");
    dir.to_str().expect("snapshot path is UTF-8").to_owned()
}

/// Spawn `SERVER_BIN` with the operator-profile env stripped (mirroring
/// `harness::server_command`) and collect its bounded output.
fn run(args: &[&str]) -> Output {
    let mut command = Command::new(SERVER_BIN);
    for var in [
        "SHAMIR_SHELL_PASSWORD",
        "SHAMIR_SHELL_TOKEN",
        "XDG_CONFIG_HOME",
        "HOME",
        "LOCALAPPDATA",
        "USERPROFILE",
    ] {
        command.env_remove(var);
    }
    command.args(args).output().expect("spawn server binary")
}

/// Lossy stdout of one run.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn valid_v1_snapshot_verifies() {
    let dir = TempDir::new().expect("tempdir");
    let path = snapshot(dir.path(), b"verified payload");
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    for needle in ["verified", "files_checked", "offline"] {
        assert!(
            text.contains(needle),
            "stdout missing `{needle}`; stdout {} bytes, stderr {} bytes",
            output.stdout.len(),
            output.stderr.len(),
        );
    }
}

#[test]
fn corrupted_content_reports_checksum_mismatch() {
    let dir = TempDir::new().expect("tempdir");
    let path = snapshot(dir.path(), b"original payload");
    fs::write(dir.path().join("data.bin"), b"tampered payload").expect("tamper payload");
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    assert!(
        text.contains("checksum_mismatch"),
        "stdout missing `checksum_mismatch`; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn missing_manifest_reports_manifest_missing() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir
        .path()
        .to_str()
        .expect("snapshot path is UTF-8")
        .to_owned();
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    assert!(
        text.contains("manifest_missing"),
        "stdout missing `manifest_missing`; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn unlisted_file_reports_unmanifested_file() {
    let dir = TempDir::new().expect("tempdir");
    let path = snapshot(dir.path(), b"listed payload");
    fs::write(dir.path().join("extra.bin"), b"unlisted payload").expect("write extra file");
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    assert!(
        text.contains("unmanifested_file"),
        "stdout missing `unmanifested_file`; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn unsafe_manifest_path_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = json!({
        "format_version": 1,
        "created_at_unix_ns": 1_700_000_000_000_000_000u64,
        "files": [{
            "path": "../escape",
            "sha256": "0".repeat(64),
            "size_bytes": 0,
        }],
    });
    fs::write(
        dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).expect("encode manifest"),
    )
    .expect("write manifest");
    let path = dir
        .path()
        .to_str()
        .expect("snapshot path is UTF-8")
        .to_owned();
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    assert!(
        text.contains("unsafe_manifest_path"),
        "stdout missing `unsafe_manifest_path`; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn malformed_manifest_reports_manifest_invalid() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = json!({"format_version": 1});
    fs::write(
        dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).expect("encode manifest"),
    )
    .expect("write manifest");
    let path = dir
        .path()
        .to_str()
        .expect("snapshot path is UTF-8")
        .to_owned();
    let output = run(&["backup-verify", &path]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let text = stdout(&output);
    assert!(
        text.contains("manifest_invalid"),
        "stdout missing `manifest_invalid`; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn missing_argument_is_a_usage_error() {
    let _dir = TempDir::new().expect("tempdir");
    let output = run(&["backup-verify"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must be empty; exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("usage: shamir-server backup-verify SNAPSHOT"),
        "stderr missing the fixed usage line; stdout {} bytes, stderr {} bytes",
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn extra_argument_is_a_usage_error() {
    let dir = TempDir::new().expect("tempdir");
    let first = snapshot(dir.path(), b"payload");
    let second = dir
        .path()
        .to_str()
        .expect("snapshot path is UTF-8")
        .to_owned();
    let output = run(&["backup-verify", &first, &second]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code {:?}; stdout {} bytes, stderr {} bytes",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len(),
    );
}

#[test]
fn pre_existing_dispatch_still_requires_config() {
    let _dir = TempDir::new().expect("tempdir");
    for args in [&["doctor"][..], &["backup", "--to", "x"][..]] {
        let output = run(args);
        assert_ne!(
            output.status.code(),
            Some(0),
            "a subcommand without --config must not succeed; exit code {:?}; stdout {} bytes, stderr {} bytes",
            output.status.code(),
            output.stdout.len(),
            output.stderr.len(),
        );
        assert!(
            output.stdout.is_empty(),
            "stdout must be empty; exit code {:?}; stdout {} bytes, stderr {} bytes",
            output.status.code(),
            output.stdout.len(),
            output.stderr.len(),
        );
    }
}
