//! Native database-route tests for stored-call actor and capability resolution.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, Security, ShamirFunction};
use shamir_types::access::{Action, ResourcePath};
use shamir_types::types::value::QueryValue;

use super::support::{
    curl_available, invoke_as, invoke_in_db_as, register_native, setup, setup_secrets_table, user,
    ActorProbe, CapabilityProbe, DefinerTableReader, Forward, LoopbackHttpFixture, NativeRootCall,
    NetPolicyProbe, SeedParentNetGateway, CALLER, OWNER,
};

struct Counted(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for Counted {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(QueryValue::Null)
    }
}

#[tokio::test]
async fn nested_execute_denial_does_not_run_target_body() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "denied_target",
        Arc::new(Counted(calls.clone())),
        user(OWNER),
        0o710,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "denial_parent",
        Arc::new(Forward("denied_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert!(invoke_as(&db, "denial_parent", user(CALLER)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nested_definer_uses_owner_table_rights_and_parent_invoker_does_not() {
    let db = setup().await;
    setup_secrets_table(&db, user(OWNER)).await;
    register_native(
        &db,
        "definer_table_target",
        Arc::new(DefinerTableReader),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "table_parent",
        Arc::new(Forward("definer_table_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "invoker_table_parent",
        Arc::new(DefinerTableReader),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_in_db_as(&db, "table_parent", user(CALLER))
            .await
            .expect("nested Definer uses the owner's table access"),
        QueryValue::Int(1)
    );
    assert!(invoke_in_db_as(&db, "invoker_table_parent", user(CALLER))
        .await
        .is_err());
    register_native(
        &db,
        "nested_target_db_gateway",
        Arc::new(CapabilityProbe),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "nested_db_gateway_parent",
        Arc::new(Forward("nested_target_db_gateway")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    let result = invoke_in_db_as(&db, "nested_db_gateway_parent", user(CALLER))
        .await
        .expect("target DB gateway should be rebuilt for nested call");
    assert_eq!(
        result.get("db_gateway_present"),
        Some(&QueryValue::Bool(true))
    );
    assert_eq!(result.get("repo"), Some(&QueryValue::Str("main".into())));
}

#[tokio::test]
async fn nested_target_does_not_inherit_parent_secret_or_network_grants() {
    if !curl_available() {
        eprintln!("SKIP nested_target_does_not_inherit_parent_secret_or_network_grants - curl not found on PATH");
        return;
    }
    let Ok(fixture) = LoopbackHttpFixture::new().await else {
        eprintln!("SKIP nested_target_does_not_inherit_parent_secret_or_network_grants - 127.0.0.2 is not bindable");
        return;
    };
    let mut db = setup().await;
    db.set_net_allowlist(vec!["127.0.0.1".into(), "127.0.0.2".into()]);
    register_native(
        &db,
        "capability_target",
        Arc::new(NetPolicyProbe),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), vec!["127.0.0.2".into()]),
    )
    .await;
    register_native(
        &db,
        "capability_parent",
        Arc::new(SeedParentNetGateway {
            target: "capability_target",
            parent_only_url: fixture.parent_only_url.clone(),
            target_only_url: fixture.target_only_url.clone(),
        }),
        user(CALLER),
        0o711,
        Security::Invoker,
        (vec!["SHAMIR_TEST_SECRET".into()], vec!["127.0.0.1".into()]),
    )
    .await;
    register_native(
        &db,
        "own_secret_target",
        Arc::new(CapabilityProbe),
        user(CALLER),
        0o711,
        Security::Invoker,
        (vec!["SHAMIR_TEST_SECRET".into()], Vec::new()),
    )
    .await;
    register_native(
        &db,
        "own_secret_parent",
        Arc::new(Forward("own_secret_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (vec!["SHAMIR_PARENT_ONLY".into()], Vec::new()),
    )
    .await;

    let invocation = async {
        invoke_as(&db, "capability_parent", user(CALLER))
            .await
            .map_err(|error| error.to_string())
    };
    let (result, ()) = tokio::try_join!(invocation, async {
        fixture.serve().await.map_err(|error| error.to_string())
    })
    .expect("nested network capability probe and loopback servers");
    assert_eq!(result.get("secret_grant"), Some(&QueryValue::Bool(false)));
    assert_eq!(
        result.get("parent_allows_parent_url"),
        Some(&QueryValue::Bool(true))
    );
    assert_eq!(
        result.get("parent_allows_target_url"),
        Some(&QueryValue::Bool(false))
    );
    assert_eq!(
        result.get("target_allows_parent_url"),
        Some(&QueryValue::Bool(false))
    );
    assert_eq!(
        result.get("target_allows_target_url"),
        Some(&QueryValue::Bool(true))
    );

    let own_grant_result = invoke_as(&db, "own_secret_parent", user(CALLER))
        .await
        .expect("target-specific secret grant probe");
    assert_eq!(
        own_grant_result.get("secret_grant"),
        Some(&QueryValue::Bool(true))
    );
}

#[tokio::test]
async fn nested_definer_uses_owner_actor_even_with_restricted_parent() {
    let db = setup().await;
    register_native(
        &db,
        "definer_actor_target",
        Arc::new(ActorProbe),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "definer_actor_parent",
        Arc::new(Forward("definer_actor_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "definer_actor_parent", user(CALLER))
            .await
            .unwrap(),
        QueryValue::Int(OWNER as i64)
    );
}

#[tokio::test]
async fn nested_invoker_retains_current_actor_and_legacy_setuid_uses_owner() {
    let db = setup().await;
    register_native(
        &db,
        "plain_invoker",
        Arc::new(ActorProbe),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "legacy_setuid",
        Arc::new(ActorProbe),
        user(OWNER),
        0o4751,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "actor_parent",
        Arc::new(Forward("plain_invoker")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "setuid_parent",
        Arc::new(Forward("legacy_setuid")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "actor_parent", user(CALLER)).await.unwrap(),
        QueryValue::Int(CALLER as i64)
    );
    assert_eq!(
        invoke_as(&db, "setuid_parent", user(CALLER)).await.unwrap(),
        QueryValue::Int(OWNER as i64)
    );
}

#[tokio::test]
async fn definer_parent_rejects_other_owner_invoker_and_allows_same_owner() {
    let db = setup().await;
    register_native(
        &db,
        "other_owner_invoker",
        Arc::new(ActorProbe),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "same_owner_invoker",
        Arc::new(ActorProbe),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "definer_other_parent",
        Arc::new(Forward("other_owner_invoker")),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "definer_same_parent",
        Arc::new(Forward("same_owner_invoker")),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;

    let guard_error = invoke_as(&db, "definer_other_parent", user(CALLER))
        .await
        .expect_err("Definer cannot call a differently-owned Invoker");
    assert!(guard_error
        .to_string()
        .contains("cannot call Invoker function owned by a different actor"));
    assert_eq!(
        invoke_as(&db, "definer_same_parent", user(CALLER))
            .await
            .unwrap(),
        QueryValue::Int(OWNER as i64)
    );
}

#[tokio::test]
async fn nested_hops_authorize_as_current_actor_not_outer_caller() {
    let db = setup().await;
    register_native(
        &db,
        "hop_leaf",
        Arc::new(ActorProbe),
        user(OWNER),
        0o710,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "hop_middle",
        Arc::new(Forward("hop_leaf")),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "hop_root",
        Arc::new(Forward("hop_middle")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "hop_root", user(CALLER)).await.unwrap(),
        QueryValue::Int(OWNER as i64)
    );
    assert!(db
        .authorize_access(
            &user(CALLER),
            &ResourcePath::function("hop_leaf"),
            Action::Execute,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn nested_call_rejects_absent_target() {
    let db = setup().await;
    register_native(
        &db,
        "absent_parent",
        Arc::new(Forward("absent_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert!(invoke_as(&db, "absent_parent", user(CALLER)).await.is_err());
}

#[tokio::test]
async fn nested_call_rejects_live_only_target_without_catalogue_row() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "live_only_target",
        Arc::new(Counted(calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "live_only_parent",
        Arc::new(Forward("live_only_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    db.system_store()
        .remove_function("live_only_target")
        .await
        .expect("remove live-only target row");

    assert!(invoke_as(&db, "live_only_parent", user(CALLER))
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nested_call_fails_closed_for_missing_metadata_and_missing_owner() {
    let db = setup().await;
    register_native(
        &db,
        "bad_metadata_parent",
        Arc::new(Forward("bad_metadata_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let missing_metadata_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "bad_metadata_target",
        Arc::new(Counted(missing_metadata_calls.clone())),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut row = db
        .system_store()
        .load_function("bad_metadata_target")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut row {
        fields.shift_remove("metadata_version");
        fields.shift_remove("security");
    }
    db.system_store()
        .remove_function("bad_metadata_target")
        .await
        .unwrap();
    db.system_store()
        .save_function(
            "bad_metadata_target",
            &row,
            &shamir_types::access::ResourceMeta {
                owner: user(OWNER),
                group: None,
                mode: 0o711,
            },
        )
        .await
        .unwrap();
    let saved_metadata = db
        .system_store()
        .load_function("bad_metadata_target")
        .await
        .unwrap()
        .unwrap();
    let QueryValue::Map(fields) = saved_metadata else {
        panic!("saved function record is not a map");
    };
    assert!(!fields.contains_key("metadata_version"));
    assert!(!fields.contains_key("security"));
    let metadata_error = invoke_as(&db, "bad_metadata_parent", user(CALLER))
        .await
        .expect_err("missing target metadata must fail closed");
    assert!(metadata_error
        .to_string()
        .contains("nested target security metadata is missing"));
    assert_eq!(missing_metadata_calls.load(Ordering::SeqCst), 0);

    register_native(
        &db,
        "missing_owner_parent",
        Arc::new(Forward("missing_owner_target")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    let missing_owner_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "missing_owner_target",
        Arc::new(Counted(missing_owner_calls.clone())),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;
    let mut ownerless = db
        .system_store()
        .load_function("missing_owner_target")
        .await
        .unwrap()
        .unwrap();
    if let QueryValue::Map(fields) = &mut ownerless {
        fields.shift_remove("owner");
    }
    db.system_store()
        .remove_function("missing_owner_target")
        .await
        .unwrap();
    db.system_store()
        .save_function_meta_record("missing_owner_target", &ownerless)
        .await
        .unwrap();
    let saved_ownerless = db
        .system_store()
        .load_function("missing_owner_target")
        .await
        .unwrap()
        .unwrap();
    let QueryValue::Map(fields) = saved_ownerless else {
        panic!("saved function record is not a map");
    };
    assert!(!fields.contains_key("owner"));
    let owner_error = invoke_as(&db, "missing_owner_parent", user(CALLER))
        .await
        .expect_err("missing target owner must fail closed");
    assert!(owner_error
        .to_string()
        .contains("nested target owner is missing or invalid"));
    assert_eq!(missing_owner_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nested_self_cycle_is_rejected_before_target_reentry() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "self_cycle",
        Arc::new(NativeRootCall {
            calls: calls.clone(),
            recurse: true,
        }),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert!(invoke_as(&db, "self_cycle", user(CALLER)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
