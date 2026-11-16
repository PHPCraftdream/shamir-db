//! Runner tests (design 9, step-1 subset): plan mode writes nothing, the
//! execute loop journals in order and stamps only after the last step, the
//! resume cursor skips finished steps, and a completed run changes nothing.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::format_stamp::codec::{decode, STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::migration::{
    plan_migration, read, run_migration, FormatMigrationOutcome, FormatStep, JournalRecord,
    JournalWriter, MigrationError, ModuleDescriptor, PlanLine, StepContext, JOURNAL_FILE_NAME,
};
use crate::format_stamp::model::{CorruptKind, StampError};
use crate::format_stamp::tests::support::apply_twice::apply_twice_is_noop;
use crate::format_stamp::tests::support::dir_snapshot::DirSnapshot;
use crate::format_stamp::tests::support::empty_step::EmptyStep;
use crate::format_stamp::tests::support::stamp_fixture::{
    policy, temp_dir, write_raw_stamp, write_stamp_with_epoch,
};

static PROBE_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0002_0003",
    from_epoch: 2,
    to_epoch: 3,
    summary: "stamp-observing test step",
    touches: &[],
    work_estimate: "none",
};

/// Test step that records the stamp epoch it observes while `apply` runs.
struct StampEpochProbe {
    observed: Arc<Mutex<Vec<u64>>>,
}

impl FormatStep for StampEpochProbe {
    fn descriptor(&self) -> &ModuleDescriptor {
        &PROBE_DESCRIPTOR
    }

    fn apply(&self, ctx: &mut StepContext<'_>) -> Result<(), MigrationError> {
        let bytes =
            fs::read(ctx.data_dir.join(STAMP_FILE_NAME)).map_err(|source| MigrationError::Io {
                dir: ctx.data_dir.to_path_buf(),
                source,
            })?;
        let stamp = decode(&bytes, ctx.data_dir).map_err(|error| MigrationError::Io {
            dir: ctx.data_dir.to_path_buf(),
            source: std::io::Error::other(error.to_string()),
        })?;
        self.observed
            .lock()
            .expect("probe log lock")
            .push(stamp.format_epoch());
        Ok(())
    }

    fn plan(&self, _ctx: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(Vec::new())
    }
}

fn integrity_ok(_dir: &Path) -> Result<(), MigrationError> {
    Ok(())
}

#[test]
fn malformed_stamp_refuses_without_writes() {
    let temp = temp_dir();
    let dir = temp.path();
    write_raw_stamp(dir, b"corrupt");
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];

    let result = run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &integrity_ok);

    assert!(matches!(
        result,
        Err(MigrationError::Refused {
            source: StampError::Corrupt {
                kind: CorruptKind::ShortFile,
                ..
            }
        })
    ));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
}

#[test]
fn newer_stamp_refuses_without_writes() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 3);
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];

    let result = run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &integrity_ok);

    assert!(matches!(
        result,
        Err(MigrationError::Refused {
            source: StampError::NewerEpoch {
                found: 3,
                supported: 2,
                ..
            }
        })
    ));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
}

#[test]
fn below_base_stamp_refuses_without_writes() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];

    let result = run_migration(dir, policy(3, 2), &steps, "0.1.0-test", &integrity_ok);

    assert!(matches!(
        result,
        Err(MigrationError::Refused {
            source: StampError::OlderEpoch {
                found: 1,
                current: 3,
                ..
            }
        })
    ));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
}

#[test]
fn absent_stamp_with_torn_tmp_refuses_without_writes() {
    let temp = temp_dir();
    let dir = temp.path();
    fs::write(dir.join(STAMP_TMP_NAME), b"interrupted").expect("write stamp tmp");
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];

    let result = run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &integrity_ok);

    assert!(matches!(
        result,
        Err(MigrationError::Refused {
            source: StampError::Corrupt {
                kind: CorruptKind::TornTmp,
                ..
            }
        })
    ));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(STAMP_FILE_NAME).exists());
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
}

#[test]
fn plan_mode_changes_nothing() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);
    let snapshot = DirSnapshot::take(dir);

    let steps: [&dyn FormatStep; 1] = [&EmptyStep];
    let lines = plan_migration(dir, policy(2, 1), &steps).expect("plan succeeds");

    assert_eq!(lines.len(), 2);
    assert!(lines
        .iter()
        .all(|line| line.description.contains("e0001_0002")));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
}

#[test]
fn plan_mode_stampless_directory_creates_nothing() {
    let temp = temp_dir();
    let dir = temp.path();
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];

    plan_migration(dir, policy(2, 1), &steps).expect("stampless plan succeeds");

    snapshot.assert_unchanged(dir);
    assert!(!dir.join(STAMP_FILE_NAME).exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
    assert!(!dir.join(".shamir.lock").exists());
}

