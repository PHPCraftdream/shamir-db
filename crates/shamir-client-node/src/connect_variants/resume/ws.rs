use napi::bindgen_prelude::*;

use super::shared::resume_with;
use crate::client::ShamirClient;
use crate::connect_variants::ResumeOptions;

/// `ShamirClient.resumeWs`.
pub(crate) fn resume_ws(env: &Env, opts: ResumeOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    resume_with(env, opts, true)
}
