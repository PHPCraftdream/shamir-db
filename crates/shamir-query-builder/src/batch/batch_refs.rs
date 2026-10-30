//! Second half of `Batch`'s impl + `$query`-ref walking helpers — see [`super`].

use super::Batch;
use crate::batch::build_error::BuildError;
use crate::batch::handle::Handle;
use crate::batch::into_batch_op::IntoBatchOp;
use crate::filter::{and, not, or};
use shamir_query_types::batch::extract_base_alias;
use shamir_query_types::batch::{BatchOp, BatchRequest, QueryEntry};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{ReadQuery, Select, SelectItem};
use shamir_types::types::value::QueryValue;

impl Batch {
    // ── wire encoding (build + encode in one step) ─────────────────

    /// Validate and encode as msgpack (named fields) — primary wire format.
    ///
    /// Runs [`Batch::try_build`] first, so deferred registration errors
    /// (duplicate alias, unknown `after`/`when` handle) and unresolved
    /// `$query` refs surface here as [`BuildError`] instead of being sent
    /// silently to the server. The rare codec failure on an already-validated
    /// request is mapped to [`BuildError::SerializationFailed`].
    pub fn to_msgpack(&self) -> Result<Vec<u8>, BuildError> {
        let req = self.try_build()?;
        rmp_serde::to_vec_named(&req).map_err(|e| BuildError::SerializationFailed {
            alias: String::new(),
            reason: e.to_string(),
        })
    }

    /// Build, encode to msgpack, and decode back into a `BatchRequest`.
    ///
    /// Round-trips the request through the wire codec (named msgpack) — the
    /// same path a real client/server uses — so callers (notably tests)
    /// exercise the builder AND the codec in one step.
    pub fn to_request_via_msgpack(&self) -> Result<BatchRequest, BuildError> {
        let bytes = self.to_msgpack()?;
        rmp_serde::from_slice(&bytes).map_err(|e| BuildError::SerializationFailed {
            alias: String::new(),
            reason: e.to_string(),
        })
    }

    // ── build ──────────────────────────────────────────────────────

    /// Infallible build — clones accumulated state into a [`BatchRequest`].
    ///
    /// # NOT a validation point
    /// This does NOT validate and does NOT surface deferred registration
    /// errors (duplicate alias, unknown `after`/`when` handle) — a batch with
    /// such errors silently builds. Production code that turns a batch into a
    /// sent/executed request MUST use [`Batch::try_build`]
    /// (or [`Batch::to_msgpack`]) instead.
    pub fn build(&self) -> BatchRequest {
        BatchRequest {
            id: self.id.clone(),
            name: self.name.clone(),
            transactional: self.transactional,
            isolation: self.isolation,
            visibility_wait: self.visibility_wait,
            durability: self.durability,
            index_visibility: self.index_visibility,
            queries: self.queries.clone(),
            return_all: self.return_all,
            return_only: self.return_only.clone(),
            limits: self.limits.clone(),
            interner_epochs: self.interner_epochs.clone(),
            result_encoding: self.result_encoding,
        }
    }

    /// Build with client-side validation.
    ///
    /// Walks typed operations for `$query` references, using a msgpack
    /// round-trip for operations without a typed walker. Normalizes each base
    /// alias (strip `@`, cut at `[`/`.`), and checks:
    /// - the base alias exists as a key in `queries`
    /// - the base alias is not the referencing entry's own alias
    ///
    /// Also validates `after` entries: each must reference a known alias
    /// and must not reference the entry's own alias.
    pub fn try_build(&self) -> Result<BatchRequest, BuildError> {
        self.validate()?;
        Ok(self.build())
    }

    /// Validate and move the accumulated request without cloning its payloads.
    pub fn try_into_request(self) -> Result<BatchRequest, BuildError> {
        self.validate()?;
        Ok(BatchRequest {
            id: self.id,
            name: self.name,
            transactional: self.transactional,
            isolation: self.isolation,
            visibility_wait: self.visibility_wait,
            durability: self.durability,
            index_visibility: self.index_visibility,
            queries: self.queries,
            return_all: self.return_all,
            return_only: self.return_only,
            limits: self.limits,
            interner_epochs: self.interner_epochs,
            result_encoding: self.result_encoding,
        })
    }

