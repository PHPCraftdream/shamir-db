use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::format_stamp::migration::{
    parse_records, read, JournalRecord, JournalWriter, MigrationError, ModuleDescriptor,
    JOURNAL_FILE_NAME,
};
use crate::format_stamp::tests::support::dir_snapshot::DirSnapshot;
use crate::format_stamp::tests::support::stamp_fixture::{
    policy, temp_dir, write_stamp_with_epoch,
};

fn record(index: usize) -> JournalRecord {
    match index {
        0 => JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 3,
            binary_version: "0.1.0-test".to_owned(),
            modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
            unix_ms: 1,
        },
        1 => JournalRecord::StepStarted {
            module: "e0001_0002".to_owned(),
            from_epoch: 1,
            to_epoch: 2,
            unix_ms: 2,
        },
        2 => JournalRecord::StepFinished {
            module: "e0001_0002".to_owned(),
            unix_ms: 3,
        },
        3 => JournalRecord::StepStarted {
            module: "e0002_0003".to_owned(),
            from_epoch: 2,
            to_epoch: 3,
            unix_ms: 4,
        },
        4 => JournalRecord::StepFinished {
            module: "e0002_0003".to_owned(),
            unix_ms: 5,
        },
        5 => JournalRecord::ChainFinished {
            to_epoch: 3,
            unix_ms: 6,
        },
        _ => JournalRecord::ChainFinished {
            to_epoch: 3,
            unix_ms: 7,
        },
    }
}

#[test]
fn journal_append_is_atomic_and_fsynced_per_record() {
    // Fresh handles prove completed-prefix visibility and torn-suffix recovery;
    // the source pin checks required sync ordering, not power-loss durability.
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let writer = JournalWriter::create(dir).expect("create journal");
    let mut expected = Vec::new();
    for index in 0..6 {
        let next = record(index);
        writer.append(&next).expect("append one complete record");
        expected.push(next);
        assert_eq!(
            read(dir).expect("fresh reader sees completed append prefix"),
            expected,
            "record {index} must be wholly visible before the next append"
        );
    }
    drop(writer);

    // Test each suffix from the same completed prefix to avoid cross-case contamination.
    let complete_bytes = fs::read(dir.join(JOURNAL_FILE_NAME)).expect("read complete journal");
    let complete = expected.clone();
    let last = record(6);
    for suffix in [
        serde_json::to_vec(&last).expect("serialize complete JSON without LF"),
        b"{invalid final JSON}\n".to_vec(),
    ] {
        fs::write(dir.join(JOURNAL_FILE_NAME), &complete_bytes)
            .expect("restore complete prefix before torn suffix");
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join(JOURNAL_FILE_NAME))
            .expect("open journal to append torn suffix");
        file.write_all(&suffix).expect("append torn suffix");
        assert_eq!(
            read(dir).expect("torn trailing record is discarded"),
            complete,
            "completed records survive suffix {:?}",
            suffix
        );
    }

    let writer_source = include_str!("../../migration/journal/writer.rs");
    let append_body = writer_source
        .split("pub fn append(&self, record: &JournalRecord)")
        .nth(1)
        .expect("source contains JournalWriter::append");
    let append_body = append_body
        .split("\n    }")
        .next()
        .expect("append function body is delimited");
    let write_position = append_body
        .find("file.write_all(&line)")
        .expect("append writes the full record line");
    let sync_position = append_body
        .find("file.sync_all()")
        .expect("each append syncs its file handle");
    let write_error = append_body[write_position..]
        .find(".map_err(|source| journal_error(&self.dir, source))?")
        .expect("write error propagates from append");
    let sync_error = append_body[sync_position..]
        .find(".map_err(|source| journal_error(&self.dir, source))")
        .expect("sync error propagates from append");
    assert!(write_position < sync_position);
    assert!(write_error < sync_position - write_position);
    assert!(sync_error > 0);
    assert!(append_body.contains("OpenOptions::new()"));
    assert!(append_body.contains(".append(true)"));
}

static FIRST_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0001_0002",
    from_epoch: 1,
    to_epoch: 2,
    summary: "first journal probe",
    touches: &[],
    work_estimate: "none",
};

