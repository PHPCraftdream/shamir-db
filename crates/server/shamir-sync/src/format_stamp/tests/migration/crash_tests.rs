use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::format_stamp::check::{check_data_dir, StampMode};
use crate::format_stamp::codec::{decode, STAMP_FILE_NAME};
use crate::format_stamp::io::IgnoreSet;
use crate::format_stamp::migration::{
    read, run_migration, FormatStep, JournalRecord, JournalWriter, MigrationError,
    ModuleDescriptor, PlanLine, StepContext, JOURNAL_FILE_NAME,
};
use crate::format_stamp::model::{EpochPolicy, FormatStamp, LastFormatMigration};
use crate::format_stamp::tests::support::dir_snapshot::DirSnapshot;
use crate::format_stamp::tests::support::stamp_fixture::{
    policy, temp_dir, write_stamp_with_epoch,
};

const BINARY_VERSION: &str = "0.1.0-crash-test";
const FIRST_ARTIFACT: &str = "crash_e0001_0002.marker";
const SECOND_ARTIFACT: &str = "crash_e0002_0003.marker";

static FIRST_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0001_0002",
    from_epoch: 1,
    to_epoch: 2,
    summary: "first crash-table step",
    touches: &[],
    work_estimate: "none",
};

static SECOND_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0002_0003",
    from_epoch: 2,
    to_epoch: 3,
    summary: "second crash-table step",
    touches: &[],
    work_estimate: "none",
};

struct TraceStep {
    descriptor: &'static ModuleDescriptor,
    artifact: &'static str,
    calls: Arc<Mutex<Vec<&'static str>>>,
    disrupt_append: bool,
}

