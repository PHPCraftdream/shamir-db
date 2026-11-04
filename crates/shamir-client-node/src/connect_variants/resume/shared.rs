use napi::bindgen_prelude::*;

use shamir_client as core;

use crate::client::{from_core, ShamirClient};
use crate::connect_variants::ResumeOptions;
use crate::secrets;
use crate::{bounded, checked_port, copy_pin, infra_error, millis};

/// Shared resume body; DNS and the core resume run under ONE handshake
/// budget.
pub(super) fn resume_with(
    env: &Env,
    opts: ResumeOptions,
    websocket: bool,
) -> Result<PromiseRaw<'_, ShamirClient>> {
    let (ticket, (port, pinned_hash)) = secrets::protect_before(opts.ticket.to_vec(), || {
        Ok::<_, Error>((checked_port(opts.port)?, copy_pin(&opts.trusted_pin)?))
    })?;
    let budget = millis(opts.handshake_timeout_ms);
    let req_timeout = millis(opts.request_timeout_ms);
    let host = opts.host;
    let server_name = opts.server_name;
    env.spawn_future(async move {
        let resumed = async move {
            let addr = core::resolve_host(&host, port).await?;
            let secure = core::SecureResumeOptions {
                addr,
                server_name,
                ticket,
                pinned_hash,
            };
            if websocket {
                core::Client::resume_ws_secure(secure).await
            } else {
                core::Client::resume_secure(secure).await
            }
        };
        let client = bounded(budget, "resume", resumed)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}
