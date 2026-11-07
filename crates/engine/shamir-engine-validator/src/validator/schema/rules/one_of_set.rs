//! Precomputed `one_of` membership with the same equality as `QueryValue`.

use shamir_collections::TFxSet;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::value::QueryValue;

/// Precomputed, bucketed `one_of` allowed-value set.
#[derive(Debug, Clone, Default)]
pub(crate) struct OneOfSet {
    null: bool,
    bools: TFxSet<bool>,
    ints: TFxSet<i64>,
    strs: TFxSet<String>,
    bins: TFxSet<Vec<u8>>,
    // Value's Hash canonicalizes NaNs and signed zero recursively, and uses
    // order-independent hashes for sets and maps, matching Value::PartialEq.
    residual: TFxSet<QueryValue>,
}

impl OneOfSet {
    /// An absent or empty list means no constraint.
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
                other => {
                    set.residual.insert(other.clone());
                }
            }
        }
        Some(set)
    }

    #[cfg(test)]
    pub(crate) fn residual_len(&self) -> usize {
        self.residual.len()
    }

    /// Probe scalar values without allocating.
    pub(crate) fn contains_scalar(&self, sr: ScalarRef<'_>) -> bool {
        match sr {
            ScalarRef::Null => self.null,
            ScalarRef::Bool(b) => self.bools.contains(&b),
            ScalarRef::Int(i) => self.ints.contains(&i),
            ScalarRef::Str(s) => self.strs.contains(s),
            ScalarRef::Bin(b) => self.bins.contains(b),
            ScalarRef::F64(f) => self.residual.contains(&QueryValue::F64(f)),
        }
    }

    /// Probe a value produced by the existing materialization conversion.
    pub(crate) fn contains_materialized(&self, qv: &QueryValue) -> bool {
        match qv {
            QueryValue::Null => self.null,
            QueryValue::Bool(b) => self.bools.contains(b),
            QueryValue::Int(i) => self.ints.contains(i),
            QueryValue::Str(s) => self.strs.contains(s.as_str()),
            QueryValue::Bin(b) => self.bins.contains(b.as_slice()),
            other => self.residual.contains(other),
        }
    }
}
