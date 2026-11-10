mod check;
mod effects;
mod flow;
mod tables;
mod types;

pub(crate) use check::check_scoped_with_catalog;
pub use check::{check, check_scoped};
pub use effects::{BuiltinCategory, BuiltinId, Capability, EffectSet};
pub use tables::{
    Callee, ResolvedCall, ResolvedTarget, TypedFunction, TypedLocal, TypedModule, TypedParameter,
};
pub use types::{RecordShape, TypeData, TypeId, TypeInterner};

#[cfg(test)]
mod tests;
