use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

use super::super::support::{create_echo, stored_row};
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::DbError;

fn artifact_id(row: &shamir_types::types::value::QueryValue) -> &str {
    row.get("artifact_id")
        .and_then(|value| value.as_str())
        .expect("row artifact id")
}

fn without_name(
    row: &shamir_types::types::value::QueryValue,
) -> shamir_types::types::value::QueryValue {
    let mut copy = row.clone();
    if let shamir_types::types::value::QueryValue::Map(map) = &mut copy {
        map.shift_remove("name");
    }
    copy
}

#[tokio::test]
async fn rename_source_remove_failure_cleans_destination_and_retry_succeeds() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "rename_source").await;
    let original_row = stored_row(&db, "rename_source").await;
    let original_binding = db
        .functions()
        .capture("rename_source")
        .expect("source binding");
    let original_id = artifact_id(&original_row).to_owned();
    assert_eq!(original_binding.artifact_id(), Some(original_id.as_str()));

    // The destination save passes; removing the source is the second gated
    // catalogue write and fails. Destination cleanup is then ungated.
    db.system_store().arm_catalogue_write_fault(2);
    let error = db
        .rename_function("rename_source", "rename_target")
        .await
        .expect_err("source catalogue removal fault must fail rename");
    assert!(
        matches!(
            error,
            DbError::Internal(ref message) if message == "injected catalogue write failure"
        ),
        "must return the original source-removal error, got: {error}"
    );

    let catalogue = db
        .system_store()
        .load_functions()
        .await
        .expect("load catalogue after failure");
    assert_eq!(catalogue.len(), 1, "compensation leaves one catalogue row");
    assert_eq!(catalogue[0], original_row, "source row is unchanged");
    assert_eq!(artifact_id(&catalogue[0]), original_id.as_str());
    assert!(db
        .system_store()
        .load_function("rename_target")
        .await
        .expect("load destination")
        .is_none());
    assert!(db
        .functions()
        .capture("rename_source")
        .expect("original source binding remains")
        .same_as(&original_binding));
    assert!(db.functions().capture("rename_target").is_none());

    db.system_store().arm_catalogue_write_fault(0);
    db.rename_function("rename_source", "rename_target")
        .await
        .expect("retry succeeds");

    let catalogue = db
        .system_store()
        .load_functions()
        .await
        .expect("load catalogue after retry");
    assert_eq!(catalogue.len(), 1);
    assert_eq!(
        catalogue[0].get("name").and_then(|value| value.as_str()),
        Some("rename_target")
    );
    assert_eq!(artifact_id(&catalogue[0]), original_id.as_str());
    assert_eq!(without_name(&catalogue[0]), without_name(&original_row));
    assert!(db.functions().capture("rename_source").is_none());
    assert!(db
        .functions()
        .capture("rename_target")
        .expect("renamed binding")
        .same_as(&original_binding));
}

#[tokio::test]
async fn registry_rename_failure_restores_source_and_preserves_host_target() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "rename_source").await;
    let original_row = stored_row(&db, "rename_source").await;
    let original_id = artifact_id(&original_row).to_owned();
    let original_binding = db
        .functions()
        .capture("rename_source")
        .expect("source binding");
    assert_eq!(original_binding.artifact_id(), Some(original_id.as_str()));
    let original_meta = db.function_meta("rename_source").expect("source metadata");

    // Inject a live-only registry collision after both catalogue writes,
    // forcing FunctionRegistry::rename to fail after the destination row was
    // saved and the source row removed. Retain the exact injected binding to
    // verify compensation does not overwrite the trusted host registration.
    let registry = Arc::clone(db.functions());
    let source_artifact = registry.get("rename_source").expect("source artifact");
    let injected_binding = Arc::new(OnceLock::new());
    let hook_binding = Arc::clone(&injected_binding);
    let hook_fired = Arc::new(AtomicBool::new(false));
    let hook_once = Arc::clone(&hook_fired);
    assert!(db
        .function_ddl_hook
        .set(Box::new(move |point| {
            let registry = Arc::clone(&registry);
            let source_artifact = Arc::clone(&source_artifact);
            let hook_binding = Arc::clone(&hook_binding);
            let hook_once = Arc::clone(&hook_once);
            Box::pin(async move {
                if point == FunctionDdlHookPoint::RenameAfterCatalogueWritesBeforeRegistryRename
                    && !hook_once.swap(true, Ordering::SeqCst)
                {
                    registry
                        .register("rename_target", source_artifact)
                        .expect("install live-only target collision");
                    assert!(hook_binding
                        .set(
                            registry
                                .capture("rename_target")
                                .expect("capture injected target")
                        )
                        .is_ok());
                }
            })
        }))
        .is_ok());

    let error = db
        .rename_function("rename_source", "rename_target")
        .await
        .expect_err("post-write registry collision must fail rename");
    assert!(
        matches!(
            error,
            DbError::Function(ref message)
                if message == "function already registered: rename_target"
        ),
        "must return the original registry error, got: {error}"
    );

    let catalogue = db
        .system_store()
        .load_functions()
        .await
        .expect("load catalogue after compensation");
    assert_eq!(catalogue.len(), 1, "only the restored source row remains");
    assert_eq!(catalogue[0], original_row, "source row is restored exactly");
    assert_eq!(artifact_id(&catalogue[0]), original_id.as_str());
    assert!(db
        .system_store()
        .load_function("rename_target")
        .await
        .expect("load destination")
        .is_none());
    assert!(db
        .functions()
        .capture("rename_source")
        .expect("source binding remains")
        .same_as(&original_binding));
    let target_binding = db
        .functions()
        .capture("rename_target")
        .expect("host-injected target is retained");
    assert!(target_binding.same_as(
        injected_binding
            .get()
            .expect("hook captured injected binding")
    ));
    assert_eq!(target_binding.artifact_id(), None);
    let restored_meta = db
        .function_meta
        .get("rename_source")
        .expect("source metadata remains");
    assert_eq!(restored_meta.visibility, original_meta.visibility);
    assert_eq!(restored_meta.security, original_meta.security);
    assert_eq!(restored_meta.secret_grants, original_meta.secret_grants);
    assert_eq!(restored_meta.net_grants, original_meta.net_grants);
    assert!(db.function_meta.get("rename_target").is_none());
    drop(restored_meta);

    assert!(db.functions().remove("rename_target"));
    db.rename_function("rename_source", "rename_target")
        .await
        .expect("retry succeeds after removing host collision");

    let catalogue = db
        .system_store()
        .load_functions()
        .await
        .expect("load catalogue after retry");
    assert_eq!(catalogue.len(), 1);
    assert_eq!(
        catalogue[0].get("name").and_then(|value| value.as_str()),
        Some("rename_target")
    );
    assert_eq!(artifact_id(&catalogue[0]), original_id.as_str());
    assert_eq!(without_name(&catalogue[0]), without_name(&original_row));
    assert!(db.functions().capture("rename_source").is_none());
    assert!(db
        .functions()
        .capture("rename_target")
        .expect("renamed binding")
        .same_as(&original_binding));
    let moved_meta = db
        .function_meta
        .get("rename_target")
        .expect("metadata moves on successful retry");
    assert_eq!(moved_meta.visibility, original_meta.visibility);
    assert_eq!(moved_meta.security, original_meta.security);
    assert_eq!(moved_meta.secret_grants, original_meta.secret_grants);
    assert_eq!(moved_meta.net_grants, original_meta.net_grants);
    assert!(db.function_meta.get("rename_source").is_none());
}
