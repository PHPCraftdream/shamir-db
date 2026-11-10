//! D2, D4, and D6 function-catalogue lifecycle security regressions.

use std::sync::Arc;

use shamir_db::shamir_db::FunctionSource;
use shamir_engine::function::{CreateFunctionOptions, FunctionMeta, Security, Visibility};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::support::{create_wasm, options, OWNER, WRITER};
use crate::native_stored_call::support::{register_native, user, NativeRootCall};

async fn function_row(db: &shamir_db::ShamirDb, name: &str) -> QueryValue {
    db.system_store()
        .load_function(name)
        .await
        .expect("load function row")
        .expect("function row exists")
}

fn native_catalogue_row(name: &str) -> QueryValue {
    let mut fields = new_map();
    fields.insert("name".into(), QueryValue::Str(name.into()));
    fields.insert("kind".into(), QueryValue::Str("native".into()));
    let mut row = QueryValue::Map(fields);
    FunctionMeta::new(
        Visibility::Public,
        Security::Invoker,
        Vec::new(),
        Vec::new(),
    )
    .inject_into(&mut row);
    row
}

#[tokio::test]
async fn rename_to_catalogue_only_function_preserves_source_and_destination() {
    let db = super::support::db_setup().await;
    register_native(
        &db,
        "rename_source",
        Arc::new(NativeRootCall {
            calls: Arc::default(),
            recurse: false,
        }),
        user(OWNER),
        0o700,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let destination = native_catalogue_row("victim");
    db.system_store()
        .save_function(
            "victim",
            &destination,
            &ResourceMeta {
                owner: user(WRITER),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .expect("save catalogue-only destination");

    let source_before = function_row(&db, "rename_source").await;
    let destination_before = function_row(&db, "victim").await;
    let source_binding = db.functions().capture("rename_source").unwrap();
    assert!(!db.functions().contains("victim"));

    let error = db
        .rename_function("rename_source", "victim")
        .await
        .expect_err("catalogue-only destination must block rename");

    assert!(error
        .to_string()
        .contains("function 'victim' already exists"));
    assert_eq!(function_row(&db, "rename_source").await, source_before);
    assert_eq!(function_row(&db, "victim").await, destination_before);
    assert!(source_binding.same_as(&db.functions().capture("rename_source").unwrap()));
    assert!(!db.functions().contains("victim"));
}

#[tokio::test]
async fn rename_requires_function_namespace_create_without_mutating_source() {
    let db = super::support::db_setup().await;
    register_native(
        &db,
        "restricted_rename_source",
        Arc::new(NativeRootCall {
            calls: Arc::default(),
            recurse: false,
        }),
        user(OWNER),
        0o700,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    db.set_resource_meta(
        &ResourcePath::FunctionNamespace,
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o555,
        },
    )
    .await
    .expect("restrict namespace Create");

    let source_before = function_row(&db, "restricted_rename_source").await;
    let source_binding = db.functions().capture("restricted_rename_source").unwrap();
    let error = db
        .rename_function_as(
            "restricted_rename_source",
            "restricted_rename_target",
            user(OWNER),
        )
        .await
        .expect_err("rename must require namespace Create");

    assert!(error.to_string().contains("access denied"), "{error}");
    assert_eq!(
        function_row(&db, "restricted_rename_source").await,
        source_before
    );
    assert!(db
        .system_store()
        .load_function("restricted_rename_target")
        .await
        .unwrap()
        .is_none());
    assert!(source_binding.same_as(&db.functions().capture("restricted_rename_source").unwrap()));
    assert!(!db.functions().contains("restricted_rename_target"));
}

#[tokio::test]
async fn rename_registry_collision_restores_source_and_destination_metadata() {
    let db = super::support::db_setup().await;
    create_wasm(
        &db,
        "rename_source_meta",
        11,
        user(OWNER),
        Security::Invoker,
    )
    .await;
    db.functions()
        .register(
            "rename_live_target",
            Arc::new(NativeRootCall {
                calls: Arc::default(),
                recurse: false,
            }),
        )
        .expect("register a live-only destination");

    let source_before = function_row(&db, "rename_source_meta").await;
    let source_binding = db.functions().capture("rename_source_meta").unwrap();
    let target_binding = db.functions().capture("rename_live_target").unwrap();
    let source_meta_before = db.function_meta("rename_source_meta").unwrap();
    assert!(db.function_meta("rename_live_target").is_none());
    assert!(db
        .system_store()
        .load_function("rename_live_target")
        .await
        .unwrap()
        .is_none());

    let error = db
        .rename_function("rename_source_meta", "rename_live_target")
        .await
        .expect_err("live registry collision must reject rename");

    // The registry (not the catalogue) rejects a live-only destination.
    assert!(error.to_string().contains("already registered"), "{error}");
    assert_eq!(function_row(&db, "rename_source_meta").await, source_before);
    assert!(db
        .system_store()
        .load_function("rename_live_target")
        .await
        .unwrap()
        .is_none());
    assert!(source_binding.same_as(&db.functions().capture("rename_source_meta").unwrap()));
    assert!(target_binding.same_as(&db.functions().capture("rename_live_target").unwrap()));
    let source_meta_after = db.function_meta("rename_source_meta").unwrap();
    assert_eq!(source_meta_after.security, source_meta_before.security);
    assert_eq!(source_meta_after.visibility, source_meta_before.visibility);
    assert_eq!(
        source_meta_after.secret_grants,
        source_meta_before.secret_grants
    );
    assert_eq!(source_meta_after.net_grants, source_meta_before.net_grants);
    assert!(db.function_meta("rename_live_target").is_none());
}

async fn create_malformed_security_function(
    db: &shamir_db::ShamirDb,
    name: &str,
    value: i64,
) -> QueryValue {
    create_wasm(db, name, value, user(OWNER), Security::Invoker).await;
    db.set_resource_meta(
        &ResourcePath::function(name),
        &ResourceMeta {
            owner: user(OWNER),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("set owner-only function Write");
    let mut row = function_row(db, name).await;
    if let QueryValue::Map(fields) = &mut row {
        fields.insert("security".into(), QueryValue::Int(42));
    } else {
        panic!("function catalogue row is not a map");
    }
    db.system_store()
        .save_function_meta_record(name, &row)
        .await
        .expect("persist malformed security metadata");
    function_row(db, name).await
}

#[tokio::test]
async fn admin_can_repair_malformed_security_metadata_while_replacing() {
    let db = super::support::db_setup().await;
    let corrupt_row = create_malformed_security_function(&db, "malformed_admin_repair", 21).await;
    let old_binding = db.functions().capture("malformed_admin_repair").unwrap();
    assert_eq!(corrupt_row.get("security"), Some(&QueryValue::Int(42)));

    let replacement = super::support::constant_wasm(QueryValue::Int(22));
    db.create_function_with_opts_as(
        "malformed_admin_repair",
        FunctionSource::Wasm(&replacement),
        options(Security::Invoker, true),
        Actor::Admin(WRITER),
    )
    .await
    .expect("admin replacement repairs malformed persisted security");

    assert_eq!(
        db.invoke_function_as("malformed_admin_repair", Default::default(), user(OWNER))
            .await
            .unwrap(),
        QueryValue::Int(22)
    );
    let repaired = function_row(&db, "malformed_admin_repair").await;
    assert_eq!(
        repaired.get("security"),
        Some(&QueryValue::Str("invoker".into()))
    );
    assert_eq!(ResourceMeta::from_record(&repaired).owner, user(OWNER));
    assert!(!old_binding.same_as(&db.functions().capture("malformed_admin_repair").unwrap()));
    assert_eq!(
        db.function_meta("malformed_admin_repair").unwrap().security,
        Security::Invoker
    );
}

#[tokio::test]
async fn unauthorized_malformed_security_replacement_fails_at_write_gate() {
    let db = super::support::db_setup().await;
    let corrupt_row = create_malformed_security_function(&db, "malformed_denied_replace", 31).await;
    let binding = db.functions().capture("malformed_denied_replace").unwrap();
    let replacement = super::support::constant_wasm(QueryValue::Int(32));

    let error = db
        .create_function_with_opts_as(
            "malformed_denied_replace",
            FunctionSource::Wasm(&replacement),
            options(Security::Invoker, true),
            user(WRITER),
        )
        .await
        .expect_err("non-owner without Write must be rejected");

    let message = error.to_string();
    assert!(message.contains("access denied"), "{message}");
    assert!(!message.contains("invalid security"), "{message}");
    assert_eq!(
        function_row(&db, "malformed_denied_replace").await,
        corrupt_row
    );
    assert!(binding.same_as(&db.functions().capture("malformed_denied_replace").unwrap()));
    assert_eq!(
        db.function_meta("malformed_denied_replace")
            .unwrap()
            .security,
        Security::Invoker
    );
}

#[tokio::test]
async fn owner_can_repair_malformed_security_metadata() {
    let db = super::support::db_setup().await;
    let _corrupt_row = create_malformed_security_function(&db, "malformed_owner_repair", 41).await;
    let old_binding = db.functions().capture("malformed_owner_repair").unwrap();
    let replacement = super::support::constant_wasm(QueryValue::Int(42));

    db.create_function_with_opts_as(
        "malformed_owner_repair",
        FunctionSource::Wasm(&replacement),
        CreateFunctionOptions {
            replace: true,
            ..CreateFunctionOptions::default()
        },
        user(OWNER),
    )
    .await
    .expect("owner replacement repairs malformed persisted security");

    assert_eq!(
        db.invoke_function_as("malformed_owner_repair", Default::default(), user(OWNER))
            .await
            .unwrap(),
        QueryValue::Int(42)
    );
    let repaired = function_row(&db, "malformed_owner_repair").await;
    assert_eq!(
        repaired.get("security"),
        Some(&QueryValue::Str("invoker".into()))
    );
    assert_eq!(ResourceMeta::from_record(&repaired).owner, user(OWNER));
    assert!(!old_binding.same_as(&db.functions().capture("malformed_owner_repair").unwrap()));
}
