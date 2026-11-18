//! Round trip: export a real multi-repository database offline, import it
//! into a fresh target, and re-read the published directory through the
//! engine read facade — keys, values, settings, TTL, indexes and ownership.

use shamir_connect::server::admin::UserDirectory;
use shamir_db::access::principal64;
use shamir_db::access::Actor;
use shamir_db::engine::repo::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::ShamirDb;
use shamir_db::SystemStoreConfig;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::rename_table;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_server::logical_dump;
use shamir_server::logical_import;
use shamir_server::logical_import::LogicalImportError;
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::LogicalValue;
use shamir_sync::replication_seed::PortableItem;
use shamir_sync::replication_seed::RecordItem;
use tokio_util::sync::CancellationToken;

use crate::logical_import::common::artifact_items;
use crate::logical_import::common::artifact_tables;
use crate::logical_import::common::build_fixture;
use crate::logical_import::common::engine_snapshot;
use crate::logical_import::common::export_args;
use crate::logical_import::common::export_now;
use crate::logical_import::common::export_waiting_for_locks;
use crate::logical_import::common::import_args;
use crate::logical_import::common::import_now;
use crate::logical_import::common::runtime;
use crate::logical_import::common::stage_timeout;

/// Resolve a catalogue principal64 owner id to its user NAME in the given
/// directory. Ownership is portable by name only: the source and the target
/// mint independent principal64 ids for the same logical user.
fn owner_name(directory: &FjallUserDirectory, principal: Option<u64>) -> Option<String> {
    let principal = principal?;
    directory
        .list_all()
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == principal)
        .map(|(_, state)| state.username)
}

/// Full round trip: every table's key set and value set survive the
/// export/import cycle exactly (stored strings stay Str, lists stay List,
/// nested maps, binary, ints, floats, null and bools), `enable_indexes` and
/// the buffer TTL are carried, the imported owner's principal64 id is what
/// the catalogue rows carry, and the report describes a real publication.
#[test]
fn export_import_round_trip_preserves_records_definitions_and_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let plain_tables: Vec<(&str, &str)> = fixture
        .tables
        .iter()
        .map(|(repo, table, _, _)| (*repo, *table))
        .collect();
    let target = root.join("target");
    let report = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    assert_eq!(report.database, "app");
    assert!(report.published);
    assert!(report.bytes > 0);
    assert!(report.items > 0);

    let expected = artifact_tables(&artifact);
    assert_eq!(expected["main/rows"].len(), 2);
    let snapshot = runtime().block_on(engine_snapshot(&root, "app", &plain_tables));
    assert_eq!(snapshot.records, expected);
    assert_eq!(snapshot.table_meta["main/users"], (true, Some(5_000)));
    assert_eq!(snapshot.table_meta["main/rows"], (false, None));
    assert_eq!(snapshot.table_meta["cold/events"], (false, None));
    // Ownership: the source catalogue owner (a named user) is minted in the
    // target's user directory under a NEW principal64 id (fresh minting by
    // design — the artifact carries owner names, never ids), so ownership is
    // compared by NAME across the two directories, while the ids themselves
    // must differ.
    let source_directory = FjallUserDirectory::open(root.join("users")).unwrap();
    let target_directory = FjallUserDirectory::open(target.join("users")).unwrap();
    let source_owner = owner_name(&source_directory, snapshot.database_owner)
        .expect("source catalogue owner resolves to a user name");
    let target_snapshot = runtime().block_on(engine_snapshot(&target, "app", &plain_tables));
    assert_eq!(target_snapshot.records, expected);
    let target_owner = owner_name(&target_directory, target_snapshot.database_owner)
        .expect("target catalogue owner resolves to a user name");
    assert_eq!(source_owner, target_owner);
    let minted = target_directory
        .user_id("owner")
        .expect("owner identity minted");
    let minted_principal = principal64(minted);
    assert_ne!(minted_principal, fixture.owner_principal);
    assert_eq!(target_snapshot.database_owner, Some(minted_principal));
    // root_meta stays System-owned here (source owner 0), no silent grants.
    assert_eq!(snapshot.root_meta_owner, None);
    assert_eq!(target_snapshot.root_meta_owner, None);
    // The stored string and list shapes round-trip: the exported `str`
    // reads back as Str and the exported list as List through the engine.
    let rows = &snapshot.records["main/rows"];
    let rich = rows
        .values()
        .find(|value| match value {
            LogicalValue::Map(fields) => fields.iter().any(|(key, _)| key == "l"),
            _ => false,
        })
        .expect("rich row present");
    match rich {
        LogicalValue::Map(fields) => {
            assert!(
                fields.iter().any(|(key, value)| key == "s"
                    && matches!(value, LogicalValue::Str(text) if text == "ambiguous")),
                "stored string stayed a string"
            );
            assert!(
                fields.iter().any(|(key, value)| key == "l"
                    && matches!(value, LogicalValue::List(items) if items.len() == 2)),
                "stored list stayed a list"
            );
        }
        _ => unreachable!("checked above"),
    }
    assert!(target.join("shamir_db_meta.redb").exists());
    assert!(target.join("shamir_db_app_main.redb").exists());
    assert!(artifact.exists(), "source artifact untouched");
}

