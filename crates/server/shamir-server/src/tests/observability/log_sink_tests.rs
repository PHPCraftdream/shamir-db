//! A configured diagnostic log sink must never land in the audit namespace.
//!
//! `logging::validate_sink_path` runs before logging is initialised, so a
//! refused sink is never even opened — let alone created — inside the audit
//! namespace of `data_dir`.

use std::fs;

use tempfile::TempDir;

use crate::logging::{validate_sink_path, LogSinkError};

/// `<data_dir>/audit.log` — the active audit log name itself.
#[test]
fn reserved_sink_in_data_dir_is_refused() {
    let data_dir = TempDir::new().expect("data dir");
    let sink = data_dir.path().join("audit.log");
    let err = validate_sink_path(&sink, data_dir.path())
        .expect_err("the active audit log name in data_dir must be refused");
    assert!(
        matches!(err, LogSinkError::ReservedAuditName),
        "got {err:?}"
    );
}

/// `<data_dir>/audit.log.<digits>` — a rotation archive name.
#[test]
fn reserved_rotation_archive_sink_is_refused() {
    let data_dir = TempDir::new().expect("data dir");
    let sink = data_dir.path().join("audit.log.00000000000000000001");
    let err = validate_sink_path(&sink, data_dir.path())
        .expect_err("a rotation archive name in data_dir must be refused");
    assert!(
        matches!(err, LogSinkError::ReservedAuditName),
        "got {err:?}"
    );
}

/// `<data_dir>/server.log` — an ordinary diagnostic sink in `data_dir`.
#[test]
fn ordinary_sink_in_data_dir_is_allowed() {
    let data_dir = TempDir::new().expect("data dir");
    let sink = data_dir.path().join("server.log");
    validate_sink_path(&sink, data_dir.path())
        .expect("an ordinary sink in data_dir must be allowed");
}

/// `<other>/audit.log` — the reserved name is only special inside `data_dir`.
#[test]
fn reserved_name_outside_data_dir_is_allowed() {
    let data_dir = TempDir::new().expect("data dir");
    let other = TempDir::new().expect("other dir");
    assert_ne!(other.path(), data_dir.path());
    let sink = other.path().join("audit.log");
    validate_sink_path(&sink, data_dir.path())
        .expect("the reserved name outside data_dir must be allowed");
}

/// `<data_dir>/logs/audit.log` — a sink in a subdirectory is not the namespace.
#[test]
fn reserved_name_in_a_subdirectory_is_allowed() {
    let data_dir = TempDir::new().expect("data dir");
    let logs = data_dir.path().join("logs");
    fs::create_dir(&logs).expect("logs subdirectory");
    let sink = logs.join("audit.log");
    validate_sink_path(&sink, data_dir.path())
        .expect("a sink in a subdirectory of data_dir must be allowed");
}