static SECOND_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0002_0003",
    from_epoch: 2,
    to_epoch: 3,
    summary: "second journal probe",
    touches: &[],
    work_estimate: "none",
};

struct ProbeStep {
    descriptor: &'static ModuleDescriptor,
    calls: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl crate::format_stamp::migration::FormatStep for ProbeStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        self.descriptor
    }

    fn apply(
        &self,
        _context: &mut crate::format_stamp::migration::StepContext<'_>,
    ) -> Result<(), MigrationError> {
        self.calls
            .lock()
            .expect("step log lock")
            .push(self.descriptor.id);
        Ok(())
    }

    fn plan(
        &self,
        _context: &crate::format_stamp::migration::StepContext<'_>,
    ) -> Result<Vec<crate::format_stamp::migration::PlanLine>, MigrationError> {
        Ok(Vec::new())
    }
}

#[test]
fn journal_foreign_chain_refused() {
    let cases = [
        (
            "foreign-module-list",
            JournalRecord::ChainStarted {
                from_epoch: 1,
                to_epoch: 3,
                binary_version: "0.1.0-test".to_owned(),
                modules: vec!["foreign_a".to_owned(), "foreign_b".to_owned()],
                unix_ms: 1,
            },
        ),
        (
            "foreign-binary-version",
            JournalRecord::ChainStarted {
                from_epoch: 1,
                to_epoch: 3,
                binary_version: "9.9.9-foreign".to_owned(),
                modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
                unix_ms: 1,
            },
        ),
        (
            "foreign-from-epoch",
            JournalRecord::ChainStarted {
                from_epoch: 2,
                to_epoch: 3,
                binary_version: "0.1.0-test".to_owned(),
                modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
                unix_ms: 1,
            },
        ),
        (
            "foreign-to-epoch",
            JournalRecord::ChainStarted {
                from_epoch: 1,
                to_epoch: 4,
                binary_version: "0.1.0-test".to_owned(),
                modules: vec!["e0001_0002".to_owned(), "e0002_0003".to_owned()],
                unix_ms: 1,
            },
        ),
    ];
    for (name, foreign_start) in cases {
        let temp = temp_dir();
        let dir = temp.path();
        write_stamp_with_epoch(dir, 1);
        fs::write(dir.join("payload.bin"), b"untouched payload").expect("seed payload");
        let writer = JournalWriter::create(dir).expect("create journal");
        writer.append(&foreign_start).expect("write foreign chain");
        drop(writer);
        let snapshot = DirSnapshot::take(dir);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let first = ProbeStep {
            descriptor: &FIRST_DESCRIPTOR,
            calls: std::sync::Arc::clone(&calls),
        };
        let second = ProbeStep {
            descriptor: &SECOND_DESCRIPTOR,
            calls: std::sync::Arc::clone(&calls),
        };
        let steps: [&dyn crate::format_stamp::migration::FormatStep; 2] = [&first, &second];
        let integrity_calls = std::cell::Cell::new(0);

        let result = crate::format_stamp::migration::run_migration(
            dir,
            policy(3, 1),
            &steps,
            "0.1.0-test",
            &|_| {
                integrity_calls.set(integrity_calls.get() + 1);
                Ok(())
            },
        );

        assert!(
            matches!(&result, Err(MigrationError::BrokenChain { .. })),
            "{name}: expected BrokenChain, got {result:?}"
        );
        assert!(calls.lock().expect("step log lock").is_empty());
        assert_eq!(integrity_calls.get(), 0);
        snapshot.assert_unchanged(dir);
    }
}

