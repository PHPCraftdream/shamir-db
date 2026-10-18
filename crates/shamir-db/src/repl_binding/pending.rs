//! Pending creation barrier (S5c-X PR8b-2 round 2, A4): durability-before-
//! start for NEW subscription rows. A row is written with
//! `state = pending_durable`, persisted and flushed (the full three-step
//! durability barrier), and ONLY THEN flipped to its requested final state.
//! No follower can start on a row whose durable barrier has not flushed,
//! and a flip failure compensates back to `pending_durable`.

use crate::query::write::SetOp;
use crate::query::TableRef;
use crate::repl_binding::binding::SubscriptionBinding;
use crate::repl_binding::validate::read_subscription_rows;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult, ShamirDb};
use shamir_types::mpack;

/// The intermediate durable state of a subscription row between its first
/// flush and the final-state flip. `ensure_subscription_absent` refuses any
/// state except this one, so a retry over a crashed creation overwrites it.
pub(crate) const PENDING_STATE: &str = "pending_durable";

/// Monotonic next binding generation over a catalogue snapshot: the maximum
/// VALID binding generation among `rows`, raised to the SYSTEM REPO'S DURABLE
/// MVCC VERSION (the global allocation sequence — it only ever grows and
/// survives restarts, so the generation strictly increases across
/// DROP + re-CREATE of the same subscription name and across restarts; a
/// recreated row can never collide with a stale task's generation), plus 1
/// (rows that fail binding validation are ignored — open-time validation
/// refuses them anyway).
pub(crate) async fn next_generation(shamir: &ShamirDb, rows: &[QueryValue]) -> DbResult<u64> {
    let from_rows = rows
        .iter()
        .filter_map(|row| SubscriptionBinding::from_row(row).ok())
        .map(|binding| binding.generation())
        .max()
        .unwrap_or(0);
    let durable_floor = shamir
        .system_store()
        .system_repo()?
        .consistent_version()
        .await?;
    Ok(std::cmp::max(from_rows, durable_floor) + 1)
}

/// One durable catalogue write of a subscription row, running
/// `set_via_implicit_tx`, then `persist_table_interner`, then
/// `flush_seed_catalogue` — the same three steps every subscription-row
/// writer uses (mirrors `catalogue::persist`).
async fn write_row(shamir: &ShamirDb, name: &str, record: QueryValue) -> DbResult<()> {
    let table = shamir.system_store().subscriptions_table().await?;
    let op = SetOp {
        set: TableRef::new("subscriptions"),
        key: mpack!({"name": @(QueryValue::Str(name.to_string()))}),
        value: record,
    };
    shamir
        .system_store()
        .set_via_implicit_tx(&table, &op)
        .await?;
    shamir
        .system_store()
        .persist_table_interner(&table, None)
        .await?;
    shamir.flush_seed_catalogue().await
}

/// Durability-before-start write of a NEW subscription row (S5c-X round 2
/// A4). Stamps the monotonic `next_generation` over the current catalogue
/// rows, writes `record` with `state = pending_durable` through the full
/// three-step durability barrier, THEN flips `state` to `final_state`
/// through the same barrier. On flip failure the pending row is rewritten
/// (compensated) back to `pending_durable` — so a half-created row can
/// never be observed startable — and the original flip error is returned.
pub(crate) async fn write_new_subscription(
    shamir: &ShamirDb,
    record: QueryValue,
    final_state: &str,
) -> DbResult<()> {
    let name = record
        .get("name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| DbError::Validation("subscription name missing".into()))?
        .to_owned();
    let rows = read_subscription_rows(shamir).await?;
    let generation = next_generation(shamir, &rows).await?;
    let mut pending = record;
    if let QueryValue::Map(fields) = &mut pending {
        fields.insert("state".into(), QueryValue::Str(PENDING_STATE.to_string()));
        // Overwrite the writer's placeholder generation with the
        // catalogue-wide monotonic value.
        fields.insert(
            "binding_generation".into(),
            QueryValue::Int(generation as i64),
        );
    }
    write_row(shamir, &name, pending.clone()).await?;
    if let QueryValue::Map(fields) = &mut pending {
        fields.insert("state".into(), QueryValue::Str(final_state.to_string()));
    }
    if let Err(flip_err) = write_row(shamir, &name, pending.clone()).await {
        // Compensate: rewrite the pending barrier so no startable row
        // survives a failed flip. If even the compensation fails, the row
        // stays at its last flushed state and the failure is logged loudly.
        if let QueryValue::Map(fields) = &mut pending {
            fields.insert("state".into(), QueryValue::Str(PENDING_STATE.to_string()));
        }
        if let Err(rollback_err) = write_row(shamir, &name, pending).await {
            log::error!(
                "write_new_subscription: compensation rewrite of subscription '{name}' to \
                 {PENDING_STATE} failed too: {rollback_err}; the row is left in its \
                 intermediate flushed state (no follower can have started on it)"
            );
        }
        return Err(flip_err);
    }
    Ok(())
}