/// Re-import of an already-published target with the same artifact is
/// accepted idempotently and reports `published == false`.
#[test]
fn idempotent_retry_with_matching_artifact_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    let first = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    assert!(first.published);
    let second = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    assert_eq!(second.database, "app");
    assert!(!second.published);
    assert_eq!(second.items, first.items);
}

/// A retry whose artifact digest differs from the receipt is refused with a
/// typed receipt error; the published target stays intact and still
/// accepts the original artifact idempotently.
#[test]
fn retry_with_a_different_artifact_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    // Append a record to the source and export a DIFFERENT artifact.
    // ONE runtime drives the whole tail of the test: the reopened engine is
    // opened, flushed and dropped on it, and only then is the export run on
    // the same runtime — no competing runtime may still hold (or be lazily
    // closing) the source's Fjall locks when the export opens the directory.
    let rt = runtime();
    let reopened = rt
        .block_on(ShamirDb::init(SystemStoreConfig::Fjall(
            root.join("shamir_db_meta.redb"),
        )))
        .unwrap();
    let mut batch = Batch::new();
    batch.op(
        "put_events_extra",
        write::Insert::with_repo("cold", "events").row(doc! { "kind" => "extra", "n" => 8_i64 }),
    );
    rt.block_on(async {
        reopened.execute("app", &batch.build()).await.unwrap();
        reopened.flush_all_waiting().await.unwrap();
    });
    drop(reopened);
    let other = root.join("dump2.mpack");
    rt.block_on(logical_dump::export(
        &root,
        &export_args("app", &other, false),
        CancellationToken::new(),
    ))
    .unwrap();
    let error = import_now(&import_args(&other, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Receipt(_)));
    // The published target is untouched and still idempotent for the original.
    assert!(import_now(&import_args(&artifact, &target), CancellationToken::new()).is_ok());
    assert_ne!(artifact, other);
}

/// Durability: records written through the buffer-TTL table must be ON DISK
/// in the published target — a fresh open (no lingering engine memory) sees
/// every buffered record exactly as the artifact carries it. Guards the
/// driver's flush-before-publish invariant.
#[test]
fn published_target_fresh_open_sees_buffered_table_records() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap();
    let expected = artifact_tables(&artifact);
    // Fresh open of the published directory only — the staged engine and
    // all its buffers are long gone.
    let snapshot = runtime().block_on(engine_snapshot(
        &target,
        "app",
        &[("main", "rows"), ("main", "users"), ("cold", "events")],
    ));
    assert_eq!(snapshot.records, expected);
    assert!(!snapshot.records["main/users"].is_empty());
    assert_eq!(snapshot.table_meta["main/users"], (true, Some(5_000)));
}

