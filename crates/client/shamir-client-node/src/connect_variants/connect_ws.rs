use napi::bindgen_prelude::*;

use shamir_client as core;

use crate::client::{from_core, ConnectOptions, PreparedConnect, ShamirClient};
use crate::{bounded, infra_error};

/// `ShamirClient.connectWs`. The host is resolved to its first address,
/// under the handshake budget, and the core then gets only the remaining
/// part of that budget for the connect.
pub(crate) fn connect_ws(env: &Env, opts: ConnectOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    let PreparedConnect {
        host,
        port,
        mut core_opts,
    } = opts.prepare()?;
    let req_timeout = core_opts.request_timeout;
    let budget = core_opts.handshake_timeout;
    env.spawn_future(async move {
        let started = tokio::time::Instant::now();
        core_opts.addr = bounded(budget, "dns", core::resolve_host(&host, port))
            .await
            .map_err(infra_error)?;
        core_opts.handshake_timeout = budget.map(|b| b.saturating_sub(started.elapsed()));
        let client = core::Client::connect_ws(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}
