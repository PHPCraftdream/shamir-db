use napi::bindgen_prelude::*;
use napi_derive::napi;

use shamir_client as core;

use crate::client::{from_core, ConnectOptions, PreparedConnect, ShamirClient};
use crate::secrets;
use crate::{bounded, checked_port, copy_pin, infra_error, millis, optional_pin, unresolved_addr};

/// Parameters of `ShamirClient.connectPlain`: SCRAM over plain TCP. The core
/// refuses any non-loopback address.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `password` is zeroized.
#[napi(object)]
pub struct ConnectPlainOptions {
    /// Server host; only its first resolved address is used.
    pub host: String,
    /// Server port.
    pub port: u32,
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub password: String,
    /// Trust-on-first-use, see `ConnectOptions.acceptNewHost`.
    pub accept_new_host: Option<bool>,
    /// Pre-pinned server identity hash, exactly 32 bytes.
    pub trusted_pin: Option<Buffer>,
    /// Deadline (ms) for the TCP connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for the whole handshake.
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}

/// Parameters of `ShamirClient.connectLocal`: SCRAM over a local IPC
/// endpoint.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `password` is zeroized.
#[napi(object)]
pub struct ConnectLocalOptions {
    /// Unix socket path or `\\.\pipe\name` Windows pipe, used as is.
    pub addr: String,
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub password: String,
    /// Trust-on-first-use, see `ConnectOptions.acceptNewHost`.
    pub accept_new_host: Option<bool>,
    /// Pre-pinned server identity hash, exactly 32 bytes.
    pub trusted_pin: Option<Buffer>,
    /// Deadline (ms) for the IPC connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}

/// Parameters of `ShamirClient.resume` / `resumeWs`.
///
/// The ticket is copied into a native buffer that is zeroized on drop,
/// including when the resume is cancelled before it starts; the JavaScript
/// `Buffer` it came from cannot be wiped by the binding.
#[napi(object)]
pub struct ResumeOptions {
    /// Server host; only its first resolved address is used.
    pub host: String,
    /// Server port.
    pub port: u32,
    /// SNI hostname for TLS.
    pub server_name: String,
    /// Resumption ticket from a previous session.
    pub ticket: Buffer,
    /// Required pin of the server identity, exactly 32 bytes.
    pub trusted_pin: Buffer,
    /// Deadline (ms) for the whole resume, DNS included.
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}

/// `ShamirClient.connectWs`. The host is resolved to its first address,
/// under the handshake budget, and the core then spends its own handshake
/// budget on the connect.
pub(crate) fn connect_ws(env: &Env, opts: ConnectOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    let PreparedConnect {
        host,
        port,
        mut core_opts,
    } = opts.prepare()?;
    let req_timeout = core_opts.request_timeout;
    let budget = core_opts.handshake_timeout;
    env.spawn_future(async move {
        core_opts.addr = bounded(budget, "dns", core::resolve_host(&host, port))
            .await
            .map_err(infra_error)?;
        let client = core::Client::connect_ws(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}

/// `ShamirClient.connectPlain`; first resolved address only.
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
        core_opts.addr = bounded(budget, "dns", core::resolve_host(&host, port))
            .await
            .map_err(infra_error)?;
        let client = core::Client::connect_plain(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}

/// `ShamirClient.connectLocal`.
pub(crate) fn connect_local(
    env: &Env,
    opts: ConnectLocalOptions,
) -> Result<PromiseRaw<'_, ShamirClient>> {
    let (password, trusted_pin) = secrets::protect_before(opts.password.into_bytes(), || {
        optional_pin(opts.trusted_pin)
    })?;
    let req_timeout = millis(opts.request_timeout_ms);
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
        let client = core::Client::connect_local(core_opts)
            .await
            .map_err(infra_error)?;
        Ok(from_core(client, req_timeout))
    })
}

/// `ShamirClient.resume`.
pub(crate) fn resume(env: &Env, opts: ResumeOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    resume_with(env, opts, false)
}

/// `ShamirClient.resumeWs`.
pub(crate) fn resume_ws(env: &Env, opts: ResumeOptions) -> Result<PromiseRaw<'_, ShamirClient>> {
    resume_with(env, opts, true)
}

fn resume_with(
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
        let addr = bounded(budget, "dns", core::resolve_host(&host, port))
            .await
            .map_err(infra_error)?;
        let secure = core::SecureResumeOptions {
            addr,
            server_name,
            ticket,
            pinned_hash,
        };
        let resumed = async {
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
