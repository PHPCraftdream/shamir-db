//! Pinned properties of the offline logical dump (design `logical-dump.md`
//! §5 "Positive cases" and §6 PR 4): canonical export determinism modulo
//! the header's diagnostic fields, verify-only on produced and handcrafted
//! artifacts (including an older-epoch header that must still import),
//! exact `ttl_ms` round trip on the streaming fixture, complete-target
//! exact retry accepted versus a changed-effective-option retry refused on
//! the 64 MiB dataset, and the no-publication/no-stage invariant with a
//! deterministic mid-load cancellation on the 64 MiB dataset.
//!
//! Small-input retries, refusals and round-trip kinds are proven by
//! `tests/admin/logical_import/` and are deliberately NOT re-tested here;
//! these tests extend the properties only where the large dataset or the
//! handcrafted container adds value. Every await is bounded by a timeout.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_server::logical_import::{self, ImportReport, LogicalImportError};
use shamir_sync::replication_seed::{
    decode_canonical, verify_dump_file, DatabaseItem, DumpReader, DumpScope, DumpWriter,
    Durability, EncryptionMode, IdentityItem, IdentityKind, LogicalDumpHeader, LogicalValue,
    PortableItem, RepositoryEngine, RepositoryItem, RepositoryVersion, SectionInventory,
    SectionKind, SettingItem, SettingKey, TableItem,
};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::logical_dump::fixture::{
    assert_no_stage_leftovers, build_small_fixture, build_streaming_fixture, export_args,
    export_now, import_args, import_now, runtime, stage_directory_present, STREAM_TTL_MS,
};

/// 60-second per-operation data budget; the import allowance for the
/// indexed `bulk/large` table adds the design's separate 180 seconds.
const DATA_BUDGET: Duration = Duration::from_secs(60);
const INDEX_BUDGET: Duration = Duration::from_secs(180);
/// Generous but bounded budget for the fixture build itself.
const BUILD_BUDGET: Duration = Duration::from_secs(300);

/// The verifier's header/frames split: 16-byte prefix (8 magic + 2 version
/// + 2 flags + 4 header length), then the header, then its 32-byte digest.
fn frames_start(bytes: &[u8]) -> usize {
    let header_len = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as usize;
    16 + header_len + 32
}

/// Decode the header between the prefix and its digest.
fn decode_artifact_header(bytes: &[u8]) -> LogicalDumpHeader {
    let header_len = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as usize;
    decode_canonical::<LogicalDumpHeader>(&bytes[16..16 + header_len]).unwrap()
}

/// Blank the header's diagnostic fields (snapshot instant *T*, per-repo
/// versions *V_r*, the epoch observation) — the only permitted differences
/// between repeated exports of the same frozen state (design finding 15).
fn strip_diagnostics(header: LogicalDumpHeader) -> LogicalDumpHeader {
    LogicalDumpHeader {
        snapshot_instant: 0,
        repository_versions: Vec::new(),
        source_storage_epoch: None,
        ..header
    }
}

/// (a) Canonical export determinism: two exports of the same frozen state
/// are byte-identical after the header, compared through the verifier's
/// header/frames split; the headers agree modulo the diagnostic fields.
#[test]
fn repeated_exports_are_byte_identical_after_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_small_fixture(&root));
    assert_eq!(fixture.tables.len(), 3);
    assert!(fixture.owner_principal > 0);
    drop(fixture.home);

    let first_path = root.join("first.mpack");
    let second_path = root.join("second.mpack");
    export_now(&root, &export_args("app", &first_path, false));
    export_now(&root, &export_args("app", &second_path, false));

    let first = fs::read(&first_path).unwrap();
    let second = fs::read(&second_path).unwrap();
    let split = frames_start(&first);
    assert_eq!(split, frames_start(&second), "headers same size");
    assert_eq!(
        first[split..],
        second[split..],
        "frames after the header must be byte-identical"
    );
    assert_eq!(
        strip_diagnostics(decode_artifact_header(&first)),
        strip_diagnostics(decode_artifact_header(&second)),
        "headers agree modulo snapshot instant, repository versions, epoch"
    );
}

