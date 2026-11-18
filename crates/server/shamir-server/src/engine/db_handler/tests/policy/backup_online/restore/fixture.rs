//! Deterministic real-facade fixture for online-copy restore tests (R2a
//! step 3b-2).
//!
//! Builds a durable engine (`ShamirDb` over the persistent fjall system
//! catalogue `shamir_db_meta.redb`, one file-backed repo with real WAL
//! segments — NOT a fabricated WAL), drives the REAL `EngineBackupPort`
//! capture/seal barriers with bounded handshakes so a deterministic
//! writer commits BETWEEN the store copy at capture and the WAL copy at
//! seal (the staged store therefore truly needs WAL replay), copies
//! store/WAL/system catalogue with the ordinary file API (Windows: no
//! renames under open handles), and writes a v2 online manifest (manifest
//! last). No lock artifacts are synthesized in the happy-path copy — the
//! restore/boot must recreate them; only the unresolved-token variant's
//! inspected copy gets a system lock, released before hashing.
//!
//! Negative variants mutate the finished snapshot and re-hash the
//! manifest from the ordinary files on disk so `verify_manifest` still
//! accepts the snapshot and restore reaches the intended typed refusal.
//!
//! One tokio runtime per fixture, owned by the test; the port bridges the
//! async facade through its handle from outside the runtime; every facade
//! call site is `runtime.block_on(bounded(..))`; the writer task is
//! joined, its commit result asserted; each test body runs on its own
//! big-stack thread (`on_big_stack`) and drives the synchronous restore
//! chain directly on it.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::shamir_db::system_store::SystemStore;
use shamir_db::{DbError, DbResult, ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::durability::Durability;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::{write, Query};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::backup_online::{EngineBackupPort, PortRunState};
use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::online::manifest::{
    self, ConsistencySection, RepoConsistency, WalDirEntry, WalSection, WalSegmentEntry,
};
use crate::sync::durability::backup::online::wal_capture::OnlineBackupPort;
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry};

/// Bounded await bound: a timeout fails the test and discards the fixture.
pub const BOUND: Duration = Duration::from_secs(60);

const DB_NAME: &str = "app";
const REPO: &str = "main";
const LATE_REPO: &str = "late";
const PROBE_TABLE: &str = "rows";
const KEPT_TABLE: &str = "extra";
/// The real system catalogue directory name (fjall keyspace under the
/// data dir; repo paths are catalogued relative to its parent).
const SYSTEM_DIR: &str = "shamir_db_meta.redb";

/// Run `body` on a thread with a roomy stack: the engine fixture and the
/// restore chain nest deep async frames a debug test thread cannot hold.
pub fn on_big_stack<F: FnOnce() + Send + 'static>(body: F) -> std::thread::Result<()> {
    std::thread::Builder::new()
        .stack_size(96 * 1024 * 1024)
        .spawn(body)
        .expect("spawn big-stack test thread")
        .join()
}

/// Bounded await helper; call sites wrap it in `runtime.block_on`.
pub async fn bounded<F: Future>(step: &str, future: F) -> F::Output {
    timeout(BOUND, Box::pin(future))
        .await
        .unwrap_or_else(|_| panic!("{step} timed out"))
}

/// The live engine, its snapshot destination root, the restore target
/// root, and the single runtime that drives the async facade. The fixture
/// owns the source and target directories outright (TempDir fields keep
/// them alive for the whole test; paths are captured before any move).
/// Drop order is load-bearing: the engine (and its background tasks) drop
/// while the runtime is still alive, and both drop before the TempDirs
/// release their directories.
pub struct Fixture {
    pub db: Arc<ShamirDb>,
    pub runtime: tokio::runtime::Runtime,
    /// The engine data dir (the protocol's `source_root`).
    pub workspace: PathBuf,
    /// Parent of the snapshot directory.
    pub dest_root: PathBuf,
    /// Parent of the (absent) restore target `data` directory.
    pub target_root: PathBuf,
    _workspace_dir: TempDir,
    _dest_dir: TempDir,
    _target_dir: TempDir,
}

