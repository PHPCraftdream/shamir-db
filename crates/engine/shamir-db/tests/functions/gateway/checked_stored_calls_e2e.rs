use shamir_db::shamir_db::{FunctionSource, SystemStoreConfig};
use shamir_db::ShamirDb;
use shamir_engine::function::{
    check_shamir_source, CompileOptions, CreateFunctionOptions, Params, ShamirStage, TierMode,
    WasmEngine, WasmLimits,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

const LEAF: &str = r#"@entry procedure leaf(n: i64) -> { answer: i64 } {
    return { answer: n + 1 };
}"#;
const CALLER: &str = r#"@entry procedure caller(n: i64) -> i64 {
    let result: { answer: i64 } = call("leaf", { n: n });
    return result.answer;
}"#;
const CALLER_STRING: &str = r#"@entry procedure caller_string(n: string) -> i64 {
    let result: { answer: i64 } = call("leaf", { n: n });
    return result.answer;
}"#;
const NULLABLE_LEAF: &str = r#"@entry procedure nullable_leaf() -> i64? {
    return null;
}"#;
const NULLABLE_CALLER: &str = r#"@entry procedure nullable_caller() -> i64? {
    let result: i64? = call("nullable_leaf", {});
    return result;
}"#;

fn push_leb128(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = u8::try_from(value & 0x7f).expect("7 bits");
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn append_custom_section(bytes: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
    let mut section = Vec::new();
    push_leb128(&mut section, name.len());
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(payload);
    let mut out = bytes.to_vec();
    out.push(0);
    push_leb128(&mut out, section.len());
    out.extend_from_slice(&section);
    out
}

async fn create(db: &ShamirDb, name: &str, source: &str, replace: bool) {
    db.create_function_with_opts(
        name,
        FunctionSource::Shamir(source),
        CreateFunctionOptions {
            replace,
            ..CreateFunctionOptions::default()
        },
    )
    .await
    .unwrap_or_else(|error| panic!("create {name}: {error}"));
}

async fn create_as(
    db: &ShamirDb,
    name: &str,
    source: &str,
    owner: Actor,
    security: shamir_engine::function::Security,
) {
    db.create_function_with_opts_as(
        name,
        FunctionSource::Shamir(source),
        CreateFunctionOptions {
            security,
            ..CreateFunctionOptions::default()
        },
        Actor::System,
    )
    .await
    .unwrap_or_else(|error| panic!("create {name}: {error}"));
    db.set_resource_meta(
        &ResourcePath::function(name),
        &ResourceMeta {
            owner,
            group: None,
            mode: 0o711,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("set owner for {name}: {error}"));
}

async fn create_pair(db: &ShamirDb) {
    create(db, "leaf", LEAF, false).await;
    create(db, "caller", CALLER, false).await;
}

async fn invoke(db: &ShamirDb, value: i64) -> Result<QueryValue, shamir_db::DbError> {
    let mut params = Params::new();
    params.set("n", QueryValue::Int(value));
    db.invoke_function("caller", params).await
}

async fn invoke_without_args(db: &ShamirDb, name: &str) -> Result<QueryValue, shamir_db::DbError> {
    db.invoke_function(name, Params::new()).await
}

#[tokio::test]
async fn checked_call_passes_arguments_and_projects_record_result() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;

    let stored = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    let signature_before = match stored.get("call_signature") {
        Some(QueryValue::Bin(signature)) => signature.clone(),
        _ => panic!("stored signature bytes"),
    };
    assert_eq!(
        signature_before.as_slice(),
        b"procedure(n:i64)->{answer:i64}"
    );
    assert_eq!(
        stored.get("lang").and_then(QueryValue::as_str),
        Some("shamir")
    );
    let stored_hash = stored
        .get("module_hash")
        .and_then(QueryValue::as_str)
        .expect("module hash");
    let bytes = db
        .export_function("leaf")
        .await
        .expect("artifact export")
        .bytes;
    assert_eq!(
        stored_hash,
        shamir_engine::function::ModuleHash::of(&bytes).to_hex()
    );
    assert_eq!(
        invoke(&db, 41).await.expect("checked call"),
        QueryValue::Int(42)
    );
}

#[tokio::test]
async fn checked_nested_execute_uses_the_parent_effective_actor() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    db.set_resource_meta(
        &ResourcePath::function("caller"),
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant caller Execute");
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(1001),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("deny restricted actor Execute on leaf");
    let mut params = Params::new();
    params.set("n", QueryValue::Int(41));

    let error = db
        .invoke_function_as("caller", params.clone(), Actor::User(2002))
        .await
        .expect_err("nested target Execute must be denied");
    assert!(error.to_string().contains("EXECUTE"), "{error}");

    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    let mut changed = row.clone();
    if let QueryValue::Map(fields) = &mut changed {
        fields.insert(
            "call_signature".into(),
            QueryValue::Bin(b"procedure(n:string)->{answer:i64}".to_vec()),
        );
    }
    db.system_store()
        .save_function("leaf", &changed, &ResourceMeta::from_record(&row))
        .await
        .expect("change only the persisted expected signature");
    let wrong_signature_error = db
        .invoke_function_as("caller", params.clone(), Actor::User(2002))
        .await
        .expect_err("Execute denial takes precedence over a wrong signature");
    assert_eq!(wrong_signature_error.to_string(), error.to_string());

    let mut changed = row.clone();
    if let QueryValue::Map(fields) = &mut changed {
        fields.insert("lang".into(), QueryValue::Str("rust".into()));
    }
    db.system_store()
        .save_function("leaf", &changed, &ResourceMeta::from_record(&row))
        .await
        .expect("change target language while retaining the correct signature");
    let non_shamir_error = db
        .invoke_function_as("caller", params.clone(), Actor::User(2002))
        .await
        .expect_err("Execute denial takes precedence over target-kind checks");
    assert_eq!(non_shamir_error.to_string(), error.to_string());
}

#[tokio::test]
async fn definer_parent_cannot_call_an_invoker_owned_by_another_actor() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_as(
        &db,
        "leaf",
        LEAF,
        Actor::User(2002),
        shamir_engine::function::Security::Invoker,
    )
    .await;
    create_as(
        &db,
        "caller",
        CALLER,
        Actor::User(1001),
        shamir_engine::function::Security::Definer,
    )
    .await;
    db.set_resource_meta(
        &ResourcePath::function("leaf"),
        &ResourceMeta {
            owner: Actor::User(2002),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant leaf Execute");
    db.set_resource_meta(
        &ResourcePath::function("caller"),
        &ResourceMeta {
            owner: Actor::User(1001),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .expect("grant caller Execute");
    let mut params = Params::new();
    params.set("n", QueryValue::Int(41));

    let error = db
        .invoke_function_as("caller", params, Actor::User(3003))
        .await
        .expect_err("Definer-to-Invoker owner mismatch must fail closed");
    assert!(
        error.to_string().contains("Definer cannot call Invoker"),
        "{error}"
    );
}

#[tokio::test]
async fn checked_call_preserves_nullable_results() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create(&db, "nullable_leaf", NULLABLE_LEAF, false).await;
    create(&db, "nullable_caller", NULLABLE_CALLER, false).await;

    assert_eq!(
        invoke_without_args(&db, "nullable_caller")
            .await
            .expect("checked nullable call"),
        QueryValue::Null
    );
}

#[tokio::test]
async fn checked_call_catalog_enforces_argument_types_during_compilation() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create(&db, "leaf", LEAF, false).await;
    let error = db
        .check_function(
            FunctionSource::Shamir(
                r#"@entry procedure wrong() -> i64 {
                    let result: { answer: i64 } = call("leaf", { n: "not an integer" });
                    return result.answer;
                }"#,
            ),
            shamir_engine::function::TierMode::Auto,
        )
        .await
        .expect_err("wrong argument type must fail compile-time checking");
    assert!(error.to_string().contains("T004"), "{error}");
    let wrong_name = db
        .check_function(
            FunctionSource::Shamir(
                r#"@entry procedure wrong_name(n: i64) -> i64 {
                    let result: { answer: i64 } = call("leaf", { wrong: n });
                    return result.answer;
                }"#,
            ),
            shamir_engine::function::TierMode::Auto,
        )
        .await
        .expect_err("wrong named argument must fail compilation");
    assert!(wrong_name.to_string().contains("T004"), "{wrong_name}");
}

