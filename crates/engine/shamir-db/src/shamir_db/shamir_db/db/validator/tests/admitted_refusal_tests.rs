//! Round 7 (G1): the ADMITTED (wire-path) validator replace and rename refuse
//! an IN-DOUBT binding state before the replacement is saved or the rename is
//! re-keyed.
//!
//! Before round 7 a doubtful table only put its database into the reservation
//! set, and a vacant fence slot reserves successfully — so an ordinary replace
//! or rename served by a free database swapped the durable record the doubtful
//! table still invokes. `BindingHolders::refuse_in_doubt` closes that; these
//! tests drive it through the same admission the wire handlers take
//! (`ValidatorTargetAdmission::Fence`), which an integration test cannot name
//! because the admitted entry points are crate-private. The public direct API
//! (`create_validator_from_wasm` / `rename_validator`) takes `ExclusionOnly`
//! and runs no holder scan by design.

use shamir_types::types::record_id::RecordId;

use super::super::validator_management::{ValidatorRegistrationError, ValidatorTargetAdmission};
use crate::access::Actor;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::engine::validator::WriteOp;
use crate::shamir_db::shamir_db::{FunctionSource, ShamirDb};
use crate::DbError;

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

/// The in-doubt refusal, out of an admitted-mutation error. Anything else means
/// the fixture reserved or refused something it should not have, so the test
/// fails loudly instead of passing on the wrong evidence.
fn in_doubt_message(error: ValidatorRegistrationError) -> String {
    match error {
        ValidatorRegistrationError::Db(DbError::Conflict(message)) => message,
        ValidatorRegistrationError::Db(other) => {
            panic!("expected the in-doubt conflict, got: {other}")
        }
        ValidatorRegistrationError::Fence { db, code } => {
            panic!("expected the in-doubt conflict, got a fence refusal for '{db}' ({code:?})")
        }
    }
}

/// One bound validator in a healthy database, one open table of ANOTHER (free,
/// never-fenced) database whose binding record is put in doubt.
async fn fixture() -> (ShamirDb, RecordId) {
    let db = ShamirDb::init_memory().await.expect("in-memory db");
    db.create_db("app").await.expect("create_db app");
    db.add_repo(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users")),
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
    db.create_validator_from_wasm("v1", &echo_wasm(), false)
        .await
        .expect("create v1");
    db.bind_validator("app", "main", "users", "v1", vec![WriteOp::Insert], 1500)
        .await
        .expect("bind v1");
    let id = db.validator_id("v1").expect("v1 must resolve");
    let doubtful = db
        .get_table("other", "main", "events")
        .await
        .expect("the other table must open");
    doubtful.mark_validator_bindings_in_doubt();
    assert!(doubtful.validator_bindings_in_doubt());
    (db, id)
}

#[tokio::test]
async fn admitted_replace_refuses_an_in_doubt_holder() {
    let (db, id) = fixture().await;

    let error = db
        .create_validator_admitted(
            "v1",
            FunctionSource::Wasm(&echo_wasm()),
            true,
            Actor::System,
            ValidatorTargetAdmission::Fence,
        )
        .await
        .expect_err("an admitted replace must be refused while a holder table is in doubt");
    let message = in_doubt_message(error);
    assert!(message.contains("in doubt"), "{message}");
    assert!(
        !message.contains("events"),
        "the refusal must not disclose the table: {message}"
    );
    assert_eq!(
        db.validator_id("v1"),
        Some(id),
        "the replacement must not be registered"
    );
}

#[tokio::test]
async fn admitted_rename_refuses_an_in_doubt_holder() {
    let (db, id) = fixture().await;

    let error = db
        .rename_validator_admitted("v1", "v2", ValidatorTargetAdmission::Fence)
        .await
        .expect_err("an admitted rename must be refused while a holder table is in doubt");
    let message = in_doubt_message(error);
    assert!(message.contains("in doubt"), "{message}");
    assert_eq!(
        db.validator_id("v1"),
        Some(id),
        "the rename must not be applied"
    );
    assert_eq!(db.validator_id("v2"), None);
}

/// Round 8: the migration rename PREFLIGHT refuses an in-doubt holder before
/// any reservation, and a refusal leaves the rename gate untouched. Refusing
/// only when the rename executes would first fence the serving database in
/// doubt and journal the intent.
#[tokio::test]
async fn migration_rename_preflight_refuses_an_in_doubt_holder_and_frees_the_gate() {
    let (db, id) = fixture().await;

    let control = db.lock_replication_control().await;
    let Err(error) = db
        .begin_validator_rename_migration(&["v1".to_string()], None, &control)
        .await
    else {
        panic!("the preflight must refuse a rename while a holder table is in doubt");
    };
    let message = in_doubt_message(error);
    assert!(message.contains("in doubt"), "{message}");
    assert!(
        !message.contains("events"),
        "the refusal must not disclose the table: {message}"
    );

    // The gate is free: a preflight over a source that resolves to nothing
    // passes and publishes the gate (a held gate would refuse it as "already
    // in progress").
    let guard = db
        .begin_validator_rename_migration(&["no_such_validator".to_string()], None, &control)
        .await;
    assert!(
        guard.is_ok(),
        "a refused preflight must not leave the rename gate held"
    );
    drop(guard);
    drop(control);
    assert_eq!(db.validator_id("v1"), Some(id));
}
