//! Apply an overlay-id → base-id remap to staged record bytes.
//!
//! After `commit_interner_overlay` merges the per-tx interner overlay
//! into the base interner, some staged record bytes may still contain
//! references to overlay ids (>= `OVERLAY_ID_BASE`). The executor calls
//! [`remap_inner_value_bytes`] for each staged value to rewrite those
//! references before the bytes hit `transact()`.
//!
//! §5b floor (#61): recovery anchor — replays staged id-keyed storage bytes
//! during commit/recovery, where the name-keyed lens does not yet apply
//! (no interner context at replay). See `docs/dev-artifacts/perf/innervalue-floor.md`
//! (Category 3 — recovery anchors).
//!
//! A8 fix: [`collect_referenced_ids`] walks the SAME `InnerValue` shape as
//! [`remap_value`] (Map keys + List elements) but COLLECTS the referenced
//! `InternerKey` ids instead of rewriting them. `pre_commit_prelock` uses
//! it (after the remap pass) to find every base id referenced by this tx's
//! staged bytes that sits above `persisted_high_water()` and ensure each
//! one is covered by `tx.interner_deltas` — closing the "first toucher
//! aborted before WAL" hole where a later committer's records reference an
//! id no surviving WAL delta mentions.

// hasher-generic boundary: caller supplies the hasher (THasher at every call site)
#[allow(clippy::disallowed_types)]
use std::collections::HashMap;
use std::hash::BuildHasher;

use bytes::Bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

/// Recursively replace `InternerKey` ids in `value` according to
/// `remap`. Keys not present in the remap are left unchanged.
// hasher-generic boundary: caller supplies the hasher (THasher at every call site)
#[allow(clippy::disallowed_types)]
pub fn remap_value<S: BuildHasher>(value: &mut InnerValue, remap: &HashMap<u64, u64, S>) {
    match value {
        InnerValue::Map(m) => {
            let entries: Vec<(InternerKey, InnerValue)> = m.drain(..).collect();
            for (k, mut v) in entries {
                let new_key = match remap.get(&k.id()) {
                    Some(&new_id) => InternerKey::new(new_id),
                    None => k,
                };
                remap_value(&mut v, remap);
                m.insert(new_key, v);
            }
        }
        InnerValue::List(l) => {
            for elem in l {
                remap_value(elem, remap);
            }
        }
        InnerValue::Set(_) => {}
        InnerValue::Null
        | InnerValue::Bool(_)
        | InnerValue::Int(_)
        | InnerValue::F64(_)
        | InnerValue::Dec(_)
        | InnerValue::Big(_)
        | InnerValue::Str(_)
        | InnerValue::Bin(_) => {}
    }
}

/// Decode `Bytes` as `InnerValue`, apply [`remap_value`], re-encode.
///
/// Returns `Err` only on serde failure. If `remap` is empty this is a
/// no-op decode+encode round-trip — caller can skip the call when the
/// remap is empty.
// hasher-generic boundary: caller supplies the hasher (THasher at every call site)
#[allow(clippy::disallowed_types)]
pub fn remap_inner_value_bytes<S: BuildHasher>(
    bytes: Bytes,
    remap: &HashMap<u64, u64, S>,
) -> Result<Bytes, rmp_serde::encode::Error> {
    let mut value = InnerValue::from_bytes(&bytes)
        .map_err(|e| rmp_serde::encode::Error::Syntax(format!("decode failed: {e}")))?;
    remap_value(&mut value, remap);
    value.to_bytes()
}

/// Recursively walk `value` and collect every `InternerKey` id used as a
/// `Map` key (descending into nested `Map`s and `List` elements — the
/// SAME traversal shape as [`remap_value`]).
///
/// A8 fix: `pre_commit_prelock` calls this (after the overlay→base remap
/// pass rewrites staged bytes to reference base ids) to find every base
/// id this tx's records reference. Any id `> persisted_high_water()` that
/// is not already in `tx.interner_deltas` is then added — so a committer
/// whose records reference an id some OTHER (possibly aborted) tx created
/// still carries that `(name, id)` in its own WAL delta, and recovery can
/// decode its records after a crash.
///
/// Appends to `out` (caller supplies the set so multiple values / tables
/// can be folded into one set without re-allocation).
// hasher-generic boundary: caller supplies the hasher (THasher at every call site)
#[allow(clippy::disallowed_types)]
pub fn collect_referenced_ids<S: BuildHasher>(value: &InnerValue, out: &mut HashMap<u64, (), S>) {
    match value {
        InnerValue::Map(m) => {
            for (k, v) in m.iter() {
                out.insert(k.id(), ());
                collect_referenced_ids(v, out);
            }
        }
        InnerValue::List(l) => {
            for elem in l {
                collect_referenced_ids(elem, out);
            }
        }
        InnerValue::Set(_) => {}
        InnerValue::Null
        | InnerValue::Bool(_)
        | InnerValue::Int(_)
        | InnerValue::F64(_)
        | InnerValue::Dec(_)
        | InnerValue::Big(_)
        | InnerValue::Str(_)
        | InnerValue::Bin(_) => {}
    }
}