#[tokio::test]
async fn checked_call_fails_when_replacement_changes_the_signature() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    let before = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row before replacement");
    let signature_before = before
        .get("call_signature")
        .expect("stored signature")
        .clone();
    create(
        &db,
        "leaf",
        r#"@entry procedure leaf(n: string) -> { answer: i64 } {
            return { answer: 1 };
        }"#,
        true,
    )
    .await;
    let after = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("replacement row");
    assert!(matches!(
        after.get("call_signature"),
        Some(QueryValue::Bin(signature))
            if signature.as_slice() == b"procedure(n:string)->{answer:i64}"
    ));
    assert_ne!(after.get("call_signature"), Some(&signature_before));

    let error = invoke(&db, 41).await.expect_err("signature mismatch");
    assert!(error.to_string().contains("signature"), "{error}");
}

#[tokio::test]
async fn checked_call_does_not_follow_a_renamed_target() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    let before = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row before rename");
    let signature_before = match before.get("call_signature") {
        Some(QueryValue::Bin(signature)) => signature.clone(),
        _ => panic!("stored signature bytes"),
    };
    db.rename_function("leaf", "renamed_leaf")
        .await
        .expect("rename procedure");
    let renamed = db
        .system_store()
        .load_function("renamed_leaf")
        .await
        .expect("catalogue read")
        .expect("renamed row");
    assert!(matches!(
        renamed.get("call_signature"),
        Some(QueryValue::Bin(signature)) if signature == &signature_before
    ));

    let error = invoke(&db, 41)
        .await
        .expect_err("old checked target missing");
    assert!(error.to_string().contains("leaf"), "{error}");
}

