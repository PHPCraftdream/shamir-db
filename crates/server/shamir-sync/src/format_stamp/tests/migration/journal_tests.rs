use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::format_stamp::migration::{
    parse_records, read, JournalRecord, JournalWriter, JOURNAL_FILE_NAME,
};
use crate::format_stamp::tests::support::dir_snapshot::DirSnapshot;

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
    let mut torn = serde_json::to_vec(&third).expect("serialize third record");
    let mut file = OpenOptions::new()
        .append(true)
        .open(dir.join(JOURNAL_FILE_NAME))
        .expect("open journal for append");
    file.write_all(&torn).expect("append torn record");
    torn.clear();
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
