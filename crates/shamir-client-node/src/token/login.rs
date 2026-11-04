use napi::bindgen_prelude::*;
use napi_derive::napi;

use shamir_client as core;

use super::endpoint::EndpointSpec;
use super::views::LoginView;
use crate::secrets;
use crate::{encode_buffer, encode_db_error, infra_error, millis};

/// Parameters of the `login` function (password login for an API token).
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `password` is zeroized.
#[napi(object)]
pub struct LoginOptions {
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
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub password: String,
    /// Label of the minted token.
    pub label: Option<String>,
    /// `"read_only"` or `"read_write"`.
    pub mode: Option<String>,
    /// `"rest"`, `"ws"` or `"both"`.
    pub channels: Option<String>,
    /// Database allowlist; omitted means every database.
    pub databases: Option<Vec<String>>,
    /// Expected listener id, exactly 16 bytes.
    pub listener_id: Option<Buffer>,
    /// Deadline (ms) for the TCP connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request.
    pub request_timeout_ms: Option<u32>,
}

fn parse_mode(mode: &str) -> Result<core::LoginMode> {
    match mode {
        "read_only" => Ok(core::LoginMode::ReadOnly),
        "read_write" => Ok(core::LoginMode::ReadWrite),
        _ => Err(Error::from_reason("invalid mode")),
    }
}

fn parse_channels(channels: &str) -> Result<core::LoginChannels> {
    match channels {
        "rest" => Ok(core::LoginChannels::Rest),
        "ws" => Ok(core::LoginChannels::Ws),
        "both" => Ok(core::LoginChannels::Both),
        _ => Err(Error::from_reason("invalid channels")),
    }
}

fn copy_listener_id(buffer: &[u8]) -> Result<[u8; 16]> {
    <[u8; 16]>::try_from(buffer)
        .map_err(|_| Error::from_reason("listenerId must be exactly 16 bytes"))
}

/// Password login over the HTTP listener. Success is the named msgpack of
/// `LoginView`, a DB error is the usual error-marker `Buffer`.
#[napi]
pub fn login<'env>(env: &'env Env, opts: LoginOptions) -> Result<PromiseRaw<'env, Buffer>> {
    let password = secrets::protect(opts.password);
    let spec = EndpointSpec::parse(
        opts.host,
        opts.port,
        opts.server_name,
        opts.ca_pem,
        opts.plain_loopback,
    )?;
    let mode = opts.mode.as_deref().map(parse_mode).transpose()?;
    let channels = opts.channels.as_deref().map(parse_channels).transpose()?;
    let listener_id = opts
        .listener_id
        .as_deref()
        .map(copy_listener_id)
        .transpose()?;
    let mut scope = core::LoginScope::default();
    if let Some(label) = opts.label {
        scope = scope.with_label(label);
    }
    if let Some(mode) = mode {
        scope = scope.with_mode(mode);
    }
    if let Some(channels) = channels {
        scope = scope.with_channels(channels);
    }
    if let Some(databases) = opts.databases {
        scope = scope.with_databases(Some(databases));
    }
    let username = opts.username;
    let connect_timeout = millis(opts.connect_timeout_ms);
    let request_timeout = millis(opts.request_timeout_ms);
    env.spawn_future(async move {
        let endpoint = spec.resolve(connect_timeout).await.map_err(infra_error)?;
        let mut core_opts = core::LoginOptions::new(endpoint, username, password)
            .with_scope(scope)
            .with_listener_id(listener_id);
        if connect_timeout.is_some() {
            core_opts = core_opts.with_connect_timeout(connect_timeout);
        }
        if request_timeout.is_some() {
            core_opts = core_opts.with_request_timeout(request_timeout);
        }
        match core::login(core_opts).await {
            Ok(outcome) => encode_buffer(&LoginView::from_outcome(outcome), "encode login"),
            Err(core::ClientError::Db { code, message }) => encode_db_error(code, message),
            Err(error) => Err(infra_error(error)),
        }
    })
}