/// A finished online snapshot with its sealed read oracle.
pub struct Snapshot {
    pub dir: PathBuf,
    /// The sorted probe rows of the SEALED source state (read at `V*`,
    /// before the post-seal write) — the restore's expected boot content.
    pub sealed_rows: Vec<String>,
}

impl Fixture {
    /// Durable engine: persistent system catalogue, db `app`, repo `main`
    /// (tables `rows` + `extra`) with one committed, flushed row.
    pub fn new() -> Fixture {
        let workspace_dir = tempfile::tempdir().expect("workspace tempdir");
        let dest_dir = tempfile::tempdir().expect("dest tempdir");
        let target_dir = tempfile::tempdir().expect("target tempdir");
        let runtime = Builder::new_multi_thread()
            .worker_threads(4)
            .thread_stack_size(64 * 1024 * 1024)
            .enable_all()
            .build()
            .expect("fixture runtime");
        let workspace = workspace_dir.path().to_owned();
        let db = Arc::new(
            runtime
                .block_on(bounded("engine init", build_engine(&workspace)))
                .expect("build engine"),
        );
        Fixture {
            runtime,
            db,
            workspace,
            dest_root: dest_dir.path().to_owned(),
            target_root: target_dir.path().to_owned(),
            _workspace_dir: workspace_dir,
            _dest_dir: dest_dir,
            _target_dir: target_dir,
        }
    }

    /// Insert one committed row `{name}` into `app/main.rows` and flush;
    /// returns the committed version from the transaction metadata.
    pub fn insert_probe_row(&self, name: &str) -> u64 {
        self.runtime
            .block_on(bounded(
                "insert probe row",
                insert_row(&self.db, REPO, PROBE_TABLE, name),
            ))
            .expect("probe row insert")
    }

    /// Sorted probe-set read of `app/main.rows`.
    pub fn probe_rows(&self) -> Vec<String> {
        self.runtime
            .block_on(bounded(
                "probe read",
                read_rows(&self.db, REPO, PROBE_TABLE),
            ))
            .expect("probe read")
    }

    /// The live repo's real WAL directory (`repos/main.shamirwal` sibling).
    pub fn repo_wal_dir(&self) -> PathBuf {
        self.workspace.join(format!("repos/{REPO}.shamirwal"))
    }

    /// The restore target `data` directory (fresh-machine case: absent).
    pub fn target_data_dir(&self) -> PathBuf {
        self.target_root.join("data")
    }

