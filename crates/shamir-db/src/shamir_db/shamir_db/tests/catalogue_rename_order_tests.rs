//! P0 catalogue ordering + persistence-error regression tests
//! (RV-DB-093EAA95 / RV-DB-6540DC7B).
//!
//! 093EAA95 — rename must write the NEW durable catalogue row BEFORE
//! removing the old one, so a failed new-row write leaves the old row
//! (and therefore the artifact) resolvable.
//!
//! 6540DC7B — `rename_db_as` / `rename_table_as` must PROPAGATE catalogue
//! write failures instead of warn+continue, so callers learn the rename
//! half-happened; the card also covers create/remove lifecycle propagation
//! (`create_db_as_checked`, `remove_db`, and the wire `CREATE DATABASE`
//! handler's `persist_failed` mapping).

use std::hash::BuildHasher;
use std::sync::Arc;

use shamir_engine::function::{
    FnBatch, FnCtx, FunctionMeta, HttpRequest, NetGatewayError, Params, Security, Visibility,
};
use shamir_engine::validator::{RecordFields, Validation, ValidatorCtx};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::shamir_db::ShamirDb;

#[tokio::test]
async fn invocation_settings_match_resource_meta_defaults_and_malformed_values() {
    let db = ShamirDb::init_memory().await.unwrap();
    let caller = Actor::User(7);
    assert_eq!(
        db.authorize_invocation("missing", &caller).await.unwrap(),
        caller
    );
    db.system_store()
        .save_setting("root_meta", &QueryValue::Int(1))
        .await
        .unwrap();
    db.system_store()
        .save_setting("fn_namespace_meta", &QueryValue::Int(1))
        .await
        .unwrap();
    assert_eq!(
        db.authorize_invocation("missing", &caller).await.unwrap(),
        caller
    );
    let mut malformed = new_map();
    malformed.insert("mode".into(), QueryValue::Str("invalid".into()));
    db.system_store()
        .save_setting("fn_namespace_meta", &QueryValue::Map(malformed))
        .await
        .unwrap();
    assert!(db.authorize_invocation("missing", &caller).await.is_err());
}

#[tokio::test]
async fn invocation_snapshot_uses_current_definer_owner() {
    let db = ShamirDb::init_memory().await.unwrap();
    let store = db.system_store();
    let mut fields = new_map();
    fields.insert("name".into(), QueryValue::Str("sample".into()));
    fields.insert("security".into(), QueryValue::Str("definer".into()));
    let record = QueryValue::Map(fields);
    let mut meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o755,
    };
    store.save_function("sample", &record, &meta).await.unwrap();
    assert_eq!(
        db.authorize_invocation("sample", &Actor::User(7))
            .await
            .unwrap(),
        Actor::User(10)
    );
    meta.owner = Actor::User(11);
    store.save_function("sample", &record, &meta).await.unwrap();
    assert_eq!(
        db.authorize_invocation("sample", &Actor::User(7))
            .await
            .unwrap(),
        Actor::User(11)
    );
    // Remove the row first: `set` merges into an existing row and would keep the owner.
    // A record without an owner field must never escalate the caller.
    store.remove_function("sample").await.unwrap();
    store
        .save_function_meta_record("sample", &record)
        .await
        .unwrap();
    assert_eq!(
        db.authorize_invocation("sample", &Actor::User(7))
            .await
            .unwrap(),
        Actor::User(7)
    );
}