#[test]
fn plan_mode_starts_from_actual_stamp_epoch() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 2);
    let snapshot = DirSnapshot::take(dir);
    let steps: [&dyn FormatStep; 1] = [&StampEpochProbe {
        observed: Arc::new(Mutex::new(Vec::new())),
    }];

    let lines = plan_migration(dir, policy(3, 1), &steps).expect("middle-epoch plan succeeds");

    assert!(lines
        .iter()
        .any(|line| line.description.contains("e0002_0003")));
    snapshot.assert_unchanged(dir);
    assert!(!dir.join(".shamir.lock").exists());
    assert!(!dir.join(STAMP_TMP_NAME).exists());
}

#[test]
fn apply_twice_is_noop_on_empty_step() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);

    apply_twice_is_noop(&EmptyStep, dir, policy(2, 1));

    assert!(!dir.join(JOURNAL_FILE_NAME).exists());
}

#[test]
fn run_migration_journals_in_order_and_stamps_last() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);

    let original_stamp = fs::read(dir.join(STAMP_FILE_NAME)).expect("old stamp present");
    let integrity_calls = Cell::new(0);
    let integrity = |path: &Path| {
        integrity_calls.set(integrity_calls.get() + 1);
        assert_eq!(
            fs::read(path.join(STAMP_FILE_NAME)).expect("stamp remains present"),
            original_stamp
        );
        let records = read(path).expect("journal readable at integrity check");
        assert_eq!(records.len(), 3);
        assert!(matches!(&records[2],
            JournalRecord::StepFinished { module, .. } if module == "e0001_0002"));
        assert!(!records
            .iter()
            .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));
        Ok(())
    };
    let steps: [&dyn FormatStep; 1] = [&EmptyStep];
    let outcome = run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &integrity)
        .expect("migration succeeds");
    assert_eq!(integrity_calls.get(), 1);

    assert_eq!(
        outcome,
        FormatMigrationOutcome {
            final_epoch: 2,
            last_module_id: Some("e0001_0002".to_owned()),
        }
    );

    let records = read(dir).expect("journal readable");
    assert_eq!(records.len(), 4);
    assert!(matches!(&records[0],
        JournalRecord::ChainStarted { from_epoch: 1, to_epoch: 2, binary_version, modules, .. }
        if binary_version == "0.1.0-test"
            && modules.len() == 1
            && modules[0] == "e0001_0002"));
    assert!(matches!(&records[1],
        JournalRecord::StepStarted { module, from_epoch: 1, to_epoch: 2, .. }
        if module == "e0001_0002"));
    assert!(matches!(&records[2],
        JournalRecord::StepFinished { module, .. } if module == "e0001_0002"));
    assert!(matches!(
        &records[3],
        JournalRecord::ChainFinished { to_epoch: 2, .. }
    ));

    let bytes = fs::read(dir.join(STAMP_FILE_NAME)).expect("stamp present");
    let stamp = decode(&bytes, dir).expect("stamp decodes");
    assert_eq!(stamp.format_epoch(), 2);
    let migration = stamp.last_format_migration().expect("migration in stamp");
    assert_eq!(migration.id, "e0001_0002");
    assert_eq!(migration.from_epoch, 1);
    assert_eq!(migration.to_epoch, 2);
}

#[test]
fn stamp_written_only_after_last_step() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);

    let observed = Arc::new(Mutex::new(Vec::new()));
    let probe = StampEpochProbe {
        observed: Arc::clone(&observed),
    };
    let original_stamp = fs::read(dir.join(STAMP_FILE_NAME)).expect("old stamp present");
    let integrity_calls = Cell::new(0);
    let integrity = |path: &Path| {
        integrity_calls.set(integrity_calls.get() + 1);
        assert_eq!(
            fs::read(path.join(STAMP_FILE_NAME)).expect("stamp remains present"),
            original_stamp
        );
        let records = read(path).expect("journal readable at integrity check");
        assert_eq!(records.len(), 5);
        assert!(matches!(&records[2],
            JournalRecord::StepFinished { module, .. } if module == "e0001_0002"));
        assert!(matches!(&records[4],
            JournalRecord::StepFinished { module, .. } if module == "e0002_0003"));
        assert!(!records
            .iter()
            .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));
        Ok(())
    };
    let steps: [&dyn FormatStep; 2] = [&EmptyStep, &probe];
    run_migration(dir, policy(3, 1), &steps, "0.1.0-test", &integrity)
        .expect("two-step migration succeeds");
    assert_eq!(integrity_calls.get(), 1);

    assert_eq!(*observed.lock().expect("probe log lock"), [1]);
    let bytes = fs::read(dir.join(STAMP_FILE_NAME)).expect("stamp present");
    let stamp = decode(&bytes, dir).expect("stamp decodes");
    assert_eq!(stamp.format_epoch(), 3);
}