    /// Drive the REAL `EngineBackupPort` capture/seal barriers with
    /// deterministic handshakes and assemble a v2 online snapshot.
    ///
    /// Sequence (every boundary a bounded handshake, no sleeps):
    /// catalogue -> pins -> capture barrier -> store copy -> writer commits
    /// `before_seal` (joined via the fixture runtime, commit result
    /// asserted) -> seal barrier `V*` -> WAL copy -> END SEAL (released
    /// BEFORE any later add_repo / flush_all_waiting, whose writer-drain
    /// waits would self-deadlock under the seal's drain pause)
    /// -> [optional created-after-capture repo + store copy]
    /// -> system catalogue capture + copy (+ WAL sibling when present)
    /// -> `backup.meta.json` -> manifest
    /// last -> SEALED read oracle captured, THEN the post-seal source-only
    /// write `after_seal`. No lock artifacts are synthesized: the copied
    /// store and catalogue directories carry no `lock` file — the restore
    /// and the restored boot must recreate them.
    pub fn capture_snapshot(&self, register_late_repo: bool) -> Snapshot {
        let snapshot_dir = self.dest_root.join("online");
        fs::create_dir(&snapshot_dir).expect("snapshot dir");
        let repos_dir = snapshot_dir.join("repos");
        fs::create_dir(&repos_dir).expect("snapshot repos dir");

        let run_state = Arc::new(PortRunState::new());
        let port = Arc::new(EngineBackupPort::new(
            Arc::clone(&self.db),
            self.workspace.clone(),
            self.runtime.handle().clone(),
            Arc::clone(&run_state),
        ));

        let sources = port.catalogue().expect("port catalogue");
        assert_eq!(sources.len(), 1, "one file-backed repo captured");
        // The catalogue reports the store dir in its runtime form; accept a
        // data-dir-relative form defensively (it must resolve under the
        // workspace either way).
        let reported_store = sources[0].store_dir.clone();
        let captured_store = if reported_store.is_absolute() {
            reported_store
        } else {
            self.workspace.join(&reported_store)
        };
        port.acquire_pins().expect("truncation pins");

        // Capture barrier: the flush point of the only commit so far.
        let captured = port.capture_point("app/main").expect("capture barrier");
        assert_eq!(captured.store_dir, format!("repos/{REPO}"));

        // Stage the store BEFORE the writer can commit again: the staged
        // copy is behind the seal point and truly needs WAL replay.
        copy_tree(&captured_store, &repos_dir.join(REPO));

        // Deterministic writer commits between the store copy and the WAL
        // copy, bridged through the fixture runtime from this thread; the
        // task handle is joined and its commit result asserted.
        let (acked_tx, acked_rx) = oneshot::channel();
        let writer = {
            let db = Arc::clone(&self.db);
            self.runtime.spawn(async move {
                let committed = bounded(
                    "between-copy commit",
                    insert_row(&db, REPO, PROBE_TABLE, "before_seal"),
                )
                .await;
                let _ = acked_tx.send(());
                committed
            })
        };
        self.runtime
            .block_on(bounded("writer commit handshake", acked_rx))
            .expect("writer commit handshake");
        let committed = self
            .runtime
            .block_on(bounded("join writer", writer))
            .expect("writer task join")
            .expect("writer commit");
        assert!(committed >= 2, "the between-copy commit must be versioned");

        // Seal barrier: `V*` covers the between-copy commit.
        let sealed = port.seal_point("app/main").expect("seal barrier");
        let v_star = sealed.wal_position;
        assert!(
            v_star > captured.wal_position,
            "seal must move past capture"
        );

        // The sealed WAL (frozen by the seal hold) is staged second.
        copy_tree(
            &self.repo_wal_dir(),
            &repos_dir.join(format!("{REPO}.shamirwal")),
        );

        // Release the live repo's seal hold BEFORE any later add_repo or
        // flush_all_waiting: the seal holds a drain pause and both of those
        // wait on the writer drain, so keeping the hold open would
        // self-deadlock this thread against itself.
        port.end_seal("app/main");

        if register_late_repo {
            let db = Arc::clone(&self.db);
            let late_store = self.workspace.join(format!("repos/{LATE_REPO}"));
            let late_staged = repos_dir.join(LATE_REPO);
            self.runtime
                .block_on(bounded("late repo registration", async move {
                    db.add_repo(
                        DB_NAME,
                        RepoConfig::new(LATE_REPO, BoxRepoFactory::fjall_raw(late_store))
                            .add_table(TableConfig::new("late_rows")),
                    )
                    .await
                    .expect("add late repo");
                    let instance = db.get_db(DB_NAME).unwrap().get_repo(LATE_REPO).unwrap();
                    instance
                        .get_table("late_rows")
                        .await
                        .expect("open late table");
                    drop(instance);
                    insert_row(&db, LATE_REPO, "late_rows", "late_seed")
                        .await
                        .expect("late seed insert");
                }));
            // The copy may carry files + catalogue for the late repo; it is
            // still NOT in the captured set (captured at step A).
            copy_tree(
                &self.workspace.join(format!("repos/{LATE_REPO}")),
                &late_staged,
            );
        }

        // System catalogue capture (writer-drain barrier, safe now that
        // the seal hold is released) then copy while quiescent: carries
        // every catalogue row committed before now, including the late
        // repo when registered.
        let system_version = port.capture_system_store().expect("system store capture");
        copy_tree(
            &self.workspace.join(SYSTEM_DIR),
            &snapshot_dir.join(SYSTEM_DIR),
        );
        // The system catalogue may have its own out-of-tree WAL sibling;
        // the capture producer copies it as residual, mirror that when it
        // exists so the staged copy is complete.
        let system_wal_rel = format!("{SYSTEM_DIR}.shamirwal");
        let system_wal_dir = self.workspace.join(&system_wal_rel);
        if system_wal_dir.exists() {
            copy_tree(&system_wal_dir, &snapshot_dir.join(&system_wal_rel));
        }

        port.release_pins();
        drop(port);

        manifest::write_backup_meta(&snapshot_dir).expect("backup meta");
        write_snapshot_manifest(&snapshot_dir, v_star, system_version);

        // SEALED read oracle BEFORE the post-seal insert...
        let sealed_rows = self.probe_rows();
        assert_eq!(
            sealed_rows,
            ["before_capture", "before_seal"],
            "sealed oracle: exactly the pre-seal probe set"
        );
        // ...then the post-seal source-only write.
        let after = self.insert_probe_row("after_seal");
        assert!(after > v_star, "the post-seal commit is above V*");
        assert_eq!(
            self.probe_rows(),
            ["after_seal", "before_capture", "before_seal"],
            "source oracle: the post-seal write must be live only"
        );
        Snapshot {
            dir: snapshot_dir,
            sealed_rows,
        }
    }
}

