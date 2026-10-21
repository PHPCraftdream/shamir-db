use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::batch::BatchLimits;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::QueryEntry;
use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;

/// Absolute, server-enforced ceiling on `ForEach` iteration count —
/// independent of whatever a client-supplied `BatchLimits.max_iterations`
/// claims. Closes a DoS gap: `max_iterations` is entirely client-supplied
/// wire data (#666), so a client could otherwise set it to `usize::MAX`
/// and defeat the gate for a `ForEach` with a dynamic (non-literal-array)
/// `over` source, whose iteration count the plan-time `virtual_units`
/// check (`BatchPlanner::plan`) cannot see ahead of time.
pub(crate) const ABSOLUTE_MAX_FOR_EACH_ITERATIONS: usize = 100_000;

/// The effective `max_iterations` gate value for a `ForEach` body's
/// `limits` — the client-supplied value, clamped down to the server's
/// absolute ceiling (`ABSOLUTE_MAX_FOR_EACH_ITERATIONS`). A `min`, never a
/// replacement: a client-supplied value BELOW the ceiling is respected
/// unchanged (#653 behavior preserved), only a value AT OR ABOVE the
/// ceiling is clamped down (#666).
pub(crate) fn effective_max_iterations(limits: &BatchLimits) -> usize {
    limits.max_iterations.min(ABSOLUTE_MAX_FOR_EACH_ITERATIONS)
}

/// Build resolved_refs map containing only DataFlow-provenance dependencies.
///
/// `after`-only edges (`EdgeKind::Explicit`) are ordering hints, NOT data
/// access grants — they must NOT leak the referenced alias's result into
/// the dependent op's `FilterContext`. Only edges with a real `$query`
/// reference (`EdgeKind::DataFlow` or `EdgeKind::Both`) resolve to data.
pub(in crate::query::batch) fn build_resolved_refs(
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
) -> TMap<String, QueryResult> {
    let cap = provenance.map_or(0, |p| p.len());
    let mut refs = new_map_wc(cap);
    if let Some(provenance) = provenance {
        for (dep_alias, kind) in provenance {
            if !kind.is_data_flow() {
                continue;
            }
            if let Some(result) = all_results.get(dep_alias) {
                refs.insert(dep_alias.clone(), result.clone());
            }
        }
    }
    refs
}

/// Build the "did not run" [`QueryResult`] for a skipped alias (Epic03/B,
/// #645) — own `when` evaluated `false`, or cascaded from a skipped
/// `DataFlow`/`Both`-provenance dependency (ADR Decision 2/4). Empty
/// records/stats/pagination/value/explain, `skipped: true` — recognizably
/// empty-but-explicit, distinct from "0 rows matched" (`skipped: false`,
/// `stats: Some(..)`) and from "filtered by `return_only`" (alias absent
/// from `results` entirely, unrelated to this flag).
pub(in crate::query::batch) fn skipped_query_result() -> QueryResult {
    QueryResult::skipped()
}

/// True if `alias`'s dependencies (per `provenance`) contain at least one
/// `DataFlow`/`Both`-provenance edge onto an already-skipped result in
/// `all_results`.
///
/// `Explicit`-only (`after`) edges do NOT cascade — pure ordering, no data
/// access, per Epic01/A's already-shipped invariant (see
/// `build_resolved_refs`'s doc comment above). Only edges that actually
/// grant data access propagate a skip: if `B` never observes `A`'s result
/// (a plain `after` edge), `B` running or not running is independent of
/// whether `A` ran.
fn has_skipped_data_flow_dep(
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
) -> bool {
    let Some(provenance) = provenance else {
        return false;
    };
    provenance.iter().any(|(dep_alias, kind)| {
        kind.is_data_flow() && all_results.get(dep_alias).is_some_and(|r| r.skipped)
    })
}

/// Decide whether `entry` should be skipped for this stage, and return the
/// skip result if so.
///
/// Precedence (documented decision — the ADR does not state this
/// explicitly, so Phase B fixes it here): **cascade wins over the entry's
/// own `when`.** If a real data dependency was skipped, the current op's
/// inputs are already missing/undefined (a `$query` ref onto a skipped
/// alias would resolve to nothing) — evaluating the op's own `when` against
/// an incomplete `resolved_refs` would be evaluating a condition over data
/// that was never produced, not a meaningful "did the branch's guard hold"
/// check. So cascade is checked FIRST and short-circuits before compiling
/// or evaluating `entry.when` at all.
pub(in crate::query::batch) fn resolve_skip(
    entry: &QueryEntry,
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
    resolved_refs: &TMap<String, QueryResult>,
    params: &TMap<String, QueryValue>,
    scalars: ScalarResolver,
) -> bool {
    // Cascade: a genuine (DataFlow/Both) dependency was itself skipped.
    // Takes precedence over the entry's own `when` — see this function's
    // doc comment above for the rationale (undocumented in the ADR;
    // decided here since a skipped dependency means this op's inputs are
    // already missing, so evaluating its own `when` would be evaluating a
    // condition over data that was never produced).
    if has_skipped_data_flow_dep(all_results, provenance) {
        return true;
    }

    let Some(filter) = &entry.when else {
        return false;
    };

    // Evaluate `when` against an empty synthetic record via a scratch
    // interner — `when` has no per-row context (ADR Decision 1): only
    // `$query`/`$fn`/`$param`/literals are meaningful inside a `when`
    // filter (no `FieldRef` support needed/meaningful), and
    // `InnerValue::Null`'s `RecordRef` impl is exactly the "no field data"
    // record already used for sub-batch `bind` resolution (see
    // `QueryRunner::run`'s `dummy_record` below). A fresh `Interner::new()`
    // is correct here (not the table's real interner) for the same reason
    // the bind-resolution path uses one: any field path in `when` folds to
    // `FilterNode::True`/`False` regardless of which interner is used,
    // since no field ever actually resolves against a real record.
    let scratch = shamir_types::core::interner::Interner::new();
    let ctx = FilterContext::new(&scratch, resolved_refs)
        .with_scalars(scalars)
        .with_params(params);
    let node = crate::query::filter::compile_filter(filter, &scratch);
    !node.matches(&InnerValue::Null, &ctx)
}
