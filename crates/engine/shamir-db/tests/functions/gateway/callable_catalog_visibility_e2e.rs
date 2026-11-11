use shamir_db::shamir_db::FunctionSource;
use shamir_db::{DbError, ShamirDb};
use shamir_engine::function::{CreateFunctionOptions, Params, TierMode};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

const OWNER: u64 = 1001;
const CALLER: u64 = 2002;
const MEMBER: u64 = 3003;
const UNAVAILABLE: &str = "callable procedure is unavailable";
const LEAF: &str = r#"@entry procedure leaf(n: i64) -> { answer: i64 } {
    return { answer: n + 1 };
}"#;
const CALL: &str = r#"@entry procedure caller(n: i64) -> i64 {
    let result: { answer: i64 } = call("leaf", { n: n });
    return result.answer;
}"#;
const COMMENTED_CALL: &str = r#"@entry procedure caller(n: i64) -> i64 {
    let result: { answer: i64 } = call /*comment*/ ("leaf", { n: n });
    return result.answer;
}"#;
const NEWLINE_CALL: &str = r#"@entry procedure caller(n: i64) -> i64 {
    let result: { answer: i64 } = call
        ("leaf", { n: n });
    return result.answer;
}"#;
const MISSING_CALL: &str = r#"@entry procedure caller(n: i64) -> i64 {
    let result: { answer: i64 } = call("void", { n: n });
    return result.answer;
}"#;

async fn fixture() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("memory database");
    db.set_resource_meta(
        &ResourcePath::FunctionNamespace,
        &ResourceMeta {
            owner: Actor::User(CALLER),
            group: None,
            mode: 0o777,
        },
    )
    .await
    .expect("grant namespace Create");
    db.set_resource_meta(
        &ResourcePath::WasmCompiler,
        &ResourceMeta {
            owner: Actor::User(CALLER),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant caller compiler Execute");
    db.create_function_with_opts_as(
        "leaf",
        FunctionSource::Shamir(LEAF),
        CreateFunctionOptions::default(),
        Actor::System,
    )
    .await
    .expect("create stored target");
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict target Execute to owner");
    db
}

async fn check(db: &ShamirDb, actor: Actor, source: &'static str) -> Result<(), DbError> {
    db.check_function_as(actor, FunctionSource::Shamir(source), TierMode::Auto)
        .await
        .map(|_| ())
}

fn assert_unavailable(error: &DbError) {
    assert!(error.to_string().contains(UNAVAILABLE), "{error}");
}

async fn save_raw_function_row(db: &ShamirDb, name: &str, row: &QueryValue) {
    db.system_store()
        .remove_function(name)
        .await
        .expect("remove prior function row before exact raw replacement");
    db.system_store()
        .save_function_meta_record(name, row)
        .await
        .expect("insert exact raw function row");
    let saved = db
        .system_store()
        .load_function(name)
        .await
        .expect("read exact raw function row")
        .expect("raw function row remains stored");
    assert_eq!(&saved, row, "raw row must match every field exactly");
}

async fn assert_visible_and_invocable(db: &ShamirDb, actor: Actor, caller_name: &str) {
    check(db, actor.clone(), CALL)
        .await
        .expect("actor can compile a call to the visible target");
    db.create_function_with_opts_as(
        caller_name,
        FunctionSource::Shamir(CALL),
        CreateFunctionOptions::default(),
        actor.clone(),
    )
    .await
    .expect("store caller after visible signature compilation");
    let mut params = Params::new();
    params.set("n", QueryValue::Int(41));
    assert_eq!(
        db.invoke_function_as(caller_name, params, actor)
            .await
            .expect("nested invocation succeeds for the actor"),
        QueryValue::Int(42)
    );
}

#[tokio::test]
async fn hidden_and_missing_targets_are_opaque_and_execute_right_is_rechecked() {
    let db = fixture().await;
    let hidden_check = check(&db, Actor::User(CALLER), CALL)
        .await
        .expect_err("caller cannot compile a call to the restricted target");
    let missing_check = check(&db, Actor::User(CALLER), MISSING_CALL)
        .await
        .expect_err("missing target is unavailable");
    assert_eq!(hidden_check.to_string(), missing_check.to_string());
    assert_unavailable(&hidden_check);

    let hidden_create = db
        .create_function_with_opts_as(
            "caller_hidden",
            FunctionSource::Shamir(CALL),
            CreateFunctionOptions::default(),
            Actor::User(CALLER),
        )
        .await
        .expect_err("create cannot compile a call to the restricted target");
    let missing_create = db
        .create_function_with_opts_as(
            "caller_missing",
            FunctionSource::Shamir(MISSING_CALL),
            CreateFunctionOptions::default(),
            Actor::User(CALLER),
        )
        .await
        .expect_err("create cannot compile a call to a missing target");
    assert_eq!(hidden_create.to_string(), missing_create.to_string());
    assert_unavailable(&hidden_create);

    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o600,
        },
    )
    .await
    .expect("clear owner and other Execute for administrator-bypass checks");
    check(&db, Actor::User(OWNER), CALL)
        .await
        .expect_err("owner is denied while its Execute bit is clear");
    check(&db, Actor::Admin(88), CALL)
        .await
        .expect("administrator bypasses target Execute");
    check(&db, Actor::System, CALL)
        .await
        .expect("system bypasses target Execute");
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("enable owner Execute");
    check(&db, Actor::User(OWNER), CALL)
        .await
        .expect("owner can see its signature through owner Execute");

    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant other Execute");
    check(&db, Actor::User(CALLER), CALL)
        .await
        .expect("granted caller can compile the same source");
    db.create_function_with_opts_as(
        "caller",
        FunctionSource::Shamir(CALL),
        CreateFunctionOptions::default(),
        Actor::User(CALLER),
    )
    .await
    .expect("store caller after Execute grant");
    let mut params = Params::new();
    params.set("n", QueryValue::Int(41));
    assert_eq!(
        db.invoke_function_as("caller", params.clone(), Actor::User(CALLER))
            .await
            .expect("stored call succeeds while Execute is granted"),
        QueryValue::Int(42)
    );

    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("revoke other Execute");
    let fresh = check(&db, Actor::User(CALLER), CALL)
        .await
        .expect_err("fresh compilation must observe revoked Execute");
    assert_unavailable(&fresh);
    let runtime = db
        .invoke_function_as("caller", params, Actor::User(CALLER))
        .await
        .expect_err("stored nested call must recheck Execute at runtime");
    let unavailable = check(&db, Actor::User(CALLER), MISSING_CALL)
        .await
        .expect_err("missing nested runtime target diagnostic oracle");
    let execute_refusal = runtime.to_string();
    assert!(execute_refusal.contains("EXECUTE"), "{execute_refusal}");
    assert!(execute_refusal.contains("leaf"), "{execute_refusal}");
    assert!(!execute_refusal.contains(UNAVAILABLE), "{execute_refusal}");
    assert!(
        unavailable.to_string().contains(UNAVAILABLE),
        "{unavailable}"
    );
}

