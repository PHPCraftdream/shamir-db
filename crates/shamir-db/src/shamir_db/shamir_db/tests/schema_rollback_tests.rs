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

use std::sync::Arc;

use shamir_engine::validator::schema::SchemaValidator;
use shamir_engine::validator::RecordValidator;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
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