    fn validate(&self) -> Result<(), BuildError> {
        // Surface deferred registration errors first — they happened at a
        // specific registration call site and must not be lost by later calls.
        if let Some(err) = self.deferred_errors.first() {
            return Err(err.clone());
        }
        for (alias, entry) in &self.queries {
            // Validate $query refs — typed walk, see collect_op_query_refs's
            // doc for which BatchOp variants get the fast (no-serialization)
            // path vs. the conservative msgpack-round-trip fallback.
            let refs = collect_op_query_refs(alias, &entry.op)?;
            for raw_ref in &refs {
                let base = extract_base_alias(raw_ref);
                if base == *alias {
                    return Err(BuildError::SelfReference {
                        alias: alias.clone(),
                    });
                }
                if !self.queries.contains_key(&base) {
                    return Err(BuildError::UnknownAlias {
                        alias: base,
                        referenced_by: alias.clone(),
                    });
                }
            }

            // Validate `after` refs.
            for raw in &entry.after {
                // `after` is alias-only ordering; a value-path tail (e.g.
                // "mk[0].id") is almost always a developer mistake expecting
                // `after` to resolve like `$query` does — reject it instead
                // of silently stripping to the base alias (mirrors the
                // planner's `BatchError::AfterPathIgnored`, Phase A).
                if let Some((base, path)) = split_path_tail(raw) {
                    return Err(BuildError::AfterPathIgnored {
                        alias: alias.clone(),
                        raw: format!("{}{}", base, path),
                    });
                }
                let base = extract_base_alias(raw);
                if base == *alias {
                    return Err(BuildError::SelfReference {
                        alias: alias.clone(),
                    });
                }
                if !self.queries.contains_key(&base) {
                    return Err(BuildError::UnknownAlias {
                        alias: base,
                        referenced_by: alias.clone(),
                    });
                }
            }

            // Validate `when` refs — a `when: Filter` may contain `$query`
            // refs (e.g. `Filter::Eq { value: FilterValue::QueryRef{..} }`)
            // that must resolve the same way WHERE-clause `$query` refs do:
            // same alias-existence/self-reference checks. `when` is already
            // a typed `Filter` (unlike `op`, which is an enum with untyped
            // fallback variants) — walked directly via `collect_filter_refs`,
            // no serialization needed at all.
            if let Some(when) = &entry.when {
                let mut refs = Vec::new();
                collect_filter_refs(when, &mut refs);
                for raw_ref in &refs {
                    let base = extract_base_alias(raw_ref);
                    if base == *alias {
                        return Err(BuildError::SelfReference {
                            alias: alias.clone(),
                        });
                    }
                    if !self.queries.contains_key(&base) {
                        return Err(BuildError::UnknownAlias {
                            alias: base,
                            referenced_by: alias.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    // ── internal ───────────────────────────────────────────────────

    /// Declare that `dependent` must execute AFTER `on` (ordering edge).
    ///
    /// `$query` creates an edge by itself; `after` is needed only where there
    /// is no data flow (e.g. DDL→DML ordering). Prefer the `*_after`
    /// registration methods (e.g. [`Batch::insert_after`]) when the
    /// dependency is known at the entry's registration site — they avoid the
    /// `dependent`/`on` argument-order footgun this post-hoc method has (both
    /// arguments are `&Handle`, so the compiler can't catch a transposition).
    ///
    /// Not to be confused with `Query::after` (`crate::query::Query::after`)
    /// — that's single-query keyset pagination; this is inter-query DAG
    /// ordering. See that method's docstring for the reverse cross-reference.
    pub fn after(&mut self, dependent: &Handle, on: &Handle) -> &mut Self {
        // Unknown handles are deferred errors (surfaced by `try_build`), not
        // silent no-ops — a discarded ordering intent must be diagnosable.
        if !self.queries.contains_key(dependent.alias()) {
            self.deferred_errors
                .push(BuildError::UnknownOrderingTarget {
                    alias: dependent.alias().to_string(),
                    operation: "after",
                });
            return self;
        }
        if !self.queries.contains_key(on.alias()) {
            self.deferred_errors
                .push(BuildError::UnknownOrderingTarget {
                    alias: on.alias().to_string(),
                    operation: "after",
                });
            return self;
        }
        if let Some(entry) = self.queries.get_mut(dependent.alias()) {
            entry.after.push(on.alias().to_string());
        }
        self
    }

    /// Set (or replace) the `when` guard on an already-registered entry.
    ///
    /// The op behind `handle` executes iff `condition` evaluates to `true`
    /// (via the same [`Filter`] evaluation machinery WHERE clauses use — see
    /// `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`,
    /// Decision 1). A skipped op produces a `skipped: true` result and
    /// cascades the skip to any dependent with a real `$query`/`when` data-flow
    /// edge onto it — `after`-only edges do not cascade (Decision 2).
    ///
    /// Post-hoc, mirroring [`Batch::after`]'s style: register the entry first
    /// via `query`/`insert`/... , then attach the guard. Prefer this when the
    /// condition is computed/known only after the entry's registration call
    /// site; for the common "declare with its guard in one call" case, thread
    /// `Filter` through the escape-hatch `op`/`op_after` call and set `when`
    /// afterwards, or use [`Batch::switch`] for multi-branch dispatch.
    pub fn when(&mut self, handle: &Handle, condition: Filter) -> &mut Self {
        // Unknown handles are deferred errors (surfaced by `try_build`), not
        // silent no-ops — a discarded guard intent must be diagnosable.
        if !self.queries.contains_key(handle.alias()) {
            self.deferred_errors
                .push(BuildError::UnknownOrderingTarget {
                    alias: handle.alias().to_string(),
                    operation: "when",
                });
            return self;
        }
        if let Some(entry) = self.queries.get_mut(handle.alias()) {
            entry.when = Some(condition);
        }
        self
    }

    /// Multi-branch conditional dispatch — builder-only sugar (no new wire
    /// primitive; see the ADR, Decision 1, "Switch-case is confirmed as
    /// builder-only sugar").
    ///
    /// Registers one `QueryEntry` per case plus one for `default`, each
    /// tagged with a complementary `when` filter so exactly one branch's op
    /// runs at execution time for a given row of runtime values:
    /// - case 1 → `when: case1_condition`
    /// - case 2 → `when: AND(NOT case1_condition, case2_condition)`
    /// - ...
    /// - default → `when: NOT(case1_condition OR case2_condition OR ...)`
    ///
    /// Lowering K cases clones earlier conditions into later guards, producing
    /// O(K²) filter nodes and deep-clone work. Changing the wire representation
    /// requires a separate design decision.
    ///
    /// `cases` is `(alias, condition, op)` triples; `default` is
    /// `(alias, op)`. Returns the [`Handle`]s for every case (in order)
    /// followed by the default's `Handle`.
    ///
    /// ```ignore
    /// let handles = b.switch(
    ///     vec![
    ///         ("vip_email", is_vip.clone(), send_vip_email_op),
    ///         ("regular_email", is_regular.clone(), send_regular_email_op),
    ///     ],
    ///     ("newbie_email", send_newbie_email_op),
    /// );
    /// ```
    pub fn switch(
        &mut self,
        cases: Vec<(impl Into<String>, Filter, impl IntoBatchOp)>,
        default: (impl Into<String>, impl IntoBatchOp),
    ) -> Vec<Handle> {
        let mut handles = Vec::with_capacity(cases.len() + 1);
        let mut seen_conditions: Vec<Filter> = Vec::with_capacity(cases.len());

        for (alias, condition, op) in cases {
            let guard = seen_conditions
                .iter()
                .cloned()
                .fold(condition.clone(), |acc, prior| and(vec![not(prior), acc]));
            let handle = self.add_entry(alias, op.into_batch_op(), true);
            self.when(&handle, guard);
            handles.push(handle);
            seen_conditions.push(condition);
        }

        let (default_alias, default_op) = default;
        let default_guard = not(or(seen_conditions));
        let default_handle = self.add_entry(default_alias, default_op.into_batch_op(), true);
        self.when(&default_handle, default_guard);
        handles.push(default_handle);

        handles
    }
    pub(super) fn add_entry_after(
        &mut self,
        alias: impl Into<String>,
        op: BatchOp,
        return_result: bool,
        after: &[&Handle],
    ) -> Handle {
        let alias = alias.into();
        // Duplicate alias: keep the FIRST registered operation intact and
        // record a deferred error (surfaced by `try_build`) — re-registering
        // an alias must never silently replace (and thus drop) an earlier op.
        if self.queries.contains_key(&alias) {
            self.deferred_errors.push(BuildError::DuplicateAlias {
                alias: alias.clone(),
            });
            return Handle { alias };
        }
        self.queries.insert(
            alias.clone(),
            QueryEntry {
                op,
                return_result,
                after: after.iter().map(|h| h.alias().to_string()).collect(),
                // Epic03/C (#646) adds builder-level `when` ergonomics; this
                // mechanical field is required here only to keep the struct
                // literal compiling after Epic03/B added `QueryEntry.when`.
                when: None,
            },
        );
        Handle { alias }
    }
}

// ============================================================================
// $query ref walking helpers (mirrors planner.rs logic)
// ============================================================================

/// Collect all `$query` string values from a `QueryValue` tree.
///
/// The `FilterValue::QueryRef` variant serializes as `{"$query": "@alias", ...}`.
/// Walking the `QueryValue::Map` for the `"$query"` key mirrors the planner's
/// logic directly on the msgpack-decoded tree.
fn collect_query_refs(value: &shamir_types::types::value::QueryValue, out: &mut Vec<String>) {
    match value {
        QueryValue::Map(map) => {
            // Reserved-key marker convention mirroring the planner's
            // `extract_deps_from_value` (shamir-query-types
            // ops/batch/plan/planner.rs, the #641 fix): a marker is a map of
            // len 1 whose single key is one of "$query"/"$fn"/"$cond"/
            // "$expr", OR a len-2 map with "$query"+"path" where `path` is a
            // STRING — the planner's typed decode (`QueryRef { path:
            // Option<String> }`) only accepts strings, so a non-string `path`
            // is literal data, not a marker. A map that merely CONTAINS a
            // "$query" key among others is literal write data
            // (RV-QUERY-BUILDER-224EF7B1), not a marker.
            let is_marker = match map.len() {
                1 => ["$query", "$fn", "$cond", "$expr"]
                    .iter()
                    .any(|k| map.contains_key(*k)),
                2 => {
                    map.contains_key("$query")
                        && matches!(map.get("path"), Some(QueryValue::Str(_)))
                }
                _ => false,
            };
            if is_marker {
                if let Some(QueryValue::Str(s)) = map.get("$query") {
                    out.push(s.clone());
                }
                // "$fn"/"$cond"/"$expr" markers carry refs INSIDE their
                // payload (nested QueryRef operands) — keep recursing so
                // those are still recognised, like the planner's decode into
                // `FilterValue` does.
            }
            for v in map.values() {
                collect_query_refs(v, out);
            }
        }
        QueryValue::List(arr) => {
            for v in arr {
                collect_query_refs(v, out);
            }
        }
        _ => {}
    }
}

// ============================================================================
// #1093: typed $query-ref walkers — no msgpack round-trip.
//
// `try_build` used to serialize EVERY entry's op to msgpack and decode it
// back into a `QueryValue` tree purely to walk it for `"$query"` keys (see
// `collect_query_refs` above), paying an O(payload) allocate+encode+decode
// pass on every call for even the largest insert/update payloads. Most of
// the query-ref-bearing data is ALREADY typed (`InsertOp.values`,
// `UpdateOp.set`, `SetOp.key`/`value` are `QueryValue` — `collect_query_refs`
// above already walks a `&QueryValue` directly, no serialization needed for
// those) or reachable through `Filter`/`FilterValue`'s own bounded,
// already-typed enum shape. These two functions walk that shape directly.
//
// Investigated (2026-08-11) whether to extend this typed path to ALL
// `BatchOp` variants (not just the 5 covered by `collect_op_query_refs`
// below) and found the full surface is deeper than the review that raised
// this task assumed: `ReadQuery.select` alone required mapping
// `Select`/`SelectItem`/`SelectExpr` (a THIRD, separate expression tree from
// `Filter`/`FilterExpr`) before this could be implemented correctly for
// reads. The other ~75 `BatchOp` variants (DDL/admin/introspection ops) have
// NOT been individually audited for a similarly-hidden query-ref-bearing
// field — `collect_op_query_refs`'s fallback arm below intentionally keeps
// the ORIGINAL, unconditionally-correct msgpack round-trip for all of them,
// rather than risk silently under-validating one. See task #1093 for the
// full investigation notes if extending this further.

/// Collect all `$query` refs from a `Filter` tree — exhaustive over every
/// variant (this codebase's convention for `Filter`/`BatchOp` matches, per
/// `BatchOp::is_write`'s own doc: a new variant must compile-error here
/// until explicitly classified, not silently fall through a wildcard).
pub(super) fn collect_filter_refs(filter: &Filter, out: &mut Vec<String>) {
    match filter {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::Contains { value, .. }
        | Filter::FieldEq { value, .. } => collect_filter_value_refs(value, out),
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                collect_filter_value_refs(v, out);
            }
        }
        Filter::Between { from, to, .. } => {
            collect_filter_value_refs(from, out);
            collect_filter_value_refs(to, out);
        }
        Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Regex { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. } => {}
        Filter::And { filters } | Filter::Or { filters } => {
            for f in filters {
                collect_filter_refs(f, out);
            }
        }
        Filter::Not { filter } => collect_filter_refs(filter, out),
        Filter::ValueCompare { left, right, .. } => {
            collect_filter_value_refs(left, out);
            collect_filter_value_refs(right, out);
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for v in args {
                    collect_filter_value_refs(v, out);
                }
            }
            collect_filter_value_refs(value, out);
        }
    }
}

/// Collect all `$query` refs from a `FilterValue` tree — exhaustive over
/// every variant, same convention as [`collect_filter_refs`].
fn collect_filter_value_refs(value: &FilterValue, out: &mut Vec<String>) {
    match value {
        FilterValue::QueryRef { alias, .. } => out.push(alias.clone()),
        FilterValue::Array(vs) => {
            for v in vs {
                collect_filter_value_refs(v, out);
            }
        }
        FilterValue::FnCall { call } => match call {
            shamir_query_types::filter::FnCall::Simple(_) => {}
            shamir_query_types::filter::FnCall::Complex { args, .. } => {
                for v in args {
                    collect_filter_value_refs(v, out);
                }
            }
        },
        FilterValue::Expr { expr } => {
            for v in &expr.args {
                collect_filter_value_refs(v, out);
            }
        }
        FilterValue::Cond { cond } => {
            collect_filter_refs(&cond.condition, out);
            collect_filter_value_refs(&cond.then, out);
            collect_filter_value_refs(&cond.or_else, out);
        }
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::Binary(_)
        | FilterValue::String(_)
        | FilterValue::FieldRef { .. }
        | FilterValue::Param { .. } => {}
    }
}

/// Collect all `$query` refs from a `Select` projection — `SelectItem`'s
/// `Function`/`AggregateFn` carry `args: Vec<FilterValue>`; both aggregate
/// variants also carry an optional per-row `input`, walked by the same collector.
/// `Expression`'s `SelectExpr` is a closed arithmetic tree over field refs
/// and literals ONLY (no `$query`/`$fn`/`$ref` capability — verified against
/// its own definition, `select_expr.rs`) so it needs no walk.
fn collect_select_refs(select: &Select, out: &mut Vec<String>) {
    for item in &select.items {
        match item {
            SelectItem::Function { args, .. } => {
                for v in args {
                    collect_filter_value_refs(v, out);
                }
            }
            SelectItem::AggregateFn { args, input, .. } => {
                for v in args {
                    collect_filter_value_refs(v, out);
                }
                if let Some(input) = input {
                    collect_filter_value_refs(input, out);
                }
            }
            SelectItem::Aggregate { input, .. } => {
                if let Some(input) = input {
                    collect_filter_value_refs(input, out);
                }
            }
            SelectItem::All
            | SelectItem::Field { .. }
            | SelectItem::CountAll { .. }
            | SelectItem::Expression { .. } => {}
        }
    }
}

/// Collect all `$query` refs from a `ReadQuery` — `where`, `select`'s
/// function-argument lists, and `group_by`'s `having` clause.
fn collect_read_query_refs(q: &ReadQuery, out: &mut Vec<String>) {
    if let Some(filter) = &q.r#where {
        collect_filter_refs(filter, out);
    }
    collect_select_refs(&q.select, out);
    if let Some(gb) = &q.group_by {
        if let Some(having) = &gb.having {
            collect_filter_refs(having, out);
        }
    }
}

/// Collect all `$query` refs from a single entry's `op`.
///
/// Typed fast paths cover read, write, call, subscribe, sub-batch and for-each
/// operations. Subscribe has no outer dependencies, matching the planner;
/// sub-batch and for-each only inspect values in the outer scope. Other
/// operations use the conservative msgpack round-trip.
pub(super) fn collect_op_query_refs(alias: &str, op: &BatchOp) -> Result<Vec<String>, BuildError> {
    let mut refs = Vec::new();
    match op {
        BatchOp::Read(q) => collect_read_query_refs(q, &mut refs),
        BatchOp::Insert(i) => {
            for v in &i.values {
                collect_query_refs(v, &mut refs);
            }
        }
        BatchOp::Update(u) => {
            if let Some(f) = &u.where_clause {
                collect_filter_refs(f, &mut refs);
            }
            collect_query_refs(&u.set, &mut refs);
        }
        BatchOp::Set(s) => {
            collect_query_refs(&s.key, &mut refs);
            collect_query_refs(&s.value, &mut refs);
        }
        BatchOp::Delete(d) => collect_filter_refs(&d.where_clause, &mut refs),
        BatchOp::Call(call) => {
            for value in &call.params {
                collect_filter_value_refs(value, &mut refs);
            }
        }
        // The planner treats subscriptions as independent operations; their
        // filters and delivery payloads execute in subscription scope.
        BatchOp::Subscribe(_) => {}
        // Sub-batch / for_each: outer deps come EXCLUSIVELY from `bind`
        // values / `over` (planner.rs `extract_entry_deps`, the
        // `BatchOp::Batch` / `BatchOp::ForEach` arms). Do NOT descend into
        // the inner batch's queries — those are planned recursively at
        // execution time, and their internal aliases must never be validated
        // against THIS batch's alias set (RV-QUERY-BUILDER-F3ED5A33).
        BatchOp::Batch(sub) => {
            for fv in sub.bind.values() {
                collect_filter_value_refs(fv, &mut refs);
            }
        }
        BatchOp::ForEach(fe) => collect_filter_value_refs(&fe.over, &mut refs),
        other => {
            let bytes =
                rmp_serde::to_vec_named(other).map_err(|e| BuildError::SerializationFailed {
                    alias: alias.to_string(),
                    reason: e.to_string(),
                })?;
            let qv: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes)
                .map_err(|e| BuildError::SerializationFailed {
                    alias: alias.to_string(),
                    reason: e.to_string(),
                })?;
            collect_query_refs(&qv, &mut refs);
        }
    }
    Ok(refs)
}

/// If `s` carries a path tail (`[`/`.` after the base alias), return
/// `(base_alias, path_tail)`.
///
/// Local port of `shamir_query_types::batch::alias::split_path_tail`
/// (that fn is `pub(crate)` there, not reachable from this crate) — kept in
/// sync by the shared `extract_base_alias` this file already imports; the
/// planner's version is the source of truth for the semantics.
pub(super) fn split_path_tail(s: &str) -> Option<(String, String)> {
    let stripped = s.strip_prefix('@').unwrap_or(s);
    stripped
        .find(['[', '.'])
        .map(|pos| (stripped[..pos].to_string(), stripped[pos..].to_string()))
}
