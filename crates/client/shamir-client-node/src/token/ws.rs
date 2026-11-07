use napi::bindgen_prelude::*;
use napi_derive::napi;

use shamir_client as core;

use super::endpoint::EndpointSpec;
use crate::client::{from_core, ShamirClient};
use crate::secrets;
use crate::{infra_error, millis};

/// Parameters of `ShamirClient.connectTokenWs`.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `token` is zeroized.
#[napi(object)]
pub struct TokenWsOptions {
    /// Server host; only its first resolved address is used.
    pub host: String,
    /// Server port.
    pub port: u32,
    /// TLS server name; defaults to `host`.
    pub server_name: Option<String>,
    /// PEM bundle of trusted CAs; defaults to the bundled WebPKI roots.
    /// Not allowed together with `plainLoopback`.
    pub ca_pem: Option<String>,
    /// Use plain HTTP; the core accepts loopback addresses only.
    pub plain_loopback: Option<bool>,
    /// `shk1.` API token. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub token: String,
    /// Deadline (ms) for the TCP connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for the whole WebSocket handshake.
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}

/// `ShamirClient.connectTokenWs`; omitted timeouts keep the core defaults.
pub(crate) fn connect_token_ws(
    env: &Env,
    opts: TokenWsOptions,
) -> Result<PromiseRaw<'_, ShamirClient>> {
    let token = secrets::protect(opts.token);
    let spec = EndpointSpec::parse(
        opts.host,
        opts.port,
        opts.server_name,
        opts.ca_pem,
        opts.plain_loopback,
    )?;
    let api_token = core::ApiToken::new(token.as_str()).map_err(infra_error)?;
    drop(token);
    let connect_timeout = millis(opts.connect_timeout_ms);
    let handshake_timeout = millis(opts.handshake_timeout_ms);
    let request_timeout = millis(opts.request_timeout_ms);
    env.spawn_future(async move {
        let endpoint = spec
            .resolve(handshake_timeout.or(connect_timeout))
            .await
            .map_err(infra_error)?;
        let mut core_opts = core::TokenWsOptions::new(endpoint, api_token);
        if connect_timeout.is_some() {
            core_opts = core_opts.with_connect_timeout(connect_timeout);
        }
        if handshake_timeout.is_some() {
            core_opts = core_opts.with_handshake_timeout(handshake_timeout);
        }
        if request_timeout.is_some() {
            core_opts = core_opts.with_request_timeout(request_timeout);
        }
        let client = core::Client::connect_token_ws(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, request_timeout))
    })
}