#[test]
fn run_migration_starts_from_actual_stamp_epoch() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 2);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let probe = StampEpochProbe {
        observed: Arc::clone(&observed),
    };
    let steps: [&dyn FormatStep; 1] = [&probe];

    run_migration(dir, policy(3, 1), &steps, "0.1.0-test", &integrity_ok)
        .expect("migration from middle epoch completes");

    assert_eq!(*observed.lock().expect("probe log lock"), [2]);
    let records = read(dir).expect("journal readable");
    assert_eq!(records.len(), 4);
    assert!(matches!(&records[0],
        JournalRecord::ChainStarted { from_epoch: 2, to_epoch: 3, modules, .. }
        if modules == &["e0002_0003"]));
    assert!(matches!(&records[1],
        JournalRecord::StepStarted { module, from_epoch: 2, to_epoch: 3, .. }
        if module == "e0002_0003"));
}

#[test]
fn run_migration_resumes_from_recorded_progress() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);

    let writer = JournalWriter::create(dir).expect("journal created");
    writer
        .append(&JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 3,
            binary_version: "0.1.0-test".to_owned(),
            modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
            unix_ms: 1,
        })
        .expect("chain started");
    writer
        .append(&JournalRecord::StepStarted {
            module: "e0001_0002".to_owned(),
            from_epoch: 1,
            to_epoch: 2,
            unix_ms: 2,
        })
        .expect("first step started");
    writer
        .append(&JournalRecord::StepFinished {
            module: "e0001_0002".to_owned(),
            unix_ms: 3,
        })
        .expect("first step finished");
    drop(writer);

    let observed = Arc::new(Mutex::new(Vec::new()));
    let probe = StampEpochProbe {
        observed: Arc::clone(&observed),
    };
    let steps: [&dyn FormatStep; 2] = [&EmptyStep, &probe];
    run_migration(dir, policy(3, 1), &steps, "0.1.0-test", &integrity_ok)
        .expect("resume completes");

    // Only the unfinished second step re-ran; it still saw the old stamp.
    assert_eq!(*observed.lock().expect("probe log lock"), [1]);

    let records = read(dir).expect("journal readable");
    assert_eq!(records.len(), 6);
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, JournalRecord::ChainStarted { .. }))
            .count(),
        1,
        "resume reuses the existing chain_started"
    );
    assert!(matches!(&records[3],
        JournalRecord::StepStarted { module, from_epoch: 2, to_epoch: 3, .. }
        if module == "e0002_0003"));
    assert!(matches!(&records[4],
        JournalRecord::StepFinished { module, .. } if module == "e0002_0003"));
    assert!(matches!(
        &records[5],
        JournalRecord::ChainFinished { to_epoch: 3, .. }
    ));

    let bytes = fs::read(dir.join(STAMP_FILE_NAME)).expect("stamp present");
    let stamp = decode(&bytes, dir).expect("stamp decodes");
    assert_eq!(stamp.format_epoch(), 3);
}

#[test]
fn integrity_failure_keeps_old_stamp_and_retry_skips_finished_steps() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);
    let original_stamp = fs::read(dir.join(STAMP_FILE_NAME)).expect("old stamp present");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let probe = StampEpochProbe {
        observed: Arc::clone(&observed),
    };
    let steps: [&dyn FormatStep; 2] = [&EmptyStep, &probe];
    let failed_integrity = |path: &Path| {
        assert_eq!(
            fs::read(path.join(STAMP_FILE_NAME)).expect("stamp present"),
            original_stamp
        );
        Err(MigrationError::Io {
            dir: path.to_path_buf(),
            source: std::io::Error::other("integrity failure"),
        })
    };

    assert!(run_migration(dir, policy(3, 1), &steps, "0.1.0-test", &failed_integrity).is_err());

    assert_eq!(
        fs::read(dir.join(STAMP_FILE_NAME)).expect("old stamp remains"),
        original_stamp
    );
    let records = read(dir).expect("journal readable");
    assert_eq!(records.len(), 5);
    assert!(matches!(&records[2],
        JournalRecord::StepFinished { module, .. } if module == "e0001_0002"));
    assert!(matches!(&records[4],
        JournalRecord::StepFinished { module, .. } if module == "e0002_0003"));
    assert!(!records
        .iter()
        .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));

    run_migration(dir, policy(3, 1), &steps, "0.1.0-test", &integrity_ok).expect("retry completes");

    assert_eq!(*observed.lock().expect("probe log lock"), [1]);
    let records = read(dir).expect("completed journal readable");
    assert_eq!(records.len(), 6);
    assert!(matches!(
        &records[5],
        JournalRecord::ChainFinished { to_epoch: 3, .. }
    ));
}

#[test]
fn rerun_after_completion_changes_nothing() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);

    let steps: [&dyn FormatStep; 1] = [&EmptyStep];
    run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &integrity_ok)
        .expect("first run completes");
    let snapshot = DirSnapshot::take(dir);

    run_migration(dir, policy(2, 1), &steps, "0.1.0-test", &|_| {
        panic!("integrity callback must not run for a completed migration")
    })
    .expect("completed run is a no-op");
    snapshot.assert_unchanged(dir);
}
