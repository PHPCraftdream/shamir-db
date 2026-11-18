//! Scope options: `--schema-only` carries definitions but no records, and
//! `--as-db` renames the imported database consistently across the
//! catalogue, ownership rows and the imported identities' database scope.

use shamir_connect::server::admin::UserDirectory;
use shamir_server::logical_import::LogicalImportError;
use shamir_server_security::user_directory::FjallUserDirectory;
use tokio_util::sync::CancellationToken;

use crate::logical_import::common::build_fixture;
use crate::logical_import::common::export_args;
use crate::logical_import::common::export_now;
use crate::logical_import::common::import_args;
use crate::logical_import::common::import_now;
use crate::logical_import::common::runtime;

/// Schema-only export imports definitions only: the database, repositories
/// and tables exist in the target and no record is present; the plain
/// (records) import of a schema-only artifact is refused by the scope check.
#[test]
fn schema_only_imports_definitions_without_records() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("schema.mpack");
    export_now(&root, &export_args("app", &artifact, true));
    let target = root.join("target");
    let mut args = import_args(&artifact, &target);
    args.schema_only = true;
    let report = import_now(&args, CancellationToken::new()).unwrap();
    assert_eq!(report.database, "app");
    assert!(report.published);

    // Definitions present, records absent.
    let home = runtime()
        .block_on(shamir_db::ShamirDb::init(
            shamir_db::SystemStoreConfig::Fjall(target.join("shamir_db_meta.redb")),
        ))
        .unwrap();
    assert!(home.get_db("app").is_some());
    let instance = home.get_db("app").unwrap();
    assert!(instance.get_repo("main").is_some());
    let tables = runtime().block_on(async {
        let mut seen = Vec::new();
        for repo in instance.list_repos() {
            for table in instance.get_repo(&repo).unwrap().list_table_names() {
                seen.push(format!("{repo}/{table}"));
            }
        }
        seen
    });
    assert_eq!(tables.len(), 3);
    // Drop the engine handle before reopening the published directory — a
    // second ShamirDb over the same Fjall keyspace fails on the file lock.
    drop(home);
    drop(instance);
    let snapshot = runtime().block_on(crate::logical_import::common::engine_snapshot(
        &target,
        "app",
        &[("main", "rows"), ("main", "users"), ("cold", "events")],
    ));
    assert!(snapshot.records.values().all(|keys| keys.is_empty()));

    // The artifact is schema-only: importing it WITHOUT --schema-only is a
    // typed preflight refusal and leaves nothing behind.
    let mismatch_target = root.join("mismatch");
    let error = import_now(
        &import_args(&artifact, &mismatch_target),
        CancellationToken::new(),
    )
    .unwrap_err();
    assert!(matches!(error, LogicalImportError::Preflight(_)));
    assert!(error.to_string().contains("schema-only"));
    crate::logical_import::common::assert_no_leftovers(&root, &mismatch_target);
}

/// `--as-db staging` renames the database in the catalogue and scopes the
/// minted identities to "staging"; the source name "prod" does not exist.
#[test]
fn as_db_renames_catalogue_and_identity_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("prod.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    let mut args = import_args(&artifact, &target);
    args.as_db = Some("staging".into());
    let report = import_now(&args, CancellationToken::new()).unwrap();
    assert_eq!(report.database, "staging");

    let home = runtime()
        .block_on(shamir_db::ShamirDb::init(
            shamir_db::SystemStoreConfig::Fjall(target.join("shamir_db_meta.redb")),
        ))
        .unwrap();
    assert!(home.get_db("staging").is_some());
    assert!(home.get_db("app").is_none());
    // Drop the engine handle before reopening the published directory — a
    // second ShamirDb over the same Fjall keyspace fails on the file lock.
    drop(home);

    // Every minted identity is disabled, carries the staging scope, and the
    // catalogue ownership rows resolve to it.
    let directory = FjallUserDirectory::open(target.join("users")).unwrap();
    let listed = directory.list_all().unwrap();
    assert!(!listed.is_empty());
    for (_, state) in &listed {
        assert_eq!(state.database.as_deref(), Some("staging"));
        assert!(!state.superuser);
        assert!(!state.replicator);
    }
    let minted = directory
        .user_id("owner")
        .expect("owner minted under staging");
    let snapshot = runtime().block_on(crate::logical_import::common::engine_snapshot(
        &target,
        "staging",
        &[("main", "rows"), ("main", "users"), ("cold", "events")],
    ));
    assert_eq!(
        snapshot.database_owner,
        Some(shamir_db::access::principal64(minted))
    );
    assert_eq!(snapshot.records["main/rows"].len(), 2);
}