#[tokio::test]
async fn checked_call_refuses_native_target() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;

    // A valid old signature lets the caller compile; replacing the live entry
    // with a native body must not leave that persisted metadata trusted.
    db.register_fn("leaf", true, |_ctx, _batch, _params| async {
        Ok(QueryValue::Int(7))
    })
    .expect("replace with native function");
    let error = invoke(&db, 41).await.expect_err("native target rejected");
    assert!(error.to_string().contains("coherent"), "{error}");
}

#[tokio::test]
async fn checked_call_refuses_raw_wasm_target() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;

    // Raw WASM writes a different language tag and does not carry the prior
    // compiler signature forward.
    let raw = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "shamir_alloc") (param i32) (result i32) i32.const 0)
            (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0))"#,
    )
    .expect("valid raw module");
    let raw = append_custom_section(&raw, "shamir.abi", &1_u32.to_le_bytes());
    let raw = append_custom_section(&raw, "shamir.origin", b"external");
    db.create_function_from_wasm("leaf", &raw, true)
        .await
        .expect("replace with raw WASM");
    let raw_row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("raw WASM row");
    assert!(raw_row.get("call_signature").is_none());
    let error = invoke(&db, 41).await.expect_err("raw WASM target rejected");
    assert!(error.to_string().contains("coherent"), "{error}");
}

#[tokio::test]
async fn checked_call_refuses_signatureless_legacy_target() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;

    // A legacy ShamIR record may identify its language but have no compiler-
    // emitted trusted signature; checked resolution must fail closed.
    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    let mut legacy = row.clone();
    if let QueryValue::Map(fields) = &mut legacy {
        fields.shift_remove("call_signature");
    }
    db.system_store()
        .save_function("leaf", &legacy, &ResourceMeta::from_record(&row))
        .await
        .expect("replace with signatureless legacy row");
    let legacy = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("signatureless legacy row");
    assert!(legacy.get("call_signature").is_none());
    let error = invoke(&db, 41)
        .await
        .expect_err("signatureless legacy target rejected");
    assert!(error.to_string().contains("signature"), "{error}");
}

