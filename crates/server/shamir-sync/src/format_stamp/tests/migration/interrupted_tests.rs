use std::fs;
use std::path::Path;

use crate::backup;
use crate::format_stamp::check::{check_data_dir, StampMode};
use crate::format_stamp::codec::{decode, STAMP_FILE_NAME};
use crate::format_stamp::io::IgnoreSet;
use crate::format_stamp::migration::{
    read, run_migration, FormatStep, JournalRecord, MigrationError, JOURNAL_FILE_NAME,
};
use crate::format_stamp::model::StampError;
use crate::format_stamp::tests::support::apply_twice::apply_twice_is_noop;
use crate::format_stamp::tests::support::dir_snapshot::DirSnapshot;
use crate::format_stamp::tests::support::empty_step::EmptyStep;
use crate::format_stamp::tests::support::failing_step::{
    FailingStep, MARKER_CONTENTS, MARKER_FILE_NAME,
};
use crate::format_stamp::tests::support::stamp_fixture::{
    policy, temp_dir, write_stamp_with_epoch,
};
use crate::restore::restore_with_probe;

const BINARY_VERSION: &str = "0.1.0-test";
const MIGRATION_ARTIFACTS: &[&str] = &[JOURNAL_FILE_NAME, MARKER_FILE_NAME];

fn integrity_ok(_dir: &Path) -> Result<(), MigrationError> {
    Ok(())
}

fn seed_interrupted_directory(dir: &Path) -> Vec<u8> {
    fs::create_dir_all(dir.join("server_meta/nested")).expect("create nested payload directory");
    fs::write(
        dir.join("server_meta/nested/records.bin"),
        b"preserve-this-payload",
    )
    .expect("write nested payload");
    fs::write(dir.join("server_meta/metadata.bin"), b"schema=3").expect("write metadata payload");
    write_stamp_with_epoch(dir, 1);
    fs::read(dir.join(STAMP_FILE_NAME)).expect("read original stamp")
}

fn assert_interrupted_state(dir: &Path, original_stamp: &[u8]) {
    assert_eq!(
        fs::read(dir.join(STAMP_FILE_NAME)).expect("old stamp remains"),
        original_stamp
    );
    let stamp = decode(original_stamp, dir).expect("original stamp decodes");
    assert_eq!(stamp.format_epoch(), 1);

    let records = read(dir).expect("interrupted journal reads");
    assert_eq!(records.len(), 2);
    assert!(matches!(
        &records[0],
        JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 2,
            binary_version,
            modules,
            ..
        } if binary_version == BINARY_VERSION
            && modules.len() == 1
            && modules[0] == "e0001_0002"
    ));
    assert!(matches!(
        &records[1],
        JournalRecord::StepStarted {
            module,
            from_epoch: 1,
            to_epoch: 2,
            ..
        } if module == "e0001_0002"
    ));
    assert!(!records.iter().any(|record| matches!(
        record,
        JournalRecord::StepFinished { .. } | JournalRecord::ChainFinished { .. }
    )));
    assert_eq!(
        fs::read(dir.join(MARKER_FILE_NAME)).expect("step artefact exists"),
        MARKER_CONTENTS
    );
}

fn assert_migration_completed(dir: &Path) {
    let stamp_bytes = fs::read(dir.join(STAMP_FILE_NAME)).expect("migrated stamp exists");
    let stamp = decode(&stamp_bytes, dir).expect("migrated stamp decodes");
    assert_eq!(stamp.format_epoch(), 2);
    let last_migration = stamp.last_format_migration().expect("migration recorded");
    assert_eq!(last_migration.id, "e0001_0002");
    assert_eq!(last_migration.from_epoch, 1);
    assert_eq!(last_migration.to_epoch, 2);

    let records = read(dir).expect("finished journal reads");
    assert_eq!(records.len(), 5);
    assert!(
        matches!(&records[2], JournalRecord::StepStarted { module, .. } if module == "e0001_0002")
    );
    assert!(
        matches!(&records[3], JournalRecord::StepFinished { module, .. } if module == "e0001_0002")
    );
    assert!(matches!(
        &records[4],
        JournalRecord::ChainFinished { to_epoch: 2, .. }
    ));
}

fn assert_nested_payloads(dir: &Path) {
    assert_eq!(
        fs::read(dir.join("server_meta/nested/records.bin")).expect("nested records survive"),
        b"preserve-this-payload"
    );
    assert_eq!(
        fs::read(dir.join("server_meta/metadata.bin")).expect("metadata survives"),
        b"schema=3"
    );
}