#[test]
fn journal_unwritable_refuses_before_first_step() {
    let temp = temp_dir();
    let dir = temp.path();
    write_stamp_with_epoch(dir, 1);
    fs::write(dir.join("payload.bin"), b"untouched payload").expect("seed payload");
    fs::create_dir(dir.join(JOURNAL_FILE_NAME)).expect("block journal creation with directory");
    let snapshot = DirSnapshot::take(dir);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let step = ProbeStep {
        descriptor: &FIRST_DESCRIPTOR,
        calls: std::sync::Arc::clone(&calls),
    };
    let steps: [&dyn crate::format_stamp::migration::FormatStep; 1] = [&step];
    let integrity_calls = std::cell::Cell::new(0);

    let result = crate::format_stamp::migration::run_migration(
        dir,
        policy(2, 1),
        &steps,
        "0.1.0-test",
        &|_| {
            integrity_calls.set(integrity_calls.get() + 1);
            Ok(())
        },
    );

    assert!(matches!(&result, Err(MigrationError::Journal { .. })));
    assert!(calls.lock().expect("step log lock").is_empty());
    assert_eq!(integrity_calls.get(), 0);
    snapshot.assert_unchanged(dir);
    assert_eq!(
        crate::format_stamp::codec::decode(
            &fs::read(dir.join(crate::format_stamp::codec::STAMP_FILE_NAME)).expect("old stamp"),
            dir,
        )
        .expect("old stamp decodes")
        .format_epoch(),
        1
    );
}

#[test]
fn journal_open_existing_repairs_torn_suffix_before_repeated_appends() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let chain = JournalRecord::ChainStarted {
        from_epoch: 1,
        to_epoch: 2,
        binary_version: "0.1.0".to_owned(),
        modules: vec!["e0001_0002".to_owned()],
        unix_ms: 1,
    };
    let started = JournalRecord::StepStarted {
        module: "e0001_0002".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 2,
    };
    let finished = JournalRecord::StepFinished {
        module: "e0001_0002".to_owned(),
        unix_ms: 3,
    };
    let chain_line = [
        serde_json::to_vec(&chain).expect("serialize chain"),
        vec![b'\n'],
    ]
    .concat();
    let torn_valid_line = serde_json::to_vec(&started).expect("serialize started");
    let mut initial_bytes = chain_line.clone();
    initial_bytes.extend_from_slice(&torn_valid_line);
    fs::write(dir.join(JOURNAL_FILE_NAME), &initial_bytes).expect("write torn valid tail");

    let writer = JournalWriter::open_existing(dir).expect("repair missing-LF tail");
    writer
        .append(&started)
        .expect("append started after repair");
    drop(writer);
    let first_resume_bytes =
        fs::read(dir.join(JOURNAL_FILE_NAME)).expect("read after first resume");
    assert!(first_resume_bytes.starts_with(&chain_line));
    assert_eq!(
        read(dir).expect("read after first resume"),
        vec![chain.clone(), started.clone()]
    );

    let mut bytes_with_invalid_tail = first_resume_bytes;
    bytes_with_invalid_tail.extend_from_slice(b"{invalid json}\n");
    fs::write(dir.join(JOURNAL_FILE_NAME), &bytes_with_invalid_tail)
        .expect("write invalid LF tail");
    let writer = JournalWriter::open_existing(dir).expect("repair invalid LF tail");
    writer
        .append(&finished)
        .expect("append finished after repair");
    drop(writer);
    let second_resume_bytes =
        fs::read(dir.join(JOURNAL_FILE_NAME)).expect("read after second resume");
    assert!(second_resume_bytes.starts_with(&chain_line));
    assert_eq!(
        read(dir).expect("read after second resume"),
        vec![chain, started, finished]
    );

    let writer = JournalWriter::open_existing(dir).expect("reopen complete journal");
    drop(writer);
    assert_eq!(
        read(dir).expect("read after repeated resume"),
        vec![
            JournalRecord::ChainStarted {
                from_epoch: 1,
                to_epoch: 2,
                binary_version: "0.1.0".to_owned(),
                modules: vec!["e0001_0002".to_owned()],
                unix_ms: 1,
            },
            JournalRecord::StepStarted {
                module: "e0001_0002".to_owned(),
                from_epoch: 1,
                to_epoch: 2,
                unix_ms: 2,
            },
            JournalRecord::StepFinished {
                module: "e0001_0002".to_owned(),
                unix_ms: 3,
            },
        ]
    );
}

#[test]
fn journal_open_existing_refuses_corrupt_middle_without_writes() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let first = serde_json::to_vec(&JournalRecord::StepStarted {
        module: "a".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 1,
    })
    .expect("serialize first record");
    let last = serde_json::to_vec(&JournalRecord::StepFinished {
        module: "a".to_owned(),
        unix_ms: 2,
    })
    .expect("serialize last record");
    let mut bytes = first;
    bytes.extend_from_slice(b"\nnot-json\n");
    bytes.extend_from_slice(&last);
    bytes.push(b'\n');
    fs::write(dir.join(JOURNAL_FILE_NAME), bytes).expect("write corrupt journal");
    let before = DirSnapshot::take(dir);

    assert!(JournalWriter::open_existing(dir).is_err());

    before.assert_unchanged(dir);
}