#[tokio::test]
async fn optimize_rebuild_retains_signature_with_unchanged_artifact() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create(&db, "leaf", LEAF, false).await;
    let before = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    let signature_before = before
        .get("call_signature")
        .expect("stored signature")
        .clone();

    db.alter_function_optimize("leaf", shamir_engine::function::TierMode::Never)
        .await
        .expect("rebuild existing artifact");
    let after = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("rebuilt leaf row");
    assert_eq!(after.get("call_signature"), Some(&signature_before));
    assert_eq!(
        after.get("module_hash"),
        before.get("module_hash"),
        "optimization rebuild keeps signature paired with unchanged bytes"
    );
}

#[tokio::test]
async fn callable_catalog_includes_a_row_published_before_its_registry_replacement() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    let prior_row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("prior leaf row");
    let replacement_db = ShamirDb::init_memory()
        .await
        .expect("second memory database");
    create(
        &replacement_db,
        "leaf",
        r#"@entry procedure leaf(n: string) -> { answer: i64 } {
            return { answer: 1 };
        }"#,
        false,
    )
    .await;
    create(&replacement_db, "caller_string", CALLER_STRING, false).await;
    let replacement = replacement_db
        .system_store()
        .load_function("leaf")
        .await
        .expect("replacement catalogue read")
        .expect("replacement leaf row");
    let replacement_signature = match replacement.get("call_signature") {
        Some(QueryValue::Bin(signature)) => signature.clone(),
        _ => panic!("replacement signature bytes"),
    };
    let mut options = CompileOptions {
        stage: ShamirStage::Procedures,
        ..CompileOptions::default()
    };
    options
        .callable_catalog
        .insert("leaf".to_string(), replacement_signature);
    check_shamir_source(
        Arc::new(WasmEngine::new().expect("WASM engine")),
        CALLER_STRING.to_string(),
        options,
        WasmLimits::default(),
        TierMode::Auto,
    )
    .await
    .expect("caller compiles against replacement's exact signature");
    db.system_store()
        .save_function("leaf", &replacement, &ResourceMeta::from_record(&prior_row))
        .await
        .expect("publish coherent replacement row before registry replacement");

    db.check_function(
        FunctionSource::Shamir(CALLER_STRING),
        shamir_engine::function::TierMode::Auto,
    )
    .await
    .expect("catalog includes the persisted signature without registry coherence checks");

    let error = invoke(&db, 41)
        .await
        .expect_err("the old captured artifact must not consume a newly published signature");
    assert!(
        error
            .to_string()
            .contains("function changed during resolution"),
        "{error}"
    );
}

#[tokio::test]
async fn checked_definer_target_uses_owner_table_rights_not_invoker_caller_rights() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    db.create_db("testdb").await.expect("create database");
    let mut create_repo = shamir_query_builder::batch::Batch::new();
    create_repo.create_repo(
        "repo",
        shamir_query_builder::ddl::create_repo("main")
            .engine("in_memory")
            .tables(["private_rows"]),
    );
    db.execute(
        "testdb",
        &create_repo
            .to_request_via_msgpack()
            .expect("create repo request"),
    )
    .await
    .expect("create private table");
    let open = ResourceMeta::open();
    db.set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .expect("open db ancestor");
    db.set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .expect("open repo ancestor");
    db.set_resource_meta(
        &ResourcePath::table("testdb", "main", "private_rows"),
        &ResourceMeta {
            owner: Actor::User(1001),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict table to owner");
    let leaf = r#"@entry procedure leaf() -> i64 {
        let row: { id: i64 } = db_insert("private_rows", { id: 17 });
        return row.id;
    }"#;
    let caller = r#"@entry procedure caller() -> i64 {
        return call("leaf", {});
    }"#;
    create_as(
        &db,
        "leaf",
        leaf,
        Actor::User(1001),
        shamir_engine::function::Security::Definer,
    )
    .await;
    create_as(
        &db,
        "caller",
        caller,
        Actor::User(1001),
        shamir_engine::function::Security::Invoker,
    )
    .await;

    assert_eq!(
        db.invoke_function_in_db_as("testdb", "main", "caller", Params::new(), Actor::User(2002))
            .await
            .expect("Invoker caller succeeds through nested Definer target"),
        QueryValue::Int(17)
    );
}

