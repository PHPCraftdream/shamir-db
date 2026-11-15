//! Aggregation helpers: accumulators, GROUP BY, aggregate-all.
//!
//! S4 (#76) — the aggregation pipeline is now lens-fed. Each row reaches the
//! accumulators as raw storage `Bytes`; a per-row `RecordView` (the zero-copy
//! msgpack lens) supplies every aggregated/group field via `scalar_at` →
//! `ScalarRef`. Owned state survives only where §5b of the anti-formal doc
//! demands it:
//!
//! - **Min/Max** hold the running extreme as a small `OwnedScalar` (leaf bytes
//!   / scalar), not a `Value<K>` tree — the incoming `ScalarRef` is compared
//!   against it directly.
//! - **Sum/Avg/Count** keep their numeric accumulators; the input is read as
//!   `ScalarRef::Int`/`F64` straight off the lens — no `Value` materialised.
//! - **funclib aggregate slots** are fed `QueryValue` built at the funclib
//!   boundary only (scalar ⇒ cheap; container ⇒ one `materialize_at` per row).
//! - **HAVING** compares the predicate against the `QueryValue` result map via
//!   a `RecordRef` adapter that re-keys String → `InternerKey` once and leaves
//!   the leaves as `QueryValue` — the `query_value_to_inner` bridge is gone.

mod accum;
mod extreme;
mod group_key;
mod params;

pub(in crate::query::read::aggregation) use accum::{
    intern_field_path_keys_with_overlay, step_aggregate_input,
};
pub(in crate::query::read) use accum::{AggAccum, AggState};
pub(in crate::query::read) use group_key::{
    group_key_item_complex, group_key_item_scalar, GroupKeyItem,
};
pub(in crate::query::read::aggregation) use params::{
    is_execution_constant, is_literal, literal_f64, literal_string, resolve_aggregate_param,
};
