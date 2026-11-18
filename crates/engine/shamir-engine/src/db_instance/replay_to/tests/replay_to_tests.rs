//! Offline coverage of staging and explicit replay refusal on persistent copies.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use shamir_storage::storage_fjall::FjallRepo;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalEntryV2, WalOpV2, WalSegment, WalStrictReadError};
use tempfile::TempDir;
use tokio::time::timeout;

use crate::db_instance::replay_to::{replay_to, ReplayScope, ReplayScopeError, ReplayToError};
use crate::table::{table_token_for, TableConfig};

pub(super) const BOUND: Duration = Duration::from_secs(30);
const SEGMENT: &str = "00000000.wal";
const TABLE: &str = "widgets";

fn table_scope() -> ReplayScope {
    // Supply the catalogue configuration directly: these refusals precede table open.
    ReplayScope::new(vec![TableConfig::new(TABLE)
        .with_indexes()
        .with_store_id(RecordId([9; 16]))])
}

struct OfflineFixture {
    _dir: TempDir,
    store: PathBuf,
    wal: PathBuf,
}

impl OfflineFixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("data");
        fs::create_dir(&store).unwrap();
        drop(FjallRepo::new(&store).expect("initialize persistent store"));
        let wal = dir.path().join("data.shamirwal");
        Self {
            _dir: dir,
            store,
            wal,
        }
    }
}

/// Include empty directories as well as every file's relative path and bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                out.insert(relative, None);
                walk(root, &path, out);
            } else {
                assert!(kind.is_file(), "unexpected fixture entry: {path:?}");
                out.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }

    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub(super) fn entry(txn_id: u64, version: u64, op: WalOpV2) -> WalEntryV2 {
    let mut entry = WalEntryV2::new(txn_id, 1, vec![op]);
    entry.commit_version = version;
    entry
}

pub(super) async fn write_wal(wal: &Path, entries: &[WalEntryV2]) {
    fs::create_dir(wal).unwrap();
    let segment = timeout(BOUND, WalSegment::open(wal.join(SEGMENT)))
        .await
        .expect("WAL open timed out")
        .unwrap();
    let max_version = entries.iter().map(|e| e.commit_version).max().unwrap();
    let payloads = entries.iter().map(|e| e.encode().unwrap()).collect();
    timeout(BOUND, segment.append_batch(Arc::new(payloads), max_version))
        .await
        .expect("WAL append timed out")
        .unwrap();
    timeout(BOUND, segment.sync())
        .await
        .expect("WAL sync timed out")
        .unwrap();
}

async fn replay_unchanged(
    fixture: &OfflineFixture,
    v_star: u64,
    scope: ReplayScope,
) -> ReplayToError {
    let store_before = snapshot(&fixture.store);
    let wal_before = snapshot(&fixture.wal);
    let result = timeout(
        BOUND,
        Box::pin(replay_to(&fixture.store, &fixture.wal, v_star, scope)),
    )
    .await
    .expect("replay_to timed out");
    assert_eq!(snapshot(&fixture.store), store_before, "store changed");
    assert_eq!(snapshot(&fixture.wal), wal_before, "WAL changed");
    result.expect_err("replay_to must refuse this staged copy")
}

fn deletes(versions: &[u64]) -> Vec<WalEntryV2> {
    versions
        .iter()
        .enumerate()
        .map(|(i, &version)| {
            entry(
                100 + u64::try_from(i).unwrap(),
                version,
                WalOpV2::Delete {
                    table_id_interned: table_token_for(TABLE),
                    rid: RecordId([7; 16]),
                },
            )
        })
        .collect()
}

async fn stage_versions(versions: &[u64], v_star: u64) -> ReplayToError {
    let fixture = OfflineFixture::new();
    write_wal(&fixture.wal, &deletes(versions)).await;
    replay_unchanged(&fixture, v_star, table_scope()).await
}

