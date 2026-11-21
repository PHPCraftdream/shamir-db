// Regression tests for release-1 AUDIT-CHAIN-RESTART follow-up findings 1, 2
// (appender monotonicity), 3 (writer clamp), 5 (oversized typed rejection)
// and 7 (directory ordering) of the committed brief
// `docs/dev-artifacts/prompts/release/80-audit-chain-review-fixes.md`.
//
// Every test states the parent-state counterfactual in its doc comment: the
// concrete observation that fails before the fix. Private seams are the
// standard `#[cfg(test)]` hook fields on the appender (module ancestors of
// these descendant tests): `dir_sync_hook` replaces the real
// `rotation::sync_parent` call; the internal anchor is asserted through the
// externally visible persisted checkpoint row (`persisted_anchor`).
//
// Windows holds fjall locks until every handle is gone: each
// `load_checkpoint` / `open_recovered_*` call below happens only after the
// appender `Arc` AND every `AuditChainWriter` holding a clone are dropped.

use super::super::super::{rotation, AppenderError, AuditRecoveryError, FjallAuditAppender};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tempfile::TempDir;

type Position = (u64, [u8; 32]);

const KEY: [u8; 32] = [0x5b; 32];
const GENESIS: Position = (1, [0; 32]);
const FLUSH_INTERVAL: Duration = Duration::from_secs(3600);
const ACTIVE: &str = "audit.log";
const ARCHIVE_PREFIX: &str = "audit.log.";
const EVENT: &str = "auth_success";
const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// Far-future rotation name seed: current wall clock plus ~3.17 years, far
/// below `u64::MAX` (~584 years in nanos).
fn future_seed() -> u64 {
    now_unix_nanos() + 100_000_000_000_000_000
}

fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn append(writer: &AuditChainWriter, details: Vec<u8>) -> AuditEntry {
    writer
        .append(
            EVENT,
            "test",
            "user",
            "127.0.0.0/24",
            [7; 8],
            "ok",
            details,
            42,
        )
        .unwrap()
}

fn after(entry: &AuditEntry) -> Position {
    (entry.seq + 1, entry.hmac)
}

/// Shared ordering log for the private test seams.
type OrderLog = Arc<Mutex<Vec<&'static str>>>;

fn push(log: &OrderLog, stage: &'static str) {
    log.lock().unwrap().push(stage);
}

/// The `(seq, hmac, persisted_anchor_at_call)` tuple the meta hook records.
type MetaCall = (Position, Option<Position>);
/// Shared call log recorded by [`meta_hook`].
type MetaCalls = Arc<Mutex<Vec<MetaCall>>>;

/// Build a server-meta hook closure plus its shared call log. The appender
/// handle is read through `holder` (a `Weak`, filled right after the open
/// returns), so the hook holds no strong reference back into the appender.
fn meta_hook(
    holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>>,
    order: OrderLog,
) -> (
    MetaCalls,
    impl Fn(u64, [u8; 32]) -> io::Result<()> + Send + Sync + 'static,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_hook = Arc::clone(&calls);
    let hook = move |seq: u64, hmac: [u8; 32]| {
        let anchor = holder
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|appender| persisted_anchor(&appender));
        calls_for_hook.lock().unwrap().push(((seq, hmac), anchor));
        push(&order, "meta");
        Ok(())
    };
    (calls, hook)
}

/// The persisted appender anchor row, read through the appender's own
/// (already open) checkpoint keyspace handle: 8-byte BE next_seq plus
/// 32-byte digest.
fn persisted_anchor(appender: &FjallAuditAppender) -> Option<Position> {
    let value = appender
        .checkpoint_ks
        .get(super::super::super::CHECKPOINT_KEY)
        .unwrap()?;
    let bytes: &[u8] = value.as_ref();
    assert_eq!(bytes.len(), 40);
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&bytes[..8]);
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&bytes[8..]);
    Some((u64::from_be_bytes(seq), hmac))
}

/// Archive suffixes (the 20-digit u64 after `audit.log.`), sorted.
fn archive_suffixes(dir: &Path) -> Vec<u64> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().into_string().unwrap();
        if let Some(suffix) = name.strip_prefix(ARCHIVE_PREFIX) {
            if suffix.len() == 20 && suffix.bytes().all(|b| b.is_ascii_digit()) {
                out.push(suffix.parse::<u64>().unwrap());
            }
        }
    }
    out.sort_unstable();
    out
}

fn archive_path(dir: &Path, suffix: u64) -> PathBuf {
    dir.join(format!("{ARCHIVE_PREFIX}{suffix:020}"))
}

