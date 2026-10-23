//! `verify_private_dir` negative cases: not a directory, and — per
//! platform — a widened ACL/mode, an unprotected DACL, a symlink/junction,
//! and regular files.
//!
//! Oracle discipline: every negative case first proves the SAME setup
//! verifies while intact (`create_private_dir` succeeded AND
//! `verify_private_dir` passes), then tampers exactly one property and
//! asserts the rejection. Any setup failure (icacls, chmod, the test-only
//! DACL rewrite) FAILS the test loudly — nothing in the setup may skip.
//! Only link creation (mklink/symlink) may skip, when the privilege is
//! missing on the host.

use std::fs;
use std::path::Path;

use crate::{create_private_dir, verify_private_dir};

#[test]
fn verify_rejects_a_regular_file() {
    // Positive control: the crate-built parent directory itself must
    // verify before a regular file INSIDE it is tested.
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("cache");
    create_private_dir(&private).unwrap();
    verify_private_dir(&private).expect("the crate-built directory must verify");

    let file = private.join("not-a-dir");
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
    // Panics loudly on failure — an unusable widen primitive is a broken
    // setup, never a skip.
    widen(&target);
    assert!(
        verify_private_dir(&target).is_err(),
        "a directory wider than owner-only must not verify"
    );
}

/// Windows-only: the SAME single owner ACE but an UNPROTECTED DACL must
/// be rejected by the `SE_DACL_PROTECTED` control-word check, even though
/// owner and ACE content are untouched. The rewrite goes through the
/// crate's `#[cfg(all(windows, test))]` helper, which calls
/// `SetNamedSecurityInfoW` with `UNPROTECTED_DACL_SECURITY_INFORMATION`
/// and a DACL holding exactly the owner ACE (`D:(A;OICI;FA;;;<sid>)`, no
/// `P`). This test NEVER skips.
#[cfg(windows)]
#[test]
fn verify_rejects_an_unprotected_dacl() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    create_private_dir(&target).unwrap();
    verify_private_dir(&target).expect("the intact directory must verify");

    crate::windows::set_unprotected_owner_only_dacl_for_test(&target)
        .expect("the SetNamedSecurityInfoW rewrite must succeed (loud setup failure)");

    let error =
        verify_private_dir(&target).expect_err("a DACL without SE_DACL_PROTECTED must not verify");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "the rejection must be PermissionDenied"
    );
    assert!(
        error.to_string().contains("not protected"),
        "the rejection must name the protected bit, got: {error}"
    );
}

#[test]
fn verify_rejects_a_linked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    // Positive control: the link TARGET is crate-built and must verify
    // before the link path is tested.
    let target = outside.path().join("private-target");
    create_private_dir(&target).unwrap();
    verify_private_dir(&target).expect("the crate-built link target must verify");
    let link = dir.path().join("link");
    if !make_link(&link, &target) {
        eprintln!("skipping: symlink/junction creation is unavailable");
        return;
    }
    assert!(
        verify_private_dir(&link).is_err(),
        "a symlinked/junctioned path must not verify"
    );
}

/// Loosens the mode to `0755`; panics loudly on failure (setup failure,
/// never a skip).
#[cfg(unix)]
fn widen(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("chmod 0755 must succeed on a user-owned temp dir");
}

#[cfg(unix)]
fn make_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// Grants Everyone full control via `icacls`; PANICS with the command
/// output on failure — an icacls failure is a broken setup, never a skip.
#[cfg(windows)]
fn widen(path: &Path) {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
        .output()
        .expect("icacls must be available on Windows");
    assert!(
        output.status.success(),
        "icacls could not widen the DACL of {path:?}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
        output => {
            let detail = output
                .map(|o| {
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    )
                })
                .unwrap_or_else(|error| error.to_string());
            eprintln!("skipping: mklink /J failed (privilege missing?): {detail}");
            false
        }
    }
}