async fn corrupt_and_assert_excluded(
    db: &ShamirDb,
    original: &QueryValue,
    field: &str,
    value: Option<QueryValue>,
) {
    let mut changed = original.clone();
    if let QueryValue::Map(fields) = &mut changed {
        match value {
            Some(value) => {
                fields.insert(field.to_string(), value);
            }
            None => {
                fields.shift_remove(field);
            }
        }
    }
    save_raw_function_row(db, "leaf", &changed).await;
    let admin_error = check(db, Actor::Admin(99), CALL)
        .await
        .expect_err("malformed row is never exposed, even to an administrator");
    let missing_error = check(db, Actor::Admin(99), MISSING_CALL)
        .await
        .expect_err("missing target remains unavailable");
    assert_eq!(admin_error.to_string(), missing_error.to_string());
    assert_unavailable(&admin_error);
}

#[tokio::test]
async fn comments_and_newlines_between_call_intrinsic_and_parenthesis_execute() {
    let db = fixture().await;
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant caller Execute");

    for (name, source) in [
        ("commented_call", COMMENTED_CALL),
        ("newline_call", NEWLINE_CALL),
    ] {
        check(&db, Actor::User(CALLER), source)
            .await
            .expect("granted actor compiles whitespace-separated call intrinsic");
        db.create_function_with_opts_as(
            name,
            FunctionSource::Shamir(source),
            CreateFunctionOptions::default(),
            Actor::User(CALLER),
        )
        .await
        .expect("store whitespace-separated call caller");
        let mut params = Params::new();
        params.set("n", QueryValue::Int(41));
        assert_eq!(
            db.invoke_function_as(name, params, Actor::User(CALLER))
                .await
                .expect("stored whitespace-separated call executes"),
            QueryValue::Int(42)
        );
    }
}

#[tokio::test]
async fn malformed_function_security_and_owner_metadata_are_excluded_for_admin() {
    let db = fixture().await;
    let original = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("read target row")
        .expect("target row");
    for (field, value) in [
        ("security", Some(QueryValue::Str("broken".into()))),
        ("security", None),
        ("visibility", Some(QueryValue::Str("broken".into()))),
        ("visibility", None),
        ("secret_grants", None),
        (
            "secret_grants",
            Some(QueryValue::List(vec![QueryValue::Int(1)])),
        ),
        ("net_grants", None),
        ("metadata_version", None),
        ("metadata_version", Some(QueryValue::Int(2))),
        ("owner", None),
        ("owner", Some(QueryValue::Str("broken".into()))),
        ("owner", Some(QueryValue::Int(-1))),
    ] {
        corrupt_and_assert_excluded(&db, &original, field, value).await;
    }
}