#[test]
fn repeat_completes_the_migration() {
    let temp = temp_dir();
    let dir = temp.path();
    let original_stamp = seed_interrupted_directory(dir);
    let before_failure = DirSnapshot::take(dir);
    let failing = FailingStep::failing();
    let failing_chain: [&dyn FormatStep; 1] = [&failing];

    assert!(matches!(
        run_migration(
            dir,
            policy(2, 1),
            &failing_chain,
            BINARY_VERSION,
            &integrity_ok,
        ),
        Err(MigrationError::StepFailed {
            module: "e0001_0002",
            ..
        })
    ));
    assert_interrupted_state(dir, &original_stamp);
    before_failure.assert_unchanged_except_top_level_files(dir, MIGRATION_ARTIFACTS);
    let before_retry = DirSnapshot::take(dir);
    let replacement = EmptyStep;
    let replacement_chain: [&dyn FormatStep; 1] = [&replacement];
    run_migration(
        dir,
        policy(2, 1),
        &replacement_chain,
        BINARY_VERSION,
        &integrity_ok,
    )
    .expect("replacement step resumes the failed migration");

    assert_migration_completed(dir);
    before_retry
        .assert_unchanged_except_top_level_files(dir, &[STAMP_FILE_NAME, JOURNAL_FILE_NAME]);
    assert_nested_payloads(dir);
    assert_eq!(
        fs::read(dir.join(MARKER_FILE_NAME)).expect("marker remains after resume"),
        MARKER_CONTENTS
    );
}

#[test]
fn plain_start_during_migration_refuses() {
    let temp = temp_dir();
    let dir = temp.path();
    let original_stamp = seed_interrupted_directory(dir);
    let before_failure = DirSnapshot::take(dir);
    let failing = FailingStep::failing();
    let failing_chain: [&dyn FormatStep; 1] = [&failing];
    assert!(matches!(
        run_migration(
            dir,
            policy(2, 1),
            &failing_chain,
            BINARY_VERSION,
            &integrity_ok,
        ),
        Err(MigrationError::StepFailed {
            module: "e0001_0002",
            ..
        })
    ));
    assert_interrupted_state(dir, &original_stamp);
    before_failure.assert_unchanged_except_top_level_files(dir, MIGRATION_ARTIFACTS);
    let before = DirSnapshot::take(dir);

    let result = check_data_dir(dir, StampMode::Writer, &IgnoreSet::none(), &policy(2, 1));

    assert!(matches!(
        result,
        Err(StampError::OlderEpoch {
            found: 1,
            current: 2,
            ..
        })
    ));
    before.assert_unchanged(dir);
    assert_interrupted_state(dir, &original_stamp);
    assert_nested_payloads(dir);
}

#[test]
fn backup_taken_mid_migration_is_safe() {
    let temp = temp_dir();
    let root = temp.path();
    let source = root.join("live");
    fs::create_dir_all(&source).expect("create source data directory");
    let original_stamp = seed_interrupted_directory(&source);
    let before_failure = DirSnapshot::take(&source);
    let failing = FailingStep::failing();
    let failing_chain: [&dyn FormatStep; 1] = [&failing];
    assert!(matches!(
        run_migration(
            &source,
            policy(2, 1),
            &failing_chain,
            BINARY_VERSION,
            &integrity_ok,
        ),
        Err(MigrationError::StepFailed {
            module: "e0001_0002",
            ..
        })
    ));
    assert_interrupted_state(&source, &original_stamp);
    before_failure.assert_unchanged_except_top_level_files(&source, MIGRATION_ARTIFACTS);

    // The failed runner has returned and dropped its journal writer before this copy.
    let backup_root = root.join("backups");
    let backup_report = backup::backup(&source, &backup_root).expect("backup interrupted run");
    let restored = root.join("restored");
    restore_with_probe(&backup_report.dest_dir, &restored, false, |_| Ok(()))
        .expect("restore mid-migration snapshot");
    assert_interrupted_state(&restored, &original_stamp);
    assert_nested_payloads(&restored);
    let before_plain_start = DirSnapshot::take(&restored);

    let plain_start = check_data_dir(
        &restored,
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(2, 1),
    );
    assert!(matches!(
        plain_start,
        Err(StampError::OlderEpoch {
            found: 1,
            current: 2,
            ..
        })
    ));
    before_plain_start.assert_unchanged(&restored);

    let before_retry = DirSnapshot::take(&restored);
    let replacement = EmptyStep;
    let replacement_chain: [&dyn FormatStep; 1] = [&replacement];
    run_migration(
        &restored,
        policy(2, 1),
        &replacement_chain,
        BINARY_VERSION,
        &integrity_ok,
    )
    .expect("restored interrupted migration resumes");

    assert_migration_completed(&restored);
    before_retry
        .assert_unchanged_except_top_level_files(&restored, &[STAMP_FILE_NAME, JOURNAL_FILE_NAME]);
    assert_nested_payloads(&restored);
    assert_eq!(
        fs::read(restored.join(MARKER_FILE_NAME)).expect("restored marker survives"),
        MARKER_CONTENTS
    );
}

#[test]
fn apply_twice_is_noop_for_failing_step_successful_mode() {
    let temp = temp_dir();
    let dir = temp.path();
    seed_interrupted_directory(dir);
    assert!(!dir.join(MARKER_FILE_NAME).exists());
    let step = FailingStep::successful();

    apply_twice_is_noop(&step, dir, policy(2, 1));

    assert_eq!(
        fs::read(dir.join(MARKER_FILE_NAME)).expect("first apply created the marker"),
        MARKER_CONTENTS
    );
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
}
