//! Online-copy restore tests over the real engine facade, the real
//! `EngineBackupPort` capture/seal and the real production `restore`
//! (R2a step 3b-2).
//!
//! Proven, one test per acceptance row:
//! * happy path — the copy (store staged BEFORE a between-copy commit,
//!   WAL staged at the seal point) replays to `V*` on a FRESH target
//!   (clean-machine case) and boots: the probe set equals the sealed
//!   source oracle captured by the fixture (`Snapshot::sealed_rows`) and
//!   the post-seal source-only write is absent;
//! * created-after-capture — a repo registered after the catalogue
//!   capture is reported in `RestoreReport::excluded_repositories` and is
//!   NOT registered on the restored boot;
//! * `EntryAboveVStar` — a manifest recording one-below the captured WAL's
//!   highest entry (files untouched, verifier still accepts) is refused
//!   with the typed replay error naming the repo; the PRE-EXISTING target
//!   content is left byte-identical (read-only online target path, run
//!   with `force = false` and a PANICKING legacy probe — proof the online
//!   path bypasses the caller probe) with no staged/rollback sibling;
//! * `MissingTableRoute` — omitting the committed `rows` route from the
//!   snapshot's system catalogue (public `SystemStore::remove_table`,
//!   keeping the `extra` table) is refused as an unresolved table token
//!   under the same pre-populated-target/panic-probe contract;
//! * missing captured repository directory — ONLY the store directory
//!   removed (WAL and its manifest listing kept), manifest re-hashed,
//!   refused by the consistency check (typed
//!   `RestoreError::Consistency { repo, reason }`) on a fresh target;
//! * captured orphan — ONLY the repo row removed from the snapshot's
//!   catalogue (`SystemStore::remove_repository`, table rows retained):
//!   restore SUCCEEDS on a fresh target with the orphan tolerated (its
//!   store/WAL files stay inert, the boot registers nothing);
//! * captured orphan, strict WAL — the same orphan with the manifest
//!   point decremented (verifier still accepts) is refused with typed
//!   `EntryAboveVStar` naming the repo on a fresh target: the orphan's
//!   WAL still replays strictly;
//! * store contents ABOVE `V*` — the staged store replaced with a fresh
//!   copy of the live store AFTER the post-seal write (WAL and recorded
//!   point untouched): strict replay REFUSES, fail-closed, with the typed
//!   `ReplayToError::Recovery(DbError::Validation(...))` carrying the
//!   exact expected-version mismatch (checked BEFORE any WAL entry is
//!   applied), the fresh target stays absent and no stage/rollback
//!   sibling is left behind.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shamir_db::engine::db_instance::replay_to::{ReplayScopeError, ReplayToError};
use shamir_db::{DbError, ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;

use super::fixture::{on_big_stack, Fixture};
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::restore::{restore_with_probe, RestoreError, RestoreReport};

const SYSTEM_DIR: &str = "shamir_db_meta.redb";

/// Deterministic pre-existing target content for the replay-refusal
/// tests: nested marker files under the restore target `data` dir, so the
/// read-only online refusal path is exercised against a NON-empty target.
const SEED_MARKERS: &[&str] = &["repos/nested/marker.dat", "stamp.preexisting"];

/// Run the synchronous production restore on the (roomy) test thread: the
/// production restore owns its internal deep stacks, so no detached test
/// thread is needed. Online restores bypass the caller probe entirely
/// (read-only target path), so the probe PANICS — if the legacy path ever
/// ran, the test would fail loudly; `force = false` keeps the real gate.
fn run_restore(fixture: &Fixture) -> Result<RestoreReport, RestoreError> {
    let from = fixture.dest_root.join("online");
    let target = fixture.target_data_dir();
    restore_with_probe(&from, &target, false, |_path| {
        panic!("legacy liveness probe must not run for an online snapshot");
    })
}

/// Boot the restored `data` directory the way the next boot performs (the
/// same init the fixture used) and return the sorted probe set.
fn boot_probe_rows(fixture: &Fixture) -> Vec<String> {
    let target = fixture.target_data_dir();
    let db = fixture
        .runtime
        .block_on(super::fixture::bounded(
            "boot restored data dir",
            ShamirDb::init(SystemStoreConfig::Fjall(target.join(SYSTEM_DIR))),
        ))
        .expect("boot restored engine");
    let mut batch = Batch::new();
    batch.query("probe", Query::with_repo("main", "rows"));
    let response = fixture
        .runtime
        .block_on(super::fixture::bounded("restored probe read", async {
            db.execute("app", &batch.build()).await
        }))
        .expect("restored probe batch");
    let mut names: Vec<String> = response.results["probe"]
        .records
        .iter()
        .map(|row| row.get_value_str("name").unwrap().to_owned())
        .collect();
    names.sort_unstable();
    names
}

/// The registered repos of `app` on the restored boot (sorted).
fn boot_repo_names(fixture: &Fixture) -> Vec<String> {
    let target = fixture.target_data_dir();
    let db = fixture
        .runtime
        .block_on(super::fixture::bounded(
            "boot restored data dir",
            ShamirDb::init(SystemStoreConfig::Fjall(target.join(SYSTEM_DIR))),
        ))
        .expect("boot restored engine");
    let mut names = db
        .get_db("app")
        .expect("restored app database")
        .list_repos();
    names.sort();
    names
}

/// Deterministic bytes for one seeded marker file.
fn marker_bytes(rel: &str) -> Vec<u8> {
    format!("pre-existing target marker: {rel}\n").into_bytes()
}

/// Full relative-path -> bytes map of the target `data` directory.
fn target_byte_map(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    collect_target_files(root, root, &mut map);
    map
}

fn collect_target_files(root: &Path, dir: &Path, map: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).expect("walk target dir") {
        let entry = entry.expect("target dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_target_files(root, &path, map);
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .expect("relative target path")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        map.insert(rel, fs::read(&path).expect("read target file"));
    }
}

/// Pre-populate the restore target `data` directory with the deterministic
/// nested markers and return the full byte map for the untouched check.
fn seed_existing_target(fixture: &Fixture) -> BTreeMap<String, Vec<u8>> {
    let data = fixture.target_data_dir();
    for rel in SEED_MARKERS {
        let path = data.join(rel);
        fs::create_dir_all(path.parent().expect("seed parent")).expect("seed parent dir");
        fs::write(&path, marker_bytes(rel)).expect("seed marker file");
    }
    target_byte_map(&data)
}

/// A refused online restore must leave the PRE-EXISTING target content
/// byte-identical and must not leave a staged `*.restore_tmp_*` or a
/// rollback `.pre_restore_backup_*` sibling next to the target.
fn assert_existing_target_untouched(fixture: &Fixture, before: &BTreeMap<String, Vec<u8>>) {
    let data = fixture.target_data_dir();
    let after = target_byte_map(&data);
    assert_eq!(
        after, *before,
        "the online refusal path must not touch the existing target `data` dir"
    );
    assert_no_stage_or_rollback_sibling(fixture);
}

/// A refused online restore on a fresh target must leave the target absent
/// and must not leave a staged or rollback sibling behind.
fn assert_clean_refusal(fixture: &Fixture) {
    assert!(
        !fixture.target_data_dir().exists(),
        "the live target tree must be untouched by a refused restore"
    );
    assert_no_stage_or_rollback_sibling(fixture);
}

fn assert_no_stage_or_rollback_sibling(fixture: &Fixture) {
    let staged: Vec<PathBuf> = fs::read_dir(&fixture.target_root)
        .expect("target root")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            name.contains("restore_tmp") || name.contains("pre_restore_backup")
        })
        .collect();
    assert!(
        staged.is_empty(),
        "a refused restore must leave no staged/rollback sibling: {staged:?}"
    );
}

