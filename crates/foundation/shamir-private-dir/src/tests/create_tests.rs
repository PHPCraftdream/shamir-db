//! `create_private_dir` cases: roundtrip, idempotence on a verified
//! directory, and adoption of a tampered directory being an error.

use std::fs;
use std::path::Path;

use crate::{create_private_dir, verify_private_dir};

#[test]
fn create_then_verify_roundtrip_on_a_temp_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    create_private_dir(&target).expect("fresh directory must be created private");
    verify_private_dir(&target).expect("the created directory must verify");
    assert!(target.is_dir());
}

#[test]
fn create_is_idempotent_on_a_verified_directory() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    create_private_dir(&target).unwrap();
    create_private_dir(&target).expect("second create on a verified dir must succeed");
    verify_private_dir(&target).unwrap();
}

#[test]
fn create_rejects_an_existing_unverified_directory() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache");
    // Tamper FIRST, then create: the pre-existing path must never be
    // silently adopted.
    if !tamper(&target) {
        eprintln!("skipping: the platform tamper primitive is unavailable");
        return;
    }
    assert!(
        create_private_dir(&target).is_err(),
        "an existing tampered directory must not be adopted"
    );
}

/// Makes `path` (creating it first) accessible beyond its owner, in a
/// platform-appropriate way. Returns false (instead of failing) when the
/// primitive is unavailable or the privilege is missing.
fn tamper(path: &Path) -> bool {
    fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).is_ok()
    }
    #[cfg(windows)]
    {
        widen_dacl_to_everyone(path)
    }
}

/// Grants Everyone full control via `icacls`; returns false when the
/// command is unavailable or fails (missing privilege).
#[cfg(windows)]
fn widen_dacl_to_everyone(path: &Path) -> bool {
    match std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
        .output()
    {
        Ok(output) if output.status.success() => true,
        _ => {
            eprintln!("skipping: icacls could not widen the DACL");
            false
        }
    }
}
