//! The single byte estimate for materialized `QueryValue`s.
//!
//! Scalar slot 16; string/bin slot 24 + byte length; decimal 56; big integer
//! 24 + decimal text length; container shell 24 + 16 per element/entry plus
//! map key byte lengths. A deterministic materialization estimate, not
//! serialized response length. The engine's `ResultSize` and the read row
//! meter delegate here; this is the one walk.

use shamir_types::types::value::QueryValue;

/// Byte-estimate namespace for materialized query values; no state.
pub struct ValueBytes;

impl ValueBytes {
    /// Container shell shared by every estimate.
    pub const SHELL: u64 = 24;
    /// Per element/entry slot shared by every estimate.
    pub const ENTRY: u64 = 16;
    /// Fixed per-row shell charged on top of the value walk.
    pub const ROW_SHELL: u64 = 48;

    /// One projected value's materialized bytes: the value walk plus the
    /// fixed per-row shell.
    pub fn row_bytes(value: &QueryValue) -> u64 {
        Self::ROW_SHELL.saturating_add(Self::value(value))
    }

    /// The one iterative value walk shared by every byte estimate.
    pub fn value(value: &QueryValue) -> u64 {
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

    /// Container shell plus per-entry slots; shared by result-level estimates.
    pub fn sequence(len: usize) -> u64 {
        Self::SHELL.saturating_add(Self::ENTRY.saturating_mul(len as u64))
    }
}
