//! Generalized write-value marker resolution — `$param`/`$query`/`$fn`/
//! `$cond`/`$expr`.
//!
//! #641: `InsertOp.values`/`UpdateOp.set`/`SetOp.{key,value}` are typed as
//! plain `QueryValue` on the wire (no dedicated "expression" wire format).
//! A `QueryValue::Map` with exactly one reserved key
//! (`$param`/`$query`/`$fn`/`$cond`/`$expr`) is a MARKER, not a literal
//! document field, and must be resolved against the batch's `FilterContext`
//! before the value reaches the storage layer — otherwise the literal
//! marker map (e.g. `{"$query": "@orders", "path": "[0].id"}`) gets written
//! to disk verbatim instead of the value it points to.
//!
//! This module originally only handled `$param` (the narrowest case, no
//! `FilterContext`/record needed — just a name→value lookup in the current
//! params scope; `contains_param_ref`/`substitute_params` were its two
//! functions). It is generalized here to recognize all FOUR additional
//! reserved markers by reusing the exact SAME "exactly one reserved key"
//! detection convention, then resolving non-`$param` markers through
//! `FilterValue`/`resolve_filter_query` — the same machinery WHERE/`when`/
//! `for_each`'s `over` already use. `contains_param_ref` keeps its original
//! name (only its detection set grew) since it is still the fast marker
//! pre-scan every write dispatch site calls first.
//!
//! **`$ref` (`FilterValue::FieldRef`) is explicitly OUT OF SCOPE.** Resolving
//! a same-document field reference (e.g. `{"total": {"$expr": {"op": "add",
//! "args": [{"$ref": ["a"]}, {"$ref": ["b"]}]}}}`) would require the
//! partially-built document as record context — but at the point this
//! resolver runs (before the row is validated/normalized by the write
//! executor) there is no meaningful "record" to resolve a field path
//! against, only the raw values supplied by the caller. This mirrors `when`'s
//! documented exclusion of field-based comparisons (`resolve_skip` in
//! `query_runner.rs`): both contexts have no per-row record, so `$ref` inside
//! either one silently falls through to `resolve_filter_query`'s existing
//! `FieldRef` handling, which resolves against the dummy `InnerValue::Null`
//! record used here — i.e. it always misses (returns `None`), the same
//! "absent" semantics every other unresolvable `FilterValue` already has in
//! this codebase. A future task can thread real partial-document context
//! through if `$ref`-inside-write-value support is ever requested.
//!
//! No wire format change: a marker is just a `QueryValue::Map` that happens
//! to carry one of the reserved keys — the SAME convention `$param` already
//! established and that OQL relies on throughout (WHERE, `when`, `for_each`'s
//! `over`).

use shamir_types::types::common::TMap;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

use crate::query::filter::{resolve_filter_query, FilterContext, FilterValue};

/// `true` iff `fv` is a `FilterValue::FieldRef` (`$ref`) itself, or contains
/// one ANYWHERE at any depth inside its operands.
///
/// Used to detect the one write-value shape the batch-level resolver
/// (`resolve_write_value`) must NOT attempt to resolve: a `$fn` call whose
/// `args` reference a sibling field in the SAME row via `$ref` (e.g.
/// `{"$fn": {"name": "strings/lower", "args": [{"$ref": ["email"]}]}}`).
/// That reference is only meaningful once the real per-row record exists —
/// which is the table layer's job (`write_helpers::resolve_computed_record`),
/// not this pre-execution marker resolver (see the `DUMMY_RECORD` doc comment
/// above for why `$ref` is out of scope here).
///
/// Recurses into `FnCall` args, `Expr` args, `Cond`'s `then`/`or_else`
/// branches, and `Array` elements. `Cond`'s `condition` field is a `Filter`
/// (a much larger, differently-shaped tree — `Eq`/`And`/`Or`/`ValueCompare`/…)
/// rather than a `FilterValue`; walking it fully would need a parallel
/// `Filter`-level recursion for a shape `is_computed_field` (table layer)
/// never supports wrapped in `$cond` anyway (see this module's own out-of-
/// scope note and the brief this fix implements). Instead, conservatively
/// treat ANY `Cond` as "may contain a `$ref`" — this only affects whether a
/// `$cond`-wrapped value takes the pass-through-to-table-layer path (which
/// the table layer doesn't understand either and will error on) vs. the
/// resolve-now path (which already errors on `$ref` misses today); either
/// way an actual `$ref` inside a `$cond`'s condition was never resolvable by
/// this resolver, so this is not a behavior regression for any case that
/// worked before.
fn filter_value_contains_field_ref(fv: &FilterValue) -> bool {
    match fv {
        FilterValue::FieldRef { .. } => true,
        FilterValue::FnCall { call } => call.args().iter().any(filter_value_contains_field_ref),
        FilterValue::Expr { expr } => expr.args.iter().any(filter_value_contains_field_ref),
        FilterValue::Cond { .. } => true,
        FilterValue::Array(items) => items.iter().any(filter_value_contains_field_ref),
        _ => false,
    }
}

