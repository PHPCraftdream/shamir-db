pub(crate) mod batch;
mod emit;
pub(crate) mod ir;
pub(crate) mod lower;
pub mod runtime;

pub use emit::emit_runtime_module;
pub(crate) use emit::{emit_compiled_module, EmitError};
pub use runtime::RuntimeError;