#[tokio::test]
async fn network_gateway_retains_distinct_grant_sets_when_alternating() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    let first_grants = vec!["first.example".to_string()];
    let slot = |grants: &[String]| shamir_collections::THasher::default().hash_one(grants) % 64;
    let first_slot = slot(&first_grants);
    let second_grants = (0..1000)
        .map(|i| vec![format!("host{i}.example")])
        .find(|grants| slot(grants) != first_slot)
        .unwrap();
    db.set_net_allowlist(vec![first_grants[0].clone(), second_grants[0].clone()]);
    for (name, grants) in [("first", first_grants), ("second", second_grants)] {
        db.function_meta.insert(
            name.into(),
            FunctionMeta::new(Visibility::Private, Security::Invoker, vec![], grants),
        );
    }
    let first = db.build_net_gateway("first");
    let second = db.build_net_gateway("second");
    assert!(Arc::ptr_eq(&first, &db.build_net_gateway("first")));
    assert!(Arc::ptr_eq(&second, &db.build_net_gateway("second")));
}

#[tokio::test]
async fn network_gateway_tracks_function_rename_drop_and_recreate() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    db.set_net_allowlist(vec!["allowed.example".into()]);
    db.register_fn("old", false, |_, _, _| async { Ok(QueryValue::Null) })
        .unwrap();
    db.function_meta.insert(
        "old".into(),
        FunctionMeta::new(Visibility::Private, Security::Invoker, vec![], vec![]),
    );
    let restricted = db.build_net_gateway("old");
    db.rename_function("old", "new").await.unwrap();
    assert!(Arc::ptr_eq(&restricted, &db.build_net_gateway("new")));
    assert!(!Arc::ptr_eq(&restricted, &db.build_net_gateway("old")));
    db.drop_function("new").await.unwrap();
    assert!(!Arc::ptr_eq(&restricted, &db.build_net_gateway("new")));
    db.register_fn("new", false, |_, _, _| async { Ok(QueryValue::Null) })
        .unwrap();
    db.function_meta.insert(
        "new".into(),
        FunctionMeta::new(
            Visibility::Private,
            Security::Invoker,
            vec![],
            vec!["allowed.example".into()],
        ),
    );
    assert!(!Arc::ptr_eq(&restricted, &db.build_net_gateway("new")));
}

#[tokio::test]
async fn network_gateway_cache_tracks_grants_and_allowlist_across_clones() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    db.set_net_allowlist(vec!["one.example".into(), "two.example".into()]);
    let mut other = db.clone();
    other.set_net_allowlist(vec!["one.example".into()]);
    let meta = |grants: Vec<String>| {
        FunctionMeta::new(Visibility::Private, Security::Invoker, vec![], grants)
    };
    db.function_meta
        .insert("f".into(), meta(vec!["one.example".into()]));
    let first = db.build_net_gateway("f");
    assert!(Arc::ptr_eq(&first, &db.build_net_gateway("f")));
    let other_first = other.build_net_gateway("f");
    let request = || HttpRequest {
        method: "GET".into(),
        url: "https://two.example/".into(),
        headers: vec![],
        body: vec![],
    };
    assert!(matches!(
        first.fetch(request()).await,
        Err(NetGatewayError::NotAllowed(_))
    ));
    db.function_meta
        .insert("f".into(), meta(vec!["two.example".into()]));
    assert!(!Arc::ptr_eq(&first, &db.build_net_gateway("f")));
    assert!(!Arc::ptr_eq(&other_first, &other.build_net_gateway("f")));
    assert!(matches!(
        other.build_net_gateway("f").fetch(request()).await,
        Err(NetGatewayError::NotAllowed(_))
    ));
    let before_allowlist = db.build_net_gateway("f");
    db.set_net_allowlist(vec![]);
    assert!(!Arc::ptr_eq(&before_allowlist, &db.build_net_gateway("f")));
    assert!(matches!(
        db.build_net_gateway("f").fetch(request()).await,
        Err(NetGatewayError::NotAllowed(_))
    ));
    db.function_meta.remove("f");
    let builtin = db.build_net_gateway("f");
    assert!(Arc::ptr_eq(&builtin, &db.build_net_gateway("unknown")));
    db.function_meta.insert("f".into(), meta(vec![]));
    assert!(!Arc::ptr_eq(&builtin, &db.build_net_gateway("f")));
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-093EAA95 — write-new-before-remove-old on rename
// ═══════════════════════════════════════════════════════════════════════
//
// The validator path is used (rather than functions) because
// `rename_validator_as` is provably fault-clean before its first catalogue
// write: `load_validator` is a read, `validators.rename` is in-memory, and
// the FIRST `SystemStore` write is `save_validator(to)`.
//
// ONE failure is scheduled for the SECOND write: the fault gate is a
// 1-indexed countdown over catalogue writes, so arm(2) starts the
// countdown at 2 and fails when it reaches 1 — i.e. the FIRST write
// (save_validator("new")) succeeds and the SECOND (remove_validator("old"))
// fails at the gate BEFORE deleting anything. The fixed order therefore yields Err with BOTH
// the old and the new durable rows present.
//
// Pre-fix ordering (`remove_validator(from)` first) makes the countdown
// land differently: the FIRST write (remove_validator("old")) succeeds and
// DURABLY DELETES the old row, and the SECOND (save_validator("new"))
// fails. The rename still returns Err, but the `load_validator("old")`
// assertion below fails — exactly the distinguishable outcome this test
// pins.

