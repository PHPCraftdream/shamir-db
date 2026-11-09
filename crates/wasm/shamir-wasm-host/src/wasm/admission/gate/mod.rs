mod check;
mod compile_gate;
mod lang_compile;

pub use check::check_module;
pub(in crate::wasm) use compile_gate::run_gated_compile;
pub use lang_compile::{check_shamir_source, create_shamir_source};
