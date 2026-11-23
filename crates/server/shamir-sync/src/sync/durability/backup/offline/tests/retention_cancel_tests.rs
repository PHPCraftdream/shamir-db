//! Per-chunk cancellation of retention's payload hashing (CI only).
//!
//! Retention's preflight used to check cancellation only AROUND
//! `verify_retention_candidate`: once a payload hash started, an in-flight
//! hash of a large file ran to the end and delayed shutdown. The fix threads
//! a cancellation probe into `hash_file_streaming`, which consults it once
//! per chunk BEFORE that chunk is read.
//!
//! Red oracle of the first test: a verifier without the per-chunk check
//! consults the probe only between whole files, so the probe is called
//! once per chunk of the WHOLE file (18 calls for the 16-chunk segment
//! below) and the `Interrupted` error never arrives — assertions (a) and
//! (b) both fail. The second test guards the other direction: the additive
//! probe must not change the digest or the size a cancellation-free verify
//! computes.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use tempfile::TempDir;

use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::online::manifest::{
    self as online_manifest, BackupKind, ConsistencySection, RepoConsistency, WalDirEntry,
    WalSection, WalSegmentEntry,
};
use crate::sync::durability::backup::{
    verify_retention_candidate, verify_retention_candidate_cancellable, BackupError,
    ManifestFileEntry,
};

use super::super::HASH_STREAM_BUFFER_SIZE;

/// Chunks of `HASH_STREAM_BUFFER_SIZE` the fixture's WAL segment spans, plus
/// a trailing partial read. The probe aborts on chunk 3, so a per-chunk
/// check stops after a handful of calls where a whole-file check would have
/// made one call per chunk (18 here).
const CHUNKS: usize = 16;
/// File size of the fixture's WAL segment: `CHUNKS` full chunks plus a
/// partial one (whose read still costs a probe call, and whose following
/// zero-length read costs another).
const PAYLOAD_BYTES: usize = HASH_STREAM_BUFFER_SIZE * CHUNKS + 7;

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

fn entry(path: &Path, root: &Path) -> ManifestFileEntry {
    let (sha256, size_bytes) = hash_file(path).expect("hash");
    ManifestFileEntry {
        path: path
            .strip_prefix(root)
            .expect("relative")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        sha256,
        size_bytes,
    }
}

/// A valid v2-online candidate whose FIRST listed file is a WAL segment
/// spanning more than [`CHUNKS`] hash chunks. The manifest lists the segment
/// before every other file, so the verifier's manifest-file loop reaches the
/// large payload first.
fn large_wal_candidate() -> (TempDir, Vec<u8>) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    let store = root.join("repos/alpha/store.bin");
    let wal0 = root.join("repos/alpha.shamirwal/00000000.wal");
    let wal0_meta = root.join("repos/alpha.shamirwal/00000000.meta");
    write_file(&store, b"store-bytes");
    let payload = vec![0u8; PAYLOAD_BYTES];
    write_file(&wal0, &payload);
    write_file(&wal0_meta, b"meta-0");

    let files = vec![
        entry(&wal0, root),
        entry(&store, root),
        entry(&wal0_meta, root),
    ];
    // Precondition for assertion (b): the payload is much longer than the
    // handful of chunks the cancelled verify is allowed to touch.
    let chunk_floor = (CHUNKS * HASH_STREAM_BUFFER_SIZE) as u64;
    assert!(files[0].size_bytes > chunk_floor);

    let mut per_repo = BTreeMap::new();
    per_repo.insert(
        "alpha".to_string(),
        RepoConsistency {
            wal_position: 1,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        },
    );
    let consistency = ConsistencySection {
        per_repo,
        system_store_version: 3,
    };
    let wal = WalSection {
        dirs: vec![WalDirEntry {
            repo: "alpha".to_string(),
            dir: "repos/alpha.shamirwal".to_string(),
            segments: vec![WalSegmentEntry {
                seq: 0,
                file: files[0].path.clone(),
                sidecar: Some(files[2].path.clone()),
                sha256: files[0].sha256.clone(),
                size: files[0].size_bytes,
            }],
        }],
    };
    let built = online_manifest::build_v2(11, files, None, None, consistency, wal);
    let encoded = online_manifest::encode(&built).expect("encode");
    assert!(matches!(built.kind, BackupKind::Online));
    fs::write(root.join("manifest.json"), &encoded).expect("write manifest");
    online_manifest::write_backup_meta(root).expect("write meta");
    (dir, encoded.into_bytes())
}

/// Cancellation is observed BETWEEN hash chunks: with a probe that turns
/// true after two calls, the verify above a 16-chunk payload returns an
/// `Interrupted` I/O error after exactly three probe calls instead of
/// hashing the whole segment.
#[test]
fn retention_cancel_interrupts_a_large_payload_hash() {
    let (dir, raw) = large_wal_candidate();
    let calls = Cell::new(0usize);
    let probe = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };

    let error = verify_retention_candidate_cancellable(dir.path(), &raw, &probe)
        .expect_err("a cancelled verify must not report success");

    // (a) the cancellation surfaces as an `Interrupted` I/O error …
    assert!(
        matches!(&error, BackupError::Io(error)
            if error.kind() == ErrorKind::Interrupted),
        "expected an Interrupted I/O error, got: {error}"
    );
    // (b) … after a SMALL, exact number of probe calls: one per chunk
    // visited, not one per chunk of the whole segment.
    assert_eq!(calls.get(), 3, "probe must be consulted once per chunk");
}

/// The same candidate WITHOUT cancellation still hashes to the
/// byte-identical digest and the full size the manifest recorded, so the
/// additive probe changed neither the digest nor the byte count.
#[test]
fn uncancelled_verify_hashes_the_whole_payload() {
    let (dir, raw) = large_wal_candidate();
    let wal = dir.path().join("repos/alpha.shamirwal/00000000.wal");

    verify_retention_candidate(dir.path(), &raw)
        .expect("an uncancelled verify of the same candidate must succeed");

    let (sha256, size_bytes) = hash_file(&wal).expect("hash");
    assert_eq!(
        size_bytes, PAYLOAD_BYTES as u64,
        "the streaming hash must report the full size"
    );
    let text = std::str::from_utf8(&raw).expect("manifest is utf8");
    let decoded = online_manifest::decode(text).expect("decode manifest");
    assert_eq!(
        decoded.files[0].sha256, sha256,
        "the streaming digest must be byte-identical to the whole-file hash"
    );
    assert_eq!(decoded.files[0].size_bytes, size_bytes);
}
