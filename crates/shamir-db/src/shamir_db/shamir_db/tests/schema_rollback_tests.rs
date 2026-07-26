//! F-24 (#817) regression tests: undo a fresh in-memory validator
//! registration when `ShamirDb::compile_table_schema` fails AFTER the
//! `ValidatorRegistry::register` step but before the function returns `Ok`.
//!
//! ## The bug
//!
//! `compile_table_schema` registers the schema validator early (its first
//! side effect) and only afterwards resolves the table, binds the validator
//! to it, and records the global `bound_in` reference. Before F-24, a failure
//! in any of those LATER steps returned `Err` while leaving the freshly-
//! registered validator permanently orphaned in `ValidatorRegistry::by_id` /
//! `name_to_id`, under the canonical name
//! `__schema__/<db>/<repo>/<table>`. Nothing ever called
//! `ValidatorRegistry::remove` for it. The caller (`admin_schema.rs`) rolls
//! the CATALOGUE back to `rec_prev` (correct, already tested in F-4's e2e
//! suite), but had no way to know an in-memory registration also needed
//! undoing.
//!
//! ## Escalation chain (investigated + confirmed; see the corrected severity)
//!
//! For a table's FIRST schema declaration that failed as above, a retry of
//! the same DDL call mints a NEW `schema_validator_id` (the rolled-back
//! catalogue has no `SCHEMA_VALIDATOR_ID_FIELD`, so `admin_schema.rs` calls
//! `RecordId::new()` again). `compile_table_schema` then sees
//! `id_for_name(&name).is_some()` (the orphan's id) and takes the
//! `replace_artifact(&new_id, ...)` branch — but `replace_artifact` only
//! mutates `by_id` for the id it is given, which does not exist yet, so it
//! silently returns `false` (a no-op), and that return value was never
//! checked. The retry then proceeds through `get_table` /
//! `add_validator_binding` / `add_binding` using the NEW id, all of which
//! succeed, and the catalogue ends up recording the NEW id as the active
//! schema validator — while `by_id` has NO entry for it (only the stale
//! orphan under the same name).
//!
//! **Corrected severity** (verified by reading the write-path validator
//! dispatch in `crates/shamir-engine/src/table/table_manager_validators.rs`):
//! this is NOT the "silent schema-validation bypass" a first-read of the
//! chain suggests. The main write-path gate, `run_validators_loop`, resolves
//! the bound validator via `reg.get_by_id(&binding.validator_id)
//! .ok_or(ValidatorFailure::Missing { .. })?` — i.e. it is FAIL-CLOSED on a
//! miss, surfacing as `DbError::ValidatorInvalid("validator <id> not found
//! in registry (fail-closed)")` and REJECTING the write. So the practical
//! impact of the escalation is an AVAILABILITY bug (the affected table
//! becomes permanently unwritable: every insert/update/delete is rejected
//! until the dangling catalogue reference is repaired), not silent wrong
//! data. (`schema_defaults` / `schema_transforms` do silently skip on a
//! `get_by_id` miss, but they run BEFORE the fail-closed validation gate, so
//! the write is rejected anyway and no silently-wrong row is persisted.)
//!
//! The F-24 fix eliminates the entire chain at the source: no orphan
//! survives the failed call, so a retry never encounters a stale
//! name collision. A secondary hardening additionally checks
//! `replace_artifact`'s return value so any FUTURE path that could recreate
//! a collision fails loudly.
//!
//! ## F-27b (#827) — the ALTER-path gap F-24 deliberately left open
//!
//! F-24 was scoped to the FRESH-registration case only: the ALTER branch
//! (`ValidatorRegistry::replace_artifact`, taken when `schema_validator_id`
//! already exists) was, by design, left un-undone on later failure, because
//! undoing a `replace_artifact` requires restoring the OLD compiled
//! validator, not just removing an entry.
//!
//! That left a real divergence: `replace_artifact` overwrites the LIVE
//! compiled validator with the NEW (attempted) rules the instant it runs —
//! well before `activation_result` is known. If a LATER step
//! (`get_table` / `add_validator_binding`) then fails, the caller
//! (`admin_schema.rs`) rolls the CATALOGUE back to `rec_prev` (the OLD
//! schema's data, unchanged `schema_validator_id`) — but the registry kept
//! enforcing the NEW (never-fully-activated) rules under that same id. The
//! catalogue said the OLD schema was active; the live enforcement gate
//! actually ran the NEW schema's rules.
//!
//! F-27b closes this: `compile_table_schema` now captures the live artifact
//! via `ValidatorRegistry::get_by_id` immediately before calling
//! `replace_artifact`, and restores it with a second `replace_artifact` call
//! if a later step fails — symmetric with the catalogue's `rec_prev`
//! rollback.

