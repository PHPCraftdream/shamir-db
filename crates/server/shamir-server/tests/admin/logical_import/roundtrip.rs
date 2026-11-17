//! Round trip: export a real multi-repository database offline, import it
//! into a fresh target, and re-read the published directory through the
//! engine read facade — keys, values, settings, TTL, indexes and ownership.

use shamir_connect::server::admin::UserDirectory;
use shamir_db::access::principal64;
use shamir_db::ShamirDb;
use shamir_db::SystemStoreConfig;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_server::logical_import::LogicalImportError;
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::LogicalValue;
use tokio_util::sync::CancellationToken;

use crate::logical_import::common::artifact_tables;
use crate::logical_import::common::build_fixture;
use crate::logical_import::common::engine_snapshot;
use crate::logical_import::common::export_args;
use crate::logical_import::common::export_now;
use crate::logical_import::common::import_args;
use crate::logical_import::common::import_now;
use crate::logical_import::common::runtime;

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
    // target's user directory and the imported principal64 id is what the
    // catalogue rows now carry.
    let directory = FjallUserDirectory::open(target.join("users")).unwrap();
    let minted = directory.user_id("owner").expect("owner identity minted");
    let minted_principal = principal64(minted);
    assert_ne!(minted_principal, fixture.owner_principal);
    assert_eq!(snapshot.database_owner, Some(minted_principal));
    // root_meta stays System-owned here (source owner 0), no silent grants.
    assert_eq!(snapshot.root_meta_owner, None);
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
    let reopened = runtime()
        .block_on(ShamirDb::init(SystemStoreConfig::Fjall(
            root.join("shamir_db_meta.redb"),
        )))
        .unwrap();
    let mut batch = Batch::new();
    batch.op(
        "put",
        write::Insert::with_repo("cold", "events").row(doc! { "kind" => "extra", "n" => 8_i64 }),
    );
    runtime().block_on(async {
        reopened.execute("app", &batch.build()).await.unwrap();
        reopened.flush_all_waiting().await.unwrap();
    });
    drop(reopened);
    let other = root.join("dump2.mpack");
    export_now(&root, &export_args("app", &other, false));
    let error = import_now(&import_args(&other, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Receipt(_)));
    // The published target is untouched and still idempotent for the original.
    assert!(import_now(&import_args(&artifact, &target), CancellationToken::new()).is_ok());
    assert_ne!(artifact, other);
}
