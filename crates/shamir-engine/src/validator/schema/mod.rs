//! Declarative schema validator — Phase A (pure) + Phase B (scalar/format/cross-field).
//!
//! Types: [`TypeTag`], [`Constraints`], [`FieldRule`], [`SchemaValidator`],
//! [`FormatKind`], [`CrossFieldCompare`], [`CompareOp`].
//! Fluent builder: [`rule`].

pub mod constraints;
pub mod cross_field;
pub mod field_rule;
pub mod foreign_key;
pub mod format;
pub mod rule_builder;
pub mod schema_validator;
pub mod type_tag;

pub use constraints::{Constraints, Num};
pub use cross_field::{CompareOp, CrossFieldCompare, CrossFieldResult};
pub use field_rule::FieldRule;
pub use foreign_key::ForeignKeyRef;
pub use format::FormatKind;
pub use rule_builder::{rule, RuleBuilder};
pub use schema_validator::SchemaValidator;
pub use shamir_query_types::admin::FkAction;
pub use type_tag::TypeTag;

#[cfg(test)]
mod tests;