/// Handcrafted but fully verifiable artifact whose header carries an OLD
/// storage-epoch observation (the offline open records the stamp check; an
/// older epoch is a diagnostic, not a refusal): identity, database,
/// repository, table, one setting and one record.
fn handcrafted_older_epoch_bytes(epoch: u32) -> Vec<u8> {
    let sections = SectionKind::ORDERED
        .into_iter()
        .map(|section| SectionInventory {
            section,
            expected_items: if section == SectionKind::End {
                Some(0)
            } else {
                None
            },
        })
        .collect();
    let header = LogicalDumpHeader {
        format_version: 1,
        required_features: Vec::new(),
        source_storage_epoch: Some(epoch),
        application_schema_version: None,
        snapshot_instant: 1_700_000_000,
        repository_versions: vec![RepositoryVersion {
            repository: "main".into(),
            version: 1,
        }],
        scope: DumpScope {
            repository: None,
            table: None,
            schema_only: false,
            include_artifacts: false,
        },
        encryption: EncryptionMode::None,
        sections,
    };
    let items: Vec<(SectionKind, Vec<PortableItem>)> = vec![
        (
            SectionKind::Identities,
            vec![PortableItem::Identity(IdentityItem {
                logical_name: "alice".into(),
                kind: IdentityKind::User,
                disabled_on_import: true,
            })],
        ),
        (
            SectionKind::Database,
            vec![PortableItem::Database(DatabaseItem {
                name: "app".into(),
                owner: None,
                group: None,
                mode: 0o644,
                settings: vec![SettingItem {
                    key: SettingKey::RootMeta,
                    value: LogicalValue::Map(vec![
                        ("owner".into(), LogicalValue::Null),
                        ("group".into(), LogicalValue::Null),
                        ("mode".into(), LogicalValue::Int(0o700)),
                    ]),
                }],
            })],
        ),
        (
            SectionKind::Repositories,
            vec![PortableItem::Repository(RepositoryItem {
                name: "main".into(),
                engine: RepositoryEngine::Fjall,
                durability: Durability::Default,
                owner: None,
                group: None,
                mode: 0o600,
            })],
        ),
        (
            SectionKind::Tables,
            vec![PortableItem::Table(TableItem {
                repository: "main".into(),
                name: "rows".into(),
                enable_indexes: false,
                schema_version: 0,
                schema: LogicalValue::Null,
                schema_validator: None,
                buffer_ttl_ms: None,
                owner: None,
                group: None,
                mode: 0o600,
            })],
        ),
        (SectionKind::PortableSchemas, Vec::new()),
        (SectionKind::ValidatorsAndFunctions, Vec::new()),
        (SectionKind::GuestWasmModules, Vec::new()),
        (SectionKind::Bindings, Vec::new()),
        (SectionKind::IndexDescriptors, Vec::new()),
        (SectionKind::Acls, Vec::new()),
        (SectionKind::Records, Vec::new()),
        (SectionKind::End, Vec::new()),
    ];
    let mut bytes = Vec::new();
    let mut writer = DumpWriter::new(&mut bytes, header).expect("preamble writes");
    for (section, items) in items {
        writer.start_section(section).expect("section starts");
        for item in &items {
            writer.write_item(item).expect("item writes");
        }
        writer.end_section().expect("section ends");
    }
    writer.finish().expect("trailer writes");
    bytes
}

/// (b) Verify-only passes on a real produced artifact AND on a handcrafted
/// artifact whose header carries an older epoch observation; the
/// older-epoch artifact then imports successfully into a fresh target.
#[test]
fn verify_only_accepts_produced_and_older_epoch_artifacts_and_the_latter_imports() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Produced artifact of the small fixture verifies standalone.
    let fixture = runtime().block_on(build_small_fixture(&root));
    assert!(fixture.owner_principal > 0);
    drop(fixture.home);
    let produced = root.join("dump.mpack");
    export_now(&root, &export_args("app", &produced, false));
    let report = verify_dump_file(&produced).unwrap();
    assert!(report.items > 0);
    assert_eq!(report.sections, 12);

    // Handcrafted older-epoch artifact verifies and imports.
    let older = root.join("older.mpack");
    fs::write(&older, handcrafted_older_epoch_bytes(1)).unwrap();
    let older_report = verify_dump_file(&older).unwrap();
    assert!(older_report.items > 0);
    let target = root.join("older-target");
    let imported = import_now(&import_args(&older, &target), CancellationToken::new()).unwrap();
    assert!(imported.published);
    assert!(target.join("shamir_db_meta.redb").exists());
}