#[tokio::test]
async fn refuses_entry_above_v_star() {
    let err = stage_versions(&[1, 2], 1).await;
    assert!(
        matches!(
            err,
            ReplayToError::EntryAboveVStar {
                version: 2,
                v_star: 1
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_torn_tail_after_complete_entry_at_v_star() {
    let fixture = OfflineFixture::new();
    write_wal(&fixture.wal, &deletes(&[1])).await;
    let path = fixture.wal.join(SEGMENT);
    let offset = fs::metadata(&path).unwrap().len();
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0xAB, 0xCD, 0x01]).unwrap();
        file.sync_all().unwrap();
    }
    let err = replay_unchanged(&fixture, 1, table_scope()).await;
    assert!(
        matches!(
            &err,
            ReplayToError::TornTail(WalStrictReadError::IncompleteFrame { path: found, offset: at })
                if found == &path && *at == offset
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_gap_in_positive_suffix() {
    let err = stage_versions(&[4, 6], 6).await;
    assert!(
        matches!(
            err,
            ReplayToError::VersionGap {
                below: 4,
                expected: 5,
                above: 6
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_duplicate_versions_including_zero() {
    for (versions, v_star, duplicate) in [([4, 4, 5], 5, 4), ([0, 0, 5], 5, 0)] {
        let err = stage_versions(&versions, v_star).await;
        assert!(
            matches!(err, ReplayToError::DuplicateVersion(v) if v == duplicate),
            "{err:?}"
        );
    }
}

#[tokio::test]
async fn refuses_short_wal() {
    let err = stage_versions(&[4, 5], 6).await;
    assert!(
        matches!(
            err,
            ReplayToError::HighestBelowVStar {
                highest: 5,
                v_star: 6
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_out_of_scope_interner_delta() {
    let fixture = OfflineFixture::new();
    let mut entries = deletes(&[4, 5]);
    entries[0].interner_delta = vec![(7, "field".into(), 1)];
    write_wal(&fixture.wal, &entries).await;
    let err = replay_unchanged(&fixture, 5, table_scope()).await;
    assert!(
        matches!(
            err,
            ReplayToError::InternerScopeUnprovable {
                scope: 7,
                txn_id: 100,
                repo_scope: 0,
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_unknown_table_token() {
    let fixture = OfflineFixture::new();
    let token = table_token_for("not_in_catalogue");
    assert_ne!(token, table_token_for(TABLE));
    let mut entries = deletes(&[1]);
    entries.push(entry(
        101,
        2,
        WalOpV2::Delete {
            table_id_interned: token,
            rid: RecordId([8; 16]),
        },
    ));
    write_wal(&fixture.wal, &entries).await;
    let err = replay_unchanged(&fixture, 2, table_scope()).await;
    assert!(
        matches!(
            err,
            ReplayToError::Scope(ReplayScopeError::MissingTableRoute {
                txn_id: 101,
                token: found,
            }) if found == token
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_nonzero_index_id_on_known_table() {
    let token = table_token_for(TABLE);
    for (idx_id, key) in [(7, b"posting-key".as_slice()), (0, b"bad".as_slice())] {
        let fixture = OfflineFixture::new();
        let mut entries = deletes(&[1]);
        entries.push(entry(
            101,
            2,
            WalOpV2::IndexDel {
                table_id_interned: token,
                idx_id,
                key: bytes::Bytes::from_static(key),
            },
        ));
        write_wal(&fixture.wal, &entries).await;
        let err = replay_unchanged(&fixture, 2, table_scope()).await;
        assert!(
            matches!(
                err,
                ReplayToError::Scope(ReplayScopeError::MissingIndexRoute {
                    txn_id: 101,
                    token: found,
                    idx_id: found_idx_id,
                }) if found == token && found_idx_id == idx_id
            ),
            "idx_id={idx_id}: {err:?}"
        );
    }
}

#[tokio::test]
async fn refuses_broadcast_index_op_with_empty_table_scope() {
    let fixture = OfflineFixture::new();
    let entries = [entry(
        100,
        1,
        WalOpV2::IndexDel {
            table_id_interned: 0,
            idx_id: 0,
            key: bytes::Bytes::from_static(b"posting-key"),
        },
    )];
    write_wal(&fixture.wal, &entries).await;
    let err = replay_unchanged(&fixture, 1, ReplayScope::new(Vec::new())).await;
    assert!(
        matches!(
            err,
            ReplayToError::Scope(ReplayScopeError::EmptyTableScope { txn_id: 100 })
        ),
        "{err:?}"
    );
}
