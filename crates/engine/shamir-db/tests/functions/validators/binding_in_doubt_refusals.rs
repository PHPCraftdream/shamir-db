//! Round 7 (G1, G3) end-to-end, through the PUBLIC facade: an IN-DOUBT binding
//! record refuses the validator DROP and a rename of the doubtful table
//! itself, without disclosing a table path and without changing the validator
//! or the table. (The admitted validator REPLACE/RENAME refusals are
//! crate-private entry points; see `db/validator/tests/admitted_refusal_tests.rs`.)
//!
//! The doubt state is raised directly on the table's already-open manager
//! (`mark_validator_bindings_in_doubt`, test-util only). The production
//! producer is a binding flush that could not be compensated, which needs a
//! fault-injection store at the repository layer; injecting it here would test
//! the fault, not the refusal. The refusal is what round 7 adds: before it a
//! doubtful table only reserved its database, and a vacant fence slot reserves
//! successfully, so a REPLACE/RENAME of a validator served by a free database
//! went through.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::ShamirDb;
use shamir_storage::error::DbError;

/// Identity-echo WAT matching the slice-2 ABI (same shape as
/// `validators_lifecycle.rs`): no Rust toolchain needed to build a validator.
const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

fn echo_wasm() -> Vec<u8> {
    wat::parse_str(ECHO_WAT).expect("WAT parse failed")
}

fn is_in_doubt(error: &DbError) -> bool {
    matches!(error, DbError::Conflict(message) if message.contains("in doubt"))
}

async fn setup() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await.expect("create_db app");
    db.add_repo(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("audit")),
    )
    .await
    .expect("add_repo app");
    db.create_db("other").await.expect("create_db other");
    db.add_repo(
        "other",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("events")),
    )
    .await
    .expect("add_repo other");
    db
}

#[tokio::test]
async fn in_doubt_table_refuses_validator_mutations_and_its_own_rename() {
    let db = setup().await;
    db.create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .unwrap();
    db.bind_validator("app", "main", "users", "v1", vec![WriteOp::Insert], 1500)
        .await
        .unwrap();
    let id = db
        .validator_id("v1")
        .expect("the bound validator must resolve");

    // A table of ANOTHER database is in doubt. Its database is free — no
    // migration fence, no reservation failure — which is exactly the case a
    // reservation alone cannot refuse.
    let doubtful = db
        .get_table("other", "main", "events")
        .await
        .expect("the other table must open");
    doubtful.mark_validator_bindings_in_doubt();
    assert!(doubtful.validator_bindings_in_doubt());

    let drop_error = db
        .drop_validator("v1")
        .await
        .expect_err("drop must be refused while a scanned table is in doubt");
    assert!(is_in_doubt(&drop_error), "{drop_error}");
    assert!(
        !drop_error.to_string().contains("events"),
        "the in-doubt refusal must not disclose the table: {drop_error}"
    );
    assert_eq!(
        db.validator_id("v1"),
        Some(id),
        "the validator must survive the refusal"
    );

    // The ADMITTED (wire-path) validator replace and rename refusal cannot be
    // reached from here: the public direct API
    // (`create_validator_from_wasm` / `rename_validator`) takes
    // `ValidatorTargetAdmission::ExclusionOnly`, which runs no holder scan by
    // design — the fence guards the wire and migration paths. That half of G1
    // is covered in-crate, at the same admission the wire handlers use
    // (`db/validator/tests/admitted_refusal_tests.rs`).

    // G3: renaming the IN-DOUBT table itself is refused — the rename would copy
    // the info store and install a fresh manager, discarding the flag.
    let table_error = db
        .rename_table("other", "main", "events", "events_v2")
        .await
        .expect_err("a rename of an in-doubt table must be refused");
    assert!(is_in_doubt(&table_error), "{table_error}");
    assert!(
        doubtful.validator_bindings_in_doubt(),
        "the flag must survive the refused rename"
    );
    db.get_table("other", "main", "events")
        .await
        .expect("the table must keep its name");
    assert!(
        db.get_table("other", "main", "events_v2").await.is_err(),
        "the destination must not exist after the refusal"
    );

    // A healthy rename in the healthy database still works.
    db.rename_table("app", "main", "audit", "audit_v2")
        .await
        .expect("a healthy rename must still work");
}