impl FormatStep for TraceStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        self.descriptor
    }

    fn apply(&self, context: &mut StepContext<'_>) -> Result<(), MigrationError> {
        self.calls
            .lock()
            .expect("step call log lock")
            .push(self.descriptor.id);
        write_marker(
            context.data_dir,
            self.artifact,
            self.descriptor.id.as_bytes(),
        )?;
        if self.disrupt_append {
            let journal = context.data_dir.join(JOURNAL_FILE_NAME);
            let saved = context.data_dir.join("journal-held-for-append-failure.bin");
            fs::rename(&journal, &saved).map_err(|source| MigrationError::Io {
                dir: context.data_dir.to_path_buf(),
                source,
            })?;
            fs::create_dir(&journal).map_err(|source| MigrationError::Io {
                dir: context.data_dir.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    fn plan(&self, _context: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(Vec::new())
    }
}

fn write_marker(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), MigrationError> {
    let path = dir.join(name);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file.write_all(bytes).map_err(|source| MigrationError::Io {
            dir: dir.to_path_buf(),
            source,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(path).map_err(|source| MigrationError::Io {
                dir: dir.to_path_buf(),
                source,
            })?;
            if existing != bytes {
                return Err(MigrationError::Io {
                    dir: dir.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "step marker has unexpected contents",
                    ),
                });
            }
            Ok(())
        }
        Err(source) => Err(MigrationError::Io {
            dir: dir.to_path_buf(),
            source,
        }),
    }
}

fn step_pair(calls: Arc<Mutex<Vec<&'static str>>>) -> (TraceStep, TraceStep) {
    (
        TraceStep {
            descriptor: &FIRST_DESCRIPTOR,
            artifact: FIRST_ARTIFACT,
            calls: Arc::clone(&calls),
            disrupt_append: false,
        },
        TraceStep {
            descriptor: &SECOND_DESCRIPTOR,
            artifact: SECOND_ARTIFACT,
            calls,
            disrupt_append: false,
        },
    )
}

fn chain_started() -> JournalRecord {
    JournalRecord::ChainStarted {
        from_epoch: 1,
        to_epoch: 3,
        binary_version: BINARY_VERSION.to_owned(),
        modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
        unix_ms: 1,
    }
}

fn first_started() -> JournalRecord {
    JournalRecord::StepStarted {
        module: "e0001_0002".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 2,
    }
}

fn first_finished() -> JournalRecord {
    JournalRecord::StepFinished {
        module: "e0001_0002".to_owned(),
        unix_ms: 3,
    }
}

fn second_started() -> JournalRecord {
    JournalRecord::StepStarted {
        module: "e0002_0003".to_owned(),
        from_epoch: 2,
        to_epoch: 3,
        unix_ms: 4,
    }
}

fn second_finished() -> JournalRecord {
    JournalRecord::StepFinished {
        module: "e0002_0003".to_owned(),
        unix_ms: 5,
    }
}

fn seed_journal(dir: &Path, records: &[JournalRecord]) {
    let writer = JournalWriter::create(dir).expect("create seeded journal");
    for record in records {
        writer.append(record).expect("append seeded record");
    }
}

fn seed_old_dir(dir: &Path) -> Vec<u8> {
    write_stamp_with_epoch(dir, 1);
    fs::write(dir.join("payload.bin"), b"keep this directory payload").expect("seed payload");
    fs::read(dir.join(STAMP_FILE_NAME)).expect("capture old stamp bytes")
}

fn assert_stamp_epoch(dir: &Path, expected: u64) {
    let stamp = decode(
        &fs::read(dir.join(STAMP_FILE_NAME)).expect("read stamp"),
        dir,
    )
    .expect("decode stamp");
    assert_eq!(stamp.format_epoch(), expected);
}

fn append_record_bytes(buffer: &mut Vec<u8>, record: &JournalRecord) {
    buffer.extend(serde_json::to_vec(record).expect("serialize journal record"));
    buffer.push(b'\n');
}

fn run_from_crash_prefix(
    dir: &Path,
    snapshot: &DirSnapshot,
    old_stamp: &[u8],
    expected_calls: &[&str],
    expected_new_artifacts: &[&str],
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (first, second) = step_pair(Arc::clone(&calls));
    let steps: [&dyn FormatStep; 2] = [&first, &second];
    let integrity_calls = std::cell::Cell::new(0);
    let result = run_migration(dir, policy(3, 1), &steps, BINARY_VERSION, &|path| {
        integrity_calls.set(integrity_calls.get() + 1);
        assert_eq!(
            fs::read(path.join(STAMP_FILE_NAME)).expect("stamp at integrity boundary"),
            old_stamp,
            "the old epoch remains until integrity succeeds"
        );
        let records = read(path).expect("all records visible to integrity check");
        assert_eq!(
            records
                .iter()
                .filter(|record| matches!(record, JournalRecord::StepFinished { .. }))
                .count(),
            2,
            "integrity check follows both completed steps"
        );
        assert!(!records
            .iter()
            .any(|record| matches!(record, JournalRecord::ChainFinished { .. })));
        Ok(())
    });
    assert!(
        result.is_ok(),
        "crash-table prefix resumes safely: {result:?}"
    );

    assert_eq!(
        calls.lock().expect("step call log lock").as_slice(),
        expected_calls
    );
    assert_eq!(integrity_calls.get(), 1);
    assert_stamp_epoch(dir, 3);
    let records = read(dir).expect("completed journal reads");
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, JournalRecord::StepFinished { .. }))
            .count(),
        2
    );
    assert!(matches!(
        records.last(),
        Some(JournalRecord::ChainFinished { to_epoch: 3, .. })
    ));
    let mut allowed = vec![STAMP_FILE_NAME, JOURNAL_FILE_NAME];
    allowed.extend_from_slice(expected_new_artifacts);
    snapshot.assert_unchanged_except_top_level_files(dir, &allowed);
}

