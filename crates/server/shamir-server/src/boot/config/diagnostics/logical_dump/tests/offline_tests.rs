use std::fs::{self, File};
use std::path::Path;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::{doc, write};
use shamir_sync::replication_seed::{verify_dump_file, DumpReader, PortableItem};
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;

use super::super::{export, LogicalExportArgs};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

fn args(output: &Path) -> LogicalExportArgs {
    LogicalExportArgs {
        db: "app".into(),
        output: output.into(),
        repo: Some("main".into()),
        table: Some("rows".into()),
        schema_only: false,
        include_artifacts: false,
    }
}

async fn fixture(root: &Path, ambiguous: bool) {
    let home = ShamirDb::init(SystemStoreConfig::Fjall(root.join("shamir_db_meta.redb")))
        .await
        .unwrap();
    home.create_db("app").await.unwrap();
    home.system_store()
        .save_setting(
            "root_meta",
            &doc! { "owner" => 0_i64, "mode" => 0o700_i64 }.into(),
        )
        .await
        .unwrap();
    home.system_store()
        .save_setting(
            "fn_namespace_meta",
            &doc! { "owner" => 0_i64, "mode" => 0o750_i64 }.into(),
        )
        .await
        .unwrap();
    home.add_repo(
        "app",
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("data"))),
    )
    .await
    .unwrap();
    home.add_table("app", "main", "rows", false).await.unwrap();
    let mut batch = Batch::new();
    if ambiguous {
        batch.op(
            "put",
            write::Insert::with_repo("main", "rows").row(doc! { "value" => "ambiguous" }),
        );
    } else {
        batch.op(
            "put",
            write::Insert::with_repo("main", "rows")
                .row(doc! { "value" => 42_i64, "live" => true }),
        );
    }
    home.execute("app", &batch.build()).await.unwrap();
    let mut deleted = Batch::new();
    deleted.op(
        "put_deleted",
        write::Insert::with_repo("main", "rows").row(doc! { "value" => 99_i64 }),
    );
    home.execute("app", &deleted.build()).await.unwrap();
    let mut deletion = Batch::new();
    deletion.op(
        "delete",
        write::Delete::with_repo("main", "rows")
            .where_(filter::eq("value", 99_i64))
            .build()
            .unwrap(),
    );
    home.execute("app", &deletion.build()).await.unwrap();
    home.flush_all_waiting().await.unwrap();
}

fn no_stage(root: &Path) {
    assert!(!fs::read_dir(root).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".logical-export-")));
}

#[test]
fn offline_writes_real_verified_container_and_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    runtime().block_on(fixture(dir.path(), false));
    let output = dir.path().join("dump.mpack");
    let options = args(&output);
    let report = runtime()
        .block_on(export(dir.path(), &options, CancellationToken::new()))
        .unwrap();
    assert_eq!(report.sections, 12);
    assert_eq!(report.header.scope.repository.as_deref(), Some("main"));
    assert_eq!(verify_dump_file(&output).unwrap(), report);
    let mut reader = DumpReader::new(File::open(&output).unwrap()).unwrap();
    let mut records = Vec::new();
    let mut setting_acls = Vec::new();
    while let Some(item) = reader.next_item().unwrap() {
        match item {
            PortableItem::Record(row) => records.push(row),
            PortableItem::Acl(acl) if acl.resource.object.is_some() => {
                setting_acls.push(acl.resource.object.unwrap())
            }
            _ => {}
        }
    }
    assert_eq!(setting_acls, ["fn_namespace_meta", "root_meta"]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].repository, "main");
    assert_eq!(records[0].table, "rows");
    assert!(!records[0].raw_key.is_empty());
    drop(reader);
    let before = fs::read(&output).unwrap();
    assert!(runtime()
        .block_on(export(dir.path(), &options, CancellationToken::new()))
        .is_err());
    assert_eq!(fs::read(&output).unwrap(), before);
    no_stage(dir.path());
    let mut corrupt = before;
    let index = corrupt.len() / 2;
    corrupt[index] ^= 1;
    let bad = dir.path().join("corrupt");
    fs::write(&bad, corrupt).unwrap();
    assert!(verify_dump_file(bad).is_err());
}

#[test]
fn locked_directory_is_refused_before_catalogue_open() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("dump");
    let lock = crate::maintenance_lock::acquire(dir.path()).unwrap();
    let error = runtime()
        .block_on(export(dir.path(), &args(&output), CancellationToken::new()))
        .unwrap_err();
    assert!(!error
        .to_string()
        .contains("source catalogue does not exist"));
    assert!(!output.exists());
    no_stage(dir.path());
    drop(lock);
    let _released = crate::maintenance_lock::acquire(dir.path()).unwrap();
}

#[test]
fn ambiguous_values_fail_and_cleanup_then_schema_only_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    runtime().block_on(fixture(dir.path(), true));
    let output = dir.path().join("dump");
    let mut options = args(&output);
    let error = runtime()
        .block_on(export(dir.path(), &options, CancellationToken::new()))
        .unwrap_err();
    assert!(
        error.to_string().contains("Str/Dec/Big fidelity"),
        "{error:#}"
    );
    assert!(!output.exists());
    no_stage(dir.path());
    options.schema_only = true;
    let report = runtime()
        .block_on(export(dir.path(), &options, CancellationToken::new()))
        .unwrap();
    assert!(report.header.scope.schema_only);
    no_stage(dir.path());
}

#[test]
fn cancelled_attempt_and_unsupported_flags_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("dump");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(runtime()
        .block_on(export(dir.path(), &args(&output), cancel))
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    let mut options = args(&output);
    options.include_artifacts = true;
    assert!(runtime()
        .block_on(export(dir.path(), &options, CancellationToken::new()))
        .unwrap_err()
        .to_string()
        .contains("include-artifacts"));
    assert!(!output.exists());
    no_stage(dir.path());
}