/// Happy path on a FRESH target (clean-machine case): the sealed copy
/// replays to `V*` and boots to the sealed source state (asserted against
/// the fixture's sealed oracle AND the expected vector); the post-seal
/// source-only write is absent from the restore.
#[test]
fn restored_online_copy_matches_source_at_seal_on_fresh_target() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        assert_eq!(
            fixture.probe_rows(),
            ["after_seal", "before_capture", "before_seal"],
            "source oracle: post-seal write is live only"
        );
        snapshot.assert_verifies();

        let report = run_restore(&fixture).expect("online restore must succeed");
        assert!(
            report.files_restored > 0,
            "the restore must copy the snapshot files"
        );
        assert_eq!(
            report.pre_restore_backup, None,
            "a fresh target has no pre-restore backup"
        );
        assert!(
            report.excluded_repositories.is_empty(),
            "no repo was created after capture: {:?}",
            report.excluded_repositories
        );

        let boot = boot_probe_rows(&fixture);
        assert_eq!(
            boot, snapshot.sealed_rows,
            "the restored boot must equal the fixture's sealed probe oracle"
        );
        assert_eq!(
            boot,
            ["before_capture", "before_seal"],
            "the restore must land at the seal point; `after_seal` is source-only"
        );

        drop(fixture);
    })
    .expect("test thread");
}

