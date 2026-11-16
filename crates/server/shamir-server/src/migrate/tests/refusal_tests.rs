use std::fs;

use super::support::{
    args, config, invoke, mapping_exit_code, older_data_dir, temp, write_adopted_epoch,
    write_epoch, DirSnapshot, FIXED_UNIX_MS,
};
use shamir_sync::format_stamp::write_stamp;
use shamir_sync::format_stamp::{FormatStamp, STAMP_FILE_NAME, STAMP_TMP_NAME};

fn assert_precondition_refusal(
    dir: &std::path::Path,
    backup: Option<std::path::PathBuf>,
) -> String {
    let before_stamp = fs::read(dir.join(STAMP_FILE_NAME)).expect("precondition stamp");
    let result = invoke(&config(dir), &args(true, false, backup, false));
    assert!(
        result.result.is_err(),
        "precondition unexpectedly passed: {}",
        result.stdout
    );
    assert_eq!(
        mapping_exit_code(&result.result),
        1,
        "returned errors map to main's exit code 1"
    );
    assert!(
        !result.stderr.is_empty(),
        "refusal should have an error message"
    );
    assert_eq!(fs::read(dir.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(
        !dir.join("format_migration.journal").exists(),
        "precondition refusal created journal"
    );
    result.stderr
}

#[test]
fn refuses_newer_epoch_before_creating_journal() {
    let root = temp();
    let data = root.path().join("data");
    write_epoch(&data, 3);
    let stderr = assert_precondition_refusal(&data, None);
    assert!(
        stderr.contains("has format epoch 3"),
        "newer epoch must be refused by the stamp check, not a later gate: {stderr}"
    );
    assert!(
        !stderr.contains("backup gate"),
        "newer epoch reached the backup gate: {stderr}"
    );
}

#[test]
fn refuses_corrupt_stamp_before_creating_journal() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(STAMP_FILE_NAME), b"corrupt stamp bytes").unwrap();
    fs::write(data.join("payload"), b"data").unwrap();
    let before = DirSnapshot::take(&data);
    let result = invoke(&config(&data), &args(true, false, None, true));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("corrupt") || result.stderr.contains("cannot be determined"));
    before.assert_unchanged(&data);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_torn_stamp_temporary_file_before_creating_journal() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(STAMP_TMP_NAME), b"interrupted stamp write").unwrap();
    fs::write(data.join("payload"), b"data").unwrap();
    let before = DirSnapshot::take(&data);
    let result = invoke(&config(&data), &args(true, false, None, true));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("interrupted"));
    before.assert_unchanged(&data);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_newer_stamp_layout_before_creating_journal() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let mut frame =
        super::support::encoded_stamp(&FormatStamp::with_epoch(1, "layout-test", FIXED_UNIX_MS));
    frame[4..6].copy_from_slice(&2u16.to_le_bytes());
    fs::write(data.join(STAMP_FILE_NAME), frame).unwrap();
    fs::write(data.join("payload"), b"data").unwrap();
    let before = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let result = invoke(&config(&data), &args(true, false, None, true));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("cannot read completely"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_while_another_process_holds_the_maintenance_lock() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let _owner = crate::maintenance_lock::acquire(&data).expect("hold lock");
    let result = invoke(&config(&data), &args(true, false, None, true));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("stop the server first"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_missing_backup_gate_without_changing_stamp_or_journal() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    assert_precondition_refusal(&data, None);
}

#[test]
fn refuses_backup_with_same_epoch_but_different_created_by_actor() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    write_stamp(
        &data,
        &FormatStamp::with_epoch(1, "different-live-actor", FIXED_UNIX_MS + 1),
    )
    .unwrap();
    let stderr = assert_precondition_refusal(&data, Some(backup));
    assert!(stderr.contains("backup stamp created_by does not match the live pre-migration stamp"));
    assert!(stderr.contains("run `backup --to <dir>` first"));
}

#[test]
fn refuses_backup_that_does_not_cover_a_new_live_top_level_entry() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    fs::write(data.join("added-after-backup"), b"not covered").unwrap();
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let result = invoke(&config(&data), &args(true, false, Some(backup), false));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result
        .stderr
        .contains("does not cover live top-level entry"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_backup_past_configured_freshness_window() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    let too_old = super::support::unix_ns_now() - 25 * 60 * 60 * 1_000_000_000;
    super::support::rewrite_backup_time(&backup, too_old);
    let before_stamp = fs::read(data.join("format.stamp")).unwrap();
    let result = invoke(&config(&data), &args(true, false, Some(backup), false));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("freshness window"));
    assert_eq!(fs::read(data.join("format.stamp")).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_backup_manifest_with_future_timestamp() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    super::support::rewrite_backup_time(&backup, super::support::future_unix_ns());
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let result = invoke(&config(&data), &args(true, false, Some(backup), false));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("timestamp is in the future"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_empty_backup_even_when_stamp_is_copied() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(data.join("cache")).unwrap();
    write_stamp(
        &data,
        &FormatStamp::with_epoch(1, "format-migrate-test", FIXED_UNIX_MS),
    )
    .unwrap();
    let backup = super::support::fresh_backup(&data, &root.path().join("empty-backups"));
    let before = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let result = invoke(&config(&data), &args(true, false, Some(backup), false));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("no manifest-backed user data"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_adopted_older_stamp_without_yes_confirmation() {
    let root = temp();
    let data = root.path().join("data");
    write_adopted_epoch(&data, 1);
    fs::write(data.join("payload"), b"data").unwrap();
    let before = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let result = invoke(&config(&data), &args(false, false, None, true));
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("adopted"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn held_lock_precedes_missing_backup_gate_and_broken_chain() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    // Backups are taken of a stopped server, before anyone holds the lock.
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    let _owner = crate::maintenance_lock::acquire(&data).expect("hold lock");

    let missing_gate = invoke(&config(&data), &args(true, false, None, false));
    assert!(missing_gate.result.is_err());
    assert_eq!(mapping_exit_code(&missing_gate.result), 1);
    assert!(missing_gate.stderr.contains("stop the server first"));
    assert!(!missing_gate.stderr.contains("backup gate refused"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());

    let no_modules: [&dyn shamir_sync::format_stamp::migration::FormatStep; 0] = [];
    let broken_chain = super::support::invoke_with(
        &config(&data),
        &args(true, false, Some(backup), false),
        &no_modules,
        &super::support::integrity_ok,
    );
    assert!(broken_chain.result.is_err());
    assert_eq!(mapping_exit_code(&broken_chain.result), 1);
    assert!(broken_chain.stderr.contains("stop the server first"));
    assert!(!broken_chain
        .stderr
        .contains("resolve format-migration chain"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn invalid_stamp_precedes_held_lock_refusal() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(STAMP_FILE_NAME), b"corrupt stamp bytes").unwrap();
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let _owner = crate::maintenance_lock::acquire(&data).expect("hold lock");

    let result = invoke(&config(&data), &args(true, false, None, true));

    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("corrupt"));
    assert!(!result.stderr.contains("stop the server first"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert!(data.join(".shamir.lock").is_file());
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn refuses_broken_chain_after_a_real_fresh_backup_passes_the_gate() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    let before = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let no_modules: [&dyn shamir_sync::format_stamp::migration::FormatStep; 0] = [];
    let result = super::support::invoke_with(
        &config(&data),
        &args(true, false, Some(backup), false),
        &no_modules,
        &super::support::integrity_ok,
    );
    assert!(result.result.is_err());
    assert_eq!(mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("resolve format-migration chain"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before);
    assert!(!data.join("format_migration.journal").exists());
}
