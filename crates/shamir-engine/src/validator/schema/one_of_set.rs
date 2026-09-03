//! Precomputed `one_of` membership set (audit group 25 / defect 2).
//!
//! `FieldRule::check_one_of` used to run a LINEAR `Vec::contains` scan over
//! the user-supplied `one_of` allowed-list, fed by an ALLOCATING
//! `materialize_as_qv` call — both **per record**. For an unbounded
//! allowed-list this is O(records × list_len), and the allocation (owned
//! `String`/`Vec<u8>` clones for `Str`/`Bin` scalars) is pure waste when all
//! we need is a membership test.
//!
//! [`OneOfSet`] is built ONCE, at [`SchemaValidator::new`](super::SchemaValidator::new)
//! time, from `Constraints::one_of`. Per-record membership is then an O(1)
//! hash lookup for the scalar kinds that dominate real `one_of` usage
//! (Null/Bool/Int/Str/Bin), probed with a **borrowed** `ScalarRef` — no
//! `materialize_as_qv` allocation on the hot path. `Str`/`Bin` lookups use
//! `std`'s `Borrow<str>`/`Borrow<[u8]>` impls for `String`/`Vec<u8>`, so
//! `TFxSet<String>::contains(&str)` and `TFxSet<Vec<u8>>::contains(&[u8])`
//! never allocate.
//!
//! `F64` and any non-scalar value (`Dec`/`Big`/`List`/`Set`/`Map`) fall back
//! to a linear scan over `residual` — `one_of` on those kinds is rare in
//! practice (enums are overwhelmingly int/str/bool), and `F64` specifically
//! cannot share the `Int` bucket's exact-bits hashing without diverging from
//! `QueryValue`'s existing `==`-based equality (NaN/±0.0 semantics).

use shamir_collections::TFxSet;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::value::QueryValue;

/// Precomputed, bucketed `one_of` allowed-value set. See module docs.
#[derive(Debug, Clone, Default)]
pub(crate) struct OneOfSet {
    null: bool,
    bools: TFxSet<bool>,
    ints: TFxSet<i64>,
    strs: TFxSet<String>,
    bins: TFxSet<Vec<u8>>,
    /// Everything that isn't cheaply bucketable: `F64` (equality semantics
    /// would diverge from bit-hashing) plus any non-scalar `QueryValue`
    /// (`Dec`/`Big`/`List`/`Set`/`Map`) reachable via the `materialize`
    /// fallback. Linear-scanned; expected to stay empty/tiny in practice.
    residual: Vec<QueryValue>,
}

impl OneOfSet {
    /// Build a set from `Constraints::one_of`. Returns `None` when `allowed`
    /// is absent or empty — mirrors the original "no `one_of` constraint"
    /// skip in `check_one_of`.
    pub(crate) fn build(allowed: Option<&[QueryValue]>) -> Option<Self> {
        let allowed = allowed.filter(|vals| !vals.is_empty())?;
        let mut set = OneOfSet::default();
        for v in allowed {
            match v {
                QueryValue::Null => set.null = true,
                QueryValue::Bool(b) => {
                    set.bools.insert(*b);
                }
                QueryValue::Int(i) => {
                    set.ints.insert(*i);
                }
                QueryValue::Str(s) => {
                    set.strs.insert(s.clone());
                }
                QueryValue::Bin(b) => {
                    set.bins.insert(b.clone());
                }
                other => set.residual.push(other.clone()),
            }
        }
        Some(set)
    }

    /// Zero-allocation membership probe for a borrowed scalar.
    pub(crate) fn contains_scalar(&self, sr: ScalarRef<'_>) -> bool {
        match sr {
            ScalarRef::Null => self.null,
            ScalarRef::Bool(b) => self.bools.contains(&b),
            ScalarRef::Int(i) => self.ints.contains(&i),
            ScalarRef::Str(s) => self.strs.contains(s),
            ScalarRef::Bin(b) => self.bins.contains(b),
            ScalarRef::F64(f) => self
                .residual
                .iter()
                .any(|qv| matches!(qv, QueryValue::F64(rf) if *rf == f)),
        }
    }

    /// Membership probe for an already-materialized (non-scalar) value —
    /// the `Dec`/`Big`/`List`/`Set`/`Map` fallback path.
    pub(crate) fn contains_materialized(&self, qv: &QueryValue) -> bool {
        self.residual.contains(qv)
    }
}