/// A repo registered after the catalogue capture is copied (its files and
/// catalogue row are in the snapshot) but is excluded from the captured
/// set: reported in the restore and NOT registered on the restored boot.
#[test]
fn repo_created_after_capture_is_excluded_and_not_registered_on_boot() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(true);
        snapshot.assert_verifies();

        let report = run_restore(&fixture).expect("online restore must succeed");
        assert!(
            report
                .excluded_repositories
                .iter()
                .any(|entry| entry.starts_with("app/late")),
            "the late repo must be named in excluded_repositories: {:?}",
            report.excluded_repositories
        );

        let repos = boot_repo_names(&fixture);
        assert!(
            repos.contains(&"main".to_string()),
            "the captured repo must be registered: {repos:?}"
        );
        assert!(
            !repos.contains(&"late".to_string()),
            "the created-after-capture repo must NOT be registered on boot: {repos:?}"
        );

        drop(fixture);
    })
    .expect("test thread");
}

/// The manifest records one BELOW the captured WAL's highest entry (files
/// untouched, verifier still accepts): replay refuses with the typed
/// `EntryAboveVStar` error naming the repo; the pre-populated target stays
/// byte-identical with no staged/rollback sibling.
#[test]
fn wal_entry_above_recorded_point_refuses_replay_naming_the_repo() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        let _recorded_point = snapshot.decrement_captured_point();
        snapshot.assert_verifies();
        let seeded = seed_existing_target(&fixture);

        match run_restore(&fixture) {
            Ok(report) => {
                panic!("replay of an entry above the recorded point must refuse: {report:?}")
            }
            Err(RestoreError::Replay { repo, source }) => {
                assert_eq!(repo, "app/main", "the refusal must name the repository");
                assert!(
                    matches!(*source, ReplayToError::EntryAboveVStar { .. }),
                    "expected EntryAboveVStar, got: {source}"
                );
            }
            Err(other) => panic!("expected RestoreError::Replay, got: {other}"),
        }
        assert_existing_target_untouched(&fixture, &seeded);

        drop(fixture);
    })
    .expect("test thread");
}

/// The committed `rows` table route is omitted from the snapshot's system
/// catalogue (public `SystemStore::remove_table`; the kept `extra` table
/// prevents `EmptyTableScope`): replay refuses with the typed unresolved
/// token error; the pre-populated target stays byte-identical with no
/// staged/rollback sibling.
#[test]
fn unresolved_table_token_refuses_replay_as_missing_table_route() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        snapshot.remove_probe_table_route(&fixture);
        snapshot.assert_verifies();
        let seeded = seed_existing_target(&fixture);

        match run_restore(&fixture) {
            Ok(report) => panic!("replay without a table route must refuse: {report:?}"),
            Err(RestoreError::Replay { repo, source }) => {
                assert_eq!(repo, "app/main", "the refusal must name the repository");
                assert!(
                    matches!(
                        *source,
                        ReplayToError::Scope(ReplayScopeError::MissingTableRoute { .. })
                    ),
                    "expected MissingTableRoute, got: {source}"
                );
            }
            Err(other) => panic!("expected RestoreError::Replay, got: {other}"),
        }
        assert_existing_target_untouched(&fixture, &seeded);

        drop(fixture);
    })
    .expect("test thread");
}

