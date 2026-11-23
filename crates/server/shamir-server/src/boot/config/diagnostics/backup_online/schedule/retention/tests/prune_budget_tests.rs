//! Pass-budget accounting and fresh-copy ownership tests: the pass charges the
//! verifier's real reads (a manifest file once, a WAL segment and a copied
//! stamp three times) and nothing else, at the exact boundary and one byte below it,
//! and the fresh copy is owned before a scan can refuse.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::BACKUP_META_FILE_NAME;
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::MANIFEST_FILE_NAME;
use crate::sync::format_stamp::STAMP_FILE_NAME;

use super::super::budget::{Budget, Refusal};
use super::super::prune::prune_inner;
use super::super::RetentionOutcome;
use super::fixture::{
    copy_with_payload, copy_with_stamp, owner_tag, private_root, retain, sidecar, OWNER, WAL_DIR,
    WAL_FILE,
};

/// Bytes one pass charges for a payload copy: the bounded manifest read, the
/// bounded sidecar read and the payload, which is a WAL segment and therefore
/// read twice by the verifier. Each bounded read reserves one extra byte to
/// detect a size change.
fn true_read_bytes(dir: &Path) -> u64 {
    let manifest = fs::metadata(dir.join(MANIFEST_FILE_NAME))
        .expect("manifest metadata")
        .len();
    let sidecar = fs::metadata(dir.join(BACKUP_META_FILE_NAME))
        .expect("sidecar metadata")
        .len();
    let payload = fs::metadata(dir.join(WAL_FILE))
        .expect("payload metadata")
        .len();
    manifest + 1 + sidecar + 1 + 2 * payload
}

/// [`true_read_bytes`] plus the stamp's three real reads: `format_stamp::inspect`
/// and the verifier's `check_stamp_agreement` both inspect the copied stamp,
/// and the verifier's manifest-file loop hashes it.
fn stamped_read_bytes(dir: &Path) -> u64 {
    true_read_bytes(dir) + 3 * stamp_bytes(dir)
}

fn stamp_bytes(dir: &Path) -> u64 {
    fs::metadata(dir.join(STAMP_FILE_NAME))
        .expect("stamp metadata")
        .len()
}

/// Total true reads of the three-copy tree below, in bytes.
fn three_copy_reads(oldest: &Path, newest: &Path, fresh: &Path) -> u64 {
    true_read_bytes(oldest) + true_read_bytes(newest) + true_read_bytes(fresh)
}

/// Total true reads of the three stamped copies below, in bytes.
fn three_stamped_reads(oldest: &Path, newest: &Path, fresh: &Path) -> u64 {
    stamped_read_bytes(oldest) + stamped_read_bytes(newest) + stamped_read_bytes(fresh)
}

#[test]
fn one_pass_charges_the_verifiers_real_reads() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let oldest = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    let newest = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    // EXACTLY the true reads, no slack: one extra sentinel byte per bounded
    // read, one extra file debit, or a duplicate manifest debit refuses the
    // pass, because the total is the only budget that fits.
    let budget_bytes = three_copy_reads(&oldest, &newest, &fresh);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        refusal.is_ok(),
        "one pass of the verifier's true reads fits a budget that covers exactly them"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(0))
    );
    assert!(!oldest.exists());
    assert!(newest.exists());
    assert!(fresh.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER))
    );
}

/// Mutants killed: a missing sentinel byte on a bounded read, or any missing
/// byte debit, leaves the pass one byte short of the true reads — this budget
/// then fits, the pass succeeds, and the assertion below fails.
#[test]
fn one_byte_short_of_the_true_reads_refuses() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let oldest = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    let newest = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    let budget_bytes = three_copy_reads(&oldest, &newest, &fresh) - 1;
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the true reads are the exact upper bound: one byte less must refuse"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None),
        "the scan ran out of budget and deletes nothing"
    );
    assert!(oldest.exists());
    assert!(newest.exists());
    assert!(fresh.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER)),
        "the fresh copy is tagged before the scan can run out"
    );
}

#[test]
fn pass_budget_refuses_when_the_second_wal_read_does_not_fit() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let oldest = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    let newest = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    // One payload short of the true reads: a pass that charges a WAL segment
    // once (three copies, three missing second reads) would still fit.
    let budget_bytes = three_copy_reads(&oldest, &newest, &fresh) - (64 << 10);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the verifier reads a WAL segment twice: the pass must run out of budget"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed),
        (0, 0),
        "a refusing scan deletes nothing"
    );
    assert!(oldest.exists());
    assert!(newest.exists());
}

/// One pass over stamped copies fits a budget of exactly their true reads,
/// stamp reads included.
#[test]
fn one_pass_charges_the_stamps_three_real_reads() {
    let (_temp, root) = private_root();
    let oldest = copy_with_stamp(&root, "20200101_000001", 1, Some(OWNER));
    let newest = copy_with_stamp(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy_with_stamp(&root, "20200101_000003", 3, None);
    let budget_bytes = three_stamped_reads(&oldest, &newest, &fresh);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        refusal.is_ok(),
        "a pass over stamped copies fits exactly its true reads"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(0))
    );
    assert!(!oldest.exists());
    assert!(newest.exists());
    assert!(fresh.exists());
}

