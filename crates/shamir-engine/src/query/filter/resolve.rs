use std::cmp::Ordering;

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use shamir_types::codecs::interned::{inner_value_to_query_value, query_value_to_inner};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::RecordRef;
use shamir_types::types::value::{InnerValue, QueryValue, Value};
use smallvec::SmallVec;

use super::compile::compile_filter;
use super::cond_cache::cond_cache_get;
use super::eval_context::FilterContext;
use super::filter_node::CompactPath;
use crate::query::filter::{FilterExpr, FilterExprOp, FilterValue};
use crate::query::read::QueryResult;

/// Extract a value from an InnerValue by a path of interned keys.
///
/// Borrowing variant: walks the record in-place without cloning the
/// resolved leaf. All filter nodes below use this — the old owned
/// variant survives only for callers outside the eval module that
/// still rely on `Option<InnerValue>`.
#[inline]
pub fn resolve_field_ref<'a>(record: &'a InnerValue, path: &[u64]) -> Option<&'a InnerValue> {
    let mut cur = record;
    for &id in path {
        match cur {
            InnerValue::Map(map) => {
                let key = InternerKey::new(id);
                cur = map.get(&key)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Owned variant — pinned by `table/read_exec.rs` (two field-resolution
/// call-sites) plus unit tests. Hot filter paths use `resolve_field_ref`
/// and never call this. §5b floor: eliminable only when read_exec moves
/// those sites to the lens.
pub fn resolve_field(record: &InnerValue, path: &[u64]) -> Option<InnerValue> {
    resolve_field_ref(record, path).cloned()
}

/// Resolve a field path (segments) into interned u64 keys.
pub fn intern_field_path(field: &[String], interner: &Interner) -> Option<Vec<u64>> {
    let mut keys = Vec::with_capacity(field.len());
    for part in field {
        let interned = interner.get_ind(part)?;
        keys.push(interned.id());
    }
    Some(keys)
}

/// Inline-allocated variant of [`intern_field_path`] for hot compile paths.
///
/// Returns a `CompactPath` (`SmallVec<[InternerKey; 4]>`) so `FilterNode`
/// callers that store the result in `field_path` avoid a heap allocation for
/// the typical 1-3 segment case AND can pass the path straight to `RecordRef`
/// methods without re-wrapping each segment into an `InternerKey` per row
/// (F10 — previously returned `SmallVec<[u64; 4]>` and every `matches()` arm
/// rebuilt a `SmallVec<[InternerKey; 4]>` from it).
pub(super) fn intern_field_path_compact(
    field: &[String],
    interner: &Interner,
) -> Option<CompactPath> {
    let mut keys: CompactPath = SmallVec::with_capacity(field.len());
    for part in field {
        // `get_ind` returns `InternerKey` directly — store it as-is so
        // `matches()` arms can pass `field_path` to `RecordRef` methods
        // without re-wrapping each segment per row (F10).
        keys.push(interner.get_ind(part)?);
    }
    Some(keys)
}

/// Lossy numeric → `f64` for cross-type comparison (NaN on overflow). Works
/// for both `Decimal` and `BigInt` via the shared `ToPrimitive` trait.
///
/// CR-C5 (#780): this remains the deliberate, ACCEPTED-approximation path
/// for `Big`↔`F64` (and `Dec`↔`F64`) only — `F64` is itself an inherently
/// imprecise IEEE-754 column type, so comparing an exact `BigInt`/`Decimal`
/// against it can only ever mean "which `f64` is this value closest to",
/// not a bug to fix. `Big`↔`Int` and `Big`↔`Dec` no longer route through
/// this helper (see [`compare_values`]'s dedicated exact arms) — those two
/// pairs compare two EXACT types, where a lossy `f64` intermediate was a
/// genuine comparison-code bug, not an inherent-approximation tradeoff.
#[inline]
fn lossy_f64<T: ToPrimitive>(v: &T) -> f64 {
    v.to_f64().unwrap_or(f64::NAN)
}

/// Exact `i64` vs `f64` comparison — CR-D3 (#784).
///
/// `f64` has an 11-bit exponent, enough to represent every integer up to
/// `2^63` in MAGNITUDE (though not every value at high magnitude, since the
/// 52-bit mantissa runs out of precision past `2^53`) — this is exactly what
/// makes a bounds-check + `floor`/`fract` technique exact without
/// arbitrary-precision arithmetic (no `BigInt` needed, unlike `Big`↔`F64`
/// which is an inherent, unfixable approximation because `F64` itself is the
/// imprecise side there).
///
/// `i64::MIN == -2^63` and `i64::MAX == 2^63 - 1` — both `-2^63` and `2^63`
/// are exact powers of two, always exactly representable as `f64` literals.
/// `f < -2^63` means `f < i64::MIN <= i`, so `i > f`. `f >= 2^63` means
/// `f >= i64::MAX + 1 > i64::MAX >= i`, so `i < f`. For finite `f` in
/// `[-2^63, 2^63)`: any `f64` with `|f| >= 2^53` has no fractional bits
/// available at all (the entire 52-bit mantissa is consumed by the integer
/// part at that exponent), so `f.fract() == 0.0` identically and
/// `f.floor() == f` exactly for that whole magnitude range; below `2^53`,
/// `floor`/`fract` behave as normal exact-integer-valued doubles. Either
/// way, `f.floor()` is an exact integer value within `[-2^63, 2^63 - 1]`,
/// i.e. `i64`'s full range, so `f.floor() as i64` is a lossless cast. From
/// there, comparing `i` against `f_floor_i64` as plain integers settles
/// everything except the exact-equal case, where comparing `f` against
/// `f_floor` directly breaks the tie: `i == f.floor()` and `f > f_floor`
/// means `f > i`. This must compare against `f_floor`, NOT `f.fract()` --
/// `f.fract()` is `f - f.trunc()` (truncation-based, sign-preserving), so
/// for negative `f` it is negative or zero, never positive, even when `f`
/// has a nonzero fractional part (e.g. `(-0.5_f64).fract() == -0.5`). Only
/// `f - f.floor()` is guaranteed `>= 0` for every finite `f`, positive or
/// negative alike.
#[inline]
fn cmp_i64_f64(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None; // preserve the EXISTING NaN convention this codebase
                     // already uses for F64<->F64 (partial_cmp's own NaN
                     // handling) -- do not invent new NaN semantics here.
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        }); // any finite i64 is < +inf, > -inf.
    }
    // f is finite from here on. Bound f against i64's range using EXACT
    // powers of two.
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if f < I64_MIN_AS_F64 {
        return Some(Ordering::Greater); // i (>= i64::MIN) > f
    }
    if f >= I64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(Ordering::Less); // i (<= i64::MAX) < f
    }
    // f is finite and within [-2^63, 2^63) -- f.floor() is an exact integer
    // value in that range, losslessly representable as i64 (see derivation
    // above).
    let f_floor = f.floor();
    let f_floor_i64 = f_floor as i64;
    match i.cmp(&f_floor_i64) {
        Ordering::Equal => {
            // i == floor(f) exactly. f >= f_floor always (floor rounds
            // DOWN, never up) -- f > f_floor iff f has ANY nonzero
            // fractional part, positive or negative f alike. Comparing
            // against f_floor directly (not f.fract(), which is
            // TRUNC-based and sign-preserving -- negative for negative
            // fractional f, the bug this replaces) is correct for every
            // sign.
            if f > f_floor {
                Some(Ordering::Less)
            } else {
                Some(Ordering::Equal)
            }
        }
        other => Some(other),
    }
}

