mod admit;
mod gate;
mod module_hash;
mod sanitizer;
mod scan;
mod types;

pub use admit::admit_module;
pub use gate::check_module;
pub(in crate::wasm) use gate::run_gated_compile;
pub use module_hash::ModuleHash;
pub use sanitizer::{verify_wasm_module, SANCTIONED_HOST_IMPORTS};
pub use types::{AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleReport};