/// Mutant killed: dropping the stamp debit that `format_stamp::inspect`'s read
/// accounts for undercharges by one stamp per copy, so this budget (one stamp
/// short of the true reads) would fit and the pass would succeed.
#[test]
fn a_pass_that_forgets_the_stamp_read_runs_out_of_budget() {
    let (_temp, root) = private_root();
    let oldest = copy_with_stamp(&root, "20200101_000001", 1, Some(OWNER));
    let newest = copy_with_stamp(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy_with_stamp(&root, "20200101_000003", 3, None);
    let budget_bytes = three_stamped_reads(&oldest, &newest, &fresh) - stamp_bytes(&fresh);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the stamp's first read is charged: one stamp short must refuse"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None),
        "the refusing scan deletes nothing and publishes no classification"
    );
    assert!(oldest.exists());
    assert!(newest.exists());
    assert!(fresh.exists());
}

#[test]
fn scan_refusal_keeps_the_fresh_copy_owned() {
    let (_temp, root) = private_root();
    let other = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &[0u8; 8 << 10]);
    let fresh = copy_with_payload(&root, "20200101_000002", 2, None, &[0u8; 1 << 10]);
    let budget_bytes = true_read_bytes(&fresh) + (4 << 10);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 1 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the other candidate cannot fit the remaining budget"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None),
        "the refusing scan deletes nothing and publishes no classification; \
         the failure counter belongs to the caller of prune_inner"
    );
    assert!(other.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER)),
        "the fresh copy is tagged before the scan can refuse"
    );

    // A later scheduled run has a NEW fresh copy: the earlier fresh copy is
    // classified by the scan like any other candidate. The floor counts the
    // new fresh copy as one of the newest, so `keep_last = 2` keeps the earlier
    // fresh copy only when it was tagged (otherwise the older copy is the
    // second newest owned one and nothing is pruned).
    let later = copy_with_payload(&root, "20200101_000003", 3, None, &[0u8; 1 << 10]);
    let next = retain(&root, &later, 2);
    assert_eq!(
        (next.deleted, next.failed, next.untouched),
        (1, 0, Some(0)),
        "the earlier fresh copy is owned and the older copy is pruned"
    );
    assert!(next.adopted.is_empty(), "nothing is adopted");
    assert!(!other.exists());
    assert!(fresh.exists());
    assert!(later.exists());
}

/// Bytes a candidate that the cheap structural reject discards still charges:
/// the bounded sidecar read (`backup.meta.json`, so the scan can classify the
/// copy as owned) and the bounded manifest read (`manifest.json`), each
/// `len() + 1` to detect a size change. Its listed WAL payload is never
/// opened, because the tree check refuses the copy first.
fn rejected_read_bytes(dir: &Path) -> u64 {
    let manifest = fs::metadata(dir.join(MANIFEST_FILE_NAME))
        .expect("manifest metadata")
        .len();
    let sidecar = fs::metadata(dir.join(BACKUP_META_FILE_NAME))
        .expect("sidecar metadata")
        .len();
    manifest + 1 + sidecar + 1
}

/// Total true reads of the three-copy tree below, in bytes: the fresh copy
/// and the verified candidate (`true_read_bytes` each: `manifest.len() + 1`,
/// `sidecar.len() + 1`, `2 * payload.len()`) plus the rejected copy's two
/// bounded reads (`manifest.len() + 1`, `sidecar.len() + 1`). No byte of the
/// rejected copy's listed WAL payload and no byte of its unlisted
/// `store.shamirwal/extra.pem` is in this total.
fn verified_plus_rejected_reads(verified: &Path, fresh: &Path, rejected: &Path) -> u64 {
    true_read_bytes(verified) + true_read_bytes(fresh) + rejected_read_bytes(rejected)
}

/// Mutant (the parent implementation): file bytes are charged before the
/// cheap structural rejects, so the rejected candidate's listed WAL payload is
/// charged twice for a verifier that never opens that copy. This budget has no
/// slack for those bytes, so the parent runs out and returns `Err(Budget)`;
/// the first assertion below then fails.
///
/// The rejected copy is owner-tagged on purpose: an untagged copy is refused
/// at the sidecar check in `prune.rs` BEFORE `verified_time`, so only an owned
/// copy reaches the manifest read and the tree check this test targets.
#[test]
fn a_structurally_rejected_candidate_charges_no_payload_bytes() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let rejected = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    // A file the manifest does not list, so the tree check refuses this copy
    // after the manifest read and before any byte charge.
    fs::write(
        rejected.join(WAL_DIR).join("extra.pem"),
        vec![7u8; 128 << 10],
    )
    .expect("unlisted payload");
    let verified = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    let budget_bytes = verified_plus_rejected_reads(&verified, &fresh, &rejected);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 1 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        refusal.is_ok(),
        "the rejected copy's payload is never opened, so it must not be charged"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(1)),
        "one verified candidate is pruned; the rejected copy stays untouched"
    );
    assert!(rejected.exists(), "the rejected copy is never deleted");
    assert!(!verified.exists());
    assert!(fresh.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER))
    );
}