/// Exact `Decimal` vs `BigInt` comparison via cross-multiplication —
/// CR-C5 (#780).
///
/// `Decimal` is a 96-bit fixed-point type: `dec == mantissa / 10^scale`
/// (`mantissa: i128` is already signed via [`Decimal::mantissa`];
/// `scale: u32` is the number of fractional digits). To compare it against
/// an arbitrary-precision `BigInt` without ever rounding through `f64`, both
/// sides are lifted to the same denominator and compared as exact integers:
///
/// `big_value / 1 == mantissa / 10^scale`
/// `<=>` (cross-multiply by the positive scale factor)
/// `big_value * 10^scale == mantissa`
///
/// `BigInt` arithmetic has no magnitude limit, so this is exact regardless
/// of how large `big` or how many significant digits `dec` carries. Sign is
/// handled naturally: `mantissa` is already signed and `BigInt::cmp` handles
/// negative values correctly, so no separate sign-case branching is needed.
#[inline]
fn cmp_big_dec(big: &BigInt, dec: &Decimal) -> Ordering {
    let scale_factor = BigInt::from(10u32).pow(dec.scale());
    let lhs = big * scale_factor;
    let rhs = BigInt::from(dec.mantissa());
    lhs.cmp(&rhs)
}

/// Compare two `Value<K>` scalars. Returns an `Ordering` if comparable.
///
/// C6 (#80): generic over the key type. Only the scalar arms
/// (Null/Bool/Int/F64/Str) participate, and those are key-agnostic — so this
/// serves BOTH the name-keyed filter path (`QueryValue`) and the id-keyed
/// aggregator path (`InnerValue`, until S4) with ZERO conversion at either
/// call site (anti-formal: no inner↔query bridge added). The ordering is
/// byte-identical to the previous `InnerValue`-only form.
///
/// #667: `(Null, Null) => Some(Equal)` below is a DELIBERATE choice, not an
/// oversight that happened to fall out of the match arms' ordering — callers
/// rely on it, specifically `FilterNode::ValueCompare::matches`
/// (`filter_node.rs`), which treats two resolved-to-literal-`null` operands
/// as genuinely equal (`Eq`/`Gte`/`Lte` all `true`). This is intentionally
/// different from that same `matches()`'s OTHER "nothing to compare" path —
/// a genuinely unresolvable operand (`resolve_filter_query` returning
/// `None`) — which instead makes only `Ne` `true`. See the doc comment on
/// `FilterNode::ValueCompare` for the full 3-way breakdown.
#[inline]
pub fn compare_values<K>(a: &Value<K>, b: &Value<K>) -> Option<Ordering>
where
    K: Eq + std::hash::Hash + Ord + Clone + serde::Serialize + std::fmt::Debug,
{
    match (a, b) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        // Int<->F64: CR-D3 (#784), follow-up to CR-C5 (#780)'s own
        // re-verification finding — the plain `as f64` cast was lossy for
        // large `i64` magnitudes with NO `Big` involved (`i64::MAX` is far
        // above `2^53`, and a plain `i64` column stays `Int` all the way to
        // `i64::MAX`; only values that overflow `i64` entirely promote to
        // `Big`). Now exact via `cmp_i64_f64`'s bounds-check + floor/fract
        // technique — no `BigInt` needed, since `f64`'s 11-bit exponent
        // already covers `i64`'s full magnitude range exactly at the
        // boundaries, and any in-range value's `floor()` is a lossless i64
        // cast (see `cmp_i64_f64`'s doc comment for the full derivation).
        (Value::Int(a), Value::F64(b)) => cmp_i64_f64(*a, *b),
        (Value::F64(a), Value::Int(b)) => cmp_i64_f64(*b, *a).map(Ordering::reverse),
        (Value::F64(a), Value::F64(b)) => a.partial_cmp(b),
        (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
        // Dec: exact for Dec/Dec and Int↔Dec (`Decimal` represents every
        // `i64` exactly); F64↔Dec uses the f64 fallback.
        (Value::Dec(a), Value::Dec(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Dec(b)) => Some(Decimal::from(*a).cmp(b)),
        (Value::Dec(a), Value::Int(b)) => Some(a.cmp(&Decimal::from(*b))),
        (Value::F64(a), Value::Dec(b)) => a.partial_cmp(&lossy_f64(b)),
        (Value::Dec(a), Value::F64(b)) => lossy_f64(a).partial_cmp(b),
        // Big: exact `BigInt::cmp` for Big/Big (unchanged — already exact).
        (Value::Big(a), Value::Big(b)) => Some(a.cmp(b)),
        // Big<->Int: CR-C5 (#780), MUST-fix. An `i64` always converts to
        // `BigInt` losslessly (unlike the reverse `f64` conversion this
        // replaces) — compare exactly via `BigInt::cmp`, no approximation.
        (Value::Int(a), Value::Big(b)) => Some(BigInt::from(*a).cmp(b)),
        (Value::Big(a), Value::Int(b)) => Some(a.cmp(&BigInt::from(*b))),
        // Big<->F64: DELIBERATE, accepted approximation — `F64` is itself an
        // inherently imprecise IEEE-754 column type, so comparing an EXACT
        // `BigInt` against an APPROXIMATE `f64` has no single "correct"
        // exact answer beyond "which f64 is this closest to". This is
        // distinct from the Int/Dec arms above/below: those compare two
        // EXACT types, where the `f64` intermediate was a genuine
        // comparison-code bug (now fixed); here the approximation is
        // inherent to the F64 column's own nature, not introduced by this
        // comparison code. Left as-is on purpose — do not "fix" this arm.
        (Value::F64(a), Value::Big(b)) => a.partial_cmp(&lossy_f64(b)),
        (Value::Big(a), Value::F64(b)) => lossy_f64(a).partial_cmp(b),
        // Big<->Dec: CR-C5 (#780), SHOULD-fix. Both sides are exact types
        // (arbitrary-precision integer vs 96-bit fixed-point) — compare via
        // cross-multiplication in `BigInt` space (see `cmp_big_dec`), never
        // rounding through `f64`.
        (Value::Dec(a), Value::Big(b)) => Some(cmp_big_dec(b, a).reverse()),
        (Value::Big(a), Value::Dec(b)) => Some(cmp_big_dec(a, b)),
        // Big<->Str: FG-6. `lit_u64`/wire encoding represent a promoted
        // `u64 > i64::MAX` as its exact decimal `String` (the same canonical
        // form `Value::Big` serialises to — see `hashable_query_value.rs`'s
        // `canonical_eq`, which already treats `Big(a) == Str(b) iff
        // a.to_string() == b` as the established cross-type equality). This
        // arm extends that convention to `Ordering`: if `b` parses as an
        // exact integer, compare numerically via `BigInt: Ord` (exact, no
        // precision loss); a non-numeric string is not comparable (`None`).
        (Value::Big(a), Value::Str(b)) => b.parse::<num_bigint::BigInt>().ok().map(|n| a.cmp(&n)),
        (Value::Str(a), Value::Big(b)) => a.parse::<num_bigint::BigInt>().ok().map(|n| n.cmp(b)),
        _ => None,
    }
}

/// Convert a literal `FilterValue` to `QueryValue` without record/context.
///
/// C6 (#80): the name-keyed analogue of the legacy `filter_value_to_inner`.
/// Returns `None` for non-literal variants (FieldRef, QueryRef, FnCall,
/// Expr, Cond, Param, Array).
#[inline]
pub fn filter_value_to_query(fv: &FilterValue) -> Option<QueryValue> {
    match fv {
        FilterValue::Null => Some(QueryValue::Null),
        FilterValue::Bool(b) => Some(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Some(QueryValue::Int(*i)),
        FilterValue::Float(f) => Some(QueryValue::F64(*f)),
        FilterValue::String(s) => Some(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(QueryValue::Bin(b.clone())),
        _ => None,
    }
}

/// Convert a literal `FilterValue` to `InnerValue` without record/context.
///
/// **Legacy adapter** — kept for out-of-scope callers that still bind to
/// `InnerValue`. New filter code uses [`filter_value_to_query`] (name-keyed).
///
/// Returns `None` for non-literal variants (FieldRef, QueryRef, FnCall, Expr, Cond).
///
/// §5b floor: pinned by `table/read_planner.rs`, which encodes filter
/// literals into InnerValue index-bound keys. Eliminable only via the
/// index key-encoding boundary — not in resolve.rs scope.
#[inline]
pub fn filter_value_to_inner(fv: &FilterValue) -> Option<InnerValue> {
    match fv {
        FilterValue::Null => Some(InnerValue::Null),
        FilterValue::Bool(b) => Some(InnerValue::Bool(*b)),
        FilterValue::Int(i) => Some(InnerValue::Int(*i)),
        FilterValue::Float(f) => Some(InnerValue::F64(*f)),
        FilterValue::String(s) => Some(InnerValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(InnerValue::Bin(b.clone())),
        _ => None,
    }
}

/// Resolve a `FilterValue` into a **`QueryValue`** for comparison.
///
/// C6 (#80) — this is the name-keyed hot-path resolver. It retires the
/// transient `inner→query→funclib→query→inner` round-trips that the
/// funclib ABI flip (#75) created on the filter-eval path:
///
/// - **literals** (`Null`/`Bool`/`Int`/`Float`/`String`/`Binary`) — built
///   directly as `QueryValue`.
/// - **`FieldRef`** — `record.materialize_at` still yields `InnerValue`
///   today (narrowing that lens output is a LATER stage, out of scope for
///   C6). We convert **once** at this boundary via
///   `inner_value_to_query_value`. This single lens→QueryValue
///   materialization *replaces* the old lens→InnerValue one (it is not a
///   net-new round-trip); everything downstream stays `QueryValue`.
/// - **`FnCall`** — arguments are now `QueryValue`, passed straight to
///   `ctx.scalars.call` (no `inner_value_to_query_value` on the way in);
///   the funclib result is already `QueryValue` and is returned directly
///   (no `query_value_to_inner` on the way out). **The round-trip is gone.**
/// - **`QueryRef`** / **`Param`** — return `QueryValue` directly.
///
/// Any failure (unresolvable arg, unknown function, arity/type/conversion
/// error) collapses to `None` so the comparison treats the value as absent.
pub fn resolve_filter_query(
    fv: &FilterValue,
    record: &(impl RecordRef + ?Sized),
    ctx: &FilterContext,
) -> Option<QueryValue> {
    match fv {
        FilterValue::Null => Some(QueryValue::Null),
        FilterValue::Bool(b) => Some(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Some(QueryValue::Int(*i)),
        FilterValue::Float(f) => Some(QueryValue::F64(*f)),
        FilterValue::String(s) => Some(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(QueryValue::Bin(b.clone())),
        FilterValue::FieldRef { path } => {
            // F1: check the pre-interned path cache first (populated once by
            // `prescan_field_path_cache`, e.g. `SelectProjection::new`) —
            // avoids re-allocating a `Vec<u64>` and re-issuing one
            // `Interner::get_ind` (DashMap shard lookup) PER SEGMENT, PER
            // record. The cache key is pointer identity of THIS `FieldRef`
            // node (`fv as *const FilterValue as usize`), which stays stable
            // for the lifetime of a `SelectProjection`-owned tree — see
            // `field_path_cache.rs`'s safety comment. When
            // `ctx.field_path_cache` is `None` (every caller that hasn't
            // opted in: WHERE, `when`, `for_each`'s `over`, write-value
            // resolution), behavior is IDENTICAL to before this cache
            // existed.
            let ipath: SmallVec<[InternerKey; 4]> = match ctx
                .field_path_cache
                .and_then(|c| c.get(&(fv as *const FilterValue as usize)))
            {
                Some(cached) => cached.clone(),
                None => {
                    let keys = intern_field_path(path, ctx.interner)?;
                    keys.iter().map(|&id| InternerKey::new(id)).collect()
                }
            };
            // Single lens→QueryValue boundary (replaces the old lens→InnerValue
            // materialization). NOT a net-new round-trip — see fn doc.
            record
                .materialize_at(&ipath)
                .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok())
        }
        FilterValue::QueryRef { alias, path } => {
            // F2: check the lazy per-scan cache first (slot RESERVED once by
            // `prescan_query_ref_cache`, e.g. `SelectProjection::new`; VALUE
            // filled lazily on the first row that hits this node via
            // `OnceLock::get_or_init`). Avoids re-running
            // `alias.strip_prefix` + `resolved_refs.get` (cheap already), the
            // STRING PATH PARSING (`find`/`usize::parse`/prefix strips, one
            // pass per segment), and the multi-step Map/List navigation walk
            // to locate the target value — on EVERY row after the first.
            //
            // UNLIKE F1's `FieldPathCache` (eagerly populated at prescan
            // time), this cache is LAZY: the resolved value depends on
            // `ctx.resolved_refs` (runtime scan data) that does NOT exist at
            // `SelectProjection::new()` time — so the prescan can only
            // RESERVE an empty `OnceLock` slot, not fill it. This is the same
            // shape `In`'s `ref_column_sets` (`filter_node.rs`) already uses.
            //
            // The cache key is pointer identity of THIS `QueryRef` node
            // (`fv as *const FilterValue as usize`), which stays stable for
            // the lifetime of a `SelectProjection`-owned tree — see
            // `query_ref_cache.rs`'s safety comment. When
            // `ctx.query_ref_cache` is `None` (every caller that hasn't
            // opted in: WHERE, `when`, `for_each`'s `over`, write-value
            // resolution), behavior is IDENTICAL to before this cache
            // existed.
            //
            // The final `QueryValue::clone` of the resolved target is NOT
            // removed by this cache (cache hit or miss) — it is unavoidable
            // within this function's `Option<QueryValue>` (owned) contract;
            // see `query_ref_cache.rs`'s "What this caches" honesty section.
            if let Some(cell) = ctx
                .query_ref_cache
                .and_then(|c| c.get(&(fv as *const FilterValue as usize)))
            {
                return cell
                    .get_or_init(|| {
                        let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
                        ctx.resolved_refs
                            .get(key)
                            .and_then(|qr| resolve_query_ref_value(qr, path.as_deref()))
                    })
                    .clone();
            }
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let qr = ctx.resolved_refs.get(key)?;
            resolve_query_ref_value(qr, path.as_deref())
        }
        FilterValue::FnCall { call } => {
            // Args are QueryValue → straight to funclib. Result is QueryValue
            // → returned directly. Zero InnerValue, zero round-trip.
            let mut qv_args = Vec::with_capacity(call.args().len());
            for a in call.args() {
                let qv = resolve_filter_query(a, record, ctx)?;
                qv_args.push(qv);
            }
            ctx.scalars.call(call.name(), &qv_args).ok()
        }
        FilterValue::Param { name } => {
            // Injected sub-batch parameter. Populated by the recursive
            // sub-batch executor (P3); empty at the top level. The param
            // scope is name-keyed (`QueryValue`) — returned directly, no conversion.
            ctx.params.get(name.as_str()).cloned()
        }
        // `$cond` ternary (#635). `condition` is compiled and evaluated
        // against the SAME `record`/`ctx` the caller already has, then the
        // selected branch (`then`/`or_else`) is itself resolved recursively
        // — it may be a literal, `$query`, `$fn`, or a nested `$cond`.
        //
        // Silent-miss inheritance: if `condition` references an undeclared
        // `$query` alias (missing from `ctx.resolved_refs`) or any other
        // unresolvable sub-value, `FilterNode::matches` treats the missing
        // comparison as non-matching (`false`) — the same silent-miss
        // semantics every other `FilterValue` already has in this codebase
        // (see `$param`-silent-miss in
        // `docs/dev-artifacts/research/oql/01-nested-batch-recursion.md`).
        // `$cond`'s condition is not special-cased; it inherits this
        // behaviour rather than introducing a new error path.
        FilterValue::Cond { cond } => {
            // #643: check the pre-compiled cache first (populated once by
            // `prescan_cond_cache`, e.g. `SelectProjection::new`) — avoids
            // re-walking/re-interning `cond.condition` on every record. When
            // `ctx.cond_cache` is `None` (every caller that hasn't opted in:
            // WHERE, `when`, `for_each`'s `over`, write-value resolution),
            // behavior is IDENTICAL to before this cache existed.
            let cached = ctx
                .cond_cache
                .and_then(|c| cond_cache_get(c, &cond.condition));
            let matched = match cached {
                Some(node) => node.matches(record, ctx),
                None => compile_filter(&cond.condition, ctx.interner).matches(record, ctx),
            };
            if matched {
                resolve_filter_query(&cond.then, record, ctx)
            } else {
                resolve_filter_query(&cond.or_else, record, ctx)
            }
        }
        // `$expr` — arithmetic/string/logic/comparison operators that have
        // no funclib scalar-fn equivalent (see ADR
        // `docs/dev-artifacts/design/oql-02-expr-fate-adr.md`). Every arg is
        // resolved recursively first (may itself be `$ref`/`$fn`/`$cond`/
        // nested `$expr`), then the operator is applied over the resolved
        // `QueryValue`s. Any unresolvable arg or type/arity mismatch
        // collapses to `None` (absent), same as `FnCall`.
        FilterValue::Expr { expr } => eval_filter_expr(expr, record, ctx),
        // Literal array (#653, ForEach's `over`): resolve each element
        // recursively (an element may itself be `$query`/`$fn`/`$cond`/
        // literal) and collect into a `QueryValue::List`. Any unresolvable
        // element collapses the whole array to `None`, consistent with
        // `FnCall`'s arg-resolution short-circuit above.
        FilterValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(resolve_filter_query(item, record, ctx)?);
            }
            Some(QueryValue::List(out))
        }
    }
}

/// Evaluate a `FilterExpr` (`$expr`) into a `QueryValue`.
///
/// Args are resolved recursively via [`resolve_filter_query`] first. Type
/// mismatches, wrong arity, or an unresolvable arg all collapse to `None`
/// (absent) — mirroring `FnCall`'s error handling, since `$expr` fills the
/// same "compute a scalar" role for operators funclib does not register.
fn eval_filter_expr(
    expr: &FilterExpr,
    record: &(impl RecordRef + ?Sized),
    ctx: &FilterContext,
) -> Option<QueryValue> {
    // Inline up to 4 args (covers every unary/binary op — the vast
    // majority — plus small `concat` calls) before spilling to heap,
    // mirroring `CompactPath`'s SmallVec convention (filter_node.rs) —
    // avoids a per-call heap allocation for the common small-arity case.
    let mut args: SmallVec<[QueryValue; 4]> = SmallVec::with_capacity(expr.args.len());
    for a in &expr.args {
        args.push(resolve_filter_query(a, record, ctx)?);
    }

    #[inline]
    fn as_f64(v: &QueryValue) -> Option<f64> {
        match v {
            QueryValue::Int(i) => Some(*i as f64),
            QueryValue::F64(f) => Some(*f),
            // Dec operand (e.g. a `$fn` result): convert via f64 so `$expr`
            // arithmetic works over Dec-valued operands. Precision loss for
            // extreme scale is accepted (same tradeoff as compare_values).
            QueryValue::Dec(d) => d.to_f64(),
            _ => None,
        }
    }

    #[inline]
    fn as_str(v: &QueryValue) -> Option<&str> {
        match v {
            QueryValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    #[inline]
    fn as_bool(v: &QueryValue) -> Option<bool> {
        match v {
            QueryValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Numeric binary op — preserves `Int` when both operands are `Int`
    /// (and the result is exact for `+`/`-`/`*`), otherwise promotes to `F64`.
    #[inline]
    fn numeric_binop(
        a: &QueryValue,
        b: &QueryValue,
        int_op: impl Fn(i64, i64) -> Option<i64>,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Option<QueryValue> {
        if let (QueryValue::Int(x), QueryValue::Int(y)) = (a, b) {
            if let Some(r) = int_op(*x, *y) {
                return Some(QueryValue::Int(r));
            }
        }
        let x = as_f64(a)?;
        let y = as_f64(b)?;
        Some(QueryValue::F64(float_op(x, y)))
    }

    match expr.op {
        FilterExprOp::Add => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            numeric_binop(a, b, |x, y| x.checked_add(y), |x, y| x + y)
        }
        FilterExprOp::Sub => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            numeric_binop(a, b, |x, y| x.checked_sub(y), |x, y| x - y)
        }
        FilterExprOp::Mul => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            numeric_binop(a, b, |x, y| x.checked_mul(y), |x, y| x * y)
        }
        FilterExprOp::Div => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            let x = as_f64(a)?;
            let y = as_f64(b)?;
            if y == 0.0 {
                return None;
            }
            Some(QueryValue::F64(x / y))
        }
        FilterExprOp::Mod => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            if let (QueryValue::Int(x), QueryValue::Int(y)) = (a, b) {
                if *y == 0 {
                    return None;
                }
                if let Some(r) = x.checked_rem(*y) {
                    return Some(QueryValue::Int(r));
                }
                // i64::MIN % -1 — two's-complement overflow artifact, not a
                // real undefined case (mathematically 0). Fall through to the
                // float path, mirroring numeric_binop's overflow-to-float
                // behaviour for Add/Sub/Mul.
            }
            let x = as_f64(a)?;
            let y = as_f64(b)?;
            if y == 0.0 {
                return None;
            }
            Some(QueryValue::F64(x % y))
        }
        FilterExprOp::Neg => {
            let [a] = args.as_slice() else {
                return None;
            };
            match a {
                QueryValue::Int(i) => i.checked_neg().map(QueryValue::Int),
                QueryValue::F64(f) => Some(QueryValue::F64(-f)),
                _ => None,
            }
        }
        FilterExprOp::Concat => {
            let mut out = String::new();
            for a in &args {
                out.push_str(as_str(a)?);
            }
            Some(QueryValue::Str(out))
        }
        FilterExprOp::Lower => {
            let [a] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Str(as_str(a)?.to_lowercase()))
        }
        FilterExprOp::Upper => {
            let [a] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Str(as_str(a)?.to_uppercase()))
        }
        FilterExprOp::Trim => {
            let [a] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Str(as_str(a)?.trim().to_string()))
        }
        FilterExprOp::Length => {
            let [a] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Int(as_str(a)?.chars().count() as i64))
        }
        FilterExprOp::And => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(as_bool(a)? && as_bool(b)?))
        }
        FilterExprOp::Or => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(as_bool(a)? || as_bool(b)?))
        }
        FilterExprOp::Not => {
            let [a] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(!as_bool(a)?))
        }
        FilterExprOp::Eq => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(
                compare_values(a, b) == Some(Ordering::Equal),
            ))
        }
        FilterExprOp::Ne => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(
                compare_values(a, b) != Some(Ordering::Equal),
            ))
        }
        FilterExprOp::Gt => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(
                compare_values(a, b) == Some(Ordering::Greater),
            ))
        }
        FilterExprOp::Gte => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(matches!(
                compare_values(a, b),
                Some(Ordering::Greater | Ordering::Equal)
            )))
        }
        FilterExprOp::Lt => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(
                compare_values(a, b) == Some(Ordering::Less),
            ))
        }
        FilterExprOp::Lte => {
            let [a, b] = args.as_slice() else {
                return None;
            };
            Some(QueryValue::Bool(matches!(
                compare_values(a, b),
                Some(Ordering::Less | Ordering::Equal)
            )))
        }
    }
}