#[tokio::test]
async fn rv_db_093eaa95_rename_validator_old_row_survives_when_second_write_fails() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Create a validator with a durable catalogue row (register_native_
    // validator persists via save_validator before registering in memory).
    shamir
        .register_native_validator(
            "old",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::accept() },
        )
        .await
        .expect("native validator registration must succeed");
    assert!(
        shamir
            .system_store()
            .load_validator("old")
            .await
            .unwrap()
            .is_some(),
        "precondition: durable row must exist for 'old'"
    );

    // Arm the seam at position 2: the first write (save_validator("new"))
    // succeeds, the second (remove_validator("old")) fails.
    shamir.system_store().arm_catalogue_write_fault(2);

    let result = shamir
        .rename_validator_as("old", "new", Actor::System)
        .await;
    assert!(
        result.is_err(),
        "rename must propagate the injected remove_validator failure"
    );

    // The OLD durable row must survive — the gate fails the remove BEFORE
    // it deletes anything.
    let old_row = shamir
        .system_store()
        .load_validator("old")
        .await
        .expect("load_validator must not fail");
    assert!(
        old_row.is_some(),
        "RV-DB-093EAA95: the old durable row must survive a failed rename"
    );
    // The NEW row was written by the (successful) first write before the
    // gated remove failed — it must be present too.
    let new_row = shamir
        .system_store()
        .load_validator("new")
        .await
        .expect("load_validator must not fail");
    assert!(
        new_row.is_some(),
        "RV-DB-093EAA95: the new durable row must be present after the \
         failed gated remove"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-6540DC7B — rename_db / rename_table propagate catalogue failures
// ═══════════════════════════════════════════════════════════════════════
//
// rename_db_as's first catalogue write is save_database(to) (everything
// before it — binding guard, existence checks, load_database — is reads);
// rename_table_as's first catalogue write is save_table(..., to, ...)
// (everything before it — existence guards, load_table_record, FK scan —
// is reads/in-memory). `save_table` is gated, so ONE armed fault genuinely
// fails the destination save. Pre-fix both warn+continued and returned Ok,
// so `is_err()` failed.

#[tokio::test]
async fn rv_db_6540dc7b_rename_db_propagates_catalogue_save_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("src").await.expect("create_db");
    assert!(
        shamir
            .system_store()
            .load_database("src")
            .await
            .unwrap()
            .is_some(),
        "precondition: durable database row must exist for 'src'"
    );

    shamir.system_store().arm_catalogue_write_fault(1);
    let result = shamir.rename_db_as("src", "dst", Actor::System).await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: rename_db_as must propagate the injected save_database failure"
    );

    // The source row must still be resolvable — the failed write targeted
    // the NEW key only.
    let src_row = shamir
        .system_store()
        .load_database("src")
        .await
        .expect("load_database must not fail");
    assert!(
        src_row.is_some(),
        "the durable 'src' row must survive the failed rename"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_rename_table_propagates_catalogue_save_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");

    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("main", ddl::create_repo("main").tables(["alpha"]));
    shamir
        .execute(
            "testdb",
            &batch
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .expect("create_repo must succeed");
    assert!(
        shamir.get_table("testdb", "main", "alpha").await.is_ok(),
        "precondition: table 'alpha' must exist"
    );

    // `save_table` is gated; it is the FIRST catalogue write on the
    // rename_table_as path (guards and the existing-meta load are
    // read-only), so arm(1) fails exactly the destination save.
    shamir.system_store().arm_catalogue_write_fault(1);
    let result = shamir.rename_table("testdb", "main", "alpha", "beta").await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: rename_table_as must propagate the injected save_table failure"
    );

    // The SOURCE catalogue row must survive: the failing write is the NEW
    // key's save, which errors out before remove_table(from) ever runs.
    // The countdown is one-shot (arm(1) is consumed by the save and the
    // gate disarms), so under the pre-fix warn+continue the subsequent
    // gated remove_table SUCCEEDS and deletes the 'alpha' row — BOTH the
    // Err assertion and this source-row assertion fail on reverted code.
    let src_row = shamir
        .system_store()
        .load_tables()
        .await
        .expect("load_tables must not fail")
        .into_iter()
        .find(|r| {
            r.get("db_name").and_then(|v| v.as_str()) == Some("testdb")
                && r.get("repo_name").and_then(|v| v.as_str()) == Some("main")
                && r.get("table_name").and_then(|v| v.as_str()) == Some("alpha")
        });
    assert!(
        src_row.is_some(),
        "RV-DB-6540DC7B: the source catalogue row ('testdb'/'main'/'alpha') \
         must survive the failed rename"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-6540DC7B — create/remove lifecycle propagation
// ═══════════════════════════════════════════════════════════════════════

// rename_repo_as's first catalogue write on the path is the destination
// save_repository (its preamble — binding guard, existence checks, the
// in-memory list_tables and the load_repository_record — is reads only),
// so arm(1) fails exactly that save. The countdown is one-shot: the save
// consumes it and the gate disarms, so under the pre-fix warn+continue the
// subsequent gated remove_repository SUCCEEDS and deletes the 'main' row —
// the source-row assertion below therefore ALSO fails on reverted code,
// pinning the write-new-before-remove-old order (not just propagation).
#[tokio::test]
async fn rv_db_6540dc7b_rename_repo_propagates_catalogue_save_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("t").await.expect("create_db");

    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("main", ddl::create_repo("main").tables(["alpha"]));
    shamir
        .execute(
            "t",
            &batch
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .expect("create_repo must succeed");
    assert!(
        shamir
            .system_store()
            .load_repository_record("t", "main")
            .await
            .unwrap()
            .is_some(),
        "precondition: durable repo row must exist for 't'/'main'"
    );

    shamir.system_store().arm_catalogue_write_fault(1);
    let result = shamir
        .rename_repo_as("t", "main", "main2", Actor::System)
        .await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: rename_repo must propagate the injected save_repository failure"
    );
    assert!(
        shamir
            .system_store()
            .load_repository_record("t", "main")
            .await
            .unwrap()
            .is_some(),
        "RV-DB-6540DC7B: the source catalogue row ('t'/'main') must survive the failed rename"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_create_db_checked_propagates_and_rolls_back() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // save_database is the first (and only) catalogue write on the
    // create_db_as_checked path; the fixed body persists BEFORE publishing
    // the live registration, so a failure leaves nothing registered.
    shamir.system_store().arm_catalogue_write_fault(1);

    let result = shamir.create_db_as_checked("ghost", Actor::System).await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: create_db_as_checked must propagate the injected \
         save_database failure"
    );
    assert!(
        !shamir.has_db("ghost"),
        "the live registration must be rolled back when the durable write fails"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_remove_db_propagates_catalogue_removal_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Create BEFORE arming: the creation's save_database must not consume
    // the fault.
    shamir.create_db("victim").await.expect("create_db");
    assert!(
        shamir.has_db("victim"),
        "precondition: 'victim' must be registered before the drop"
    );

    // remove_database is the first catalogue write on the remove_db path
    // (`ensure_destinations_unbound` only READS subscription rows and the
    // quarantine cache); arm(1) fails exactly that removal.
    shamir.system_store().arm_catalogue_write_fault(1);

    let result = shamir.remove_db("victim").await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: remove_db must propagate the injected \
         remove_database failure instead of reporting Ok(true)"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_wire_create_db_reports_persist_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Session db for the batch: must exist, and its creation happens
    // BEFORE arming so it cannot consume the fault.
    shamir.create_db("router").await.expect("create_db");

    // `handle_create_db` routes through create_db_as_checked; save_database
    // is its first catalogue write (the authorize/exists probes are
    // read-only), so arm(1) fails the durable creation.
    shamir.system_store().arm_catalogue_write_fault(1);

    let mut b = Batch::new();
    b.id(1);
    b.create_db("cd", ddl::create_db("ghost"));
    let err = shamir
        .execute(
            "router",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .expect_err(
            "RV-DB-6540DC7B: wire CREATE DATABASE must fail when the durable \
             save fails",
        );
    match err {
        crate::query::batch::BatchError::QueryError { code, message, .. } => {
            assert_eq!(
                code.as_deref(),
                Some("persist_failed"),
                "expected persist_failed code, got code={code:?}, msg={message}"
            );
        }
        other => panic!("expected QueryError, got {other:?}"),
    }
    assert!(
        !shamir.has_db("ghost"),
        "no live registration may survive a failed wire CREATE DATABASE"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-6540DC7B — the ORIGINAL creation APIs propagate too
// ═══════════════════════════════════════════════════════════════════════
//
// `create_db` / `create_db_as` now delegate to `create_db_as_checked`.
// save_database is the first (and only) catalogue write on the path, so
// arm(1) fails exactly the durable creation. Under the pre-fix warn+
// continue body (register live, log warn, return Ok(db)) both methods
// returned Ok and left a live registration — so the `is_err()` and
// `has_db` assertions below fail on the reverted original implementation
// while the checked-method and rename tests above stay green.

#[tokio::test]
async fn rv_db_6540dc7b_create_db_propagates_and_leaves_no_live_registration() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.system_store().arm_catalogue_write_fault(1);

    let result = shamir.create_db("ghost").await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: create_db must propagate the injected save_database failure"
    );
    assert!(
        !shamir.has_db("ghost"),
        "no live registration may survive a failed original-API creation"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_create_db_as_propagates_and_leaves_no_live_registration() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.system_store().arm_catalogue_write_fault(1);

    let result = shamir.create_db_as("ghost", Actor::System).await;
    assert!(
        result.is_err(),
        "RV-DB-6540DC7B: create_db_as must propagate the injected save_database failure"
    );
    assert!(
        !shamir.has_db("ghost"),
        "no live registration may survive a failed original-API creation"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-6540DC7B — artifact drop propagates catalogue removal failure
// ═══════════════════════════════════════════════════════════════════════
//
// drop_function_as / drop_validator_as must PROPAGATE catalogue removal
// failures instead of warn+continue, and must perform the durable removal
// BEFORE the live registry removal, so a PREWRITE removal failure leaves
// the live binding intact. (A remove that commits and then fails at flush
// is a different, post-commit case — no restart-visibility claim is made
// here.) The remove_function/remove_validator delete is gated and is the
// FIRST and ONLY catalogue write of the drop, so arm(1) fails it at the
// prewrite gate in BOTH versions: the durable-row assertions below pass
// either way, and only the Err and live-registry assertions discriminate
// fixed vs reverted (warn+continue, live-removal-first) code.

#[tokio::test]
async fn rv_db_6540dc7b_drop_function_propagates_catalogue_removal_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Native live registration (register_fn writes NO catalogue row), plus
    // the durable catalogue row a persisted user function carries.
    shamir
        .register_fn("uf", false, |_: &FnCtx, _: &FnBatch, _: &Params| async {
            Ok(QueryValue::Null)
        })
        .expect("native function registration must succeed");
    let mut rec = new_map();
    rec.insert("name".to_string(), QueryValue::Str("uf".to_string()));
    shamir
        .system_store()
        .save_function(
            "uf",
            &QueryValue::Map(rec),
            &ResourceMeta::owned_enforced(Actor::System),
        )
        .await
        .expect("save_function must succeed");
    assert!(
        shamir
            .system_store()
            .load_functions()
            .await
            .unwrap()
            .iter()
            .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("uf")),
        "precondition: durable function row must exist for 'uf'"
    );
    assert!(
        shamir.functions().contains("uf"),
        "precondition: live registry must contain 'uf'"
    );

    // remove_function is the first catalogue write on the fixed drop path.
    shamir.system_store().arm_catalogue_write_fault(1);

    match shamir.drop_function_as("uf", Actor::System).await {
        Err(crate::DbError::Internal(msg)) => assert!(
            msg.contains("injected catalogue write failure"),
            "expected the injected catalogue-write-fault error, got: {msg}"
        ),
        other => panic!(
            "RV-DB-6540DC7B: drop_function_as must propagate the injected \
             remove_function failure as DbError::Internal, got {other:?}"
        ),
    }

    // The durable row must survive: the gated delete errors BEFORE deleting.
    let row = shamir
        .system_store()
        .load_functions()
        .await
        .expect("load_functions must not fail")
        .into_iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("uf"));
    assert!(
        row.is_some(),
        "RV-DB-6540DC7B: the durable 'uf' row must survive the failed drop"
    );

    // The live binding must survive: durable removal runs BEFORE the live
    // registry removal, so the failing write precedes any in-memory change.
    assert!(
        shamir.functions().contains("uf"),
        "RV-DB-6540DC7B: the live 'uf' binding must survive the failed drop"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_drop_validator_propagates_catalogue_removal_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir
        .register_native_validator(
            "uv",
            false,
            |_new: Option<&dyn RecordFields>,
             _prev: Option<&dyn RecordFields>,
             _ctx: &ValidatorCtx<'_>| { Validation::accept() },
        )
        .await
        .expect("native validator registration must succeed");
    assert!(
        shamir
            .system_store()
            .load_validator("uv")
            .await
            .unwrap()
            .is_some(),
        "precondition: durable validator row must exist for 'uv'"
    );
    assert!(
        shamir.validator_id("uv").is_some(),
        "precondition: live registry must contain 'uv'"
    );

    // remove_validator is the first catalogue write on the fixed drop path
    // (the bound check before it only touches the live registry).
    shamir.system_store().arm_catalogue_write_fault(1);

    match shamir.drop_validator_as("uv", Actor::System).await {
        Err(crate::DbError::Internal(msg)) => assert!(
            msg.contains("injected catalogue write failure"),
            "expected the injected catalogue-write-fault error, got: {msg}"
        ),
        other => panic!(
            "RV-DB-6540DC7B: drop_validator_as must propagate the injected \
             remove_validator failure as DbError::Internal, got {other:?}"
        ),
    }

    // The durable row must survive the failed drop.
    let row = shamir
        .system_store()
        .load_validator("uv")
        .await
        .expect("load_validator must not fail");
    assert!(
        row.is_some(),
        "RV-DB-6540DC7B: the durable 'uv' row must survive the failed drop"
    );

    // The live binding must survive: durable removal runs BEFORE the live
    // registry removal.
    assert!(
        shamir.validator_id("uv").is_some(),
        "RV-DB-6540DC7B: the live 'uv' binding must survive the failed drop"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RV-DB-6540DC7B — duplicate create_db_as_checked is idempotent
// ═══════════════════════════════════════════════════════════════════════
//
// The buggy checked body created a NEW DbInstance, OVERWROTE the existing
// registration, and on a failing catalogue save called `dbs.remove(name)` —
// deleting the ORIGINAL database's live state. The fixed body returns the
// original instance and attempts NO catalogue write for an already-live
// name (the replication-control lock serializes the check against the
// rename/drop lifecycle, so the same-instance check cannot race a
// concurrent drop between the check and the early return).
//
// Fault trace: arm(1) is NEVER consumed by the duplicate (no write), so
// the FRESH creation below fails at the prewrite gate — proving the gate
// was still armed AND that the duplicate skipped persistence. Under the
// buggy body the duplicate returns Err (save fails) and `orig` is gone —
// every assertion below fails.
#[tokio::test]
async fn rv_db_6540dc7b_checked_duplicate_returns_original_and_skips_persist() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("orig").await.expect("original create_db");

    // Live probe repo added DIRECTLY to the original instance (no
    // unrelated catalogue writes — DbInstance.add_repo is in-memory only).
    let original = shamir.get_db("orig").expect("original live instance");
    original
        .add_repo(crate::engine::repo::RepoConfig::new(
            "probe",
            crate::engine::repo::BoxRepoFactory::in_memory(),
        ))
        .await
        .expect("probe repo on original instance");
    let original_catalogue = shamir
        .system_store()
        .load_database("orig")
        .await
        .expect("load_database must not fail")
        .expect("durable 'orig' row");

    // The duplicate uses a DIFFERENT actor: it must not re-stamp ownership.
    shamir.system_store().arm_catalogue_write_fault(1);

    let again = shamir
        .create_db_as_checked("orig", Actor::Admin(4242))
        .await
        .expect("RV-DB-6540DC7B: duplicate creation must be idempotent");
    assert!(
        Arc::ptr_eq(again.scalars(), original.scalars())
            && again.has_repo("probe")
            && again.repo_count() == original.repo_count(),
        "RV-DB-6540DC7B: the duplicate must return the ORIGINAL live \
         instance (probe repo + scalar layer intact), not a fresh one"
    );
    let row = shamir
        .system_store()
        .load_database("orig")
        .await
        .expect("load_database must not fail")
        .expect("durable 'orig' row must survive the duplicate");
    // Catalogue unchanged: same durable owner (ResourceMeta is PartialEq)
    // and same created_at (a duplicate write would re-stamp a new one).
    let created_at = |r: &QueryValue| r.as_object().and_then(|m| m.get("created_at")).cloned();
    assert_eq!(
        created_at(&row),
        created_at(&original_catalogue),
        "RV-DB-6540DC7B: no catalogue write may be attempted for an \
         already-live name (created_at unchanged)"
    );
    assert_eq!(
        ResourceMeta::from_record(&row),
        ResourceMeta::from_record(&original_catalogue),
        "RV-DB-6540DC7B: no catalogue write may be attempted for an \
         already-live name (durable owner unchanged despite the \
         different actor)"
    );

    // The fault was NOT consumed by the duplicate: a FRESH creation fails
    // immediately at the prewrite gate, leaves no live registration, and
    // the original stays reachable.
    assert!(
        shamir
            .create_db_as_checked("fresh", Actor::System)
            .await
            .is_err(),
        "the armed fault must still be armed after the duplicate"
    );
    assert!(!shamir.has_db("fresh"), "no live 'fresh' registration");
    let original_now = shamir.get_db("orig").expect("'orig' still live");
    assert!(
        original_now.has_repo("probe"),
        "RV-DB-6540DC7B: the original 'orig' state (probe repo) must \
         survive the failed duplicate; the buggy body deleted it"
    );
}
