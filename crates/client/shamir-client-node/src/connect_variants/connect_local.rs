use napi::bindgen_prelude::*;

use shamir_client as core;

use super::options::ConnectLocalOptions;
use crate::client::{from_core, ShamirClient};
use crate::secrets;
use crate::{bounded, infra_error, millis, optional_pin};

/// `ShamirClient.connectLocal`.
pub(crate) fn connect_local(
    env: &Env,
    opts: ConnectLocalOptions,
) -> Result<PromiseRaw<'_, ShamirClient>> {
    let (password, trusted_pin) = secrets::protect_before(opts.password.into_bytes(), || {
        optional_pin(opts.trusted_pin)
    })?;
    let req_timeout = millis(opts.request_timeout_ms);
    let budget = millis(opts.handshake_timeout_ms);
    let core_opts = core::ConnectLocalOptions {
        addr: opts.addr,
        username: opts.username,
        password,
        accept_new_host: opts.accept_new_host.unwrap_or(false),
        trusted_pin,
        connect_timeout: millis(opts.connect_timeout_ms),
        request_timeout: req_timeout,
    };
    env.spawn_future(async move {
        let client = bounded(budget, "handshake", core::Client::connect_local(core_opts))
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}
