//! RI-7 regression tests — validators see the CALLER's actor, not `Actor::System`.
//!
//! Before RI-7, every `execute_*_tx` call site hardcoded `&Actor::System` when
//! invoking `run_validators_qv` / `run_validators_view`, so a bound validator's
//! `ValidatorCtx.actor` was ALWAYS `Actor::System` regardless of who issued the
//! write — a privilege-escalation primitive for any cross-table (`foreign_key` /
//! `unique`) or custom validator whose reads are actor-gated.
//!
//! These tests prove the validator now sees the REAL caller's actor, threaded
//! from `execute_*_tx`'s `actor` parameter. [`CallerActorValidator`] accepts
//! iff the actor it observes equals an expected value, so its accept/reject
//! decision is a direct observable of which actor the write path threaded in.
//!
//! Revert-and-confirm-fail proof: under the OLD hardcoded-`Actor::System`
//! behavior, [`validator_sees_caller_actor_not_system`] would FAIL — the
//! validator would observe `Actor::System != Actor::User(42)` and REJECT the
//! write. Re-applying the fix makes it pass (the validator observes the
//! caller's `Actor::User(42)` and accepts).

use std::sync::Arc;

use async_trait::async_trait;

use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use crate::validator::{
    encode::Validation, record_fields::RecordFields, record_validator::RecordValidator,
    record_validator::ValidatorCtx, ValidatorBinding, ValidatorRegistry, WriteOp,
};

// ── Validator under test ───────────────────────────────────────────────────

/// A validator that ACCEPTS iff the actor it observes (`ctx.actor`) equals an
/// expected actor.
///
/// This makes the validator's accept/reject outcome a direct observable of
/// which actor the write path threaded into [`ValidatorCtx`] — no shared
/// mutable state required (the security-relevant signal is the decision
/// itself, not a captured side value).
struct CallerActorValidator {
    expected: Actor,
}

#[async_trait]
impl RecordValidator for CallerActorValidator {
    async fn validate(
        &self,
        _new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        if ctx.actor == &self.expected {
            Validation::accept()
        } else {
            // Reject + stop: a mismatched actor must abort the write.
            let mut v = Validation::reject("actor_mismatch");
            v.stop = true;
            v
        }
    }
}

// ── Setup ──────────────────────────────────────────────────────────────────

/// Build a repo with one table (`items`) carrying a `CallerActorValidator`
/// bound for `WriteOp::Insert` that expects `expected`.
///
/// Uses [`TableManager::add_validator_binding`] (a direct ArcSwap swap) rather
/// than `save_validators_metadata` so the binding is unconditionally live —
/// `execute_insert_tx`'s `is_ok()` assertion alone does not prove a validator
/// fired (a no-binding table also returns `Ok`), so the negative test
/// (different actor → rejection) is what makes the proof airtight.
async fn setup(expected: Actor) -> (RepoInstance, TableManager) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("items")],
    };
    let repo = RepoInstance::from_factory(
        repo_config.name.clone(),
        repo_config.factory,
        repo_config.tables,
    )
    .await
    .unwrap();

    let validator = Arc::new(CallerActorValidator { expected }) as Arc<dyn RecordValidator>;
    let reg = Arc::new(ValidatorRegistry::new());
    let validator_id = shamir_types::types::record_id::RecordId::system("actor_val");
    reg.register(validator_id, "actor_val", validator).unwrap();

    let mut table = repo.get_table("items").await.unwrap();
    table.set_validator_registry(Arc::clone(&reg));
    table
        .add_validator_binding(ValidatorBinding {
            validator_id,
            ops: smallvec::smallvec![WriteOp::Insert],
            priority: 1000,
        })
        .await
        .unwrap();

    (repo, table)
}

fn items_op() -> crate::query::write::InsertOp {
    let mut row = new_map();
    row.insert("name".to_string(), QueryValue::Str("widget".into()));
    crate::query::write::InsertOp {
        insert_into: TableRef::new("items"),
        values: vec![QueryValue::Map(row)],
        records_idmsgpack: Vec::new(),
        select: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// The validator expects `Actor::User(42)` and the caller passes exactly that.
/// The write MUST succeed — proving the validator observed the caller's actor.
///
/// Revert-and-confirm-fail proof: under the OLD hardcoded-`Actor::System`
/// behavior the validator would have observed `Actor::System` and REJECTED the
/// write, so `result.is_ok()` would be `false` (test fails).
#[tokio::test]
async fn validator_sees_caller_actor_not_system() {
    let (repo, table) = setup(Actor::User(42)).await;
    let op = items_op();

    let (mut tx, _guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    tx.set_implicit(true);

    let result = table
        .execute_insert_tx(&op, &mut tx, false, None, &Actor::User(42))
        .await;
    assert!(
        result.is_ok(),
        "validator must see the caller's actor (User(42)), not Actor::System; \
         got err: {:?}",
        result.err()
    );

    repo.commit_tx(tx).await.unwrap();
}

/// The validator expects `Actor::User(42)` but the caller passes
/// `Actor::User(7)`. The write MUST be rejected — proving the validator's
/// accept/reject tracks the REAL caller, not a constant (if the actor were
/// hardcoded, BOTH this and the matching-actor case would be rejected, so the
/// matching-actor test above is what proves acceptance tracks the caller).
#[tokio::test]
async fn validator_rejects_when_caller_actor_differs() {
    let (repo, table) = setup(Actor::User(42)).await;
    let op = items_op();

    let (mut tx, _guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    tx.set_implicit(true);

    let result = table
        .execute_insert_tx(&op, &mut tx, false, None, &Actor::User(7))
        .await;
    assert!(
        result.is_err(),
        "validator must reject when the caller's actor (User(7)) != expected (User(42)); \
         an Ok here means the validator did NOT fire or saw the wrong actor"
    );
}
