use std::fs;

use super::support::{
    args, config, fresh_backup, invoke, older_data_dir, read_stamp, temp, DirSnapshot,
};

#[test]
fn real_pre_migration_backup_passes_gate_and_is_compared_to_epoch_one_before_lock() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup_root = root.path().join("backups");
    let backup = fresh_backup(&data, &backup_root);
    assert_eq!(read_stamp(&backup).format_epoch(), 1);
    assert_eq!(super::support::policy().current, 2);
    assert!(
        !backup.join(".shamir.lock").exists(),
        "backup was taken before lock creation"
    );
    let before = DirSnapshot::take(&data);

    let result = invoke(
        &config(&data),
        &args(false, true, Some(backup.clone()), false),
    );

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result.stdout.contains("backup gate: PASS:"));
    assert!(result.stdout.contains("format migration plan"));
    assert!(result.stdout.contains("backup gate: PASS:"));
    before.assert_unchanged(&data);
    assert!(backup.join("format.stamp").is_file());
    assert_eq!(read_stamp(&data).format_epoch(), 1);
    assert!(!data.join(".shamir.lock").exists());
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn real_backup_gate_rejects_snapshot_with_no_manifest_backed_user_data() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(data.join("cache")).unwrap();
    shamir_sync::format_stamp::write_stamp(
        &data,
        &shamir_sync::format_stamp::FormatStamp::with_epoch(
            1,
            "format-migrate-test",
            super::support::FIXED_UNIX_MS,
        ),
    )
    .unwrap();
    let backup = fresh_backup(&data, &root.path().join("empty-backups"));
    let before = DirSnapshot::take(&data);

    let result = invoke(&config(&data), &args(true, false, Some(backup), false));

    assert!(result.result.is_err());
    assert_eq!(super::support::mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("no manifest-backed user data"));
    before.assert_unchanged_except_top_level_files(&data, &[".shamir.lock"]);
    assert!(data.join(".shamir.lock").is_file());
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn corrupted_manifest_payload_is_refused_before_stamp_or_journal_changes() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = fresh_backup(&data, &root.path().join("backups"));
    let snapshot_payload = backup.join("payload.bin");
    fs::write(&snapshot_payload, b"tampered snapshot payload").unwrap();
    let before_stamp = fs::read(data.join("format.stamp")).unwrap();

    let result = invoke(&config(&data), &args(true, false, Some(backup), false));

    assert!(result.result.is_err());
    assert_eq!(super::support::mapping_exit_code(&result.result), 1);
    assert!(
        result.stderr.contains("checksum mismatch"),
        "{}",
        result.stderr
    );
    assert_eq!(fs::read(data.join("format.stamp")).unwrap(), before_stamp);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn explicit_operator_backup_override_warns_and_proceeds() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);

    let result = invoke(&config(&data), &args(true, false, None, true));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert_eq!(super::support::mapping_exit_code(&result.result), 0);
    assert!(result
        .stderr
        .contains("WARNING: --i-have-a-backup bypasses manifest"));
    assert!(result.stdout.contains("format migration complete: epoch 2"));
}

#[test]
fn adopted_stamp_with_yes_warns_and_advances() {
    let root = temp();
    let data = root.path().join("data");
    super::support::write_adopted_epoch(&data, 1);
    fs::write(data.join("payload.bin"), b"adopted payload").unwrap();

    let result = invoke(&config(&data), &args(true, false, None, true));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result.stderr.contains("adopted stamp"));
    assert_eq!(read_stamp(&data).format_epoch(), 2);
    assert_eq!(read_stamp(&data).created_by().adopted, None);
}