/// Sorted probe rows of one repo table through the facade; returns the
/// read response rows with `name` string fields.
async fn read_rows(db: &ShamirDb, repo: &str, table: &str) -> DbResult<Vec<String>> {
    let mut batch = Batch::new();
    batch.query("probe", Query::with_repo(repo, table));
    let response = db
        .execute(DB_NAME, &batch.build())
        .await
        .map_err(|error| DbError::Internal(error.to_string()))?;
    let mut names: Vec<String> = response.results["probe"]
        .records
        .iter()
        .map(|row| row.get_value_str("name").unwrap().to_owned())
        .collect();
    names.sort_unstable();
    Ok(names)
}

/// One committed, flushed insert through the explicit transactional batch
/// builder; returns the committed version from the transaction metadata.
async fn insert_row(db: &ShamirDb, repo: &str, table: &str, name: &str) -> DbResult<u64> {
    let mut fields = new_map();
    fields.insert("name".into(), QueryValue::Str(name.into()));
    let mut batch = Batch::new();
    batch.transactional();
    batch.durability(Durability::Synced);
    batch.insert(
        format!("ins_{name}"),
        write::Insert::with_repo(repo, table).row(QueryValue::Map(fields)),
    );
    let response = db
        .execute(DB_NAME, &batch.build())
        .await
        .map_err(|error| DbError::Internal(error.to_string()))?;
    let committed = response
        .transaction
        .and_then(|tx| tx.commit_version)
        .expect("transactional insert carries a commit version");
    db.flush_all_waiting().await?;
    Ok(committed)
}

/// Boot the durable engine fixture on `root`.
async fn build_engine(root: &Path) -> DbResult<ShamirDb> {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join(SYSTEM_DIR))).await?;
    db.create_db(DB_NAME).await?;
    db.add_repo(
        DB_NAME,
        RepoConfig::new(
            REPO,
            BoxRepoFactory::fjall_raw(root.join(format!("repos/{REPO}"))),
        )
        .add_table(TableConfig::new(PROBE_TABLE).with_new_store_id())
        .add_table(TableConfig::new(KEPT_TABLE).with_new_store_id()),
    )
    .await?;
    // Open every configured table before it is read.
    let instance = db.get_db(DB_NAME).unwrap().get_repo(REPO).unwrap();
    instance.get_table(PROBE_TABLE).await?;
    instance.get_table(KEPT_TABLE).await?;
    drop(instance);
    insert_row(&db, REPO, PROBE_TABLE, "before_capture").await?;
    Ok(db)
}