/// A SUCCESSFULLY renamed plain populated persistent table (the rename fence
/// is released: not in doubt) exports under its CURRENT name with its rows
/// and metadata; the physical rename provenance is validated as input but
/// never projected into the artifact. Before the closed table-field
/// allowlist admitted the provenance fields, this export was refused
/// outright. The full export → import (empty target) → export cycle then
/// yields the SAME canonical portable content: the full verified item
/// vectors — including every record's physical raw key — are equal, and the
/// two record values are asserted by exact typed content.
#[test]
fn renamed_plain_table_exports_current_name_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rt = runtime();
    // Plain isolated persistent fixture: no user directory, System-owned
    // catalogue rows — nothing but the rename differs between the two
    // exports. ONE runtime drives the whole test; each stage is bounded by
    // `stage_timeout`, whose timer lives inside `block_on` (a timer created
    // outside a runtime has no reactor and panics).
    stage_timeout(&rt, "fixture and rename stage", async {
        let home = ShamirDb::init(SystemStoreConfig::Fjall(root.join("shamir_db_meta.redb")))
            .await
            .unwrap();
        home.create_db_as("app", Actor::System).await.unwrap();
        home.add_repo_as_with_durability(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("data_main"))),
            Actor::System,
            None,
        )
        .await
        .unwrap();
        home.add_table_as("app", "main", "rows", false, Actor::System)
            .await
            .unwrap();
        let mut batch = Batch::new();
        batch.op(
            "put_one",
            write::Insert::with_repo("main", "rows").row(doc! { "s" => "one", "n" => 1_i64 }),
        );
        batch.op(
            "put_two",
            write::Insert::with_repo("main", "rows").row(doc! { "s" => "two", "n" => 2_i64 }),
        );
        let write_response = home
            .execute("app", &batch.build())
            .await
            .unwrap_or_else(|error| panic!("fixture writes refused: {error}"));
        // Both write aliases must be present with real results (a per-op
        // failure would have surfaced as the batch's own error).
        assert!(
            write_response.results.contains_key("put_one")
                && write_response.results.contains_key("put_two"),
            "both fixture writes produced per-op results"
        );
        // The committed rename through the builder: a catalogue rekey that
        // persists the physical provenance and releases the rename fence.
        // A per-op failure surfaces as the batch's own error (its data may
        // sit in the error), and a success must carry the rename receipt —
        // the response IS checked, not assumed.
        let mut rename = Batch::new();
        rename.op("rename_rows", rename_table("rows", "renamed"));
        let rename_response = home
            .execute("app", &rename.build())
            .await
            .unwrap_or_else(|error| panic!("rename_table op refused: {error}"));
        let rename_result = rename_response
            .results
            .get("rename_rows")
            .expect("rename_rows per-op result present");
        assert_eq!(rename_result.records.len(), 1);
        assert_eq!(
            rename_result.records[0].get_value_str("renamed_table"),
            Some("rows")
        );
        assert_eq!(
            rename_result.records[0].get_value_str("to"),
            Some("renamed")
        );
        assert_eq!(rename_result.records[0].get_value_str("repo"), Some("main"));
        // Post-rename invariants, asserted on the still-open engine BEFORE
        // flushing and dropping it — no reopen is needed to know the rename
        // committed.
        let instance = home.get_db("app").unwrap();
        let repo = instance.get_repo("main").expect("main repo present");
        assert!(
            !repo.has_rename_in_doubt(),
            "committed rename is not in doubt"
        );
        repo.get_table("renamed")
            .await
            .expect("renamed table resolves under its current name");
        assert!(
            repo.get_table("rows").await.is_err(),
            "old table name is gone"
        );
        home.flush_all_waiting().await.unwrap();
        // `home` drops here, at the end of the bounded stage.
    });
    // The dropped engine may hold its Fjall directory locks for a while; the
    // export driver's MaintenanceOpen reports exactly that as a `Locked`
    // error, which `export_waiting_for_locks` retries within a budget. No
    // reopen of the source engine happens — the export facade opens its own
    // reader.
    let artifact = root.join("dump.mpack");
    stage_timeout(
        &rt,
        "source export stage timed out",
        export_waiting_for_locks(&root, &export_args("app", &artifact, false)),
    );

    // Oracle 1 — the portable table item carries the current name only:
    // "renamed" present, the old name "rows" absent from every item.
    let source_items = artifact_items(&artifact);
    let table_items: Vec<_> = source_items
        .iter()
        .filter_map(|item| match item {
            PortableItem::Table(table) => Some(table),
            _ => None,
        })
        .collect();
    assert_eq!(table_items.len(), 1, "exactly one table item");
    assert_eq!(table_items[0].repository, "main");
    assert_eq!(table_items[0].name, "renamed");
    assert!(!table_items[0].enable_indexes);
    assert!(
        source_items.iter().all(|item| match item {
            PortableItem::Table(table) => table.name != "rows",
            PortableItem::Record(record) => record.table != "rows",
            _ => true,
        }),
        "no item anywhere still names the pre-rename table"
    );
    // Oracle 2 — both records travel under the current name, with their
    // physical raw keys (portable content, part of the canonical
    // comparison below) and their exact typed values.
    let record_items: Vec<&RecordItem> = source_items
        .iter()
        .filter_map(|item| match item {
            PortableItem::Record(record) => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(record_items.len(), 2, "exactly the fixture's two records");
    assert!(
        record_items
            .iter()
            .all(|record| record.repository == "main" && record.table == "renamed"),
        "both records are attributed to main/renamed"
    );
    assert!(
        record_items.iter().all(|record| !record.raw_key.is_empty()),
        "every record carries its physical raw key"
    );
    assert_ne!(
        record_items[0].raw_key, record_items[1].raw_key,
        "the two records have distinct physical keys"
    );
    assert!(
        record_items.iter().all(|record| record.raw_key.len() == 16),
        "every physical key is exactly a 16-byte RecordId"
    );
    // The export sorts records by physical raw key, which is independent of
    // fixture insertion order — so the two typed values are compared as a
    // SET (length 2, each side contained in the other), while each map's
    // field insertion order ("s" then "n") is still asserted exactly.
    let expected_values: Vec<LogicalValue> = vec![
        LogicalValue::Map(vec![
            ("s".to_owned(), LogicalValue::Str("one".to_owned())),
            ("n".to_owned(), LogicalValue::Int(1)),
        ]),
        LogicalValue::Map(vec![
            ("s".to_owned(), LogicalValue::Str("two".to_owned())),
            ("n".to_owned(), LogicalValue::Int(2)),
        ]),
    ];
    let record_values: Vec<LogicalValue> = record_items
        .iter()
        .map(|record| record.value.clone())
        .collect();
    assert_eq!(record_values.len(), 2, "exactly two record values");
    for expected in &expected_values {
        assert!(
            record_values.contains(expected),
            "expected record value missing: {expected:?}"
        );
    }
    for actual in &record_values {
        assert!(
            expected_values.contains(actual),
            "unexpected record value (or rename provenance fields): {actual:?}"
        );
    }
    assert!(
        source_items
            .iter()
            .all(|item| !format!("{item:?}").contains("rename_from")),
        "physical rename provenance is never emitted"
    );

    // Import into an EMPTY target, then export the target again and compare
    // the FULL canonical portable content: every item — records with their
    // raw keys included — must be equal, not a Debug substring of it. The
    // diagnostic header fields are not items and so are excluded by
    // construction.
    let target = root.join("target");
    let report = stage_timeout(
        &rt,
        "import stage timed out",
        logical_import::import(import_args(&artifact, &target), CancellationToken::new()),
    )
    .unwrap();
    assert_eq!(report.database, "app");
    assert!(report.published);
    let reexport = root.join("reexport.mpack");
    stage_timeout(
        &rt,
        "re-export stage timed out",
        export_waiting_for_locks(&target, &export_args("app", &reexport, false)),
    );
    let target_items = artifact_items(&reexport);
    assert_eq!(
        source_items, target_items,
        "full canonical portable content survives the round trip"
    );
    assert!(
        artifact_tables(&reexport).contains_key("main/renamed"),
        "the re-export still names the table by its current name"
    );
}