/// The reserved marker keys this resolver recognizes.
const RESERVED_MARKER_KEYS: [&str; 5] = ["$param", "$query", "$fn", "$cond", "$expr"];

/// `true` iff `map` is a marker node: it carries EXACTLY ONE of the reserved
/// marker keys (`$param`/`$query`/`$fn`/`$cond`/`$expr`), plus — for
/// `$query` only — its own optional companion `path` key.
///
/// This mirrors `$param`'s original single-key check, generalized to the
/// wire shape each marker actually has: `$param`/`$fn`/`$cond`/`$expr` are
/// ALWAYS a single-key map (`{"$param": "name"}`, `{"$fn": ...}`,
/// `{"$cond": {...}}`, `{"$expr": {...}}`), but `FilterValue::QueryRef`'s
/// serde shape is `{"$query": "<alias>", "path"?: "<path>"}` — a SECOND
/// top-level key (`path`) is the normal, common case whenever the `$query`
/// ref carries a path (e.g. `qref("users", "[0].id")` — see
/// `shamir-query-builder`'s `val::filter_value::qref`). Treating `path`'s
/// presence as disqualifying would silently fail to detect the single most
/// common `$query` shape in the entire codebase.
///
/// A map with a reserved key AND any OTHER unexpected extra key (not `path`
/// alongside `$query`) is intentionally NOT treated as a marker here — it
/// recurses as a plain nested object instead, so a document that
/// legitimately has a field literally named one of these 5 reserved words
/// (extremely unlikely, but not disallowed) doesn't accidentally trip
/// resolution when it also happens to carry unrelated sibling keys.
#[inline]
fn is_marker_map(map: &TMap<String, QueryValue>) -> bool {
    match map.len() {
        1 => RESERVED_MARKER_KEYS.iter().any(|k| map.contains_key(*k)),
        2 => map.contains_key("$query") && map.contains_key("path"),
        _ => false,
    }
}

/// Return `true` if `value` contains any reserved marker node
/// (`$param`/`$query`/`$fn`/`$cond`/`$expr`) at any depth.
///
/// This is the fast pre-scan every write dispatch site (`Insert`/`Update`/
/// `Set` in `query_runner.rs`) calls first: the overwhelming common case
/// (plain literal document writes) has zero marker nodes, so callers skip
/// the per-node resolution walk entirely and pay only a cheap clone.
pub(super) fn contains_param_ref(value: &QueryValue) -> bool {
    match value {
        Value::Map(map) => is_marker_map(map) || map.values().any(contains_param_ref),
        Value::List(arr) => arr.iter().any(contains_param_ref),
        _ => false,
    }
}

/// A dummy/null record used as the `RecordRef` argument to
/// `resolve_filter_query` when resolving write-value markers. There is no
/// real record at this point in the write path (the row itself is what's
/// being constructed) — this is the SAME "no real record" pattern already
/// established by `when`'s `resolve_skip` and `for_each`'s `over` resolution
/// (both use `InnerValue::Null` for the identical reason: `$ref`/`FieldRef`
/// is not meaningful in this context and is out of scope here too).
const DUMMY_RECORD: InnerValue = InnerValue::Null;

/// Error detail for a write-value marker that failed to resolve.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum WriteValueError {
    /// `$param '<name>'` is not bound in this sub-batch (unchanged from the
    /// pre-existing `$param`-only behavior).
    UnboundParam(String),
    /// A `$query`/`$fn`/`$cond`/`$expr` marker failed to resolve (malformed
    /// payload, unknown alias/function, etc). Carries a human-readable
    /// description of the offending marker for the error message.
    MalformedMarker(String),
}