use std::sync::Arc;

use shamir_engine::validator::schema::SchemaValidator;
use shamir_engine::validator::RecordValidator;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

use super::super::schema_management::schema_validator_name;
use crate::ShamirDb;

/// An empty rule list is sufficient — these tests exercise the registry
/// lifecycle, not rule semantics. `SchemaValidator::new(vec![])` is a valid
/// always-passing validator (and is the shape the boot-pass materialises for a
/// table whose persisted schema list is empty).
fn no_rules() -> Vec<shamir_engine::validator::schema::FieldRule> {
    Vec::new()
}

/// Build a fresh in-memory `ShamirDb` with a single empty database `testdb`
/// and NO repos/tables. `get_table("testdb", "main", <anything>)` therefore
/// fails — the simplest already-available way to reach the
/// register-then-fail code path inside `compile_table_schema` (step B,
/// `get_table`, returns `Err` right after step A, `register`, succeeds) with
/// no I/O fault-injection seam.
async fn db_without_tables() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    shamir
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1 — core regression: a FRESH registration is undone when a later
// step fails, so no orphan survives in the registry.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fresh_register_is_undone_when_activation_fails() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "ghost_table");
    let id = RecordId::system("f24_fresh");

    // Precondition: nothing registered under this name yet.
    assert!(
        db.validators().id_for_name(&name).is_none(),
        "precondition: name must be free before the call"
    );

    // "ghost_table" does not exist → get_table (step B) fails right after the
    // fresh register (step A) succeeds.
    let result = db
        .compile_table_schema("testdb", "main", "ghost_table", id, no_rules())
        .await;
    assert!(result.is_err(), "activation must fail for a missing table");

    // F-24 core guarantee: the orphan is gone — neither the id nor the name
    // resolve. Before the fix, `id_for_name(&name)` returned `Some(id)` here.
    assert_eq!(
        db.validators().id_for_name(&name),
        None,
        "F-24: fresh registration must be undone on failure — name must be free"
    );
    assert!(
        db.validators().get_by_id(&id).is_none(),
        "F-24: fresh registration must be undone on failure — id must not resolve"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2 — ALTER case is NOT undone: a pre-existing validator reached via
// the `replace_artifact` branch that fails at step B/C must still resolve
// normally afterward. Regression guard against the fix over-reaching into
// the ALTER path.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn alter_replace_artifact_is_not_undone_on_failure() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "ghost_table");
    let id = RecordId::system("f24_alter");

    // Simulate a pre-existing, previously-active schema validator (the state
    // an ALTER starts from): register it under the canonical name directly.
    let pre_existing: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(no_rules()));
    db.validators()
        .register(id, &name, pre_existing)
        .expect("precondition register must succeed");
    assert_eq!(db.validators().id_for_name(&name), Some(id));

    // Now call compile_table_schema for a NON-existent table with the SAME id
    // (exactly what an ALTER does — `admin_schema.rs` reuses the catalogue's
    // `schema_validator_id`). `id_for_name` finds `id` → takes the
    // `replace_artifact` branch (NOT a fresh register) → get_table then fails.
    let result = db
        .compile_table_schema("testdb", "main", "ghost_table", id, no_rules())
        .await;
    assert!(
        result.is_err(),
        "activation must fail for a missing table even on the ALTER path"
    );

    // F-24 ALTER guarantee: the pre-existing validator is STILL registered —
    // the fix must not remove it. The catalogue-level `rec_prev` rollback is
    // what restores the old schema state, not a bare registry removal.
    assert_eq!(
        db.validators().id_for_name(&name),
        Some(id),
        "ALTER: pre-existing validator name must still resolve"
    );
    assert!(
        db.validators().get_by_id(&id).is_some(),
        "ALTER: pre-existing validator id must still resolve"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3 — retry-after-failure now activates cleanly. This is the practical
// proof the escalation chain can no longer occur: after a failed first
// attempt (orphan removed by the fix), a second attempt with a freshly
// minted id actually activates and is resolvable via `get_by_id`.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn retry_after_failed_first_schema_activates_cleanly() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "users");

    // ── First attempt: "users" does not exist yet → fails after fresh
    //    register. With the fix the orphan is removed; without it, a stale
    //    entry under `name` would linger with id1.
    let id1 = RecordId::system("f24_retry_first");
    let r1 = db
        .compile_table_schema("testdb", "main", "users", id1, no_rules())
        .await;
    assert!(r1.is_err(), "first attempt must fail (table missing)");
    assert!(
        db.validators().id_for_name(&name).is_none(),
        "after failed first attempt, name must be free (orphan cleaned)"
    );

    // ── Create the table so the retry can reach the success path.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("create_repo must succeed");

    // ── Retry: the DDL path mints a FRESH id (the rolled-back catalogue has
    //    no `schema_validator_id`), distinct from id1.
    let id2 = RecordId::system("f24_retry_second");
    assert_ne!(id1, id2, "retry must mint a distinct id");
    db.compile_table_schema("testdb", "main", "users", id2, no_rules())
        .await
        .expect("retry must activate cleanly now that the table exists");

    // The retry actually activated: id2 is the live, resolvable validator for
    // the table. Before the fix, the retry would have taken the
    // `replace_artifact(&id2)` no-op branch (name held by the id1 orphan),
    // `get_by_id(&id2)` would return `None`, and the catalogue would point at
    // an unregistered id — the availability bug described in the module docs.
    assert_eq!(
        db.validators().id_for_name(&name),
        Some(id2),
        "retry: name must resolve to the new id"
    );
    assert!(
        db.validators().get_by_id(&id2).is_some(),
        "retry: new validator id must be resolvable (escalation chain eliminated)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4 — secondary hardening: a stale name collision (name registered
// under a DIFFERENT id than the one passed) is rejected loudly via the
// `replace_artifact` return-value check, not silently no-op'ed. Defends
// against any future path that could recreate the collision despite the
// primary fix.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn stale_name_collision_is_rejected_not_silently_no_oped() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "ghost_table");

    // Plant a validator under `name` with id1 (simulating a stale orphan).
    // (Ids use short names: `RecordId::system` truncates to 12 bytes, so the
    // two ids below must stay distinct after truncation.)
    let id1 = RecordId::system("f24c_reg");
    let pre_existing: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(no_rules()));
    db.validators()
        .register(id1, &name, pre_existing)
        .expect("precondition register must succeed");

    // Attempt to activate under a DIFFERENT id2 → the name resolves (to id1),
    // so the `replace_artifact(&id2)` branch is taken, but id2 is not
    // registered → `replace_artifact` returns false → the defensive check
    // must surface a loud error.
    let id2 = RecordId::system("f24c_try");
    assert_ne!(
        id1, id2,
        "test ids must be distinct (RecordId::system truncates to 12 bytes)"
    );
    let result = db
        .compile_table_schema("testdb", "main", "ghost_table", id2, no_rules())
        .await;
    let err = result.expect_err("stale collision must be rejected");
    assert!(
        err.to_string().contains("stale name collision"),
        "error must name the stale collision, got: {err}"
    );

    // Neither side was mutated: id1 (the planted validator) is untouched, and
    // id2 (the attempted one) was never inserted.
    assert_eq!(
        db.validators().id_for_name(&name),
        Some(id1),
        "planted validator must remain under the name"
    );
    assert!(
        db.validators().get_by_id(&id1).is_some(),
        "planted validator must still resolve"
    );
    assert!(
        db.validators().get_by_id(&id2).is_none(),
        "attempted id2 must not have been inserted"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// F-27b (#827) tests below — the ALTER-path gap F-24 deliberately left
// open: `replace_artifact` swaps in the NEW compiled validator immediately,
// and (before this fix) a later activation failure left it live forever
// even though the caller rolls the CATALOGUE back to the OLD schema.
// ═══════════════════════════════════════════════════════════════════════

/// Build a `FieldRule` with a distinct, single-segment `path` so tests can
/// tell an "old" rule set apart from a "new" one after the fact via
/// `RecordValidator::as_schema_rules()` — `SchemaValidator` is the only
/// implementor that overrides this downcast hook, returning `&[FieldRule]`
/// directly (no `Arc::ptr_eq` needed: the rule content itself is the marker).
fn tagged_rule(tag: &str) -> shamir_engine::validator::schema::FieldRule {
    use shamir_engine::validator::schema::constraints::Constraints;
    use shamir_engine::validator::schema::type_tag::TypeTag;
    shamir_engine::validator::schema::FieldRule {
        path: vec![tag.to_string()],
        ty: TypeTag::String,
        constraints: Constraints::default(),
        keyset_safe: false,
    }
}

/// Resolve the live validator for `id` and return its first rule's `path[0]`
/// tag — the marker `tagged_rule` embeds — via the `as_schema_rules()`
/// downcast hook. Panics if the id doesn't resolve or isn't a
/// `SchemaValidator`, which would itself be a test-setup bug.
fn live_rule_tag(db: &ShamirDb, id: &RecordId) -> String {
    let artifact = db
        .validators()
        .get_by_id(id)
        .expect("validator id must resolve");
    let rules = artifact
        .as_schema_rules()
        .expect("artifact must be a SchemaValidator");
    rules[0].path[0].clone()
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5 — core F-27b regression: an ALTER's `replace_artifact` swap is
// undone (restoring the OLD compiled artifact) when a LATER step fails.
// This is the divergence this task closes: before the fix, the registry
// kept enforcing the NEW (never-fully-activated) rules while the catalogue
// (at the `admin_schema.rs` level, not exercised directly here) rolls back
// to the OLD schema under the same `schema_validator_id`.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn alter_replace_artifact_restores_old_artifact_on_activation_failure() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "ghost_table");
    let id = RecordId::system("f27b_alter");

    // Simulate a pre-existing, previously-active schema validator (the state
    // an ALTER starts from), tagged "old" so it can be told apart from the
    // attempted "new" rules below.
    let old: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(vec![tagged_rule("old")]));
    db.validators()
        .register(id, &name, old)
        .expect("precondition register must succeed");
    assert_eq!(live_rule_tag(&db, &id), "old", "precondition: old is live");

    // ALTER attempt with NEW rules for a non-existent table — `id_for_name`
    // finds `id` → takes the `replace_artifact` branch → `get_table` (a
    // LATER step) then fails.
    let result = db
        .compile_table_schema(
            "testdb",
            "main",
            "ghost_table",
            id,
            vec![tagged_rule("new")],
        )
        .await;
    assert!(
        result.is_err(),
        "activation must fail for a missing table even on the ALTER path"
    );

    // F-27b core guarantee: the registry's live artifact is the OLD one
    // again, not the NEW (failed) one. Before the fix, `replace_artifact`
    // had already swapped in "new" and nothing put "old" back.
    assert_eq!(
        db.validators().id_for_name(&name),
        Some(id),
        "ALTER: the validator name must still resolve to the same id"
    );
    assert_eq!(
        live_rule_tag(&db, &id),
        "old",
        "F-27b: a failed ALTER activation must restore the OLD compiled \
         artifact — the live registry must match the catalogue's rec_prev \
         rollback, not the never-fully-activated NEW rules"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6 — fresh-registration case is unaffected by the F-27b change. This
// duplicates F-24's own assertions inline (rather than merely relying on
// the untouched original test elsewhere in this file) so this task's own
// test run demonstrates the fresh-registration path still behaves
// identically after the ALTER-branch change.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn f27b_fresh_registration_path_is_unaffected() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "ghost_table");
    let id = RecordId::system("f27b_fresh");

    assert!(db.validators().id_for_name(&name).is_none());

    let result = db
        .compile_table_schema("testdb", "main", "ghost_table", id, no_rules())
        .await;
    assert!(result.is_err(), "activation must fail for a missing table");

    // Same as F-24's guarantee: a fresh registration is fully undone, not
    // "restored" to some prior artifact (there was none).
    assert_eq!(
        db.validators().id_for_name(&name),
        None,
        "fresh registration must still be undone on failure after F-27b"
    );
    assert!(
        db.validators().get_by_id(&id).is_none(),
        "fresh registration must still be undone on failure after F-27b"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 7 — successful ALTER still ends with the NEW artifact live. Guards
// against the restore logic firing unconditionally instead of only on
// `activation_result.is_err()`.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn alter_replace_artifact_keeps_new_artifact_on_success() {
    let db = db_without_tables().await;
    let name = schema_validator_name("testdb", "main", "users");
    let id = RecordId::system("f27b_ok");

    // Pre-existing "old" validator + a real table so activation can succeed.
    let old: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(vec![tagged_rule("old")]));
    db.validators()
        .register(id, &name, old)
        .expect("precondition register must succeed");

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("create_repo must succeed");

    // Full happy-path ALTER: replace_artifact swaps in "new", and activation
    // (get_table + add_validator_binding + add_binding) succeeds.
    db.compile_table_schema("testdb", "main", "users", id, vec![tagged_rule("new")])
        .await
        .expect("ALTER activation must succeed for an existing table");

    // The NEW artifact must be the one left live — the restore-on-failure
    // logic must not have fired for a successful call.
    assert_eq!(
        live_rule_tag(&db, &id),
        "new",
        "a successful ALTER must leave the NEW artifact live, not restore the old one"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// F-28 Step 4 (#831) — the cached reverse-FK map (`RepoInstance::
// fk_reverse_cache`) must reflect the RESTORED (old) state after a
// rolled-back `compile_table_schema` call, never a stale view of the
// failed attempt. `compile_table_schema` invalidates the whole repo's
// cache at its natural completion point — AFTER any F-27b restore-on-
// failure has already settled — so the cache's next rebuild always
// observes whatever ACTUALLY ended up live.
//
// This test builds a REAL repo with a REAL `foreign_key` reference (via a
// successful `compile_table_schema` call — exactly the production DDL
// path), warms the cache with a real DELETE that must be RESTRICTed, THEN
// forces a SEPARATE `compile_table_schema` call (same repo, a distinct
// `ghost_table` target) to fail — exercising the exact
// F-24/F-27b-established failure trigger (`get_table` on a nonexistent
// table) this file's other tests already use. Because invalidation is
// whole-repo, this failed call still clears the cache for the WHOLE repo,
// including the real FK-bearing table's entry. The assertion: after the
// failed call, a delete on the FK's parent table must STILL be correctly
// RESTRICTed — proving the post-invalidation rebuild reflects the
// unchanged (old, still-live) FK reference, not something corrupted by the
// failed, never-fully-activated attempt on `ghost_table`.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_cache_reflects_restored_state_after_rolled_back_alter_elsewhere_in_repo() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;

    // Real repo with a real FK: employees.dept_id -> departments.dept_id,
    // on_delete = Restrict.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo(
        "cr",
        ddl::create_repo("main").tables(["departments", "employees"]),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let fk_id = RecordId::system("f28s4_fk");
    let rules = vec![shamir_engine::validator::schema::FieldRule {
        path: vec!["dept_id".to_string()],
        ty: shamir_engine::validator::schema::type_tag::TypeTag::Int,
        constraints: shamir_engine::validator::schema::constraints::Constraints {
            foreign_key: Some(
                shamir_engine::validator::schema::ForeignKeyRef::with_on_delete(
                    "departments",
                    "dept_id",
                    shamir_query_types::admin::FkAction::Restrict,
                ),
            ),
            required: true,
            ..Default::default()
        },
        keyset_safe: false,
    }];
    db.compile_table_schema("testdb", "main", "employees", fk_id, rules)
        .await
        .expect("real FK schema activation must succeed");

    // Insert a department + a referencing employee (autocommit, real write
    // path — same shape as declarative_schema_fk_autocommit_e2e.rs).
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_dept",
        shamir_query_builder::write::insert("departments")
            .row(mpack!({ "dept_id": 100, "name": "Engineering" })),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ins_emp",
        shamir_query_builder::write::insert("employees")
            .row(mpack!({ "name": "Alice", "dept_id": 100 })),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Warm the cache: deleting the department must be REJECTED (RESTRICT) —
    // this is a real cache-aside miss+rebuild via the production delete path.
    let mut b = Batch::new();
    b.id(4);
    b.delete(
        "del_dept",
        shamir_query_builder::write::delete("departments")
            .where_(shamir_query_builder::filter::eq("dept_id", 100)),
    );
    let pre = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(
        pre.is_err(),
        "precondition: delete must be RESTRICTed while the FK is live: {pre:?}"
    );

    // A SEPARATE compile_table_schema call in the SAME repo, targeting a
    // nonexistent table, so `get_table` (the activation step) fails AFTER
    // `compile_table_schema`'s fresh registration already ran — same
    // deterministic failure trigger as this file's other tests. This call's
    // whole-repo cache invalidation (F-28 Step 4) fires regardless of
    // success/failure, clearing the warm cache built above.
    let ghost_id = RecordId::system("f28s4_ghost");
    let ghost_result = db
        .compile_table_schema("testdb", "main", "ghost_table", ghost_id, no_rules())
        .await;
    assert!(
        ghost_result.is_err(),
        "activation must fail for a missing table"
    );

    // F-27b guarantee (already covered elsewhere): the real FK validator on
    // `employees` is untouched by the unrelated failed call.
    assert!(
        db.validators().get_by_id(&fk_id).is_some(),
        "the unrelated failed ghost_table call must not disturb employees' live FK validator"
    );

    // F-28 Step 4 core guarantee: the cache's next rebuild (triggered by the
    // delete below) must reflect the RESTORED/unchanged state — the FK is
    // still live, so the delete must STILL be rejected. Before this task's
    // fix (no invalidation call, or an invalidation that failed to rebuild
    // correctly) this could wrongly serve a stale/corrupted view.
    let mut b = Batch::new();
    b.id(5);
    b.delete(
        "del_dept",
        shamir_query_builder::write::delete("departments")
            .where_(shamir_query_builder::filter::eq("dept_id", 100)),
    );
    let post = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(
        post.is_err(),
        "F-28 Step 4: delete must STILL be RESTRICTed after the unrelated \
         rolled-back ALTER — the cache must rebuild to the correct live state, \
         not a stale/corrupted view: {post:?}"
    );
}
