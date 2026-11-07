mod fn_error;
mod gate;

pub use fn_error::verify_wasm_module;
pub use gate::check_module;
pub(in crate::wasm) use gate::run_gated_compile;
pub use shamir_wasm_admission::{
    admit_module, AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleHash, ModuleReport,
    SANCTIONED_HOST_IMPORTS,
};
