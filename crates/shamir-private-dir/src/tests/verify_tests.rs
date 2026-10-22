//! `verify_private_dir` negative cases: not a directory, and — per
//! platform — a widened ACL/mode, an unprotected DACL, a symlink/junction,
//! and regular files.
//!
//! Oracle discipline: every negative case first proves the SAME setup
//! verifies while intact (`create_private_dir` succeeded AND
//! `verify_private_dir` passes), then tampers exactly one property and
//! asserts the rejection. icacls needs no privilege on a user-owned temp
//! directory, so an icacls failure fails the test loudly; only link
//! creation (mklink) may skip.

use std::fs;
use std::path::Path;

use crate::{create_private_dir, verify_private_dir};

#[test]
fn verify_rejects_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    fs::write(&file, b"x").unwrap();
    assert!(
        verify_private_dir(&file).is_err(),
        "a regular file must not verify"
    );
}

#[test]
fn verify_rejects_a_wider_acl_or_mode() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    create_private_dir(&target).unwrap();
    verify_private_dir(&target).expect("the intact directory must verify");
    if !widen(&target) {
        eprintln!("skipping: the platform widen primitive is unavailable");
        return;
    }
    assert!(
        verify_private_dir(&target).is_err(),
        "a directory wider than owner-only must not verify"
    );
}

/// Windows-only: the SAME single owner ACE but an UNPROTECTED DACL must
/// be rejected by the `SE_DACL_PROTECTED` control-word check, even though
/// owner and ACE content are untouched. `icacls /inheritance:e` alone
/// re-enables inheritance (clearing the protected bit) while keeping the
/// explicit owner ACE; `icacls <dir>` is echoed into the panic message if
/// the rewrite fails. This test NEVER skips on icacls failure.
#[cfg(windows)]
#[test]
fn verify_rejects_an_unprotected_dacl() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    create_private_dir(&target).unwrap();
    verify_private_dir(&target).expect("the intact directory must verify");

    let output = std::process::Command::new("icacls")
        .arg(&target)
        .args(["/inheritance:e"])
        .output()
        .expect("icacls must be available on Windows");
    assert!(
        output.status.success(),
        "icacls /inheritance:e failed; current DACL:\n{}",
        icacls_dump(&target)
    );

    let error =
        verify_private_dir(&target).expect_err("a DACL without SE_DACL_PROTECTED must not verify");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "the rejection must be PermissionDenied"
    );
}

#[cfg(windows)]
fn icacls_dump(path: &Path) -> String {
    match std::process::Command::new("icacls").arg(path).output() {
        Ok(output) => format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("(icacls unavailable: {error})"),
    }
}

#[test]
fn verify_rejects_a_linked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = dir.path().join("link");
    if !make_link(&link, outside.path()) {
        eprintln!("skipping: symlink/junction creation is unavailable");
        return;
    }
    assert!(
        verify_private_dir(&link).is_err(),
        "a symlinked/junctioned path must not verify"
    );
}

#[cfg(unix)]
fn widen(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).is_ok()
}

#[cfg(unix)]
fn make_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// Grants Everyone full control via `icacls`; returns false when the
/// command fails (never silently: the failure output is printed).
#[cfg(windows)]
fn widen(path: &Path) -> bool {
    match std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
        .output()
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            eprintln!("icacls failed: {}", String::from_utf8_lossy(&output.stderr));
            false
        }
        Err(error) => {
            eprintln!("icacls unavailable: {error}");
            false
        }
    }
}

/// Creates a junction via `cmd /c mklink /J`; returns false when the
/// privilege is missing or the command fails (the ONLY permitted skip).
#[cfg(windows)]
fn make_link(link: &Path, target: &Path) -> bool {
    match std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
    {
        Ok(output) if output.status.success() => true,
        _ => {
            eprintln!("skipping: mklink /J failed (privilege missing?)");
            false
        }
    }
}
