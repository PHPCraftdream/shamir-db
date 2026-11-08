mod check;
mod effects;
mod flow;
mod tables;
mod types;

pub use check::check;
pub use effects::{BuiltinCategory, BuiltinId, Capability, EffectSet};
pub use tables::{
    Callee, ResolvedCall, ResolvedTarget, TypedFunction, TypedLocal, TypedModule, TypedParameter,
};
pub use types::{RecordShape, TypeData, TypeId, TypeInterner};

#[cfg(test)]
mod tests;
