//! Payload-free refusals: a corrupt stamp and a listed `cache` entry are
//! decided BEFORE the candidate's payload is charged.
//!
//! Each test gives the pass a budget that covers exactly the bounded control
//! reads it really performs up to the refusal, with no room for the listed
//! payload. Mutant (the pre-fix order): the payload is charged before the
//! decision, the pass runs out of budget and returns `Refusal::Budget` instead
//! of the structural `Refusal::Unverifiable` — the first assertion fails.

use std::collections::BTreeSet;
use std::fs;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    decode, encode, write_manifest, BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{ManifestFileEntry, MANIFEST_FILE_NAME};
use crate::sync::format_stamp::{CURRENT_FORMAT_EPOCH, STAMP_FILE_NAME};

use sha2::Digest;

use super::super::budget::{Budget, Refusal};
use super::super::prune::prune_inner;
use super::super::RetentionOutcome;
use super::fixture::{copy_with_payload, private_root, OWNER, WAL_FILE};

const PAYLOAD: usize = 64 << 10;

/// The bounded control reads up to a stamp refusal: `manifest.len() + 1`,
/// `sidecar.len() + 1`, plus the ONE `format_stamp::inspect` read of
/// `stamp.len()` when the copy carries a stamp. The verifier's second stamp
/// read is charged only for a stamp that passes the verdict.
fn control_reads(dir: &std::path::Path, with_stamp: bool) -> u64 {
    let manifest = fs::metadata(dir.join(MANIFEST_FILE_NAME))
        .expect("manifest metadata")
        .len();
    let sidecar = fs::metadata(dir.join(BACKUP_META_FILE_NAME))
        .expect("sidecar metadata")
        .len();
    let stamp = if with_stamp {
        fs::metadata(dir.join(STAMP_FILE_NAME))
            .expect("stamp metadata")
            .len()
    } else {
        0
    };
    manifest + 1 + sidecar + 1 + stamp
}

/// Mutant (removing the early stamp verdict): the corrupt stamp is LISTED in
/// the manifest with its real SHA-256/size and the manifest carries the epoch
/// a valid stamp of this layout would have, so the tree, size and listing
/// checks all accept the copy. Without the preflight's `inspect` verdict the
/// pass charges the listed WAL payload, the verifier's `check_stamp_agreement`
/// then reports the corrupt stamp, and this budget — one stamp read, no
/// payload — is exhausted first: the pass returns `Refusal::Budget` (or
/// verifies and tags the copy) instead of the structural
/// `Refusal::Unverifiable`, so the first assertion fails. The stamp is NOT
/// left unlisted on purpose: then the unknown-tree check alone would refuse
/// `Unverifiable`, and the verdict would not be the oracle.
#[test]
fn a_corrupt_stamp_is_refused_before_the_payload_is_charged() {
    let (_temp, root) = private_root();
    let fresh = copy_with_payload(&root, "20200101_000001", 1, None, &vec![0u8; PAYLOAD]);
    // A stamp body that does not decode, beside a large listed WAL segment.
    let corrupt = b"not a format stamp";
    fs::write(fresh.join(STAMP_FILE_NAME), corrupt).expect("corrupt stamp");
    // List the corrupt stamp with its REAL digest and size and keep the epoch
    // a valid stamp of this layout would carry, so only the stamp verdict can
    // refuse the copy.
    let raw = fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("manifest");
    let mut manifest = decode(std::str::from_utf8(&raw).expect("utf-8")).expect("decode");
    manifest.format_epoch = Some(CURRENT_FORMAT_EPOCH);
    manifest.files.push(ManifestFileEntry {
        path: STAMP_FILE_NAME.to_owned(),
        sha256: hex::encode(sha2::Sha256::digest(corrupt)),
        size_bytes: u64::try_from(corrupt.len()).expect("size"),
    });
    write_manifest(&fresh, &encode(&manifest).expect("encode")).expect("rewrite manifest");
    let budget_bytes = control_reads(&fresh, true);
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
        matches!(refusal, Err(Refusal::Unverifiable)),
        "a corrupt stamp is a structural refusal, not a budget refusal \
         (mutant: the payload is charged before the stamp verdict)"
    );
    assert!(fresh.exists(), "a refused fresh copy is never deleted");
}

#[test]
fn a_listed_cache_entry_is_refused_before_the_payload_is_charged() {
    let (_temp, root) = private_root();
    let fresh = copy_with_payload(&root, "20200101_000001", 1, None, &vec![0u8; PAYLOAD]);
    // List a `cache/` payload in the manifest (the verifier refuses it) and
    // create the file so the tree check cannot refuse it first.
    let raw = fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("manifest");
    let mut manifest = decode(std::str::from_utf8(&raw).expect("utf-8")).expect("decode");
    let cache_bytes = vec![1u8; PAYLOAD];
    fs::create_dir_all(fresh.join("cache")).expect("cache dir");
    fs::write(fresh.join("cache/big.bin"), &cache_bytes).expect("cache payload");
    manifest.files.push(ManifestFileEntry {
        path: "cache/big.bin".to_owned(),
        sha256: hex::encode(sha2::Sha256::digest(&cache_bytes)),
        size_bytes: u64::try_from(cache_bytes.len()).expect("size"),
    });
    write_manifest(&fresh, &encode(&manifest).expect("encode")).expect("rewrite manifest");
    // Keep the WAL segment listed and present; the cache entry is the refusal.
    let budget_bytes = control_reads(&fresh, false);
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
        matches!(refusal, Err(Refusal::Unverifiable)),
        "a listed cache payload is a structural refusal, not a budget refusal \
         (mutant: the payload is charged before the cache rule)"
    );
    assert!(fresh.exists(), "a refused fresh copy is never deleted");
    assert!(
        fresh.join(WAL_FILE).is_file(),
        "the fixture stays a real copy: the WAL segment is untouched"
    );
}
