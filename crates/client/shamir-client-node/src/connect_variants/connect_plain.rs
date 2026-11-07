use napi::bindgen_prelude::*;

use shamir_client as core;

use super::options::ConnectPlainOptions;
use crate::client::{from_core, ShamirClient};
use crate::secrets;
use crate::{bounded, checked_port, infra_error, millis, optional_pin, unresolved_addr};

/// `ShamirClient.connectPlain`; first resolved address only. DNS and the
/// core handshake share one handshake budget.
pub(crate) fn connect_plain(
    env: &Env,
    opts: ConnectPlainOptions,
) -> Result<PromiseRaw<'_, ShamirClient>> {
    let (password, (port, trusted_pin)) =
        secrets::protect_before(opts.password.into_bytes(), || {
            Ok::<_, Error>((checked_port(opts.port)?, optional_pin(opts.trusted_pin)?))
        })?;
    let req_timeout = millis(opts.request_timeout_ms);
    let budget = millis(opts.handshake_timeout_ms);
    let host = opts.host;
    let mut core_opts = core::ConnectOptions {
        addr: unresolved_addr(),
        server_name: String::new(),
        username: opts.username,
        password,
        accept_new_host: opts.accept_new_host.unwrap_or(false),
        trusted_pin,
        connect_timeout: millis(opts.connect_timeout_ms),
        handshake_timeout: budget,
        request_timeout: req_timeout,
    };
    env.spawn_future(async move {
        let started = tokio::time::Instant::now();
        core_opts.addr = bounded(budget, "dns", core::resolve_host(&host, port))
            .await
            .map_err(infra_error)?;
        core_opts.handshake_timeout = budget.map(|b| b.saturating_sub(started.elapsed()));
        let client = core::Client::connect_plain(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}