/// The captured repository's catalogue row is missing from the snapshot
/// (ONLY the row removed via the public `SystemStore::remove_repository`
/// — the repo row deletion that retains the table rows; the store/WAL
/// directories and the manifest's WAL route stay, manifest re-hashed so
/// verification passes): restore SUCCEEDS on a fresh target — the orphan
/// is tolerated, NOT registered on the restored boot, its staged files
/// stay in place as inert data, and the reported exclusions stay empty
/// (nothing was in the staged catalogue to exclude).
#[test]
fn captured_repo_without_catalogue_row_is_tolerated_inert_orphan() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        snapshot.drop_captured_repo_row(&fixture);
        snapshot.assert_verifies();

        let report = run_restore(&fixture).expect("a captured orphan must be tolerated");
        assert!(
            report.excluded_repositories.is_empty(),
            "the orphan's row was already gone from the staged catalogue, so \
             nothing was excluded during restore: {:?}",
            report.excluded_repositories
        );
        assert_eq!(
            report.pre_restore_backup, None,
            "a fresh target has no pre-restore backup"
        );

        // The orphan's store/WAL directories are copied into the target as
        // inert data (their files exist), but the restored boot has no
        // repository row for them: `app` opens with zero repos.
        let target = fixture.target_data_dir();
        assert!(
            target.join("repos/main").is_dir(),
            "the orphan store directory must be present in the restored tree"
        );
        assert!(
            target.join("repos/main.shamirwal").is_dir(),
            "the orphan WAL directory must be present in the restored tree"
        );
        let repos = boot_repo_names(&fixture);
        assert!(
            !repos.contains(&"main".to_string()),
            "the orphan must NOT be registered on the restored boot: {repos:?}"
        );

        drop(fixture);
    })
    .expect("test thread");
}

/// Companion negative: the SAME orphan snapshot with the manifest point
/// decremented below the staged WAL's highest entry (verifier still
/// accepts). The orphan still gets a replay job (nothing is silently
/// skipped), so its WAL is checked STRICTLY: replay refuses with the
/// typed `EntryAboveVStar` error naming the repo even though the repo has
/// no catalogue row, and the fresh target stays absent with staging
/// cleaned.
#[test]
fn captured_orphan_wal_is_replayed_strictly_on_tampered_point() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        snapshot.drop_captured_repo_row(&fixture);
        let _recorded_point = snapshot.decrement_captured_point();
        snapshot.assert_verifies();

        match run_restore(&fixture) {
            Ok(report) => {
                panic!("an orphan replayed past its recorded point must refuse: {report:?}")
            }
            Err(RestoreError::Replay { repo, source }) => {
                assert_eq!(repo, "app/main", "the refusal must name the repository");
                assert!(
                    matches!(*source, ReplayToError::EntryAboveVStar { .. }),
                    "expected EntryAboveVStar, got: {source}"
                );
            }
            Err(other) => panic!("expected RestoreError::Replay, got: {other}"),
        }
        assert_clean_refusal(&fixture);

        drop(fixture);
    })
    .expect("test thread");
}

/// The recorded captured point (`wal_position`) of `app/main` in the
/// snapshot's manifest.
fn captured_point(snapshot_dir: &Path) -> u64 {
    let raw = fs::read_to_string(snapshot_dir.join("manifest.json")).expect("read manifest");
    let decoded = manifest::decode(&raw).expect("decode manifest");
    decoded
        .consistency
        .expect("v2 consistency section")
        .per_repo
        .get("app/main")
        .expect("captured repo row")
        .wal_position
}

