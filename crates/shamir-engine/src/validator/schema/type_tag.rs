//! Type tags for declarative schema field rules.
//!
//! Each [`TypeTag`] corresponds to a [`Value`] variant (or the wildcard `Any`).
//! The tag is used by [`FieldRule::check`] to assert the runtime type of a
//! field value.

use std::fmt;

/// Coarse type tag for a declarative field rule.
///
/// Maps 1:1 to `Value<K>` variants (plus `Any` = no type constraint).
/// `Dec` and `Big` are distinguishable **only** on the `OwnedFields` path
/// (incoming INSERT/UPDATE before storage encoding); on the lens path
/// (`ViewFields`) they collapse to `Bin`.  See doc 01 for the full story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeTag {
    /// `Value::Str`
    String,
    /// `Value::Int`
    Int,
    /// `Value::F64`
    F64,
    /// `Value::Dec` (only on OwnedFields path)
    Dec,
    /// `Value::Bool`
    Bool,
    /// `Value::Bin`
    Bin,
    /// `Value::List`
    List,
    /// `Value::Map`
    Map,
    /// `Value::Set`
    Set,
    /// `Value::Null`
    Null,
    /// Wildcard: accept any type (only constraints are checked).
    Any,
}

impl fmt::Display for TypeTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::String => "string",
            Self::Int => "int",
            Self::F64 => "f64",
            Self::Dec => "dec",
            Self::Bool => "bool",
            Self::Bin => "bin",
            Self::List => "list",
            Self::Map => "map",
            Self::Set => "set",
            Self::Null => "null",
            Self::Any => "any",
        };
        f.write_str(s)
    }
}
