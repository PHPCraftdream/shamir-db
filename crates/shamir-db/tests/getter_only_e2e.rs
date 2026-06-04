//! End-to-end proof of **getter-only via setuid (SECURITY DEFINER)** — the
//! "data firewall through procedures" pattern (Phase 2b / part C).
//!
//! The scenario: a user (`B`) who has **no read permission** on a table
//! invokes a setuid function owned by the table's owner (`A`). Because the
//! function carries the setuid bit, the engine resolves the *effective actor*
//! to the function's owner (`A`), and the function reads the table **as A**.
//! `B` thus receives the filtered result without ever being granted direct
//! read on the table.
//!
//! Why this proves the whole chain through real code paths:
//! - `chmod 0o4750` on the function → `effective_fn_actor` returns the owner
//!   (`Mode::is_setuid` on the persisted catalogue meta).
//! - `invoke_function_in_db_as(Actor::User(B))` builds a `FacadeDbGateway`
//!   carrying that *effective* actor.
//! - The function reads through `ctx.db_gateway().query(..)`, which routes to
//!   `execute_as(effective_actor, ..)` — so the Phase-2a DML gate runs.
//! - With setuid the read runs as owner `A` (allowed by `0o750`); without
//!   setuid it runs as caller `B` (denied), proving the firewall is setuid.
//!
//! # Reader function mechanism
//!
//! No WASM toolchain is required. The reader is a **native registered
//! `ShamirFunction`** (`TableReader`) that reads the table through the public
//! `DbGateway` it receives on `FnCtx`. We first create a real catalogue entry
//! via `create_function_from_wasm_as` (so the function has persisted
//! owner/mode meta that `chmod`/`set_resource_meta` and `effective_fn_actor`
//! can act on), then swap the live registry artifact for the native reader via
//! `functions().replace(..)`. The catalogue meta (owner + setuid) drives
//! actor resolution; the native artifact performs the actual table read.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_types::access::{Actor, Mode, ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

/// A native function that reads every row of `secrets` (in repo `main`) via
/// the DB gateway it was handed on `FnCtx`, and returns the row count.
///
/// The gateway carries the *effective actor* (owner under setuid, caller
/// otherwise), so the read is subject to the Phase-2a per-table ACL. A denied
/// read surfaces as `Err(String)` from the gateway, which we propagate as a
/// `FunctionError` — i.e. the whole invocation fails when the effective actor
/// lacks read.
struct TableReader;

#[async_trait]
impl ShamirFunction for TableReader {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let gw = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".to_string()))?;
        let rows = gw
            .query("main", "secrets", None)
            .await
            .map_err(FunctionError::Compute)?;
        Ok(QueryValue::Int(rows.len() as i64))
    }
}

