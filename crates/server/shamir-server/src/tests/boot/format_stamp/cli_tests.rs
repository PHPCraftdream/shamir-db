//! Offline diagnostic wiring tests (design 6.2): doctor and access-tree
//! never create a stamp and refuse a newer epoch with the directory
//! unchanged.

use std::fs;

use shamir_sync::format_stamp::{inspect, StampState, STAMP_FILE_NAME};

use crate::access_tree::AccessTreeArgs;
use crate::doctor::{DoctorArgs, DoctorReport};

use super::support::{add_data_entry, config, write_same_version_stamp, DirSnapshot};

fn stampless(dir: &std::path::Path) -> bool {
    matches!(
        inspect(dir),
        StampState::Absent {
            leftover_tmp: false
        }
    )
}

fn doctor_args(apply: bool) -> DoctorArgs {
    DoctorArgs {
        db: None,
        repo: None,
        table: None,
        apply,
        pretty: false,
        json: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_does_not_create_a_stamp_in_stampless_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    let report: DoctorReport = crate::doctor::run(&config(&dir), &doctor_args(false))
        .await
        .expect("doctor runs");
    assert!(!report.healthy || report.total_tables == 0);
    assert!(stampless(&dir), "doctor must not stamp");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_tree_does_not_create_a_stamp_in_stampless_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    let args = AccessTreeArgs {
        depth: None,
        db: None,
        pretty: false,
        connect: None,
        server_name: "shamir".into(),
        user: None,
        password: None,
    };
    let _ = crate::access_tree::fetch_tree(&config(&dir), &args)
        .await
        .expect("access tree offline");
    assert!(stampless(&dir), "access-tree must not stamp");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_refuses_newer_epoch_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    write_same_version_stamp(&dir, u64::MAX);
    let before = DirSnapshot::take(&dir);
    let err = crate::doctor::run(&config(&dir), &doctor_args(false))
        .await
        .expect_err("refused");
    assert!(
        err.to_string().contains("format epoch"),
        "unexpected error: {err}"
    );
    before.assert_unchanged(&dir);
    assert!(dir.join(STAMP_FILE_NAME).is_file(), "stamp still there");
}
