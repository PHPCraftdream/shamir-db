use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::support::{
    args, config, fresh_backup, invoke, older_data_dir, read_stamp, temp, DirSnapshot,
};
use shamir_sync::format_stamp::{write_stamp, FormatStamp};

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

#[test]
fn format_migrate_backup_gate_rejects_other_directory_same_epoch() {
    let root = temp();
    let live = root.path().join("live");
    let other = root.path().join("other");
    older_data_dir(&live);
    older_data_dir(&other);
    write_stamp(
        &other,
        &FormatStamp::with_epoch(
            1,
            "different-directory-actor",
            super::support::FIXED_UNIX_MS,
        ),
    )
    .expect("write distinct actor stamp");
    assert_eq!(
        read_stamp(&live).format_epoch(),
        read_stamp(&other).format_epoch()
    );
    assert_eq!(
        fs::read(live.join("payload.bin")).unwrap(),
        fs::read(other.join("payload.bin")).unwrap()
    );

    let backup = fresh_backup(&other, &root.path().join("other-backups"));
    assert!(backup.join("format.stamp").is_file());
    assert!(!backup.join(".shamir.lock").exists());
    let before = DirSnapshot::take(&live);

    let result = invoke(&config(&live), &args(true, false, Some(backup), false));

    assert!(result.result.is_err());
    assert_eq!(super::support::mapping_exit_code(&result.result), 1);
    assert!(result
        .stderr
        .contains("backup stamp created_by does not match the live pre-migration stamp"));
    before.assert_unchanged_except_top_level_files(&live, &[".shamir.lock"]);
    assert!(live.join(".shamir.lock").is_file());
    assert!(!live.join("format_migration.journal").exists());
}

/// Append a syntactically valid scheduled-retention owner-tag sidecar
/// (`backup.meta.json`, the shape `retention/ownership.rs` tags) to a
/// copied snapshot and list it in the snapshot's `manifest.json` so the
/// fixture stays a complete, verifiable snapshot.
fn add_owner_tag_sidecar(snapshot_dir: &Path) {
    let sidecar = serde_json::json!({
        "scheduled_retention": {
            "version": 1,
            "owner": "migration-fixture",
        },
    });
    let sidecar_bytes = serde_json::to_vec(&sidecar).expect("encode owner-tag sidecar");
    let sidecar_path = snapshot_dir.join("backup.meta.json");
    std::fs::write(&sidecar_path, &sidecar_bytes).expect("write owner-tag sidecar");

    let digest = Sha256::digest(&sidecar_bytes);
    let entry = serde_json::json!({
        "path": "backup.meta.json",
        "sha256": hex::encode(digest),
        "size_bytes": sidecar_bytes.len(),
    });

    let manifest_path = snapshot_dir.join(crate::backup::MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    manifest["files"]
        .as_array_mut()
        .expect("manifest files array")
        .push(entry);
    let encoded = serde_json::to_vec_pretty(&manifest).expect("encode manifest");
    std::fs::write(&manifest_path, encoded).expect("rewrite manifest");
}

#[test]
fn plan_gate_verifies_timestamped_exported_snapshot_with_owner_tag_and_writes_nothing() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup_root = root.path().join("backups");
    let backup = fresh_backup(&data, &backup_root);
    add_owner_tag_sidecar(&backup);
    let before = DirSnapshot::take(&data);
    let backup_before = DirSnapshot::take(&backup_root);

    let result = invoke(&config(&data), &args(false, true, Some(backup), false));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(
        result.stdout.contains("backup gate: PASS:"),
        "{}",
        result.stdout
    );
    // The plan gate never locks or writes: the source, the export parent
    // (including any lock file) and the target are byte- and mtime-identical.
    before.assert_unchanged(&data);
    backup_before.assert_unchanged(&backup_root);
    assert!(!backup_root
        .join(crate::backup::SNAPSHOT_LEASE_FILE_NAME)
        .exists());
}

#[test]
fn plan_gate_refuses_managed_timestamped_snapshot_without_locking_or_writing() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup_root = root.path().join("backups");
    let backup = fresh_backup(&data, &backup_root);
    add_owner_tag_sidecar(&backup);
    // Provision the managed marker through the real API: the timestamped
    // child is now a managed scheduled snapshot, not an export.
    crate::backup::provision_snapshot_lease(&backup_root).expect("provision marker");
    // Baselines are taken BEFORE the exclusive lease: DirSnapshot reads the
    // bytes of every file in the tree (only `.shamir.lock` is exempted), and
    // reading a byte-range-locked marker fails on Windows while the lease is
    // held.
    let before = DirSnapshot::take(&data);
    let backup_before = DirSnapshot::take(&backup_root);
    // Hold an independent exclusive lease, as retention would, THROUGH the
    // gate invocation: the refusal must not depend on the gate locking or
    // writing the managed root.
    let delete_lease = crate::backup::try_snapshot_delete_lease(&backup_root)
        .expect("probe the marker lease")
        .expect("marker must be exclusively lockable before the gate runs");

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let config = config(&data);
        let command = args(false, true, Some(backup), false);
        scope.spawn(move || {
            tx.send(invoke(&config, &command))
                .expect("send gate result");
        });
        let received = rx.recv_timeout(std::time::Duration::from_secs(120));
        // Drop the lease before the scope auto-joins so a reverted shared
        // lock acquisition cannot wedge the join.
        drop(delete_lease);
        let result = received.expect("gate finished within 120 s while the lease was held");

        assert!(result.result.is_ok(), "{}", result.stderr);
        assert!(
            result.stdout.contains("backup gate: FAIL:"),
            "{}",
            result.stdout
        );
        assert!(
            result
                .stdout
                .contains("managed scheduled snapshot still owned by retention"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("separate offline backup"));
    });
    // Nothing was written into the managed root or the live data directory.
    before.assert_unchanged(&data);
    backup_before.assert_unchanged(&backup_root);
}
