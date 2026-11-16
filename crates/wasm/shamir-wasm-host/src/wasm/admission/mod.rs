mod fn_error;
pub(crate) mod metrics;
mod gate;

pub use fn_error::verify_wasm_module;
pub(in crate::wasm) use gate::run_gated_compile;
pub use gate::{check_module, check_shamir_source, create_shamir_source};
pub use metrics::admit_module;
pub use shamir_wasm_admission::{
    AdmissionError, AdmissionLimits, AdmissionPolicy, ModuleHash, ModuleReport,
    SANCTIONED_HOST_IMPORTS,
};