impl Snapshot {
    /// The finished snapshot must pass the offline verifier.
    pub fn assert_verifies(&self) {
        verify_manifest(&self.dir).expect("snapshot must verify");
    }

    /// Decrement the captured point in the manifest to one below the
    /// recorded seal WITHOUT touching any file: the staged (captured) WAL
    /// is unchanged and still verifies, but now carries entries ABOVE the
    /// recorded point `V* - 1` — the typed `EntryAboveVStar` refusal.
    pub fn decrement_captured_point(&self) -> u64 {
        let raw = fs::read_to_string(self.dir.join("manifest.json")).expect("read manifest");
        let mut decoded = manifest::decode(&raw).expect("decode manifest");
        let consistency = decoded
            .consistency
            .as_mut()
            .expect("v2 consistency section");
        let point = consistency
            .per_repo
            .get_mut(&format!("{DB_NAME}/{REPO}"))
            .expect("captured repo");
        assert!(
            point.wal_position > 1,
            "the captured point must be decrementable"
        );
        point.wal_position -= 1;
        let recorded = point.wal_position;
        let encoded = manifest::encode(&decoded).expect("encode manifest");
        manifest::write_manifest(&self.dir, &encoded).expect("rewrite manifest");
        recorded
    }

    /// Remove ONLY the captured repo's store directory from the snapshot,
    /// keeping the WAL directory and the manifest's WAL listing route, and
    /// re-hash the manifest so the verifier passes and restore reaches the
    /// consistency pass.
    pub fn drop_captured_store_dir(&self) {
        fs::remove_dir_all(self.dir.join("repos").join(REPO)).expect("remove staged store");
        self.rewrite_manifest();
    }

