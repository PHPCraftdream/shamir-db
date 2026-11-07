use napi::bindgen_prelude::*;

use super::shared::resume_with;
use crate::client::ShamirClient;
use crate::connect_variants::ResumeOptions;

/// `ShamirClient.resume`.
pub(crate) fn resume(env: &Env, opts: ResumeOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    resume_with(env, opts, false)
}
