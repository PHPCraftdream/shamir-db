use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_sync::format_stamp::migration::{parse_records, JournalRecord};
use shamir_sync::format_stamp::{
    check_data_dir, decode, CorruptKind, EpochPolicy, FormatStamp, IgnoreSet, StampError,
    StampMode, STAMP_FILE_NAME, STAMP_TMP_NAME,
};

use super::support::{
    args, config, invoke, invoke_with, older_data_dir, read_stamp, temp, write_epoch, CountingStep,
    DirSnapshot,
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
fn format_migrate_resolves_rows_9_12_5b_and_leaves_8_11_18_20_refused() {
    let root = temp();
    let cases = [
        ("row9-empty", FormatRow::OlderEmpty),
        ("row12-data", FormatRow::OlderData),
        ("row5b-stampless", FormatRow::StamplessData),
        ("row8-newer-empty", FormatRow::NewerEmpty),
        ("row11-newer-data", FormatRow::NewerData),
        ("row18-corrupt-data", FormatRow::CorruptData),
        ("row20-newer-layout", FormatRow::NewerLayoutData),
        ("row6-torn-tmp", FormatRow::TornTmpData),
    ];

    for (name, row) in cases {
        let data = root.path().join(name);
        seed_format_row(&data, row);
        let before = DirSnapshot::take(&data);

        let direct = check_data_dir(
            &data,
            StampMode::Writer,
            &IgnoreSet::none(),
            &EpochPolicy::new(2, 1),
        );
        assert_writer_refusal(
            row,
            direct.expect_err("pre-migration Writer check must refuse this row"),
        );
        before.assert_unchanged(&data);

        let result = invoke(&config(&data), &args(true, false, None, true));
        if row.resolves() {
            assert!(result.result.is_ok(), "{name}: {}", result.stderr);
            assert_eq!(super::support::mapping_exit_code(&result.result), 0);
            let stamp = read_stamp(&data);
            assert_eq!(stamp.format_epoch(), 2);
            let migration = stamp
                .last_format_migration()
                .expect("resolved row records migration metadata");
            assert_eq!(migration.id, "e0001_0002");
            assert_eq!((migration.from_epoch, migration.to_epoch), (1, 2));
            assert_eq!(stamp.created_by().adopted, None);

            let records = parse_records(
                &fs::read(data.join("format_migration.journal")).expect("completed journal"),
            )
            .expect("parse completed migration journal");
            assert!(matches!(
                records.last(),
                Some(JournalRecord::ChainFinished { to_epoch: 2, .. })
            ));
            assert!(check_data_dir(
                &data,
                StampMode::Writer,
                &IgnoreSet::none(),
                &EpochPolicy::new(2, 1),
            )
            .is_ok());
            before.assert_unchanged_except_top_level_files(
                &data,
                &[STAMP_FILE_NAME, "format_migration.journal", ".shamir.lock"],
            );
        } else {
            assert!(result.result.is_err(), "{name} unexpectedly migrated");
            assert_eq!(super::support::mapping_exit_code(&result.result), 1);
            assert!(
                result.stderr.contains(row.diagnostic()),
                "{name} refusal diagnostic missing {:?}: {}",
                row.diagnostic(),
                result.stderr
            );
            before.assert_unchanged(&data);
            assert!(!data.join("format_migration.journal").exists());
            assert!(!data.join(".shamir.lock").exists());
        }
    }
}

#[derive(Clone, Copy)]
enum FormatRow {
    OlderEmpty,
    OlderData,
    StamplessData,
    NewerEmpty,
    NewerData,
    CorruptData,
    NewerLayoutData,
    TornTmpData,
}

impl FormatRow {
    fn resolves(self) -> bool {
        matches!(
            self,
            Self::OlderEmpty | Self::OlderData | Self::StamplessData
        )
    }

    fn diagnostic(self) -> &'static str {
        match self {
            Self::OlderEmpty | Self::OlderData => "has format epoch 1",
            Self::StamplessData => "no format stamp",
            Self::NewerEmpty | Self::NewerData => "has format epoch 3",
            Self::CorruptData => "corrupt (BadChecksum)",
            Self::NewerLayoutData => "uses layout 2",
            Self::TornTmpData => "format stamp write",
        }
    }
}