#[tokio::test]
async fn compiled_stored_call_cycle_is_rejected_at_repeated_identity() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create(
        &db,
        "cycle_a",
        "@entry procedure cycle_a() -> i64 { return 1; }",
        false,
    )
    .await;
    create(
        &db,
        "cycle_b",
        "@entry procedure cycle_b() -> i64 { let value: i64 = call(\"cycle_a\", {}); return value; }",
        false,
    )
    .await;
    create(
        &db,
        "cycle_a",
        "@entry procedure cycle_a() -> i64 { let value: i64 = call(\"cycle_b\", {}); return value; }",
        true,
    )
    .await;

    let error = db
        .invoke_function("cycle_a", Params::new())
        .await
        .expect_err("A -> B -> A repeats captured identity");
    assert!(error.to_string().contains("cycle"), "{error}");
}

async fn create_compiled_call_chain(db: &ShamirDb, count: usize) {
    let last = count - 1;
    create(
        db,
        &format!("chain_{last}"),
        &format!("@entry procedure chain_{last}() -> i64 {{ return 1; }}"),
        false,
    )
    .await;
    for index in (0..last).rev() {
        let name = format!("chain_{index}");
        let target = format!("chain_{}", index + 1);
        let source = format!(
            "@entry procedure {name}() -> i64 {{ let value: i64 = call(\"{target}\", {{}}); return value; }}"
        );
        create(db, &name, &source, false).await;
    }
}

#[tokio::test]
async fn compiled_call_chain_accepts_32_entries_and_rejects_33() {
    let at_limit = ShamirDb::init_memory().await.expect("memory database");
    create_compiled_call_chain(&at_limit, 32).await;
    assert_eq!(
        at_limit
            .invoke_function("chain_0", Params::new())
            .await
            .expect("32 entries including root are allowed"),
        QueryValue::Int(1)
    );

    let over_limit = ShamirDb::init_memory()
        .await
        .expect("second memory database");
    create_compiled_call_chain(&over_limit, 33).await;
    let error = over_limit
        .invoke_function("chain_0", Params::new())
        .await
        .expect_err("33 entries including root are rejected");
    assert!(error.to_string().contains("path limit"), "{error}");
}

#[tokio::test]
async fn checked_call_accepts_unchanged_catalog_row() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    db.system_store()
        .save_function("leaf", &row, &ResourceMeta::from_record(&row))
        .await
        .expect("save unchanged catalogue row");

    assert_eq!(
        invoke(&db, 41).await.expect("checked call"),
        QueryValue::Int(42)
    );
}

#[tokio::test]
async fn checked_stored_calls_survive_durable_reopen_and_reject_replaced_signatures() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("system.db");
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
            .await
            .expect("durable database");
        create_pair(&db).await;
    }
    {
        let db = crate::lifecycle::init_redb_retry(&path).await;
        assert_eq!(
            invoke(&db, 41).await.expect("call after reopen"),
            QueryValue::Int(42)
        );
        create(
            &db,
            "leaf",
            r#"@entry procedure leaf(n: string) -> { answer: i64 } {
                return { answer: 1 };
            }"#,
            true,
        )
        .await;
    }
    let db = crate::lifecycle::init_redb_retry(&path).await;
    let error = invoke(&db, 41)
        .await
        .expect_err("reopened replacement must reject the caller's persisted signature");
    assert!(error.to_string().contains("signature"), "{error}");
}

#[tokio::test]
async fn checked_call_rejects_catalog_row_with_mismatched_module_hash() {
    let db = ShamirDb::init_memory().await.expect("memory database");
    create_pair(&db).await;
    let row = db
        .system_store()
        .load_function("leaf")
        .await
        .expect("catalogue read")
        .expect("leaf row");
    let mut changed = row.clone();
    if let QueryValue::Map(fields) = &mut changed {
        fields.insert(
            "module_hash".into(),
            QueryValue::Str("00000000000000000000000000000000".into()),
        );
    }
    // Direct catalogue mutation models a publication window where metadata
    // has advanced but the captured registry artifact has not.
    db.system_store()
        .save_function("leaf", &changed, &ResourceMeta::from_record(&row))
        .await
        .expect("publish row with mismatched module hash");

    let error = invoke(&db, 41)
        .await
        .expect_err("catalogue row does not match the captured artifact");
    assert!(error.to_string().contains("coherent"), "{error}");
}