/// Build a history of `count` chained entries in `dir` (strict appender, no
/// rotation), returning the entries.
fn strict_history(dir: &Path, count: usize) -> Vec<AuditEntry> {
    let appender = FjallAuditAppender::open_strict(dir).unwrap();
    let writer = AuditChainWriter::new(AuditChain::new(KEY), appender);
    let mut entries = Vec::new();
    for _ in 0..count {
        entries.push(append(&writer, vec![0xc0]));
    }
    entries
}

/// Split the active log: move the first `lines` terminated records into a
/// new archive named `audit.log.<suffix>`, leaving the rest active.
fn split_off_archive(dir: &Path, suffix: u64, lines: usize) {
    let active = dir.join(ACTIVE);
    let bytes = fs::read(&active).unwrap();
    let mut ends = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            ends.push(index + 1);
        }
    }
    let split = ends[lines - 1];
    fs::write(archive_path(dir, suffix), &bytes[..split]).unwrap();
    fs::write(&active, &bytes[split..]).unwrap();
}

/// Entries still readable from the directory (archives + active), verifying
/// the whole genesis-rooted chain under `KEY`. Only for unpruned
/// (genesis-containing) histories.
fn assert_full_history(dir: &Path, expected: &[AuditEntry]) {
    let actual = FjallAuditAppender::read_log_for_verify(dir).unwrap();
    AuditChain::verify_chain(&KEY, &actual).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!((actual.seq, actual.hmac), (expected.seq, expected.hmac));
    }
}

/// Sequences of all retained records, without chain verification (used for
/// pruned, non-genesis suffixes where `verify_chain` does not apply).
fn retained_sequences(dir: &Path) -> Vec<u64> {
    FjallAuditAppender::read_log_for_verify(dir)
        .unwrap()
        .into_iter()
        .map(|entry| entry.seq)
        .collect()
}

/// Writer continuing the recovered chain at `position` — never a genesis
/// chain; the writer must resume at the verified tail.
fn resume_writer(position: Position, appender: &Arc<FjallAuditAppender>) -> AuditChainWriter {
    AuditChainWriter::new(
        AuditChain::from_checkpoint(KEY, position.0, position.1),
        appender.clone(),
    )
}

/// Finding 1 + 7 — an actual replacement directory-sync seam runs the REAL
/// `rotation::sync_parent` after asserting the layout; the persisted anchor
/// is written only after the sync, and the meta hook only after the anchor.
///
/// Parent counterfactual: no anchor and no meta write exist at rotation at
/// all, so `order` never contains `"sync"`/`"meta"` and the persisted
/// anchor stays empty after the rotation.
#[tokio::test]
async fn rotation_syncs_directory_then_persists_anchor_then_meta() {
    let dir = TempDir::new().unwrap();
    let mut entries = strict_history(dir.path(), 3);
    let order: OrderLog = Arc::new(Mutex::new(Vec::new()));
    let holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>> = Arc::new(Mutex::new(None));
    let (meta_calls, meta) = meta_hook(Arc::clone(&holder), Arc::clone(&order));

    let (appender, position) = FjallAuditAppender::open_recovered_batched_with_meta(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1), // every flush rotates
        0,
        meta,
    )
    .unwrap();
    *holder.lock().unwrap() = Some(Arc::downgrade(&appender));
    assert_eq!(position, after(&entries[2]));
    assert!(meta_calls.lock().unwrap().is_empty());

    let weak = Arc::downgrade(&appender);
    let sync_order = Arc::clone(&order);
    *appender.dir_sync_hook.lock() = Some(Arc::new(move |_path: PathBuf| {
        let appender = weak.upgrade().unwrap();
        // Rename + reopen already happened: exactly one archive exists and
        // the new active file is empty — but no anchor covers the layout.
        assert_eq!(
            archive_suffixes(appender.log_path.parent().unwrap()).len(),
            1
        );
        assert_eq!(fs::metadata(&appender.log_path).unwrap().len(), 0);
        assert_eq!(persisted_anchor(&appender), None);
        push(&sync_order, "sync");
        rotation::sync_parent(&appender.log_path)
    }));

    let writer = resume_writer(position, &appender);
    let fourth = append(&writer, vec![0xc0]);
    let tail = after(&fourth);
    appender.flush_now().await.unwrap();

    assert_eq!(*order.lock().unwrap(), ["sync", "meta"]);
    let calls = meta_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (meta_position, anchor_at_meta) = calls[0];
    assert_eq!(meta_position, tail);
    assert_eq!(anchor_at_meta, Some(tail));
    drop(calls);
    drop(writer);
    assert_eq!(persisted_anchor(&appender), Some(tail));
    assert_eq!(archive_suffixes(dir.path()).len(), 1);
    assert_eq!(fs::metadata(dir.path().join(ACTIVE)).unwrap().len(), 0);
    entries.push(fourth);
    assert_full_history(dir.path(), &entries);
    drop(appender);
    *holder.lock().unwrap() = None;
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );
}