/// Store contents ABOVE the recorded point are REFUSED, fail-closed: the
/// staged store is replaced with a fresh copy of the live store AFTER the
/// post-seal write `after_seal` (the staged WAL and the manifest's
/// recorded point `V*` stay untouched, the manifest re-hashed so the
/// verifier accepts). Strict replay checks the version read from the
/// opened store against `V*` BEFORE applying any WAL entry, so restore
/// REFUSES with the typed `ReplayToError::Recovery` wrapping a
/// `DbError::Validation` whose message is exactly the expected-version
/// mismatch, the absent fresh target stays absent and no staged/rollback
/// sibling is left behind.
#[test]
fn store_contents_above_recorded_point_are_refused_fail_closed_on_restore() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        let recorded = captured_point(&snapshot.dir);
        snapshot.stage_above_v_star_store(&fixture);
        assert_eq!(
            captured_point(&snapshot.dir),
            recorded,
            "the captured point V* must be unchanged by the store replacement"
        );
        snapshot.assert_verifies();

        match run_restore(&fixture) {
            Ok(report) => {
                panic!("replay against a store above the recorded point must refuse: {report:?}")
            }
            Err(RestoreError::Replay { repo, source }) => {
                assert_eq!(repo, "app/main", "the refusal must name the repository");
                match &*source {
                    ReplayToError::Recovery(DbError::Validation(message)) => {
                        assert_expected_version_mismatch(message, recorded);
                    }
                    other => panic!(
                        "expected Recovery(Validation(expected-version mismatch)), \
                         got: {other}"
                    ),
                }
            }
            Err(other) => panic!("expected RestoreError::Replay, got: {other}"),
        }
        assert_clean_refusal(&fixture);

        drop(fixture);
    })
    .expect("test thread");
}

/// The validation message must be exactly the strict-replay expected-
/// version mismatch: `strict WAL replay: expected version {v_star}, got
/// committed {committed}, allocated {allocated}` with `v_star == recorded`
/// and the store-opened versions equal to each other and ABOVE `recorded`
/// (the live store carries the post-seal commit). The committed/allocated
/// values are parsed from the message, not assumed.
fn assert_expected_version_mismatch(message: &str, recorded: u64) {
    let prefix = format!("strict WAL replay: expected version {recorded}, got committed ");
    let rest = message.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("expected the expected-version mismatch naming V*={recorded}, got: {message:?}")
    });
    let (committed, allocated) = rest
        .split_once(", allocated ")
        .unwrap_or_else(|| panic!("missing allocated version: {message:?}"));
    let committed: u64 = committed.parse().expect("committed version");
    let allocated: u64 = allocated.parse().expect("allocated version");
    assert_eq!(
        committed, allocated,
        "the store-opened versions must agree: {message:?}"
    );
    assert!(
        committed > recorded,
        "the store's versions must sit above V*: {message:?}"
    );
}

/// The captured repository's store directory is missing from the snapshot
/// (ONLY the store removed — the WAL directory and its manifest listing
/// stay; the manifest is re-hashed so verification passes): the
/// consistency check refuses with the typed `RestoreError::Consistency`,
/// and the fresh target stays absent with staging cleaned.
#[test]
fn missing_captured_store_dir_refuses_consistency() {
    on_big_stack(|| {
        let fixture = Fixture::new();
        let snapshot = fixture.capture_snapshot(false);
        snapshot.drop_captured_store_dir();
        snapshot.assert_verifies();

        match run_restore(&fixture) {
            Ok(report) => panic!("a missing captured repo directory must refuse: {report:?}"),
            Err(RestoreError::Consistency { repo, .. }) => {
                assert_eq!(repo, "app/main", "the refusal must name the repository");
            }
            Err(other) => panic!("expected RestoreError::Consistency, got: {other}"),
        }
        assert_clean_refusal(&fixture);

        drop(fixture);
    })
    .expect("test thread");
}
