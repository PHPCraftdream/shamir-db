//! F4 root resolution, capability isolation, F3 artifact binding, and F5 fallback tests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::shamir_db::{FunctionSource, SystemStoreConfig};
use shamir_db::ShamirDb;
use shamir_engine::function::{
    FnBatch, FnCtx, FunctionError, FunctionMeta, FunctionRegistry, NetGatewayError, Params,
    Security, ShamirFunction, Visibility,
};
use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::value::QueryValue;

use super::support::{constant_wasm, create_wasm, options, OWNER};
use crate::native_stored_call::support::{
    invoke_as, register_native, user, Forward, NativeRootCall, CALLER,
};

struct Returns(QueryValue);

#[async_trait]
impl ShamirFunction for Returns {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(self.0.clone())
    }
}

struct MetadataProbe;

#[async_trait]
impl ShamirFunction for MetadataProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::List(vec![
            QueryValue::Int(ctx.actor().to_owner_id() as i64),
            QueryValue::Bool(ctx.secret_grants().contains(&"NEW_GRANT".to_string())),
        ]))
    }
}

#[tokio::test]
async fn root_call_denies_managed_function_with_missing_catalogue_row() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "managed_missing", 1, user(OWNER), Security::Invoker).await;
    assert!(db.function_meta("managed_missing").is_some());
    let calls = Arc::new(AtomicUsize::new(0));
    db.functions().replace(
        "managed_missing",
        Arc::new(NativeRootCall {
            calls: calls.clone(),
            recurse: false,
        }),
    );
    db.system_store()
        .remove_function("managed_missing")
        .await
        .unwrap();

    let error = invoke_as(&db, "managed_missing", user(OWNER))
        .await
        .expect_err("a managed function without its row must fail closed");
    assert!(
        error.to_string().contains("catalogue"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn builtin_without_catalogue_or_grants_remains_callable() {
    let mut db = super::support::db_setup().await;
    db.set_net_allowlist(vec!["example.com".into()]);
    let mut params = Params::new();
    params.set("password", QueryValue::Bin(b"pw".to_vec()));
    params.set("salt", QueryValue::Bin(b"0123456789abcdef".to_vec()));
    let result = db
        .invoke_function_as("argon2id", params, user(OWNER))
        .await
        .expect("builtin KDF is allowed without a catalogue row");
    assert!(matches!(result, QueryValue::Bin(bytes) if bytes.len() == 32));

    let calls = Arc::new(AtomicUsize::new(0));
    db.functions()
        .replace("argon2id", Arc::new(NetworkProbe(calls.clone())));
    assert!(db
        .functions()
        .capture("argon2id")
        .unwrap()
        .is_database_builtin());
    assert_eq!(
        invoke_as(&db, "argon2id", user(OWNER)).await.unwrap(),
        QueryValue::List(vec![QueryValue::Bool(true), QueryValue::Bool(false)])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct NetworkProbe(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for NetworkProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let gateway = ctx
            .net_gateway()
            .ok_or_else(|| FunctionError::Compute("network gateway missing".into()))?;
        let network_allowed = match gateway
            .fetch(shamir_engine::function::HttpRequest {
                method: "GET".into(),
                url: "https://example.com/".into(),
                headers: Vec::new(),
                body: Vec::new(),
            })
            .await
        {
            Err(NetGatewayError::NotAllowed(_)) => false,
            Ok(_) => true,
            Err(error) => return Err(FunctionError::Compute(error.to_string())),
        };
        Ok(QueryValue::List(vec![
            QueryValue::Bool(ctx.secret_grants().is_empty()),
            QueryValue::Bool(network_allowed),
        ]))
    }
}

#[tokio::test]
async fn native_definer_root_is_refused_when_its_catalogue_row_is_dropped() {
    let db = super::support::db_setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "dropped_native_definer",
        Arc::new(NativeRootCall {
            calls: calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o700,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    assert_native_definer_drop_window(&db, "dropped_native_definer").await;

    db.system_store()
        .remove_function("dropped_native_definer")
        .await
        .unwrap();
    let error = invoke_as(&db, "dropped_native_definer", user(CALLER))
        .await
        .expect_err("a native Definer without its catalogue row must fail closed");
    assert!(
        error.to_string().contains("catalogue"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_definer_root_is_refused_in_registry_first_rename_window() {
    let db = super::support::db_setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "rename_window_old",
        Arc::new(NativeRootCall {
            calls: calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o700,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    assert_native_definer_drop_window(&db, "rename_window_old").await;

    db.functions()
        .rename("rename_window_old", "rename_window_new")
        .unwrap();
    assert!(db
        .system_store()
        .load_function("rename_window_old")
        .await
        .unwrap()
        .is_some());
    assert!(db
        .system_store()
        .load_function("rename_window_new")
        .await
        .unwrap()
        .is_none());
    let error = invoke_as(&db, "rename_window_new", user(CALLER))
        .await
        .expect_err("the registry-first rename window must fail closed");
    assert!(
        error.to_string().contains("catalogue"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

async fn assert_native_definer_drop_window(db: &ShamirDb, name: &str) {
    let row = db
        .system_store()
        .load_function(name)
        .await
        .unwrap()
        .expect("legacy native row exists before the drop/rename window");
    assert!(db.functions().contains(name));
    let binding = db
        .functions()
        .capture(name)
        .expect("registered native binding");
    assert!(!binding.is_database_builtin());
    assert!(db.function_meta(name).is_none());
    assert_eq!(row.get("kind"), Some(&QueryValue::Str("native".into())));
    assert!(row.get("artifact_id").is_none());
    assert_eq!(
        FunctionMeta::try_from_record(&row).unwrap().security,
        Security::Definer
    );
    let resource = ResourceMeta::from_record(&row);
    assert_eq!(resource.owner, user(OWNER));
    assert_eq!(resource.mode, 0o700);
    assert!(db
        .functions()
        .capture(name)
        .unwrap()
        .artifact_id()
        .is_none());
}

#[tokio::test]
async fn ordinary_registry_only_function_refuses_missing_catalogue_row() {
    let mut db = super::support::db_setup().await;
    db.set_net_allowlist(vec!["example.com".into()]);
    let calls = Arc::new(AtomicUsize::new(0));
    db.functions()
        .register("registry_only_net", Arc::new(NetworkProbe(calls.clone())))
        .unwrap();
    assert!(!db
        .functions()
        .capture("registry_only_net")
        .unwrap()
        .is_database_builtin());
    let error = invoke_as(&db, "registry_only_net", user(OWNER))
        .await
        .expect_err("ordinary registry registrations are not database builtins");
    assert!(
        error.to_string().contains("catalogue"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn catalogue_first_root_resolution_rejects_old_binding_against_new_row_id() {
    let db = super::support::db_setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let body = Arc::new(NativeRootCall {
        calls: calls.clone(),
        recurse: false,
    });
    register_native(
        &db,
        "mismatch_root",
        body.clone(),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut row = db
        .system_store()
        .load_function("mismatch_root")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut row {
        fields.insert("artifact_id".into(), QueryValue::Str("new-row-id".into()));
    }
    db.system_store()
        .remove_function("mismatch_root")
        .await
        .unwrap();
    db.system_store()
        .save_function("mismatch_root", &row, &ResourceMeta::from_record(&row))
        .await
        .unwrap();
    db.functions()
        .replace_with_artifact_id("mismatch_root", body, Some("old-binding-id".into()));

    let error = invoke_as(&db, "mismatch_root", user(OWNER))
        .await
        .expect_err("artifact mismatch must fail resolution");
    assert!(
        error.to_string().contains("changed during resolution"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn catalogue_row_id_without_binding_id_fails_closed() {
    let db = super::support::db_setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "row_id_no_binding_id",
        Arc::new(NativeRootCall {
            calls: calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut row = db
        .system_store()
        .load_function("row_id_no_binding_id")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut row {
        fields.insert("artifact_id".into(), QueryValue::Str("persisted-id".into()));
    }
    db.system_store()
        .remove_function("row_id_no_binding_id")
        .await
        .unwrap();
    db.system_store()
        .save_function_meta_record("row_id_no_binding_id", &row)
        .await
        .unwrap();

    let error = invoke_as(&db, "row_id_no_binding_id", user(OWNER))
        .await
        .expect_err("one-sided artifact id must mismatch");
    assert!(
        error.to_string().contains("changed during resolution"),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nested_resolver_rejects_row_id_without_binding_id() {
    let db = super::support::db_setup().await;
    let target_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "nested_id_target",
        Arc::new(NativeRootCall {
            calls: target_calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "nested_id_parent",
        Arc::new(Forward("nested_id_target")),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut row = db
        .system_store()
        .load_function("nested_id_target")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut row {
        fields.insert(
            "artifact_id".into(),
            QueryValue::Str("replacement-row-id".into()),
        );
    }
    db.system_store()
        .remove_function("nested_id_target")
        .await
        .unwrap();
    db.system_store()
        .save_function_meta_record("nested_id_target", &row)
        .await
        .unwrap();

    let error = invoke_as(&db, "nested_id_parent", user(OWNER))
        .await
        .expect_err("nested resolver must compare ids");
    assert!(
        error.to_string().contains("changed during resolution"),
        "unexpected error: {error}"
    );
    assert_eq!(target_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn normal_wasm_replace_uses_new_result_and_fresh_catalogue_artifact_id() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "normal_replace", 81, user(OWNER), Security::Invoker).await;
    let first = db
        .functions()
        .capture("normal_replace")
        .unwrap()
        .artifact_id()
        .map(str::to_owned);
    let wasm = constant_wasm(QueryValue::Int(82));
    db.create_function_with_opts_as(
        "normal_replace",
        FunctionSource::Wasm(&wasm),
        options(Security::Invoker, true),
        user(OWNER),
    )
    .await
    .unwrap();
    let second = db
        .functions()
        .capture("normal_replace")
        .unwrap()
        .artifact_id()
        .map(str::to_owned);
    assert!(
        first.is_some() && second.is_some() && first != second,
        "replacement rotates artifact id"
    );
    let row = db
        .system_store()
        .load_function("normal_replace")
        .await
        .unwrap()
        .unwrap();
    let stored_id = row
        .get("artifact_id")
        .and_then(|value| value.as_str().map(str::to_owned));
    assert_eq!(stored_id.as_deref(), second.as_deref());
    assert_eq!(
        db.invoke_function_as("normal_replace", Params::new(), user(OWNER))
            .await
            .unwrap(),
        QueryValue::Int(82)
    );
}

#[tokio::test]
async fn native_replace_uses_new_artifact_and_new_catalogue_metadata() {
    let db = super::support::db_setup().await;
    let old_body: Arc<dyn ShamirFunction> = Arc::new(Returns(QueryValue::Int(-1)));
    register_native(
        &db,
        "native_replace_meta",
        old_body.clone(),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut old_row = db
        .system_store()
        .load_function("native_replace_meta")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut old_row {
        fields.insert(
            "artifact_id".into(),
            QueryValue::Str("old-native-id".into()),
        );
    }
    db.system_store()
        .remove_function("native_replace_meta")
        .await
        .unwrap();
    db.system_store()
        .save_function(
            "native_replace_meta",
            &old_row,
            &ResourceMeta::from_record(&old_row),
        )
        .await
        .unwrap();
    db.functions().replace_with_artifact_id(
        "native_replace_meta",
        old_body,
        Some("old-native-id".into()),
    );

    let mut new_row = old_row;
    FunctionMeta::new(
        Visibility::Public,
        Security::Definer,
        vec!["NEW_GRANT".into()],
        Vec::new(),
    )
    .inject_into(&mut new_row);
    if let QueryValue::Map(fields) = &mut new_row {
        fields.insert(
            "artifact_id".into(),
            QueryValue::Str("new-native-id".into()),
        );
    }
    db.system_store()
        .remove_function("native_replace_meta")
        .await
        .unwrap();
    db.system_store()
        .save_function(
            "native_replace_meta",
            &new_row,
            &ResourceMeta::from_record(&new_row),
        )
        .await
        .unwrap();
    db.functions().replace_with_artifact_id(
        "native_replace_meta",
        Arc::new(MetadataProbe),
        Some("new-native-id".into()),
    );

    assert_eq!(
        db.functions()
            .capture("native_replace_meta")
            .unwrap()
            .artifact_id(),
        Some("new-native-id")
    );
    assert_eq!(
        invoke_as(&db, "native_replace_meta", user(CALLER))
            .await
            .unwrap(),
        QueryValue::List(vec![QueryValue::Int(OWNER as i64), QueryValue::Bool(true)])
    );
}

#[tokio::test]
async fn rename_preserves_artifact_id() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "before_rename", 91, Actor::System, Security::Invoker).await;
    let before = db
        .functions()
        .capture("before_rename")
        .unwrap()
        .artifact_id()
        .map(str::to_owned);
    db.rename_function("before_rename", "after_rename")
        .await
        .unwrap();
    assert_eq!(
        db.functions()
            .capture("after_rename")
            .unwrap()
            .artifact_id()
            .map(str::to_owned),
        before
    );
}

#[tokio::test]
async fn reboot_restores_persisted_artifact_id() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("catalogue-security");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
        .await
        .expect("open durable database");
    create_wasm(&db, "reboot_function", 92, Actor::System, Security::Invoker).await;
    let before = db
        .functions()
        .capture("reboot_function")
        .unwrap()
        .artifact_id()
        .map(str::to_owned);
    drop(db);

    let reopened = crate::lifecycle::init_redb_retry(&path).await;
    assert_eq!(
        reopened
            .functions()
            .capture("reboot_function")
            .unwrap()
            .artifact_id(),
        before.as_deref()
    );
    assert_eq!(
        reopened
            .invoke_function_as("reboot_function", Params::new(), Actor::System)
            .await
            .unwrap(),
        QueryValue::Int(92)
    );
}

#[tokio::test]
async fn catalogue_bound_public_registry_paths_fail_but_host_registry_fallback_remains() {
    let db = super::support::db_setup().await;
    let live = db.functions().capture("argon2id").unwrap();
    let context = FnCtx::new().with_registry(db.functions().clone());
    let invoke_error = db
        .functions()
        .invoke("argon2id", &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(
        matches!(invoke_error, FunctionError::Compute(message) if message.contains("resolver"))
    );
    let call_error = context
        .call_stored("argon2id", &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(
        matches!(call_error, FunctionError::Compute(message) if message.contains("captured root"))
    );
    assert!(live.same_as(&db.functions().capture("argon2id").unwrap()));

    let host = Arc::new(FunctionRegistry::new());
    host.register("host_only", Arc::new(Returns(QueryValue::Int(101))))
        .unwrap();
    let host_ctx = FnCtx::new().with_registry(host.clone());
    assert_eq!(
        host.invoke("host_only", &host_ctx, &FnBatch::new(), &Params::new())
            .await
            .unwrap(),
        QueryValue::Int(101)
    );
    assert_eq!(
        host_ctx
            .call_stored("host_only", &FnBatch::new(), &Params::new())
            .await
            .unwrap(),
        QueryValue::Int(101)
    );
}

#[test]
fn register_capture_and_replace_keep_expected_artifact_ids() {
    let registry = FunctionRegistry::new();
    registry
        .register_with_artifact_id(
            "probe",
            Arc::new(Returns(QueryValue::Null)),
            Some("id-a".into()),
        )
        .unwrap();
    assert_eq!(
        registry.capture("probe").unwrap().artifact_id(),
        Some("id-a")
    );
    registry.replace_with_artifact_id(
        "probe",
        Arc::new(Returns(QueryValue::Null)),
        Some("id-b".into()),
    );
    assert_eq!(
        registry.capture("probe").unwrap().artifact_id(),
        Some("id-b")
    );
    registry
        .register("legacy", Arc::new(Returns(QueryValue::Null)))
        .unwrap();
    assert_eq!(registry.capture("legacy").unwrap().artifact_id(), None);
}

#[tokio::test]
async fn legacy_none_ids_match_and_binding_only_id_mismatch_fails() {
    let db = super::support::db_setup().await;
    let legacy_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "legacy_none_ids",
        Arc::new(NativeRootCall {
            calls: legacy_calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    assert_eq!(
        db.functions()
            .capture("legacy_none_ids")
            .unwrap()
            .artifact_id(),
        None
    );
    assert_eq!(
        invoke_as(&db, "legacy_none_ids", user(OWNER))
            .await
            .unwrap(),
        QueryValue::Int(1)
    );
    assert_eq!(legacy_calls.load(Ordering::SeqCst), 1);

    let binding_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "binding_id_no_row_id",
        Arc::new(NativeRootCall {
            calls: binding_calls.clone(),
            recurse: false,
        }),
        user(OWNER),
        0o777,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut row = db
        .system_store()
        .load_function("binding_id_no_row_id")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut row {
        fields.shift_remove("artifact_id");
    }
    db.system_store()
        .remove_function("binding_id_no_row_id")
        .await
        .unwrap();
    db.system_store()
        .save_function_meta_record("binding_id_no_row_id", &row)
        .await
        .unwrap();
    db.functions().replace_with_artifact_id(
        "binding_id_no_row_id",
        Arc::new(NativeRootCall {
            calls: binding_calls.clone(),
            recurse: false,
        }),
        Some("registry-artifact".into()),
    );

    let error = invoke_as(&db, "binding_id_no_row_id", user(OWNER))
        .await
        .expect_err("binding-only id is a mismatch");
    assert!(
        error.to_string().contains("changed during resolution"),
        "unexpected error: {error}"
    );
    assert_eq!(binding_calls.load(Ordering::SeqCst), 0);
}
