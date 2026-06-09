//! Ergonomic constructors for [`FilterValue`].
//!
//! Every function in this module returns a
//! [`shamir_query_types::filter::FilterValue`] — the universal expression
//! type that drives filters, function arguments, and computed write-values
//! on the wire.

use shamir_query_types::filter::{FieldPath, FilterValue, FnCall};

// ── literal passthrough ──────────────────────────────────────────────

/// Wrap any value that already implements `Into<FilterValue>` into a
/// [`FilterValue`].
pub fn lit(v: impl Into<FilterValue>) -> FilterValue {
    v.into()
}

/// Create a [`FilterValue::Int`] from a `u64`.
///
/// This is an explicit lossy escape-hatch for values that may exceed
/// `i64::MAX`. Values above `i64::MAX` will wrap silently.
/// For all other integer widths, use `lit(v)` (which goes through
/// `From<i8/i16/i32/i64/u8/u16/u32>`).
pub fn lit_u64(v: u64) -> FilterValue {
    FilterValue::Int(v as i64)
}

// ── binary / null ────────────────────────────────────────────────────

/// Create a [`FilterValue::Binary`] from raw bytes.
pub fn bin(bytes: impl Into<Vec<u8>>) -> FilterValue {
    FilterValue::Binary(bytes.into())
}

/// Create a [`FilterValue::Null`].
pub fn null() -> FilterValue {
    FilterValue::Null
}

// ── IntoFieldPath trait ──────────────────────────────────────────────

/// Anything that can be converted into a [`FieldPath`] (a `Vec<String>`
/// of path segments).
pub trait IntoFieldPath {
    /// Convert into a field path.
    fn into_field_path(self) -> FieldPath;
}

impl IntoFieldPath for &str {
    fn into_field_path(self) -> FieldPath {
        vec![self.to_owned()]
    }
}

impl IntoFieldPath for String {
    fn into_field_path(self) -> FieldPath {
        vec![self]
    }
}

impl<const N: usize> IntoFieldPath for [&str; N] {
    fn into_field_path(self) -> FieldPath {
        self.iter().map(|s| (*s).to_owned()).collect()
    }
}

impl IntoFieldPath for &[&str] {
    fn into_field_path(self) -> FieldPath {
        self.iter().map(|s| (*s).to_owned()).collect()
    }
}

impl IntoFieldPath for Vec<String> {
    fn into_field_path(self) -> FieldPath {
        self
    }
}

impl IntoFieldPath for Vec<&str> {
    fn into_field_path(self) -> FieldPath {
        self.into_iter().map(|s| s.to_owned()).collect()
    }
}

// ── field reference ──────────────────────────────────────────────────

/// Create a [`FilterValue::FieldRef`] pointing at a field in the
/// current record.
///
/// ```ignore
/// col("email")             // → FieldRef { path: ["email"] }
/// col(["address", "zip"])  // → FieldRef { path: ["address","zip"] }
/// ```
pub fn col(path: impl IntoFieldPath) -> FilterValue {
    FilterValue::FieldRef {
        path: path.into_field_path(),
    }
}

// ── function call ────────────────────────────────────────────────────

/// Create a [`FilterValue::FnCall`] with named arguments.
///
/// Uses [`FnCall::complex`] under the hood.
pub fn func(name: impl Into<String>, args: impl IntoIterator<Item = FilterValue>) -> FilterValue {
    FilterValue::FnCall {
        call: FnCall::complex(name, args.into_iter().collect()),
    }
}

// ── parameter reference ──────────────────────────────────────────────

/// Create a [`FilterValue::Param`] referencing a named binding from the
/// enclosing sub-batch's `bind` map.
///
/// Use inside a nested `BatchRequest` that is passed to
/// [`crate::batch::Batch::sub_batch`]. The engine resolves the name at
/// execution time from the outer batch's bind map.
///
/// ```ignore
/// param("uid")  // → FilterValue::Param { name: "uid" }
///               // serialises as {"$param":"uid"}
/// ```
pub fn param(name: impl Into<String>) -> FilterValue {
    FilterValue::Param { name: name.into() }
}

// ── query reference ──────────────────────────────────────────────────

/// Normalize an alias so it always starts with `@`.
fn normalize_alias(alias: String) -> String {
    if alias.starts_with('@') {
        alias
    } else {
        format!("@{alias}")
    }
}

/// Create a [`FilterValue::QueryRef`] referencing another query's
/// result with a path into it.
///
/// The alias is `@`-normalized: if it does not start with `@`, one is
/// prepended automatically.
pub fn qref(alias: impl Into<String>, path: impl Into<String>) -> FilterValue {
    FilterValue::QueryRef {
        alias: normalize_alias(alias.into()),
        path: Some(path.into()),
    }
}

/// Create a [`FilterValue::QueryRef`] referencing the entire result of
/// another query (no path).
///
/// The alias is `@`-normalized.
pub fn qref_all(alias: impl Into<String>) -> FilterValue {
    FilterValue::QueryRef {
        alias: normalize_alias(alias.into()),
        path: None,
    }
}

#[cfg(test)]
mod tests;