/// Resolve a `FilterValue` into an `InnerValue` for comparison.
///
/// **Legacy adapter (C6 #80).** After E6, the only remaining caller is the
/// cold projection twin (`SelectProjection::project` in
/// `query/read/select_projection.rs`), which feeds `inner_to_query_value`.
/// The hot QueryValue paths (aggregate scalar-fn, `project_value`) were
/// migrated to [`resolve_filter_query`] directly (E6). This entry delegates
/// to [`resolve_filter_query`] (the name-keyed hot path) and performs a
/// single trailing `query_value_to_inner` conversion at the legacy boundary
/// — a documented cold adapter, NOT a hot-path round-trip. The internal
/// filter eval tree (`FilterNode::matches`) uses `resolve_filter_query`
/// directly and never crosses this seam.
///
/// §5b floor: survives only for the legacy projection twin; dies with the
/// InnerValue axis — not reducible within resolve.rs scope.
pub fn resolve_filter_value(
    fv: &FilterValue,
    record: &(impl RecordRef + ?Sized),
    ctx: &FilterContext,
) -> Option<InnerValue> {
    let qv = resolve_filter_query(fv, record, ctx)?;
    query_value_to_inner(&qv, ctx.interner).ok()
}

/// Extract a value from a QueryResult by a simple path like "[0].id".
///
/// **Phase 2 — Call-aware**: when the `QueryResult` carries a
/// `value` (a stored-procedure / `BatchOp::Call` return — object /
/// array / scalar), the path is applied to that `value` instead of
/// to the records array. This lets later batch ops reference a Call's
/// result with the same `$query` syntax used for Read results:
///
/// - `@proc`              → entire `value` (scalar / object / array).
/// - `@proc.id`           → object field.
/// - `@proc[0]`           → array index.
/// - `@proc[0].name`      → chained.
///
/// For ordinary Read results (`value` is `None`), the records-based
/// behaviour is preserved unchanged.
///
/// C6 (#80): returns `QueryValue` (name-keyed) — the filter comparison
/// layer is now QueryValue-native.
pub(super) fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<QueryValue> {
    // Call-result path: source is `QueryResult.value`.
    if let Some(value) = &qr.value {
        return resolve_query_value_path(value, path).cloned();
    }

    // Read-result path: source is `QueryResult.records`.
    let path = path?;
    if !path.starts_with('[') {
        return None;
    }
    let bracket_end = path.find(']')?;
    let index: usize = path[1..bracket_end].parse().ok()?;
    let record = qr.records.get(index)?;

    let rest = &path[bracket_end + 1..];
    let record_qv = record.as_value();
    if rest.is_empty() {
        return Some(record_qv.into_owned());
    }
    let rest = rest.strip_prefix('.')?;
    Some(record_qv.get(rest)?.clone())
}