/// In-memory ShamirDb with db "testdb", repo "main", table "secrets",
/// seeded (as System) with two rows.
async fn setup_with_secrets() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let setup: BatchRequest = serde_json::from_value(json!({
        "id": "setup",
        "queries": {
            "repo": {
                "create_repo": "main",
                "engine": "in_memory",
                "tables": ["secrets"]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &setup).await.unwrap();

    // Seed rows as System (bypasses ACLs).
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": "seed",
        "queries": {
            "ins": {
                "insert_into": "secrets",
                "values": [
                    {
                        "id": 1,
                        "label": "alpha"
                    },
                    {
                        "id": 2,
                        "label": "beta"
                    }
                ]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    shamir
}

/// Create a setuid reader function owned by `owner`, reading the `secrets`
/// table. Returns nothing; the function is named `read_secrets`.
///
/// Steps:
/// 1. `create_function_from_wasm_as(owner)` — minimal valid WASM module just
///    to materialise a catalogue record owned by `owner`.
/// 2. `functions().replace(..)` — swap the live artifact for the native
///    `TableReader` (catalogue meta/owner is untouched).
async fn create_setuid_reader(shamir: &ShamirDb, owner: Actor, mode: u16) {
    // Minimal valid empty WASM module (magic + version) — accepted by the
    // validator, never actually invoked (we replace the live artifact below).
    let empty_wasm = wat::parse_str("(module)").unwrap();
    shamir
        .create_function_from_wasm_as("read_secrets", &empty_wasm, false, owner.clone())
        .await
        .unwrap();

    // Swap the live registry artifact for the native table reader.
    shamir
        .functions()
        .replace("read_secrets", Arc::new(TableReader));

    // Stamp owner + mode (carrying the setuid bit) onto the catalogue meta.
    shamir
        .set_resource_meta(
            &ResourcePath::function("read_secrets"),
            &ResourceMeta {
                owner,
                group: None,
                mode,
            },
        )
        .await
        .unwrap();
}

/// Lock the `secrets` table to owner `A`, mode `0o750` (owner rwx, group r-x,
/// other ---) — so `A` can read but a stranger cannot.
async fn lock_table_to(shamir: &ShamirDb, owner: Actor) {
    shamir
        .set_resource_meta(
            &ResourcePath::table("testdb", "main", "secrets"),
            &ResourceMeta {
                owner,
                group: None,
                mode: 0o750,
            },
        )
        .await
        .unwrap();
}

// ============================================================================
// The headline getter-only proof.
// ============================================================================

/// User(B) has NO read on the `secrets` table. They invoke a setuid function
/// owned by User(A) (who DOES have read). The function reads the table as A
/// and returns the row count to B — the data firewall through a procedure.
#[tokio::test]
async fn setuid_function_lets_stranger_read_via_owner() {
    let user_a = Actor::User(1001);
    let user_b = Actor::User(2002);

    let shamir = setup_with_secrets().await;

    // Table owned by A, mode 0o750 — A reads, B (other) cannot.
    lock_table_to(&shamir, user_a.clone()).await;

    // setuid reader owned by A; mode 0o4750 = setuid + owner rwx, group r-x,
    // other ---. The `other` execute bit is 0, but B is granted Execute below.
    create_setuid_reader(&shamir, user_a.clone(), Mode::with_setuid(0o4750, true)).await;

    // Sanity: A really can read; B really cannot (direct gateway path).
    assert!(
        shamir
            .authorize_access(
                &user_a,
                &ResourcePath::table("testdb", "main", "secrets"),
                shamir_types::access::Action::Read,
            )
            .await
            .is_ok(),
        "owner A must have direct read"
    );
    assert!(
        shamir
            .authorize_access(
                &user_b,
                &ResourcePath::table("testdb", "main", "secrets"),
                shamir_types::access::Action::Read,
            )
            .await
            .is_err(),
        "stranger B must NOT have direct read on the locked table"
    );

    // Grant B Execute on the function (other-execute bit) WITHOUT touching
    // the setuid bit, so B is allowed to invoke but the table stays locked.
    // 0o4751 = setuid + owner rwx + group r-x + other --x.
    shamir
        .set_resource_meta(
            &ResourcePath::function("read_secrets"),
            &ResourceMeta {
                owner: user_a.clone(),
                group: None,
                mode: Mode::with_setuid(0o4751, true),
            },
        )
        .await
        .unwrap();

    // The effective actor for B's invocation resolves to the owner A.
    assert_eq!(
        shamir.effective_fn_actor("read_secrets", &user_b).await,
        user_a,
        "setuid must switch the effective actor to the function owner"
    );

    // B invokes the setuid function → succeeds, sees both rows (read as A).
    let result = shamir
        .invoke_function_in_db_as(
            "testdb",
            "main",
            "read_secrets",
            Params::new(),
            user_b.clone(),
        )
        .await
        .expect("B must read through the setuid function as owner A");

    assert_eq!(
        result,
        QueryValue::Int(2),
        "B should receive the 2 rows the function read as owner A"
    );
}

// ============================================================================
// Control: WITHOUT setuid the same function is denied — proving the firewall
// is the setuid bit, not the function itself.
// ============================================================================

/// Same table, same reader, but the setuid bit is CLEARED. Now B's invocation
/// reads the table as B (the caller), who lacks read → the gateway read is
/// denied and the invocation fails.
#[tokio::test]
async fn without_setuid_stranger_is_denied() {
    let user_a = Actor::User(1001);
    let user_b = Actor::User(2002);

    let shamir = setup_with_secrets().await;
    lock_table_to(&shamir, user_a.clone()).await;

    // NON-setuid reader, but other-execute granted so B may invoke it.
    // 0o0751 = owner rwx + group r-x + other --x, setuid OFF.
    create_setuid_reader(&shamir, user_a.clone(), 0o0751).await;

    // Effective actor stays the caller B (no setuid).
    assert_eq!(
        shamir.effective_fn_actor("read_secrets", &user_b).await,
        user_b,
        "without setuid the effective actor must remain the caller"
    );

    // B invokes → the function reads the table AS B → denied.
    let err = shamir
        .invoke_function_in_db_as(
            "testdb",
            "main",
            "read_secrets",
            Params::new(),
            user_b.clone(),
        )
        .await
        .expect_err("without setuid, B's read of the locked table must be denied");

    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "denial must come from the per-table ACL gate, got: {msg}"
    );
}

// ============================================================================
// Cross-check: with setuid, the function owner A invoking it also works
// (definer == invoker here), and B's success is genuinely the owner's read.
// ============================================================================

/// The setuid function read returns exactly the rows visible to the owner —
/// confirming the data really flows from the owner's authority, identical
/// whether A or B invokes it.
#[tokio::test]
async fn setuid_read_identical_for_owner_and_stranger() {
    let user_a = Actor::User(1001);
    let user_b = Actor::User(2002);

    let shamir = setup_with_secrets().await;
    lock_table_to(&shamir, user_a.clone()).await;
    // setuid + owner rwx + group r-x + other --x (B may invoke).
    create_setuid_reader(&shamir, user_a.clone(), Mode::with_setuid(0o4751, true)).await;

    let as_owner = shamir
        .invoke_function_in_db_as(
            "testdb",
            "main",
            "read_secrets",
            Params::new(),
            user_a.clone(),
        )
        .await
        .expect("owner invocation must succeed");

    let as_stranger = shamir
        .invoke_function_in_db_as(
            "testdb",
            "main",
            "read_secrets",
            Params::new(),
            user_b.clone(),
        )
        .await
        .expect("stranger invocation must succeed under setuid");

    assert_eq!(
        as_owner, as_stranger,
        "setuid read must yield the owner's view regardless of the caller"
    );
    assert_eq!(as_owner, QueryValue::Int(2));
}