    /// Remove the committed `rows` table route from the SNAPSHOT's system
    /// catalogue through the public `SystemStore::remove_table` (opened
    /// offline on the inspected copy; the live engine is untouched). The
    /// kept `extra` table remains, so replay raises `MissingTableRoute`,
    /// not `EmptyTableScope`. This variant may synthesize ONLY the system
    /// `lock` for the inspected copy; the handle is dropped (fjall lock
    /// released) BEFORE the manifest is re-hashed.
    pub fn remove_probe_table_route(&self, fixture: &Fixture) {
        let system_dir = self.dir.join(SYSTEM_DIR);
        let store_dir = system_dir.clone();
        fs::File::create_new(system_dir.join("lock")).expect("inspected copy system lock");
        let system = fixture
            .runtime
            .block_on(bounded("open snapshot system store", async {
                SystemStore::init(SystemStoreConfig::Fjall(store_dir)).await
            }))
            .expect("open snapshot system store");
        fixture
            .runtime
            .block_on(bounded(
                "remove table route",
                system.remove_table(DB_NAME, REPO, PROBE_TABLE),
            ))
            .expect("remove table route");
        drop(system);
        // The fjall handle release is asynchronous relative to the OS lock;
        // probe the lock from the fixture runtime in a bounded loop until
        // the directory can actually be reopened, then re-hash whatever
        // was written.
        let deadline = Instant::now() + BOUND;
        loop {
            let probe_dir = system_dir.clone();
            match fixture
                .runtime
                .block_on(bounded("probe system lock", async {
                    SystemStore::init(SystemStoreConfig::Fjall(probe_dir)).await
                })) {
                Ok(probe) => {
                    drop(probe);
                    break;
                }
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "snapshot system lock not released within the bound: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        self.rewrite_manifest();
    }

    /// Re-walk the snapshot, re-hash every ordinary file, rebuild the WAL
    /// section from disk, and rewrite the manifest (manifest last).
    pub fn rewrite_manifest(&self) {
        let raw = fs::read_to_string(self.dir.join("manifest.json")).expect("read manifest");
        let mut decoded = manifest::decode(&raw).expect("decode manifest");
        decoded.files = collect_files(&self.dir);
        decoded.wal = Some(build_wal_section(&self.dir));
        let encoded = manifest::encode(&decoded).expect("encode manifest");
        manifest::write_manifest(&self.dir, &encoded).expect("rewrite manifest");
    }
}

/// Encode and write the v2 online manifest (LAST write into the snapshot).
fn write_snapshot_manifest(snapshot_dir: &Path, v_star: u64, system_version: u64) {
    let mut per_repo = BTreeMap::new();
    per_repo.insert(
        format!("{DB_NAME}/{REPO}"),
        RepoConsistency {
            wal_position: v_star,
            store_dir: format!("repos/{REPO}"),
            schema_version: 0,
        },
    );
    let manifest_v2 = manifest::build_v2(
        0,
        collect_files(snapshot_dir),
        None,
        None,
        ConsistencySection {
            per_repo,
            system_store_version: system_version,
        },
        build_wal_section(snapshot_dir),
    );
    let encoded = manifest::encode(&manifest_v2).expect("encode manifest");
    manifest::write_manifest(snapshot_dir, &encoded).expect("write manifest");
}

/// Recursively list every ordinary file under `root` except the manifest
/// itself, as snapshot-relative forward-slash entries with fresh hashes.
fn collect_files(root: &Path) -> Vec<ManifestFileEntry> {
    let mut entries = Vec::new();
    walk_files(root, root, &mut entries);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn walk_files(root: &Path, dir: &Path, entries: &mut Vec<ManifestFileEntry>) {
    for entry in fs::read_dir(dir).expect("walk dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            walk_files(root, &path, entries);
            continue;
        }
        if path.file_name().is_some_and(|name| name == "manifest.json") {
            continue;
        }
        let (sha256, size_bytes) = hash_file(&path).expect("hash file");
        let rel = path
            .strip_prefix(root)
            .expect("relative path")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        entries.push(ManifestFileEntry {
            path: rel,
            sha256,
            size_bytes,
        });
    }
}

/// Build the manifest WAL section from the staged WAL directory on disk.
/// Repos whose staged WAL is gone are omitted entirely.
fn build_wal_section(snapshot_dir: &Path) -> WalSection {
    let wal_rel = format!("repos/{REPO}.shamirwal");
    let wal_dir = snapshot_dir.join(&wal_rel);
    let mut segments: Vec<WalSegmentEntry> = Vec::new();
    if let Ok(listing) = fs::read_dir(&wal_dir) {
        for entry in listing.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let Some(stem) = name.strip_suffix(".wal") else {
                continue;
            };
            let Ok(seq) = stem.parse::<u64>() else {
                continue;
            };
            let (sha256, size) = hash_file(&path).expect("hash WAL segment");
            let sidecar = wal_dir.join(format!("{stem}.meta")).exists();
            segments.push(WalSegmentEntry {
                seq,
                file: format!("{wal_rel}/{name}"),
                sidecar: sidecar.then(|| format!("{wal_rel}/{stem}.meta")),
                sha256,
                size,
            });
        }
    }
    segments.sort_by_key(|s| s.seq);
    if segments.is_empty() {
        return WalSection { dirs: Vec::new() };
    }
    WalSection {
        dirs: vec![WalDirEntry {
            repo: format!("{DB_NAME}/{REPO}"),
            dir: wal_rel,
            segments,
        }],
    }
}

/// Ordinary file-API recursive copy (Windows: never rename under open
/// handles). Skips live fjall `lock` files and does NOT synthesize one —
/// the restore and the restored boot must recreate locks themselves.
fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create target dir");
    for entry in fs::read_dir(source).expect("read source dir") {
        let entry = entry.expect("source entry");
        let name = entry.file_name();
        if name == "lock" {
            continue;
        }
        let destination = target.join(&name);
        if entry.file_type().expect("entry type").is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).expect("copy file");
        }
    }
}
