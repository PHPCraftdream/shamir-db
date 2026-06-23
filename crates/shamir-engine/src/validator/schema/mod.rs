//! Declarative schema validator — Phase A.
//!
//! Types: [`TypeTag`], [`Constraints`], [`FieldRule`], [`SchemaValidator`].
//! Fluent builder: [`rule`].

pub mod constraints;
pub mod field_rule;
pub mod rule_builder;
pub mod schema_validator;
pub mod type_tag;

pub use constraints::{Constraints, Num};
pub use field_rule::FieldRule;
pub use rule_builder::{rule, RuleBuilder};
pub use schema_validator::SchemaValidator;
pub use type_tag::TypeTag;

#[cfg(test)]
mod tests;
