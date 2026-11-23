//! Evidence-cell tests for `shamir_backup_last_success_timestamp_seconds`
//! (`online-backup.md` §4.8, item B): one exported stamp, seeded at boot from
//! the newest durable row, raised by every in-process success, and clamped to
//! "now" exactly ONCE when a future stamp appears.
//!
//! The export cell, the replaceable durable handle and the injected wall clock
//! are all PROCESS-GLOBAL, so every test here is `#[serial]`, starts from a
//! known cell (`set_export_last_success_for_test(0)`) and restores it before it
//! returns — a test in the same binary can never inherit or leave evidence.

use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;
use tempfile::TempDir;

use super::super::super::metrics::{
    backup_metrics_snapshot, install_backup_meta_store, note_success,
    set_export_last_success_for_test, ClockOverride,
};
use crate::server_meta::ServerMetaStore;

/// 2023-11-14T22:13:20Z — arbitrary but fixed, so no assertion reads the real
/// clock. Every second below is derived from it.
const NOW: u64 = 1_700_000_000;

/// A REAL store on a `TempDir` the caller keeps alive for the test.
fn temp_store() -> (TempDir, Arc<ServerMetaStore>) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    let store = Arc::new(ServerMetaStore::open_or_init(&path).expect("open_or_init"));
    (dir, store)
}

/// The exported stamp is the NEWEST of the durable row, the armed-at row and
/// the in-memory success, and a future stamp is clamped to "now" once — the
/// clamped value is what later scrapes report, so the exported age grows.
///
/// Mutants caught: (a) seeding from the durable last-success row alone without
/// the in-memory max → the success assertion sees the old row; (b)
/// `note_success` without `fetch_max` → same assertion; (c) the old per-scrape
/// `min(now)` without write-back → the third scrape follows the advanced clock
/// instead of staying at the first clamped value; (d) `fetch_min` returning the
/// PREVIOUS value without the trailing `min` → the first scrape hands out the
/// future stamp instead of "now".
#[serial]
#[test]
fn last_success_stamp_takes_newest_source_and_never_the_future() {
    set_export_last_success_for_test(0);
    let clock = ClockOverride::set(NOW);

    let (_dir, store) = temp_store();
    store
        .store_backup_last_success_unix_ns(NOW - 900)
        .expect("seed the durable last-success row");
    // Boot WITHOUT a schedule: nothing may be armed, so the export comes from
    // the last-success row alone.
    install_backup_meta_store(Arc::clone(&store), false);
    assert_eq!(
        backup_metrics_snapshot().last_success_timestamp_seconds,
        NOW - 900,
        "a boot must seed the exported stamp from the durable last-success row"
    );

    // A newer in-process success wins over the older row; duration and bytes
    // keep their in-memory source.
    clock.pin(NOW - 300);
    note_success(17, Duration::from_secs(2));
    let after_success = backup_metrics_snapshot();
    assert_eq!(
        after_success.last_success_timestamp_seconds,
        NOW - 300,
        "a newer in-process success must win over the older durable row"
    );
    assert_eq!(after_success.last_bytes, 17, "last_bytes stays in-memory");

    // A forward clock step at success stamps the FUTURE.
    clock.pin(NOW + 86_400);
    note_success(0, Duration::ZERO);
    clock.pin(NOW);
    let first = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(
        first, NOW,
        "the first scrape must report now, not the future"
    );
    let second = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(
        second, first,
        "the second scrape must report the SAME clamped value, not move with the clock"
    );
    clock.advance(120);
    let third = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(
        third, first,
        "age grows: the clamp does not follow the clock"
    );

    drop(clock);
    set_export_last_success_for_test(0);
}

/// Every launch installs its OWN store: a success after the second install is
/// persisted through B, and A is never written again.
///
/// Mutant caught: the former first-wins `OnceLock<Weak<ServerMetaStore>>`,
/// which kept the first boot's handle for the whole process — B's row would
/// stay absent and A's row would take the second stamp.
#[serial]
#[test]
fn holder_replaces_across_installs() {
    set_export_last_success_for_test(0);
    let clock = ClockOverride::set(NOW);

    let (_dir_a, store_a) = temp_store();
    let (_dir_b, store_b) = temp_store();

    install_backup_meta_store(Arc::clone(&store_a), false);
    clock.pin(NOW + 10);
    note_success(1, Duration::from_secs(1));
    let stamp_a = store_a.backup_last_success_unix_ns().expect("read A");
    assert_eq!(stamp_a, Some(NOW + 10), "store A takes the first stamp");

    install_backup_meta_store(Arc::clone(&store_b), false);
    clock.pin(NOW + 20);
    note_success(2, Duration::from_secs(1));
    let stamp_b = store_b.backup_last_success_unix_ns().expect("read B");
    assert_eq!(stamp_b, Some(NOW + 20), "store B takes the second stamp");
    let stamp_a_again = store_a.backup_last_success_unix_ns().expect("A again");
    assert_eq!(stamp_a_again, Some(NOW + 10), "store A is left untouched");

    drop(clock);
    set_export_last_success_for_test(0);
}
