//! Fixed row-byte estimate for query values.
//!
//! The walk mirrors the engine batch's `ResultSize::value` formula (scalar
//! slot 16; string/bin slot 24 + length; decimal 56; big integer 24 + text;
//! container shell 24 + 16 per element/entry). Both are deterministic
//! materialization estimates, pinned by the batch byte-budget tests; the
//! constants must change together.

use shamir_types::types::value::QueryValue;

/// Byte-estimate namespace for query values; no state.
pub struct RowByteMeter;

impl RowByteMeter {
    /// Estimate of one projected value's materialized bytes: the value
    /// walk plus the fixed per-row shell.
    pub fn value_size(value: &shamir_types::types::value::QueryValue) -> u64 {
        48u64.saturating_add(Self::sequence_walk(value))
    }

    /// Iterative container walk; mirrors `ResultSize::value` exactly.
    fn sequence_walk(value: &QueryValue) -> u64 {
        let mut stack = vec![value];
        let mut total = 0u64;
        while let Some(value) = stack.pop() {
            let size = match value {
                QueryValue::Null
                | QueryValue::Bool(_)
                | QueryValue::Int(_)
                | QueryValue::F64(_) => 16,
                QueryValue::Dec(_) => 56,
                QueryValue::Big(v) => 24u64.saturating_add(v.to_string().len() as u64),
                QueryValue::Str(v) => 24u64.saturating_add(v.len() as u64),
                QueryValue::Bin(v) => 24u64.saturating_add(v.len() as u64),
                QueryValue::List(values) => {
                    stack.extend(values);
                    Self::sequence(values.len())
                }
                QueryValue::Set(values) => {
                    stack.extend(values);
                    Self::sequence(values.len())
                }
                QueryValue::Map(values) => {
                    let mut size = Self::sequence(values.len());
                    for (key, value) in values {
                        size = size.saturating_add(key.len() as u64);
                        stack.push(value);
                    }
                    size
                }
            };
            total = total.saturating_add(size);
        }
        total
    }

    fn sequence(len: usize) -> u64 {
        24u64.saturating_add(16u64.saturating_mul(len as u64))
    }
}