#[test]
fn journal_crash_at_every_point() {
    // Design §4.3 rows, in order; the current-stamp cleanup row is deliberately last.
    #[derive(Clone, Copy)]
    enum CrashPoint {
        BeforeChainStarted,
        AfterChainStarted,
        InsideFirstStep,
        AfterFirstStepFinished,
        InsideLastStep,
        AllStepsFinishedBeforeIntegrity,
        TornLastRecord,
        CorruptNonFinalRecord,
        RuntimeAppendFailure,
        RacingLockLoser,
        StampWrittenBeforeChainFinished,
    }
    let points = [
        CrashPoint::BeforeChainStarted,
        CrashPoint::AfterChainStarted,
        CrashPoint::InsideFirstStep,
        CrashPoint::AfterFirstStepFinished,
        CrashPoint::InsideLastStep,
        CrashPoint::AllStepsFinishedBeforeIntegrity,
        CrashPoint::TornLastRecord,
        CrashPoint::CorruptNonFinalRecord,
        CrashPoint::RuntimeAppendFailure,
        CrashPoint::RacingLockLoser,
        CrashPoint::StampWrittenBeforeChainFinished,
    ];

    for point in points {
        let temp = temp_dir();
        let dir = temp.path();
        let old_stamp = seed_old_dir(dir);
        match point {
            CrashPoint::BeforeChainStarted => {
                for create_empty_journal in [false, true] {
                    if create_empty_journal {
                        fs::write(dir.join(JOURNAL_FILE_NAME), [])
                            .expect("seed empty pre-chain journal");
                    }
                    let snapshot = DirSnapshot::take(dir);
                    run_from_crash_prefix(
                        dir,
                        &snapshot,
                        &old_stamp,
                        &["e0001_0002", "e0002_0003"],
                        &[FIRST_ARTIFACT, SECOND_ARTIFACT],
                    );
                    if create_empty_journal {
                        break;
                    }
                    fs::remove_file(dir.join(JOURNAL_FILE_NAME))
                        .expect("restore absent-journal crash point");
                    fs::remove_file(dir.join(FIRST_ARTIFACT)).expect("remove first marker");
                    fs::remove_file(dir.join(SECOND_ARTIFACT)).expect("remove second marker");
                    fs::write(dir.join(STAMP_FILE_NAME), &old_stamp).expect("restore old stamp");
                }
            }
            CrashPoint::AfterChainStarted => {
                seed_journal(dir, &[chain_started()]);
                let snapshot = DirSnapshot::take(dir);
                run_from_crash_prefix(
                    dir,
                    &snapshot,
                    &old_stamp,
                    &["e0001_0002", "e0002_0003"],
                    &[FIRST_ARTIFACT, SECOND_ARTIFACT],
                );
            }
            CrashPoint::InsideFirstStep => {
                seed_journal(dir, &[chain_started(), first_started()]);
                write_marker(dir, FIRST_ARTIFACT, b"e0001_0002").expect("seed partial first step");
                let snapshot = DirSnapshot::take(dir);
                run_from_crash_prefix(
                    dir,
                    &snapshot,
                    &old_stamp,
                    &["e0001_0002", "e0002_0003"],
                    &[SECOND_ARTIFACT],
                );
            }
            CrashPoint::AfterFirstStepFinished => {
                seed_journal(dir, &[chain_started(), first_started(), first_finished()]);
                write_marker(dir, FIRST_ARTIFACT, b"e0001_0002").expect("seed finished first step");
                let snapshot = DirSnapshot::take(dir);
                run_from_crash_prefix(
                    dir,
                    &snapshot,
                    &old_stamp,
                    &["e0002_0003"],
                    &[SECOND_ARTIFACT],
                );
            }
            CrashPoint::InsideLastStep => {
                seed_journal(
                    dir,
                    &[
                        chain_started(),
                        first_started(),
                        first_finished(),
                        second_started(),
                    ],
                );
                write_marker(dir, FIRST_ARTIFACT, b"e0001_0002").expect("seed first step");
                write_marker(dir, SECOND_ARTIFACT, b"e0002_0003").expect("seed partial last step");
                let snapshot = DirSnapshot::take(dir);
                run_from_crash_prefix(dir, &snapshot, &old_stamp, &["e0002_0003"], &[]);
            }
            CrashPoint::AllStepsFinishedBeforeIntegrity => {
                seed_journal(
                    dir,
                    &[
                        chain_started(),
                        first_started(),
                        first_finished(),
                        second_started(),
                        second_finished(),
                    ],
                );
                write_marker(dir, FIRST_ARTIFACT, b"e0001_0002").expect("seed first step");
                write_marker(dir, SECOND_ARTIFACT, b"e0002_0003").expect("seed second step");
                let snapshot = DirSnapshot::take(dir);
                run_from_crash_prefix(dir, &snapshot, &old_stamp, &[], &[]);
            }
            CrashPoint::TornLastRecord => {
                for invalid_tail in [false, true] {
                    let mut bytes = Vec::new();
                    append_record_bytes(&mut bytes, &chain_started());
                    if invalid_tail {
                        bytes.extend_from_slice(b"{invalid final JSON}\n");
                    } else {
                        bytes.extend(
                            serde_json::to_vec(&first_started())
                                .expect("serialize missing-LF step record"),
                        );
                    }
                    fs::write(dir.join(JOURNAL_FILE_NAME), bytes).expect("write torn final record");
                    let snapshot = DirSnapshot::take(dir);
                    run_from_crash_prefix(
                        dir,
                        &snapshot,
                        &old_stamp,
                        &["e0001_0002", "e0002_0003"],
                        &[FIRST_ARTIFACT, SECOND_ARTIFACT],
                    );
                    if !invalid_tail {
                        fs::remove_file(dir.join(JOURNAL_FILE_NAME))
                            .expect("remove resumed journal");
                        fs::remove_file(dir.join(FIRST_ARTIFACT))
                            .expect("remove first step marker");
                        fs::remove_file(dir.join(SECOND_ARTIFACT))
                            .expect("remove second step marker");
                        fs::write(dir.join(STAMP_FILE_NAME), &old_stamp)
                            .expect("restore old stamp bytes");
                    }
                }
            }
            CrashPoint::CorruptNonFinalRecord => {
                let mut bytes = Vec::new();
                append_record_bytes(&mut bytes, &chain_started());
                bytes.extend_from_slice(b"not-json\n");
                bytes.extend(
                    serde_json::to_vec(&first_started()).expect("serialize trailing record"),
                );
                fs::write(dir.join(JOURNAL_FILE_NAME), bytes).expect("write corrupt middle record");
                let snapshot = DirSnapshot::take(dir);
                let calls = Arc::new(Mutex::new(Vec::new()));
                let (first, second) = step_pair(Arc::clone(&calls));
                let steps: [&dyn FormatStep; 2] = [&first, &second];
                let integrity_calls = std::cell::Cell::new(0);
                let invalid = run_migration(dir, policy(3, 1), &steps, BINARY_VERSION, &|_| {
                    integrity_calls.set(integrity_calls.get() + 1);
                    Ok(())
                });
                assert!(matches!(&invalid, Err(MigrationError::Journal { .. })));
                assert!(calls.lock().expect("step call log lock").is_empty());
                assert_eq!(integrity_calls.get(), 0);
                snapshot.assert_unchanged(dir);
            }
            CrashPoint::RuntimeAppendFailure => {
                let calls = Arc::new(Mutex::new(Vec::new()));
                let first = TraceStep {
                    descriptor: &FIRST_DESCRIPTOR,
                    artifact: FIRST_ARTIFACT,
                    calls: Arc::clone(&calls),
                    disrupt_append: true,
                };
                let second = TraceStep {
                    descriptor: &SECOND_DESCRIPTOR,
                    artifact: SECOND_ARTIFACT,
                    calls: Arc::clone(&calls),
                    disrupt_append: false,
                };
                let steps: [&dyn FormatStep; 2] = [&first, &second];
                let integrity_calls = std::cell::Cell::new(0);
                let invalid = run_migration(dir, policy(3, 1), &steps, BINARY_VERSION, &|_| {
                    integrity_calls.set(integrity_calls.get() + 1);
                    Ok(())
                });
                assert!(matches!(&invalid, Err(MigrationError::Journal { .. })));
                assert_eq!(
                    calls.lock().expect("step call log lock").as_slice(),
                    ["e0001_0002"]
                );
                assert_eq!(integrity_calls.get(), 0);
                assert_stamp_epoch(dir, 1);
                let saved = dir.join("journal-held-for-append-failure.bin");
                let saved_records = read_records_from_file(&saved);
                assert!(matches!(saved_records.as_slice(), [
                    JournalRecord::ChainStarted { .. },
                    JournalRecord::StepStarted { module, .. }
                ] if module == "e0001_0002"));
                fs::remove_dir(dir.join(JOURNAL_FILE_NAME)).expect("remove journal obstruction");
                fs::rename(&saved, dir.join(JOURNAL_FILE_NAME))
                    .expect("restore interrupted journal");
                let retry_calls = Arc::new(Mutex::new(Vec::new()));
                let (retry_first, retry_second) = step_pair(Arc::clone(&retry_calls));
                let retry_steps: [&dyn FormatStep; 2] = [&retry_first, &retry_second];
                let before_retry = DirSnapshot::take(dir);
                run_migration(dir, policy(3, 1), &retry_steps, BINARY_VERSION, &|_| Ok(()))
                    .expect("retry resumes after append path becomes writable");
                assert_eq!(
                    retry_calls.lock().expect("retry call log lock").as_slice(),
                    ["e0001_0002", "e0002_0003"]
                );
                before_retry.assert_unchanged_except_top_level_files(
                    dir,
                    &[
                        STAMP_FILE_NAME,
                        JOURNAL_FILE_NAME,
                        SECOND_ARTIFACT,
                        "journal-held-for-append-failure.bin",
                    ],
                );
                assert!(!saved.exists());
            }
            CrashPoint::RacingLockLoser => {
                seed_journal(dir, &[chain_started()]);
                let owner = crate::maintenance_lock::acquire(dir).expect("first run owns lock");
                let snapshot = DirSnapshot::take(dir);
                let before = fs::read(dir.join(JOURNAL_FILE_NAME)).expect("capture journal bytes");
                assert!(crate::maintenance_lock::acquire(dir).is_err());
                drop(owner);
                snapshot.assert_unchanged(dir);
                assert_eq!(
                    fs::read(dir.join(JOURNAL_FILE_NAME)).expect("loser leaves journal"),
                    before
                );
                // The runner has no lock boundary; never call it as an unguarded contender.
            }
            CrashPoint::StampWrittenBeforeChainFinished => {
                seed_journal(
                    dir,
                    &[
                        chain_started(),
                        first_started(),
                        first_finished(),
                        second_started(),
                        second_finished(),
                    ],
                );
                write_marker(dir, FIRST_ARTIFACT, b"e0001_0002").expect("seed first step");
                write_marker(dir, SECOND_ARTIFACT, b"e0002_0003").expect("seed second step");
                let final_stamp = FormatStamp::with_epoch(3, BINARY_VERSION, 10)
                    .with_last_format_migration(LastFormatMigration {
                        id: "e0002_0003".to_owned(),
                        from_epoch: 2,
                        to_epoch: 3,
                        unix_ms: 10,
                    });
                crate::format_stamp::write_stamp(dir, &final_stamp).expect("seed new stamp");
                assert_eq!(
                    decode(
                        &fs::read(dir.join(STAMP_FILE_NAME)).expect("current stamp is retained"),
                        dir,
                    )
                    .expect("current stamp decodes")
                    .format_epoch(),
                    3
                );
                let stamp_snapshot = DirSnapshot::take(dir);
                let calls = Arc::new(Mutex::new(Vec::new()));
                let (first, second) = step_pair(Arc::clone(&calls));
                let steps: [&dyn FormatStep; 2] = [&first, &second];
                let integrity_calls = std::cell::Cell::new(0);
                let already_current =
                    run_migration(dir, policy(3, 1), &steps, BINARY_VERSION, &|_| {
                        integrity_calls.set(integrity_calls.get() + 1);
                        Ok(())
                    });
                assert!(already_current.is_ok(), "current-stamp rerun is a no-op");
                assert!(calls.lock().expect("step call log lock").is_empty());
                assert_eq!(integrity_calls.get(), 0);
                stamp_snapshot.assert_unchanged(dir);

                let writer_check = check_data_dir(
                    dir,
                    StampMode::Writer,
                    &IgnoreSet::none(),
                    &EpochPolicy::new(3, 1),
                );
                assert!(
                    writer_check.is_ok(),
                    "current stamp remains writer-openable"
                );
                stamp_snapshot.assert_unchanged_except_top_level_files(
                    dir,
                    &[STAMP_FILE_NAME, JOURNAL_FILE_NAME],
                );
                assert!(
                    !dir.join(JOURNAL_FILE_NAME).exists(),
                    "design §4.3 requires writer startup to remove a leftover completed journal"
                );
            }
        }
    }
}

fn read_records_from_file(path: &Path) -> Vec<JournalRecord> {
    crate::format_stamp::migration::parse_records(
        &fs::read(path).expect("read preserved append-failure journal"),
    )
    .expect("parse preserved append-failure journal")
}
