//! The admission-error → host-error boundary.

use crate::runtime::error::{FnResult, FunctionError};
use shamir_wasm_admission::AdmissionError;

impl From<AdmissionError> for FunctionError {
    fn from(err: AdmissionError) -> Self {
        FunctionError::Compute(err.to_string())
    }
}

/// Thin wrapper preserving the host's public `FnResult<()>` signature around
/// the admission crate's [`verify_wasm_module`].
///
/// [`verify_wasm_module`]: shamir_wasm_admission::verify_wasm_module
pub fn verify_wasm_module(bytes: &[u8]) -> FnResult<()> {
    shamir_wasm_admission::verify_wasm_module(bytes).map_err(FunctionError::from)
}