/// Finding 7 (negative) — the replacement seam returns `Err` WITHOUT
/// running the real sync: the rotation latches fail-closed, the anchor is
/// not persisted, the meta hook never fires and the retention sweep never
/// runs.
///
/// Parent counterfactual: the parent has no seam and no rotation-time
/// anchor, so `flush_now` succeeds there.
#[tokio::test]
async fn directory_sync_failure_latches_before_anchor_meta_and_sweep() {
    let dir = TempDir::new().unwrap();
    let mut entries = strict_history(dir.path(), 4);
    let old_suffix = now_unix_nanos() - 10 * NANOS_PER_DAY;
    split_off_archive(dir.path(), old_suffix, 2);

    let order: OrderLog = Arc::new(Mutex::new(Vec::new()));
    let holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>> = Arc::new(Mutex::new(None));
    let (meta_calls, meta) = meta_hook(Arc::clone(&holder), Arc::clone(&order));
    let (appender, position) = FjallAuditAppender::open_recovered_batched_with_meta(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1),
        1, // sweep eligible so the counterfactual is observable
        meta,
    )
    .unwrap();
    *holder.lock().unwrap() = Some(Arc::downgrade(&appender));
    *appender.dir_sync_hook.lock() = Some(Arc::new(|_path: PathBuf| {
        Err(io::Error::other("injected sync failure"))
    }));

    let writer = resume_writer(position, &appender);
    let fifth = append(&writer, vec![0xc0]);
    let error = appender.flush_now().await.unwrap_err().to_string();
    assert!(error.contains("injected sync failure"), "{error}");
    assert_eq!(*order.lock().unwrap(), [] as [&str; 0]);
    assert!(meta_calls.lock().unwrap().is_empty());
    assert_eq!(persisted_anchor(&appender), None);
    assert!(archive_path(dir.path(), old_suffix).exists());
    assert!(appender.flush_now().await.is_err());
    drop(writer);
    drop(appender);
    entries.push(fifth);
    assert_full_history(dir.path(), &entries);
}

/// Finding 1 — a clean batched shutdown checkpoints the final position with
/// NO manual checkpoint call, an explicitly flushed (empty) final drain
/// still checkpoints, and a reopen with anchors re-fires the meta hook
/// exactly once at that same position on the second shutdown.
///
/// Parent counterfactual: shutdown only drains; the persisted checkpoint
/// stays empty after a fresh history, so the first load assertion fails.
#[tokio::test]
async fn clean_shutdown_checkpoints_final_position_and_reopen_fires_meta() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2);
    let order: OrderLog = Arc::new(Mutex::new(Vec::new()));
    let holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>> = Arc::new(Mutex::new(None));
    let (meta_calls, meta) = meta_hook(Arc::clone(&holder), Arc::clone(&order));
    let (appender, position) = FjallAuditAppender::open_recovered_batched_with_meta(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        None,
        0,
        meta,
    )
    .unwrap();
    *holder.lock().unwrap() = Some(Arc::downgrade(&appender));
    assert_eq!(position, after(&entries[1]));

    let writer = resume_writer(position, &appender);
    let third = append(&writer, vec![0xc0]);
    appender.flush_now().await.unwrap(); // explicit flush: final drain is empty
    appender.shutdown().await;
    let tail = after(&third);
    assert_eq!(*meta_calls.lock().unwrap(), vec![(tail, Some(tail))]);
    drop(writer);
    drop(appender);
    *holder.lock().unwrap() = None;
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );

    // Reopen with anchors; the meta hook fires only at the second shutdown,
    // at the same position, again after the anchor persist.
    let order: OrderLog = Arc::new(Mutex::new(Vec::new()));
    let holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>> = Arc::new(Mutex::new(None));
    let (meta_calls, meta) = meta_hook(Arc::clone(&holder), Arc::clone(&order));
    let (appender, position) = FjallAuditAppender::open_recovered_batched_with_meta(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        None,
        0,
        meta,
    )
    .unwrap();
    *holder.lock().unwrap() = Some(Arc::downgrade(&appender));
    assert_eq!(position, tail);
    assert!(meta_calls.lock().unwrap().is_empty());
    appender.shutdown().await;
    assert_eq!(*meta_calls.lock().unwrap(), vec![(tail, Some(tail))]);
    drop(appender);
    *holder.lock().unwrap() = None;
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );
    assert!(order.lock().unwrap().iter().all(|stage| *stage == "meta"));
}

