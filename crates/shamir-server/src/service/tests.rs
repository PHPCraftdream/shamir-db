//! Tests for the pure service-generation functions.
//!
//! These are cross-platform (run on Windows, Linux, macOS) and exercise
//! ONLY the generation logic — no OS side-effects.

use std::path::Path;

use super::*;

#[test]
fn systemd_unit_contains_execstart_and_sigterm() {
    let exe = Path::new("/usr/local/bin/shamir-server");
    let config = Path::new("/etc/shamir/server.ktav");
    let unit = systemd_unit(exe, config, None);

    assert!(unit.contains("ExecStart="));
    assert!(unit.contains("/usr/local/bin/shamir-server"));
    assert!(unit.contains("--config"));
    assert!(unit.contains("/etc/shamir/server.ktav"));
    assert!(unit.contains("run"));
    assert!(unit.contains("KillSignal=SIGTERM"));
    assert!(unit.contains("WantedBy=multi-user.target"));
    assert!(unit.contains("After=network.target"));
    assert!(unit.contains("Type=simple"));
    assert!(unit.contains("Restart=on-failure"));

    // Without a user, no User= line should appear.
    assert!(!unit.contains("User="));
}

#[test]
fn systemd_unit_with_user() {
    let exe = Path::new("/usr/local/bin/shamir-server");
    let config = Path::new("/etc/shamir/server.ktav");
    let unit = systemd_unit(exe, config, Some("svc"));

    assert!(unit.contains("User=svc"));
}

#[test]
fn windows_image_path_quotes_and_appends_run() {
    let exe = Path::new(r"C:\Program Files\shamir-server.exe");
    let config = Path::new(r"C:\ProgramData\shamir\server.ktav");
    let image_path = windows_image_path(exe, config);

    assert!(image_path.starts_with('"'));
    assert!(image_path.ends_with("run"));
    assert!(image_path.contains("--config"));
    assert!(image_path.contains(r"shamir-server.exe"));
    assert!(image_path.contains(r"server.ktav"));
}

#[test]
fn absolute_resolves_relative() {
    let rel = Path::new("some/relative/path.ktav");
    let abs = absolute(rel).expect("absolute should succeed for a relative path");
    assert!(abs.is_absolute(), "expected absolute, got {abs:?}");
    assert!(
        abs.ends_with("some/relative/path.ktav"),
        "expected path ending with the input, got {abs:?}"
    );
}

#[test]
fn absolute_idempotent_for_already_absolute() {
    let already = if cfg!(windows) {
        Path::new(r"C:\shamir\server.ktav")
    } else {
        Path::new("/etc/shamir/server.ktav")
    };
    let abs = absolute(already).expect("absolute should succeed");
    assert_eq!(abs, already);
}

#[test]
fn service_name_is_shamir_server() {
    assert_eq!(SERVICE_NAME, "shamir-server");
}
