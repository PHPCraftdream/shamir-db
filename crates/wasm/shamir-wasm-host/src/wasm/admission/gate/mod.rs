mod check;
mod compile_gate;

pub use check::check_module;
pub(in crate::wasm) use compile_gate::run_gated_compile;