/// Finding 1 — retention prunes only the old contiguous archive prefix,
/// keeps the newest archive, and the rotation-time anchor makes the pruned
/// (non-genesis) suffix bootable via `open_recovered_strict`.
///
/// Parent counterfactual: no anchor is persisted at rotation, so the
/// strict reopen returns `UnanchoredSuffix`.
#[tokio::test]
async fn retention_prunes_old_prefix_and_leaves_a_bootable_anchored_suffix() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 4);
    let old_suffix = now_unix_nanos() - 10 * NANOS_PER_DAY;
    split_off_archive(dir.path(), old_suffix, 2);

    let order: OrderLog = Arc::new(Mutex::new(Vec::new()));
    let holder: Arc<Mutex<Option<Weak<FjallAuditAppender>>>> = Arc::new(Mutex::new(None));
    let (meta_calls, meta) = meta_hook(Arc::clone(&holder), Arc::clone(&order));
    let (appender, position) = FjallAuditAppender::open_recovered_batched_with_meta(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1),
        1,
        meta,
    )
    .unwrap();
    *holder.lock().unwrap() = Some(Arc::downgrade(&appender));
    assert_eq!(position, after(&entries[3]));

    let weak = Arc::downgrade(&appender);
    let old = old_suffix;
    *appender.dir_sync_hook.lock() = Some(Arc::new(move |_path: PathBuf| {
        let appender = weak.upgrade().unwrap();
        // The prune runs only after anchor + meta: the old archive still
        // exists at directory-sync time.
        assert!(archive_path(appender.log_path.parent().unwrap(), old).exists());
        rotation::sync_parent(&appender.log_path)
    }));

    let writer = resume_writer(position, &appender);
    let fifth = append(&writer, vec![0xc0]);
    appender.flush_now().await.unwrap();
    let tail = after(&fifth);

    assert_eq!(meta_calls.lock().unwrap().len(), 1);
    let (meta_position, anchor_at_meta) = meta_calls.lock().unwrap()[0];
    assert_eq!(meta_position, tail);
    assert_eq!(anchor_at_meta, Some(tail));
    assert!(!archive_path(dir.path(), old_suffix).exists());
    assert_eq!(archive_suffixes(dir.path()).len(), 1);
    drop(writer);
    drop(appender);

    // The pruned suffix keeps exactly the post-prefix rows; it has no
    // genesis, so chain verification here is the anchor's job, not
    // `verify_chain`'s.
    assert_eq!(retained_sequences(dir.path()), [3, 4, 5]);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );

    let (strict, strict_position) =
        match FjallAuditAppender::open_recovered_strict(dir.path(), &KEY, None) {
            Ok(opened) => opened,
            Err(AuditRecoveryError::UnanchoredSuffix { .. }) => {
                panic!("pruned suffix must boot from the rotation-time anchor")
            }
            Err(error) => panic!("unexpected recovery error: {error:?}"),
        };
    assert_eq!(strict_position, tail);
    assert_eq!(persisted_anchor(&strict), Some(tail));
    drop(strict);
}

/// Finding 3 — rotation names stay strictly increasing even against a
/// far-future existing archive, without any clock-injection API.
///
/// Parent counterfactual: the name comes from `SystemTime` alone, so the
/// new archive suffix equals ~now and is LOWER than the seed.
#[tokio::test]
async fn rotation_name_monotonic_past_future_archive() {
    let dir = TempDir::new().unwrap();
    let mut entries = strict_history(dir.path(), 3);
    let seed = future_seed();
    split_off_archive(dir.path(), seed, 2);

    let (appender, position) = FjallAuditAppender::open_recovered_batched(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1),
        0,
    )
    .unwrap();
    let writer = resume_writer(position, &appender);
    let fourth = append(&writer, vec![0xc0]);
    let tail = after(&fourth);
    appender.flush_now().await.unwrap();

    let suffixes = archive_suffixes(dir.path());
    assert_eq!(suffixes.len(), 2);
    let new_suffix = suffixes.into_iter().find(|suffix| *suffix != seed).unwrap();
    assert!(
        new_suffix > seed,
        "new archive name {new_suffix} must exceed the existing {seed}"
    );
    assert!(archive_path(dir.path(), seed).exists());
    entries.push(fourth);
    assert_full_history(dir.path(), &entries);
    drop(writer);
    drop(appender);
    let (reopened, reopened_position) =
        FjallAuditAppender::open_recovered_strict(dir.path(), &KEY, None).unwrap();
    assert_eq!(reopened_position, tail);
    drop(reopened);
}