fn seed_format_row(dir: &std::path::Path, row: FormatRow) {
    fs::create_dir_all(dir).expect("create fixture data directory");
    match row {
        FormatRow::OlderEmpty => write_epoch(dir, 1),
        FormatRow::OlderData => {
            write_epoch(dir, 1);
            fs::write(dir.join("payload.bin"), b"row 12 payload").expect("write row 12 payload");
        }
        FormatRow::StamplessData => {
            fs::write(dir.join("payload.bin"), b"row 5b payload").expect("write row 5b payload");
        }
        FormatRow::NewerEmpty => write_epoch(dir, 3),
        FormatRow::NewerData => {
            write_epoch(dir, 3);
            fs::write(dir.join("payload.bin"), b"row 11 payload").expect("write row 11 payload");
        }
        FormatRow::CorruptData => {
            let mut stamp = super::support::encoded_stamp(&FormatStamp::with_epoch(
                1,
                "format-migrate-test",
                super::support::FIXED_UNIX_MS,
            ));
            let checksum_byte = stamp.last_mut().expect("encoded stamp checksum byte");
            *checksum_byte ^= 1;
            fs::write(dir.join(STAMP_FILE_NAME), stamp).expect("write corrupt stamp");
            fs::write(dir.join("payload.bin"), b"row 18 payload").expect("write row 18 payload");
        }
        FormatRow::NewerLayoutData => {
            let mut stamp = super::support::encoded_stamp(&FormatStamp::with_epoch(
                2,
                "format-migrate-test",
                super::support::FIXED_UNIX_MS,
            ));
            stamp[4..6].copy_from_slice(&2_u16.to_le_bytes());
            fs::write(dir.join(STAMP_FILE_NAME), stamp).expect("write newer-layout stamp");
            fs::write(dir.join("payload.bin"), b"row 20 payload").expect("write row 20 payload");
        }
        FormatRow::TornTmpData => {
            fs::write(dir.join(STAMP_TMP_NAME), b"interrupted stamp write")
                .expect("write torn temporary stamp");
            fs::write(dir.join("payload.bin"), b"row 6 payload").expect("write row 6 payload");
        }
    }
}

fn assert_writer_refusal(row: FormatRow, error: StampError) {
    match row {
        FormatRow::OlderEmpty | FormatRow::OlderData => match error {
            StampError::OlderEpoch {
                found: 1,
                current: 2,
                ..
            } => {}
            other => panic!("expected OlderEpoch for rows 9/12, got {other:?}"),
        },
        FormatRow::StamplessData => match error {
            StampError::AbsentOlderBase { current: 2, .. } => {}
            other => panic!("expected AbsentOlderBase for row 5b, got {other:?}"),
        },
        FormatRow::NewerEmpty | FormatRow::NewerData => match error {
            StampError::NewerEpoch {
                found: 3,
                supported: 2,
                ..
            } => {}
            other => panic!("expected NewerEpoch for rows 8/11, got {other:?}"),
        },
        FormatRow::CorruptData => match error {
            StampError::Corrupt {
                kind: CorruptKind::BadChecksum,
                ..
            } => {}
            other => panic!("expected BadChecksum for row 18, got {other:?}"),
        },
        FormatRow::NewerLayoutData => match error {
            StampError::NewerLayout {
                layout: 2,
                epoch: 2,
                ..
            } => {}
            other => panic!("expected NewerLayout for row 20, got {other:?}"),
        },
        FormatRow::TornTmpData => match error {
            StampError::Corrupt {
                kind: CorruptKind::TornTmp,
                ..
            } => {}
            other => panic!("expected TornTmp for row 6, got {other:?}"),
        },
    }
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