#[tokio::test]
async fn non_shamir_scalar_and_signatureless_rows_share_unavailable_diagnostic() {
    let db = fixture().await;
    let missing = check(&db, Actor::System, MISSING_CALL)
        .await
        .expect_err("missing target diagnostic oracle");

    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("read target row")
        .expect("target row");
    let mut non_shamir = row.clone();
    if let QueryValue::Map(fields) = &mut non_shamir {
        fields.insert("lang".into(), QueryValue::Str("rust".into()));
    }
    save_raw_function_row(&db, "leaf", &non_shamir).await;
    let non_shamir_error = check(&db, Actor::System, CALL)
        .await
        .expect_err("non-shamir row is excluded");
    assert_eq!(non_shamir_error.to_string(), missing.to_string());

    let mut signatureless = non_shamir;
    if let QueryValue::Map(fields) = &mut signatureless {
        fields.shift_remove("call_signature");
        fields.insert("lang".into(), QueryValue::Str("shamir".into()));
    }
    save_raw_function_row(&db, "leaf", &signatureless).await;
    let signatureless_error = check(&db, Actor::System, CALL)
        .await
        .expect_err("signatureless row is excluded");
    assert_eq!(signatureless_error.to_string(), missing.to_string());

    db.create_function_with_opts_as(
        "leaf",
        FunctionSource::Shamir("@entry scalar leaf(n: i64) -> i64 { return n; }"),
        CreateFunctionOptions {
            replace: true,
            ..CreateFunctionOptions::default()
        },
        Actor::System,
    )
    .await
    .expect("replace with scalar shamir artifact");
    let scalar_error = check(&db, Actor::System, CALL)
        .await
        .expect_err("scalar entry has no callable procedure signature");
    assert_eq!(scalar_error.to_string(), missing.to_string());
    assert_unavailable(&scalar_error);
}

#[tokio::test]
async fn definer_target_does_not_replace_the_compiling_creator_actor() {
    let db = fixture().await;
    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("read target row")
        .expect("target row");
    let mut definer = row;
    if let QueryValue::Map(fields) = &mut definer {
        fields.insert("security".into(), QueryValue::Str("definer".into()));
    }
    save_raw_function_row(&db, "leaf", &definer).await;

    let denied = check(&db, Actor::User(CALLER), CALL)
        .await
        .expect_err("target Definer ownership must not grant Execute to the checker");
    assert_unavailable(&denied);
    let denied_create = db
        .create_function_with_opts_as(
            "definer_caller",
            FunctionSource::Shamir(CALL),
            CreateFunctionOptions::default(),
            Actor::User(CALLER),
        )
        .await
        .expect_err("target Definer ownership must not grant Execute to the creator");
    assert_unavailable(&denied_create);
    check(&db, Actor::Admin(88), CALL)
        .await
        .expect("Admin still sees the Definer target signature");
}

#[tokio::test]
async fn resource_metadata_shape_matches_runtime_visibility_and_invocation() {
    let db = fixture().await;
    let original = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("read target row")
        .expect("target row");

    let mut legacy = original.clone();
    if let QueryValue::Map(fields) = &mut legacy {
        fields.shift_remove("mode");
        fields.shift_remove("group");
    }
    save_raw_function_row(&db, "leaf", &legacy).await;
    assert_visible_and_invocable(&db, Actor::User(CALLER), "legacy_meta_caller").await;

    let mut invalid_group = original.clone();
    if let QueryValue::Map(fields) = &mut invalid_group {
        fields.insert("mode".into(), QueryValue::Int(0o711));
        fields.insert("group".into(), QueryValue::Str("not-a-group-id".into()));
    }
    save_raw_function_row(&db, "leaf", &invalid_group).await;
    assert_visible_and_invocable(&db, Actor::User(CALLER), "invalid_group_caller").await;

    let mut invalid_mode = original;
    if let QueryValue::Map(fields) = &mut invalid_mode {
        fields.insert("mode".into(), QueryValue::Int(65536));
    }
    save_raw_function_row(&db, "leaf", &invalid_mode).await;
    let denied = check(&db, Actor::User(CALLER), CALL)
        .await
        .expect_err("invalid mode falls back to no rights for a user");
    assert_unavailable(&denied);
    assert_visible_and_invocable(&db, Actor::Admin(99), "invalid_mode_admin_caller").await;
}

#[tokio::test]
async fn group_execute_permission_makes_a_callable_signature_visible() {
    let db = fixture().await;
    let group = db.create_group("callers").await.expect("create group");
    db.add_group_member(group, MEMBER)
        .await
        .expect("add group member");
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(OWNER),
            group: Some(group),
            mode: 0o070,
        },
    )
    .await
    .expect("grant group Execute");
    check(&db, Actor::User(MEMBER), CALL)
        .await
        .expect("group member can compile against the procedure signature");
}