/// Finding 5 — the writer refuses a record whose emitted log line exceeds
/// the recovery read ceiling with the typed `AuditError::RecordTooLarge`,
/// before anything reaches the file or the chain.
///
/// Parent counterfactual: the parent writer accepts and persists the
/// record; `append` returns `Ok`.
#[tokio::test]
async fn writer_refuses_oversized_record_typed_and_unchanged() {
    let dir = TempDir::new().unwrap();
    let (appender, position) =
        FjallAuditAppender::open_recovered_batched(dir.path(), &KEY, None, FLUSH_INTERVAL, None, 0)
            .unwrap();
    assert_eq!(position, GENESIS);
    let writer = resume_writer(position, &appender);

    // Valid MessagePack bin32: 0xC6 header + u32 BE length + 17 MiB payload.
    let payload_len = 17 * 1024 * 1024usize;
    let total_len = payload_len + 5;
    let mut details = Vec::with_capacity(total_len);
    details.push(0xc6);
    details.extend_from_slice(&(payload_len as u32).to_be_bytes());
    details.resize(total_len, 0xc0);
    let error = writer
        .append(
            EVENT,
            "test",
            "user",
            "127.0.0.0/24",
            [7; 8],
            "ok",
            details,
            42,
        )
        .unwrap_err();
    assert!(matches!(error, AuditError::RecordTooLarge), "{error:?}");

    assert_eq!(fs::metadata(dir.path().join(ACTIVE)).unwrap().len(), 0);
    let accepted = append(&writer, vec![0xc0]);
    assert_eq!(accepted.seq, 1);
    appender.flush_now().await.unwrap();
    let actual = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(actual.len(), 1);
    AuditChain::verify_chain(&KEY, &actual).unwrap();
    assert_eq!(actual[0].seq, 1);
    drop(writer);
    drop(appender);
}

/// Finding 2 (appender half) — anchor writes are monotonic against the
/// ACTUAL persisted row: a lower position is refused as an anchor
/// regression and a same-position different digest as a conflict; the
/// persisted row keeps its position in both cases. A refused checkpoint
/// latches the appender fail-closed (existing policy), so each refusal runs
/// on its own appender instance and no append follows a refusal.
///
/// Parent counterfactual: every checkpoint write lands unconditionally, so
/// both attempts succeed and LOWER the persisted anchor to `GENESIS`.
#[tokio::test]
async fn anchor_writes_are_monotonic_lower_and_conflict_refused() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 1);
    let tail = after(&entries[0]);

    // Instance A: a lower position is refused, row unchanged.
    let (appender, position) =
        FjallAuditAppender::open_recovered_strict(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, tail);
    AuditAppender::checkpoint(&*appender, tail.0, &tail.1).unwrap();
    assert_eq!(persisted_anchor(&appender), Some(tail));
    let error = AuditAppender::checkpoint(&*appender, GENESIS.0, &GENESIS.1)
        .expect_err("lowering an anchor must be refused");
    let cause = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<AppenderError>())
        .expect("checkpoint refusal must carry the typed AppenderError");
    assert!(
        matches!(cause, AppenderError::AnchorRegression { persisted, attempted }
            if *persisted == tail.0 && *attempted == GENESIS.0),
        "{cause:?}"
    );
    assert_eq!(persisted_anchor(&appender), Some(tail));
    drop(appender);

    // Instance B (fresh open of the same directory): a same-position
    // different digest is refused, row unchanged.
    let (appender, position) =
        FjallAuditAppender::open_recovered_strict(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, tail);
    let mut forged = tail.1;
    forged[0] ^= 0xff;
    let error = AuditAppender::checkpoint(&*appender, tail.0, &forged)
        .expect_err("a conflicting digest at the same position must be refused");
    let cause = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<AppenderError>())
        .expect("checkpoint refusal must carry the typed AppenderError");
    assert!(
        matches!(cause, AppenderError::AnchorConflict { next_seq } if *next_seq == tail.0),
        "{cause:?}"
    );
    assert_eq!(persisted_anchor(&appender), Some(tail));
    drop(appender);

    // Both refusals left the persisted anchor at the fsynced position.
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );
}