/// Extract a column of values from all records in a QueryResult.
///
/// Supports `[].field` pattern — iterates all records, extracts `field` from each.
///
/// C6 (#80): returns `Vec<QueryValue>` (name-keyed).
pub(crate) fn resolve_query_ref_column(qr: &QueryResult, path: Option<&str>) -> Vec<QueryValue> {
    let path = match path {
        Some(p) => p,
        None => return Vec::new(),
    };
    if !path.starts_with("[]") {
        return Vec::new();
    }
    let rest = &path[2..];
    let field = match rest.strip_prefix('.') {
        Some(f) => f,
        None => return Vec::new(),
    };

    qr.records
        .iter()
        .filter_map(|record| {
            let record_qv = record.as_value();
            Some(record_qv.get(field)?.clone())
        })
        .collect()
}

/// Walk a path like `.field`, `[0]`, `[0].name`, or `None` (root) through a
/// `QueryValue`. Used by [`resolve_query_ref_value`] when the source is a
/// Call result (`QueryResult.value`).
///
/// Supported segments:
/// - `.field`     → Map field access.
/// - `[n]`        → List index.
/// - `[n].field`  → chained.
///
/// The path is intentionally a subset of the full `QueryReference` grammar —
/// it is what the `QueryRef.path` string carries in practice for Call refs.
pub(super) fn resolve_query_value_path<'a>(
    mut cur: &'a QueryValue,
    path: Option<&str>,
) -> Option<&'a QueryValue> {
    // Preserve the original semantics: a `None` path returns the root
    // value itself (Some(cur)), not None.
    let mut rest = match path {
        Some(p) => p,
        None => return Some(cur),
    };
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            let field = &after_dot[..end];
            cur = match cur {
                QueryValue::Map(m) => m.get(field)?,
                _ => return None,
            };
            rest = &after_dot[end..];
        } else if rest.starts_with('[') {
            let bracket_end = rest.find(']')?;
            let idx: usize = rest[1..bracket_end].parse().ok()?;
            cur = match cur {
                QueryValue::List(l) => l.get(idx)?,
                _ => return None,
            };
            rest = &rest[bracket_end + 1..];
        } else {
            return None;
        }
    }
    Some(cur)
}

pub(crate) fn is_column_query_ref(fv: &FilterValue) -> bool {
    matches!(fv, FilterValue::QueryRef { path: Some(p), .. } if p.starts_with("[]"))
}
