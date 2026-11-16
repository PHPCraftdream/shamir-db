use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_sync::format_stamp::migration::{parse_records, JournalRecord};
use shamir_sync::format_stamp::{decode, STAMP_FILE_NAME};

use super::support::{
    args, config, invoke, invoke_with, older_data_dir, read_stamp, temp, CountingStep, DirSnapshot,
};

#[test]
fn full_run_advances_epoch_finishes_journal_and_preserves_all_non_migration_data() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let payload = fs::read(data.join("payload.bin")).unwrap();
    let before = DirSnapshot::take(&data);
    let result = invoke(&config(&data), &args(true, false, None, true));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert_eq!(super::support::mapping_exit_code(&result.result), 0);
    assert!(result
        .stdout
        .contains("[1/1] e0001_0002 empty test step … ok ("));
    assert!(result
        .stdout
        .contains("format migration complete: epoch 2 (last module: e0001_0002)"));
    before.assert_unchanged_except_top_level_files(
        &data,
        &[STAMP_FILE_NAME, "format_migration.journal", ".shamir.lock"],
    );
    assert_eq!(fs::read(data.join("payload.bin")).unwrap(), payload);
    assert!(data.join("format_migration.journal").is_file());
    assert!(data.join(".shamir.lock").is_file());

    let stamp = read_stamp(&data);
    assert_eq!(stamp.format_epoch(), 2);
    let migration = stamp
        .last_format_migration()
        .expect("final migration stamp metadata");
    assert_eq!(migration.id, "e0001_0002");
    assert_eq!((migration.from_epoch, migration.to_epoch), (1, 2));
    let journal_bytes = fs::read(data.join("format_migration.journal")).expect("finished journal");
    let records = parse_records(&journal_bytes).expect("parse completed migration journal");
    assert!(matches!(
        records.first(),
        Some(JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 2,
            ..
        })
    ));
    assert!(records.iter().any(|record| matches!(record, JournalRecord::StepFinished { module, .. } if module == "e0001_0002")));
    assert!(matches!(
        records.last(),
        Some(JournalRecord::ChainFinished { to_epoch: 2, .. })
    ));
}

#[test]
fn stampless_base_epoch_data_with_yes_override_warns_and_advances() {
    let root = temp();
    let data = root.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("legacy-data"), b"old format data").unwrap();

    let result = invoke(&config(&data), &args(true, false, None, true));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result
        .stderr
        .contains("stampless data treated as base epoch 1"));
    assert_eq!(read_stamp(&data).format_epoch(), 2);
    assert_eq!(
        fs::read(data.join("legacy-data")).unwrap(),
        b"old format data"
    );
}

#[test]
fn malformed_registry_integrity_failure_keeps_old_stamp_after_step_finished() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    fs::write(
        data.join("wire_tables.mpack"),
        b"malformed MessagePack registry",
    )
    .unwrap();
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let before_payload = fs::read(data.join("payload.bin")).unwrap();
    let modules: [&dyn shamir_sync::format_stamp::migration::FormatStep; 1] =
        [super::support::default_step()];

    let result = invoke_with(
        &config(&data),
        &args(true, false, None, true),
        &modules,
        &super::super::integrity::check_wire_tables_registry,
    );

    assert!(result.result.is_err());
    assert_eq!(super::support::mapping_exit_code(&result.result), 1);
    assert!(result.stderr.contains("wire_tables registry decode failed"));
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    assert_eq!(fs::read(data.join("payload.bin")).unwrap(), before_payload);
    let journal = fs::read(data.join("format_migration.journal")).unwrap();
    let records = parse_records(&journal).expect("parse failure journal");
    assert!(records.iter().any(|record| matches!(record, JournalRecord::StepFinished { module, .. } if module == "e0001_0002")));
    assert!(!records
        .iter()
        .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));
}

#[test]
fn retry_after_finished_step_runs_integrity_without_reapplying_step() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let registry = data.join("wire_tables.mpack");
    fs::write(&registry, b"malformed MessagePack registry").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let step = CountingStep::new(Arc::clone(&calls));
    let modules: [&dyn shamir_sync::format_stamp::migration::FormatStep; 1] = [&step];
    let before_stamp = fs::read(data.join(STAMP_FILE_NAME)).unwrap();
    let first = invoke_with(
        &config(&data),
        &args(true, false, None, true),
        &modules,
        &super::super::integrity::check_wire_tables_registry,
    );
    assert!(first.result.is_err());
    assert!(first.stderr.contains("wire_tables registry decode failed"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "first run must apply one step"
    );
    assert_eq!(fs::read(data.join(STAMP_FILE_NAME)).unwrap(), before_stamp);
    let records = parse_records(&fs::read(data.join("format_migration.journal")).unwrap())
        .expect("parse failed-run journal");
    assert!(records.iter().any(|record| matches!(record, JournalRecord::StepFinished { module, .. } if module == "e0001_0002")));
    assert!(!records
        .iter()
        .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));

    fs::remove_file(registry).unwrap();
    let second = invoke_with(
        &config(&data),
        &args(true, false, None, true),
        &modules,
        &super::super::integrity::check_wire_tables_registry,
    );
    assert!(second.result.is_ok(), "{}", second.stderr);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "resume must skip the completed step"
    );
    assert_eq!(read_stamp(&data).format_epoch(), 2);
}

#[test]
fn real_backup_can_complete_the_run_after_lock_file_is_created() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let backup = super::support::fresh_backup(&data, &root.path().join("backups"));
    assert_eq!(read_stamp(&backup).format_epoch(), 1);
    assert_eq!(super::support::policy().current, 2);
    let result = invoke(
        &config(&data),
        &args(true, false, Some(backup.clone()), false),
    );

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(data.join(".shamir.lock").exists());
    assert_eq!(
        decode(&fs::read(data.join(STAMP_FILE_NAME)).unwrap(), &data)
            .unwrap()
            .format_epoch(),
        2
    );
    assert!(result.stdout.contains("backup gate: PASS:"));
}
