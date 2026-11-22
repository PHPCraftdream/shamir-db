//! Audit log rotation: when the active file crosses `max_size_bytes`,
//! it's renamed to `audit_log.log.<unix_nanos>` and a fresh file is
//! opened for subsequent writes. The HMAC chain continues unbroken.

use std::time::Duration;

use shamir_connect::server::audit_chain::{AuditAppender, AuditChain, AuditEntry};
use shamir_server::audit_appender::FjallAuditAppender;
use tempfile::TempDir;

fn make_entry(seq: u64) -> AuditEntry {
    AuditEntry {
        seq,
        ts_ns: 1_000_000_000 + seq,
        event: format!("evt_{seq:04}"),
        transport: "tcp".into(),
        user: "alice".into(),
        ip_subnet: "127.0.0.0/24".into(),
        session_id_prefix: [0u8; 8],
        result: "ok".into(),
        details_canonical_msgpack: vec![],
        prev_hmac: [0xAB; 32],
        hmac: [(seq & 0xff) as u8; 32],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_kicks_in_after_threshold() {
    let temp = TempDir::new().unwrap();
    // Tiny 1 KB threshold so the test only has to write a handful of entries.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1024), 0).unwrap();

    // Write enough entries to comfortably cross 1 KB. Each log line is
    // ~250 bytes, so 10 entries = ~2.5 KB.
    for i in 0..10 {
        appender
            .append_entry(&make_entry(i))
            .expect("append succeeds");
    }

    // Wait a tick for any in-flight rotation rename to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Find rotated files: the active file is `audit.log` (constant from
    // crate); rotated files have names like `audit.log.<digits>`.
    let entries: Vec<_> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        !entries.is_empty(),
        "expected at least one rotated audit.log.* file, found {:?}",
        entries
    );

    // Active file should still be present (just freshly opened post-rotation).
    let active = temp.path().join("audit.log");
    assert!(
        active.exists(),
        "active audit.log must be reopened after rotation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_rotation_below_threshold() {
    let temp = TempDir::new().unwrap();
    // 1 MB threshold — well above what 5 entries write.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1_000_000), 0).unwrap();
    for i in 0..5 {
        appender
            .append_entry(&make_entry(i))
            .expect("append succeeds");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(rotated_count, 0, "no rotation expected below threshold");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_disabled_when_max_size_is_none() {
    let temp = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict_with_rotation(temp.path(), None, 0).unwrap();
    for i in 0..50 {
        appender
            .append_entry(&make_entry(i))
            .expect("append succeeds");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(rotated_count, 0, "rotation must be off when None");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hmac_chain_is_intact_across_rotation() {
    // Critical security regression test: rotation must NOT break the
    // HMAC chain. After multiple rotations, reading every file in
    // chronological order must yield a chain that
    // `AuditChain::verify_chain` accepts (every prev_hmac matches the
    // previous entry's hmac, hmacs are recomputable from the chain key).

    let temp = TempDir::new().unwrap();
    let key = [0xC0u8; 32];
    let chain = AuditChain::new(key);
    // Tiny 1 KB threshold to force several rotations.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1024), 0).unwrap();

    // Write 30 entries via the chain so each one carries a real
    // `prev_hmac → hmac` link (the make_entry helper above uses fake
    // hmacs and would fail verify_chain by design).
    for i in 0..30u64 {
        let entry = chain
            .append(
                "auth_success",
                "tcp",
                format!("user_{i:02}"),
                "127.0.0.0/24",
                [0u8; 8],
                "ok",
                Vec::new(),
                1_000_000_000 + i,
            )
            .expect("valid audit test entry");
        appender.append_entry(&entry).expect("append succeeds");
    }
    // Brief pause so any rename/file-handle settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Sanity: there ARE rotated files (otherwise this test trivially
    // passes by reading just the active file).
    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert!(
        rotated_count >= 1,
        "expected at least one rotation to actually exercise the chain check"
    );

    // The fix in `read_log_for_verify`: it must read rotated files
    // (lex-sorted) plus the active file, in chronological order.
    let entries =
        FjallAuditAppender::read_log_for_verify(temp.path()).expect("read_log_for_verify");
    assert_eq!(entries.len(), 30, "every entry survives across rotation");

    // The headline assertion: the chain still verifies after rotation.
    AuditChain::verify_chain(&key, &entries).expect("HMAC chain unbroken across rotation");
}

// ---------------------------------------------------------------------------
// Retention sweep tests (audit.retention_days).
//
// The sweep runs piggyback on rotation: when a new rotated file is created,
// sibling rotated files older than `retention_days` are deleted. File age is
// determined from the filesystem mtime of each rotated file, not from the
// embedded `unix_nanos` suffix in the filename; the fixtures below seed
// explicit mtimes with `std::fs::FileTimes` so old/young ages are
// independent of the (legacy, backwards-named) archive names.
// ---------------------------------------------------------------------------

/// Set the filesystem modified time of `path` to `age_ns` nanoseconds ago,
/// cross-platform via `std::fs::FileTimes`.
fn set_mtime_age(path: &std::path::Path, age_ns: u64) {
    let modified = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_nanos(age_ns))
        .expect("mtime before now");
    let f = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open archive to set mtime");
    f.set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("set archive mtime");
}

fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

const NANOS_PER_SEC: u64 = 1_000_000_000;
const SECS_PER_DAY: u64 = 86400;
const NANOS_PER_DAY: u64 = SECS_PER_DAY * NANOS_PER_SEC;

/// Seed one rotated archive with genuine chain-generated records: the
/// entries are appended through the shared HMAC chain (so every row
/// authenticates and the sequences are sequential), the strict appender
/// is closed/dropped to release its Fjall checkpoint store, and the
/// active log is renamed to the requested `audit.log.<nanos>` archive
/// name — the same shape `rotate_locked` produces.
fn seed_archive(
    dir: &std::path::Path,
    chain: &AuditChain,
    seqs: std::ops::Range<u64>,
    archive_ns: u64,
) {
    let appender = FjallAuditAppender::open_strict(dir).unwrap();
    for i in seqs {
        let entry = chain
            .append(
                "auth_success",
                "tcp",
                format!("user_{i:02}"),
                "127.0.0.0/24",
                [0u8; 8],
                "ok",
                Vec::new(),
                1_000_000_000 + i,
            )
            .expect("valid audit test entry");
        appender.append_entry(&entry).expect("append succeeds");
    }
    drop(appender);
    std::fs::rename(
        dir.join("audit.log"),
        dir.join(format!("audit.log.{archive_ns:020}")),
    )
    .expect("rename active log to archive");
    set_mtime_age(
        &dir.join(format!("audit.log.{archive_ns:020}")),
        now_unix_nanos().saturating_sub(archive_ns),
    );
}

/// Append genuinely chain-linked entries (continuing the archive chain's
/// sequence and HMAC links) to force the size threshold to rotate.
fn append_chain_entries(
    appender: &FjallAuditAppender,
    chain: &AuditChain,
    seqs: std::ops::Range<u64>,
) {
    for i in seqs {
        let entry = chain
            .append(
                "auth_success",
                "tcp",
                format!("user_{i:02}"),
                "127.0.0.0/24",
                [0u8; 8],
                "ok",
                Vec::new(),
                1_000_000_000 + i,
            )
            .expect("valid audit test entry");
        appender.append_entry(&entry).expect("append succeeds");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_deletes_old_rotated_files_only() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 10 * NANOS_PER_DAY; // 10 days ago
    let recent_ns = now - NANOS_PER_DAY; // 1 day ago

    // Pre-create rotated archives containing genuine chain-generated,
    // sequential, authenticated rows (the sweep's ordering relies on the
    // archives' first content sequence).
    let key = [0xC0u8; 32];
    let chain = AuditChain::new(key);
    seed_archive(dir, &chain, 1..11u64, old_ns);
    seed_archive(dir, &chain, 11..21u64, recent_ns);

    // retention_days = 5: the 10-day-old archive exceeds it, the 1-day-old does not.
    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 5).unwrap();

    // Trigger a rotation (1 KB threshold → ~10 entries suffice) with
    // chain-linked rows continuing the same sequence.
    append_chain_entries(appender.as_ref(), &chain, 21..31u64);

    // The old file is gone; the recent file survives.
    assert!(
        !dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "old rotated file should be swept"
    );
    assert!(
        dir.join(format!("audit.log.{recent_ns:020}")).exists(),
        "recent rotated file should survive retention window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_zero_means_no_sweep() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let ancient_ns = now - 365 * NANOS_PER_DAY; // 1 year ago

    std::fs::write(
        dir.join(format!("audit.log.{ancient_ns:020}")),
        b"ancient-should-survive-when-retention-is-0",
    )
    .unwrap();
    set_mtime_age(
        &dir.join(format!("audit.log.{ancient_ns:020}")),
        now - ancient_ns,
    );

    // retention_days = 0: the documented off-switch — no sweep ever.
    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 0).unwrap();

    for i in 0..10 {
        appender
            .append_entry(&make_entry(i))
            .expect("append succeeds");
    }

    assert!(
        dir.join(format!("audit.log.{ancient_ns:020}")).exists(),
        "retention_days=0 must NOT sweep even very old files"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_failure_does_not_break_writes() {
    // A rotated-file-shaped entry that `remove_file` cannot delete (a
    // directory) must NOT propagate an error to the write/rotation caller.
    // Its mtime is age-eligible, yet the sweep must never unlink it.
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 100 * NANOS_PER_DAY; // 100 days ago — well past retention

    // Create a DIRECTORY whose name matches the rotated-file pattern.
    // `fs::remove_file` on a directory fails on both Unix and Windows.
    std::fs::create_dir(dir.join(format!("audit.log.{old_ns:020}"))).unwrap();

    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 5).unwrap();

    // Trigger a rotation — the sweep encounters the undeleatable directory,
    // logs a warn, but the write path is unaffected.
    for i in 0..10 {
        appender
            .append_entry(&make_entry(i))
            .expect("append succeeds"); // must not panic
    }

    // Writes landed successfully despite the sweep failure: the active log
    // file has content. (Cannot use `read_log_for_verify` here because it
    // tries to open the directory-shaped entry as a file, which fails on
    // Windows — the assertion is about write-path resilience, not the chain.)
    let active_contents =
        std::fs::read_to_string(dir.join("audit.log")).expect("active audit.log must be readable");
    assert!(
        !active_contents.is_empty(),
        "entries must be written even when the retention sweep hits an error"
    );

    // The directory survives (remove_file failed).
    assert!(
        dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "directory-shaped rotated entry survives failed remove_file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_runs_in_batched_mode_too() {
    // Regression: the batched-mode constructor must also thread
    // retention_days and sweep on rotation, not just strict mode.
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 10 * NANOS_PER_DAY;

    // A genuine chain-generated archive with sequences 1-10 under an
    // age-eligible (10-day-old) rotation name.
    let key = [0xC1u8; 32];
    let chain = AuditChain::new(key);
    seed_archive(dir, &chain, 1..11u64, old_ns);

    let appender = FjallAuditAppender::open_batched_with_rotation(
        dir,
        Duration::from_millis(50),
        Some(1024),
        5,
    )
    .unwrap();

    // Write enough chain-linked rows to cross the 1 KB threshold, then
    // flush deterministically: `flush_now` drains the buffer, and the
    // flush path performs the rotation and its piggyback retention sweep
    // synchronously — no timing polling needed.
    append_chain_entries(appender.as_ref(), &chain, 11..21u64);
    appender
        .flush_now()
        .await
        .expect("flush lands writes, rotation and sweep");

    assert!(
        !dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "batched-mode retention sweep must delete old rotated files"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_preserves_backwards_named_archive_when_earlier_sequence_archive_is_younger(
) {
    // Regression: the sweep may prune only the oldest contiguous
    // age-eligible prefix in first-content-sequence order. Here the
    // archive holding the EARLIER sequences (1-10) carries a YOUNG name
    // and a YOUNG mtime (1 day ago), while a later-sequence archive
    // (11-20) carries an older, backwards rotation name (10 days ago) with
    // an explicitly seeded OLD mtime (10 days ago) that is age-eligible on
    // its own. Because the earlier-sequence archive is still inside the
    // retention window, the eligible prefix is empty and the backwards
    // named archive must be preserved — its eligible name and mtime alone
    // must not cause deletion.
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let young_ns = now - NANOS_PER_DAY; // 1 day ago — earlier sequences
    let backwards_old_ns = now - 10 * NANOS_PER_DAY; // 10 days ago — later sequences

    let key = [0xC2u8; 32];
    let chain = AuditChain::new(key);
    seed_archive(dir, &chain, 1..11u64, young_ns);
    seed_archive(dir, &chain, 11..21u64, backwards_old_ns);
    // Independent mtimes, decoupled from the legacy suffix names: the
    // earlier-sequence archive stays young, the backwards named one is old.
    set_mtime_age(
        &dir.join(format!("audit.log.{young_ns:020}")),
        NANOS_PER_DAY,
    );
    set_mtime_age(
        &dir.join(format!("audit.log.{backwards_old_ns:020}")),
        10 * NANOS_PER_DAY,
    );

    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 5).unwrap();
    append_chain_entries(appender.as_ref(), &chain, 21..31u64);

    assert!(
        dir.join(format!("audit.log.{backwards_old_ns:020}"))
            .exists(),
        "age-eligible backwards named archive must be preserved while an \
         earlier-sequence archive is inside the retention window"
    );
    assert!(
        dir.join(format!("audit.log.{young_ns:020}")).exists(),
        "earlier-sequence archive inside the retention window must survive"
    );
}