/// (c) Exact `ttl_ms` round trip on the streaming fixture: the exported
/// TableItem carries `bulk/large`'s buffer TTL unchanged, and a fresh open
/// of the imported target (no lingering engine memory) reports the same
/// non-default value through the buffer-config facade.
#[test]
fn ttl_ms_round_trips_exactly_through_the_streaming_dataset() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rt = runtime();
    let _enter = rt.enter();
    let fixture = rt
        .block_on(timeout(BUILD_BUDGET, build_streaming_fixture(&root)))
        .unwrap();
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));

    // Artifact side: the table definition carries the exact TTL.
    let mut reader = DumpReader::new(fs::File::open(&artifact).unwrap()).unwrap();
    let mut artifact_ttl = None;
    while let Some(item) = reader.next_item().unwrap() {
        if let PortableItem::Table(table) = item {
            if (table.repository.as_str(), table.name.as_str()) == ("bulk", "large") {
                artifact_ttl = table.buffer_ttl_ms;
            }
        }
    }
    assert_eq!(artifact_ttl, Some(STREAM_TTL_MS));

    // Target side: fresh open of the published directory.
    let target = root.join("target");
    rt.block_on(timeout(
        DATA_BUDGET + INDEX_BUDGET,
        logical_import::import(import_args(&artifact, &target), CancellationToken::new()),
    ))
    .unwrap()
    .unwrap();
    let imported = rt
        .block_on(timeout(
            DATA_BUDGET,
            ShamirDb::init(SystemStoreConfig::Fjall(target.join("shamir_db_meta.redb"))),
        ))
        .unwrap()
        .unwrap();
    rt.block_on(imported.check_seed_catalogue()).unwrap();
    let instance = imported.get_db("app").unwrap();
    let view = rt
        .block_on(timeout(DATA_BUDGET, instance.get_table("bulk", "large")))
        .unwrap()
        .unwrap();
    let ttl = rt
        .block_on(timeout(DATA_BUDGET, view.get_buffer_config()))
        .unwrap()
        .unwrap()
        .and_then(|config| config.ttl_ms);
    assert_eq!(ttl, Some(STREAM_TTL_MS));
    drop(imported);
}

/// (d) Complete-target exact retry on the 64 MiB dataset is accepted
/// idempotently, while a retry with a changed effective option
/// (`--schema-only`) is refused by the receipt and leaves the published
/// target intact. Extends — does not repeat — the small-input proofs in
/// `logical_import/roundtrip.rs` by exercising receipt verification on a
/// full-size published target.
#[test]
fn streaming_target_accepts_exact_retry_and_refuses_changed_options() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rt = runtime();
    let _enter = rt.enter();
    let fixture = rt
        .block_on(timeout(BUILD_BUDGET, build_streaming_fixture(&root)))
        .unwrap();
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    let first: ImportReport = rt
        .block_on(timeout(
            DATA_BUDGET + INDEX_BUDGET,
            logical_import::import(import_args(&artifact, &target), CancellationToken::new()),
        ))
        .unwrap()
        .unwrap();
    assert!(first.published);

    let mut changed = import_args(&artifact, &target);
    changed.schema_only = true;
    let error = import_now(&changed, CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Receipt(_)));
    assert!(target.join("shamir_db_meta.redb").exists(), "target intact");
    assert!(
        !stage_directory_present(&root),
        "a refused retry leaves no stage"
    );

    let retry = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    assert!(!retry.published);
    assert_eq!(retry.items, first.items);
}

/// Names of every entry directly under `root` (deterministic before/after
/// snapshot; no sleeps — the handshake is filesystem state).
fn snapshot_root(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// (e) A cancelled mid-load import on the 64 MiB dataset publishes no
/// destination and leaves no stage: the load future is aborted through the
/// deterministic handshake of the stage directory appearing (cancellation
/// is checked between frames), after which the root's directory snapshot is
/// byte-for-byte the before snapshot and `assert_no_stage_leftovers` holds.
#[test]
fn cancelled_mid_load_on_streaming_dataset_publishes_nothing_and_leaves_no_stage() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rt = runtime();
    let _enter = rt.enter();
    let fixture = rt
        .block_on(timeout(BUILD_BUDGET, build_streaming_fixture(&root)))
        .unwrap();
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let before = snapshot_root(&root);
    let target = root.join("target");

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let args = import_args(&artifact, &target);
    let handle = tokio::spawn(async move { logical_import::import(args, token).await });
    // Deterministic handshake: wait until the attempt-owned stage directory
    // exists (preflight is over, the load has begun), or the import exits
    // early (a failure we must not hide), bounded so a regression fails
    // instead of hanging.
    rt.block_on(timeout(DATA_BUDGET, async {
        loop {
            if stage_directory_present(&root) {
                break;
            }
            assert!(
                !handle.is_finished(),
                "import finished before its stage appeared"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }))
    .unwrap();
    cancel.cancel();
    let outcome = rt
        .block_on(timeout(DATA_BUDGET + INDEX_BUDGET, handle))
        .unwrap()
        .unwrap();
    assert!(
        matches!(outcome, Err(LogicalImportError::Cancelled)),
        "expected the typed Cancelled refusal, got {outcome:?}"
    );

    assert_no_stage_leftovers(&root, &target);
    assert_eq!(snapshot_root(&root), before, "root untouched by the abort");
}
