//! Data-presence and ignore-set tests (decision D8, design 3.1, 6.2).

use crate::format_stamp::io::{data_present, IgnoreSet};

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, add_leftover_tmp, add_lock, temp_dir,
};

#[test]
fn data_presence_ignores_lock_stamp_tmp_and_configured_log() {
    let dir = temp_dir();
    // Absent directory counts as empty.
    assert!(!data_present(dir.path(), &IgnoreSet::none()));
    add_lock(dir.path());
    assert!(!data_present(dir.path(), &IgnoreSet::none()));
    add_leftover_tmp(dir.path());
    assert!(!data_present(dir.path(), &IgnoreSet::none()));
    let stamp = dir.path().join("format.stamp");
    std::fs::write(&stamp, b"frame").expect("write stamp");
    assert!(!data_present(dir.path(), &IgnoreSet::none()));
    // A configured log file inside the data dir is not data either.
    let log = dir.path().join("audit.log");
    std::fs::write(&log, b"log").expect("write log");
    assert!(!data_present(
        dir.path(),
        &IgnoreSet::with_log_file("audit.log")
    ));
    // Without the configured name it IS data.
    assert!(data_present(dir.path(), &IgnoreSet::none()));
}

#[test]
fn data_presence_counts_any_other_entry() {
    // A lone directory entry (e.g. server_meta/).
    let dir = temp_dir();
    add_data(dir.path());
    assert!(data_present(dir.path(), &IgnoreSet::none()));

    // A lone hidden file.
    let dir = temp_dir();
    std::fs::write(dir.path().join(".shamir_restore_epoch"), b"1").expect("write marker");
    assert!(data_present(dir.path(), &IgnoreSet::none()));

    // A lone visible file.
    let dir = temp_dir();
    std::fs::write(dir.path().join("bootstrap_token.txt"), b"t").expect("write token");
    assert!(data_present(dir.path(), &IgnoreSet::none()));
}
