//! A handcrafted v2 copy that LISTS the root control file `backup.meta.json`
//! in `files`: the verifier would hash those bytes and retention's tagging
//! then rewrites them, so the preflight must refuse the candidate before it
//! verifies and tags it.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    decode, encode, write_manifest, BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{ManifestFileEntry, MANIFEST_FILE_NAME};

use super::super::budget::{Budget, Refusal};
use super::super::prune::prune_inner;
use super::super::RetentionOutcome;
use super::fixture::{copy_with_payload, private_root, sidecar, OWNER};

/// Add `backup.meta.json` to `dir`'s manifest `files` list with the sidecar's
/// real SHA-256 and size, so the verifier would have to hash a root control
/// file. `verify_manifest` is deliberately NOT called on the result: the
/// fixture is only valid for this preflight.
fn list_the_root_sidecar(dir: &Path) {
    let raw = fs::read(dir.join(MANIFEST_FILE_NAME)).expect("read manifest");
    let text = std::str::from_utf8(&raw).expect("manifest is UTF-8");
    let mut manifest = decode(text).expect("decode manifest");
    let sidecar_bytes = fs::read(dir.join(BACKUP_META_FILE_NAME)).expect("read sidecar");
    manifest.files.push(ManifestFileEntry {
        path: BACKUP_META_FILE_NAME.to_owned(),
        sha256: hex::encode(Sha256::digest(&sidecar_bytes)),
        size_bytes: u64::try_from(sidecar_bytes.len()).expect("sidecar size fits u64"),
    });
    let encoded = encode(&manifest).expect("encode manifest");
    write_manifest(dir, &encoded).expect("write manifest");
}

/// Mutant (the parent implementation): without the root-control-file refusal
/// the preflight charges and hashes `backup.meta.json` as an ordinary listed
/// file, and `tag_fresh` then rewrites that very file, so the bytes a verifier
/// just hashed change. The pass succeeds and the crafted copy comes back
/// tagged, so both the refusal and the no-tag assertion below fail.
///
/// `prune_inner` verifies and tags the FRESH copy before it scans and lets a
/// refusal there propagate out, while a refusal for any other copy is a scan
/// candidate outcome counted in `outcome.untouched` (never returned). The
/// crafted copy is therefore the fresh copy: only that slot can make the
/// preflight's refusal reach the assertion. It is also UNTAGGED, so the
/// sidecar bytes the parent's verifier hashes are exactly the ones on disk
/// before the pass (tagging is what changes them) and "no `scheduled_retention`
/// tag" is a real observation. A second, normal, owner-tagged copy sits beside
/// it with `keep_last: 2`, so on the parent the scan keeps both copies and the
/// pass completes without deleting anything.
#[test]
fn a_manifest_listing_the_root_sidecar_is_refused() {
    let (_temp, root) = private_root();
    let crafted = copy_with_payload(&root, "20200101_000001", 1, None, &[0u8; 1 << 10]);
    list_the_root_sidecar(&crafted);
    let other = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &[0u8; 1 << 10]);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, u64::MAX);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &crafted,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Unverifiable)),
        "a manifest that lists a root control file is not a copy this server produced"
    );
    assert!(crafted.exists(), "the refused copy is never deleted");
    assert!(
        other.exists(),
        "the separate normal copy is untouched by the refusal"
    );
    assert_eq!(outcome.deleted, 0, "nothing is pruned");
    assert_eq!(
        sidecar(&crafted).get("scheduled_retention"),
        None,
        "the refused copy must stay untagged: tagging rewrites the bytes the verifier hashed"
    );
}
