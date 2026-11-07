//! Declarative schema validator — Phase A (pure) + Phase B (scalar/format/cross-field).
//!
//! Types: [`TypeTag`], [`Constraints`], [`FieldRule`], [`SchemaValidator`],
//! [`FormatKind`], [`CrossFieldCompare`], [`CompareOp`].
//! Fluent builder: [`rule`].

mod rules;

pub mod foreign_key;
pub mod rule_builder;
pub mod schema_validator;

// Module-path facades: the pre-sweep rule file names stay reachable at
// `crate::validator::schema::<name>` so no caller in any crate changes.
pub use rules::{constraints, cross_field, field_rule, format, one_of_set, type_tag};

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
