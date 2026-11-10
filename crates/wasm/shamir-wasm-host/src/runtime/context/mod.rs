mod core;
mod globals;

pub use core::{BatchContext, FnBatch, FnCtx};
pub use globals::{GlobalVars, GlobalsQuota, GlobalsQuotaError};

#[cfg(test)]
pub(crate) use globals::AdmitProbe;