#[test]
fn journal_round_trip_preserves_records() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let writer = JournalWriter::create(dir).expect("create journal");
    let expected = vec![
        JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 2,
            binary_version: "0.1.0".to_owned(),
            modules: vec!["e0001_0002".to_owned()],
            unix_ms: 10,
        },
        JournalRecord::StepStarted {
            module: "e0001_0002".to_owned(),
            from_epoch: 1,
            to_epoch: 2,
            unix_ms: 11,
        },
        JournalRecord::StepFinished {
            module: "e0001_0002".to_owned(),
            unix_ms: 12,
        },
        JournalRecord::ChainFinished {
            to_epoch: 2,
            unix_ms: 13,
        },
    ];
    for record in &expected {
        writer.append(record).expect("append journal record");
    }
    drop(writer);

    assert_eq!(read(dir).expect("read journal"), expected);
    let bytes = fs::read(dir.join(JOURNAL_FILE_NAME)).expect("read raw journal");
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        assert!(line.ends_with(b"\n"));
        serde_json::from_slice::<serde_json::Value>(&line[..line.len() - 1])
            .expect("journal line is valid JSON");
    }
}

#[test]
fn journal_torn_final_line_is_discarded() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let writer = JournalWriter::create(dir).expect("create journal");
    let first = JournalRecord::StepStarted {
        module: "a".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 1,
    };
    let second = JournalRecord::StepFinished {
        module: "a".to_owned(),
        unix_ms: 2,
    };
    writer.append(&first).expect("append first record");
    writer.append(&second).expect("append second record");
    drop(writer);
    let third = JournalRecord::ChainFinished {
        to_epoch: 2,
        unix_ms: 3,
    };
    let torn = serde_json::to_vec(&third).expect("serialize torn record");
    let mut file = OpenOptions::new()
        .append(true)
        .open(dir.join(JOURNAL_FILE_NAME))
        .expect("open journal for append");
    file.write_all(&torn).expect("append torn record");
    assert_eq!(read(dir).expect("read journal"), vec![first, second]);
}

#[test]
fn journal_torn_final_line_invalid_json_is_discarded() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    let writer = JournalWriter::create(dir).expect("create journal");
    let first = JournalRecord::StepStarted {
        module: "a".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 1,
    };
    let second = JournalRecord::StepFinished {
        module: "a".to_owned(),
        unix_ms: 2,
    };
    writer.append(&first).expect("append first record");
    writer.append(&second).expect("append second record");
    drop(writer);
    let mut file = OpenOptions::new()
        .append(true)
        .open(dir.join(JOURNAL_FILE_NAME))
        .expect("open journal for append");
    file.write_all(b"{not json")
        .expect("append invalid torn line");
    assert_eq!(read(dir).expect("read journal"), vec![first, second]);
}

#[test]
fn journal_corrupt_middle_line_refused() {
    let first = serde_json::to_vec(&JournalRecord::StepStarted {
        module: "a".to_owned(),
        from_epoch: 1,
        to_epoch: 2,
        unix_ms: 1,
    })
    .expect("serialize first record");
    let last = serde_json::to_vec(&JournalRecord::StepFinished {
        module: "a".to_owned(),
        unix_ms: 2,
    })
    .expect("serialize last record");
    let mut bytes = first;
    bytes.extend_from_slice(b"\ngarbage\n");
    bytes.extend_from_slice(&last);
    bytes.push(b'\n');
    assert!(parse_records(&bytes).is_err());
}

#[test]
fn journal_absent_or_empty_means_fresh() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let dir = temp.path();
    assert_eq!(read(dir).expect("read absent journal"), Vec::new());
    fs::write(dir.join(JOURNAL_FILE_NAME), []).expect("create empty journal");
    assert_eq!(read(dir).expect("read empty journal"), Vec::new());
}
