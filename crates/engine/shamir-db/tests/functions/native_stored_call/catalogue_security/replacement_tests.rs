//! F6 replacement authorization regressions through the public function API.

use shamir_types::types::value::QueryValue;

use super::support::{create_wasm, make_writeable, options, OWNER, WRITER};
use crate::native_stored_call::support::user;
use shamir_db::shamir_db::FunctionSource;
use shamir_engine::function::Security;
use shamir_types::access::Actor;

async fn row(db: &shamir_db::ShamirDb, name: &str) -> QueryValue {
    db.system_store()
        .load_function(name)
        .await
        .expect("load function row")
        .expect("function row exists")
}

#[tokio::test]
async fn nonowner_write_cannot_switch_invoker_to_definer() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "switch_definer", 17, user(OWNER), Security::Invoker).await;
    make_writeable(&db, "switch_definer").await;
    let before = row(&db, "switch_definer").await;
    let binding = db.functions().capture("switch_definer").unwrap();

    let result = db
        .create_function_with_opts_as(
            "switch_definer",
            FunctionSource::Wasm(&super::support::constant_wasm(QueryValue::Int(99))),
            options(Security::Definer, true),
            user(WRITER),
        )
        .await;
    assert!(
        result.is_err(),
        "a Write holder cannot grant Definer security"
    );
    assert_eq!(row(&db, "switch_definer").await, before);
    assert!(binding.same_as(&db.functions().capture("switch_definer").unwrap()));
    assert_eq!(
        db.invoke_function_as("switch_definer", Default::default(), user(WRITER))
            .await
            .unwrap(),
        QueryValue::Int(17)
    );
}

#[tokio::test]
async fn nonowner_cannot_replace_existing_definer() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "replace_definer", 21, user(OWNER), Security::Definer).await;
    make_writeable(&db, "replace_definer").await;
    let before = row(&db, "replace_definer").await;
    let binding = db.functions().capture("replace_definer").unwrap();

    let result = db
        .create_function_with_opts_as(
            "replace_definer",
            FunctionSource::Wasm(&super::support::constant_wasm(QueryValue::Int(22))),
            options(Security::Definer, true),
            user(WRITER),
        )
        .await;
    assert!(
        result.is_err(),
        "non-owner cannot substitute privileged code"
    );
    assert_eq!(row(&db, "replace_definer").await, before);
    assert!(binding.same_as(&db.functions().capture("replace_definer").unwrap()));
}

#[tokio::test]
async fn nonowner_cannot_replace_setuid_invoker() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "replace_setuid", 31, user(OWNER), Security::Invoker).await;
    db.set_resource_meta(
        &shamir_types::access::ResourcePath::function("replace_setuid"),
        &shamir_types::access::ResourceMeta {
            owner: user(OWNER),
            group: None,
            mode: 0o4777,
        },
    )
    .await
    .unwrap();
    let before = row(&db, "replace_setuid").await;
    let binding = db.functions().capture("replace_setuid").unwrap();

    let result = db
        .create_function_with_opts_as(
            "replace_setuid",
            FunctionSource::Wasm(&super::support::constant_wasm(QueryValue::Int(32))),
            options(Security::Invoker, true),
            user(WRITER),
        )
        .await;
    assert!(
        result.is_err(),
        "setuid function replacement is owner/admin-only"
    );
    assert_eq!(row(&db, "replace_setuid").await, before);
    assert!(binding.same_as(&db.functions().capture("replace_setuid").unwrap()));
}

#[tokio::test]
async fn function_owner_can_replace_and_change_security() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "owner_replace", 41, user(OWNER), Security::Invoker).await;
    make_writeable(&db, "owner_replace").await;
    let before = row(&db, "owner_replace").await;
    let wasm = super::support::constant_wasm(QueryValue::Int(42));
    db.create_function_with_opts_as(
        "owner_replace",
        FunctionSource::Wasm(&wasm),
        options(Security::Definer, true),
        user(OWNER),
    )
    .await
    .expect("owner may replace and set Definer");
    assert_eq!(
        db.invoke_function_as("owner_replace", Default::default(), user(WRITER))
            .await
            .unwrap(),
        QueryValue::Int(42)
    );
    let after = row(&db, "owner_replace").await;
    assert_eq!(
        shamir_types::access::ResourceMeta::from_record(&after),
        shamir_types::access::ResourceMeta::from_record(&before),
        "replacement retains the existing resource metadata"
    );
    assert_eq!(
        db.function_meta("owner_replace").unwrap().security,
        Security::Definer
    );
}

#[tokio::test]
async fn system_may_replace_function() {
    assert_admin_replace_allowed(Actor::System, "system_replace").await;
}

#[tokio::test]
async fn admin_may_replace_function() {
    assert_admin_replace_allowed(Actor::Admin(WRITER), "admin_replace").await;
}

async fn assert_admin_replace_allowed(actor: Actor, name: &str) {
    let db = super::support::db_setup().await;
    create_wasm(&db, name, 51, user(OWNER), Security::Definer).await;
    make_writeable(&db, name).await;
    let wasm = super::support::constant_wasm(QueryValue::Int(52));
    db.create_function_with_opts_as(
        name,
        FunctionSource::Wasm(&wasm),
        options(Security::Definer, true),
        actor,
    )
    .await
    .expect("administrator may replace privileged function");
    assert_eq!(
        db.invoke_function_as(name, Default::default(), user(WRITER))
            .await
            .unwrap(),
        QueryValue::Int(52)
    );
}

#[tokio::test]
async fn nonowner_write_can_replace_plain_invoker() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "plain_replace", 61, user(OWNER), Security::Invoker).await;
    make_writeable(&db, "plain_replace").await;
    let wasm = super::support::constant_wasm(QueryValue::Int(62));
    db.create_function_with_opts_as(
        "plain_replace",
        FunctionSource::Wasm(&wasm),
        options(Security::Invoker, true),
        user(WRITER),
    )
    .await
    .expect("Write holder may replace Invoker with Invoker");
    assert_eq!(
        db.invoke_function_as("plain_replace", Default::default(), user(WRITER))
            .await
            .unwrap(),
        QueryValue::Int(62)
    );
}

#[tokio::test]
async fn nonadmin_cannot_replace_ownerless_legacy_row_without_mode() {
    let db = super::support::db_setup().await;
    create_wasm(&db, "ownerless_legacy", 71, user(OWNER), Security::Invoker).await;
    let mut record = row(&db, "ownerless_legacy").await;
    let QueryValue::Map(fields) = &mut record else {
        panic!("catalogue row is not a map")
    };
    fields.shift_remove("owner");
    fields.shift_remove("mode");
    fields.shift_remove("group");
    db.system_store()
        .remove_function("ownerless_legacy")
        .await
        .unwrap();
    db.system_store()
        .save_function_meta_record("ownerless_legacy", &record)
        .await
        .unwrap();
    let before = row(&db, "ownerless_legacy").await;

    let result = db
        .create_function_with_opts_as(
            "ownerless_legacy",
            FunctionSource::Wasm(&super::support::constant_wasm(QueryValue::Int(72))),
            options(Security::Invoker, true),
            user(WRITER),
        )
        .await;
    assert!(
        result.is_err(),
        "missing owner cannot be inferred from open legacy mode"
    );
    assert_eq!(row(&db, "ownerless_legacy").await, before);
}