/// Recursively resolve ALL reserved markers (`$param`/`$query`/`$fn`/`$cond`/
/// `$expr`) inside a write-row value (`InsertOp.values[i]`/`UpdateOp.set`/
/// `SetOp.{key,value}`).
///
/// This is the generalized resolver the Insert/Update/Set dispatch arms in
/// `query_runner.rs` call. It walks the value tree and, at each `Map` node,
/// checks FIRST whether the node is itself a full marker (`is_marker_map`)
/// — if so, the marker is resolved (see below) — otherwise it recurses into
/// the map's fields/list elements.
///
/// **Resolution strategy**:
/// - `$param` keeps its original cheap, context-free path: a bare name→value
///   lookup directly in `ctx.params` (no `FilterContext`/`FilterValue`
///   round-trip needed, since its payload is always a bare string) — this is
///   the pre-existing behavior, unchanged, and pinned by the regression
///   test.
/// - `$query`/`$fn`/`$cond`/`$expr`: the single-key `QueryValue::Map` is
///   converted to a `FilterValue` via a raw msgpack round-trip (serialize
///   the `QueryValue`, deserialize as `FilterValue` — this works because a
///   `FilterValue`'s wire encoding for `QueryRef`/`FnCall`/`Cond`/`Expr` IS
///   exactly this single-reserved-key map shape; see
///   `FilterValue::from(QueryValue)`'s doc comment on the msgpack round-trip
///   tier), then resolved via `resolve_filter_query` against `ctx` and the
///   dummy/null record (see [`DUMMY_RECORD`]'s doc comment — `$ref` is out
///   of scope here).
///
/// **Fast path**: pre-scan the tree for ANY marker node via
/// `contains_param_ref`. If none exist, return a clone immediately — no
/// per-node allocation for the common case where write values are plain
/// records with no markers at all.
///
/// **Errors are HARD failures** — never a silent literal pass-through. These
/// 5 reserved key names should not naturally collide with real document
/// field names, so a marker that fails to resolve (malformed payload,
/// unbound `$param`, unknown alias/function, …) is virtually always a caller
/// mistake, matching `$param`'s pre-existing "error on unbound" philosophy.
/// See [`WriteValueError`].
pub(super) fn resolve_write_value(
    value: &QueryValue,
    ctx: &FilterContext,
) -> Result<QueryValue, WriteValueError> {
    // Fast path: if the tree has no marker nodes at all, skip the walk
    // entirely (the overwhelming common case: plain literal document writes).
    if !contains_param_ref(value) {
        return Ok(value.clone());
    }
    resolve_write_value_inner(value, ctx)
}

fn resolve_write_value_inner(
    value: &QueryValue,
    ctx: &FilterContext,
) -> Result<QueryValue, WriteValueError> {
    match value {
        Value::Map(map) => {
            if is_marker_map(map) {
                // `$param` keeps the original cheap, context-free path (no
                // msgpack round-trip / FilterContext needed for a bare
                // name→value lookup).
                if let Some(Value::Str(name)) = map.get("$param") {
                    return match ctx.params.get(name.as_str()) {
                        Some(qv) => Ok(qv.clone()),
                        None => Err(WriteValueError::UnboundParam(name.clone())),
                    };
                }
                // `$query`/`$fn`/`$cond`/`$expr`: msgpack round-trip into a
                // `FilterValue`, then resolve via `resolve_filter_query`
                // against the dummy record + current `FilterContext`.
                let fv: FilterValue = rmp_serde::to_vec_named(value)
                    .ok()
                    .and_then(|bytes| rmp_serde::from_slice(&bytes).ok())
                    .ok_or_else(|| {
                        WriteValueError::MalformedMarker(format!(
                            "marker {:?} could not be decoded as a filter expression",
                            value
                        ))
                    })?;
                // `$fn` calls whose args contain a `$ref` (anywhere, at any
                // depth) are explicitly out of scope for THIS resolver (see
                // the module doc comment's "$ref is explicitly OUT OF SCOPE"
                // section) — resolving `$ref` needs the real per-row record,
                // which only exists at the table layer. Pass the marker
                // through COMPLETELY UNCHANGED so it reaches
                // `write_helpers::resolve_computed_record`, the existing
                // mechanism that already knows how to resolve `$fn`+`$ref`
                // against the row's own literal sibling fields. Every other
                // marker kind (`$query`/`$cond`/`$expr`, and a `$fn` with no
                // `$ref` in its args) keeps resolving here exactly as before.
                if let FilterValue::FnCall { call } = &fv {
                    if call.args().iter().any(filter_value_contains_field_ref) {
                        return Ok(value.clone());
                    }
                }
                return resolve_filter_query(&fv, &DUMMY_RECORD, ctx).ok_or_else(|| {
                    WriteValueError::MalformedMarker(format!(
                        "marker {:?} failed to resolve (unknown alias/function, or a nested \
                         reference that itself did not resolve)",
                        value
                    ))
                });
            }
            // Not a marker itself — recurse into fields.
            let mut new_map = shamir_types::types::common::new_map();
            for (k, v) in map {
                new_map.insert(k.clone(), resolve_write_value_inner(v, ctx)?);
            }
            Ok(Value::Map(new_map))
        }
        Value::List(arr) => {
            let mut new_arr = Vec::with_capacity(arr.len());
            for v in arr {
                new_arr.push(resolve_write_value_inner(v, ctx)?);
            }
            Ok(Value::List(new_arr))
        }
        other => Ok(other.clone()),
    }
}
