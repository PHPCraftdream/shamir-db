use std::sync::Arc;
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use tokio::sync::{watch, RwLock};

use shamir_client as core;

use super::endpoint::EndpointSpec;
use crate::request_state::State;
use crate::secrets;
use crate::{
    encode_buffer, encode_db_error, infra_error, millis, request_timeout_error, run_request,
    snapshot_bytes,
};

/// Parameters of `ShamirTokenHttpClient.connect`.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `token` is zeroized.
#[napi(object)]
pub struct TokenHttpOptions {
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
    /// Deadline (ms) for each request.
    pub request_timeout_ms: Option<u32>,
}

type SharedHttpState = State<core::TokenHttpClient>;

/// Token-authenticated REST client: `ping` and `execute` only.
#[napi]
pub struct ShamirTokenHttpClient {
    inner: SharedHttpState,
    shutdown: watch::Sender<bool>,
    request_timeout: Option<Duration>,
}

#[napi]
impl ShamirTokenHttpClient {
    /// Dial the REST endpoint; resolves to a connected client.
    #[napi]
    pub fn connect<'env>(
        env: &'env Env,
        opts: TokenHttpOptions,
    ) -> Result<PromiseRaw<'env, ShamirTokenHttpClient>> {
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
        let request_timeout = millis(opts.request_timeout_ms);
        env.spawn_future(async move {
            let endpoint = spec.resolve(connect_timeout).await.map_err(infra_error)?;
            let mut core_opts = core::TokenHttpOptions::new(endpoint, api_token);
            if connect_timeout.is_some() {
                core_opts = core_opts.with_connect_timeout(connect_timeout);
            }
            if request_timeout.is_some() {
                core_opts = core_opts.with_request_timeout(request_timeout);
            }
            let client = core::TokenHttpClient::connect(core_opts)
                .await
                .map_err(infra_error)?;
            let (shutdown, _) = watch::channel(false);
            Ok(ShamirTokenHttpClient {
                inner: Arc::new(RwLock::new(Some(client))),
                shutdown,
                request_timeout,
            })
        })
    }

    /// Health check.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn ping<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let (state, mut shutdown, deadline, timeout) = self.request_parts();
        env.spawn_future(async move {
            let result: std::result::Result<(), core::ClientError> =
                run_request(&state, &mut shutdown, deadline, timeout, |client| {
                    Box::pin(client.ping())
                })
                .await?;
            result.map_err(infra_error)
        })
    }

    /// Execute a MessagePack-encoded `BatchRequest`; resolves to the
    /// MessagePack `BatchResponse`, or a DB-error marker `Buffer`.
    #[napi]
    pub fn execute<'env>(
        &self,
        env: &'env Env,
        db: String,
        batch: Buffer,
    ) -> Result<PromiseRaw<'env, Buffer>> {
        let (state, mut shutdown, deadline, timeout) = self.request_parts();
        let batch = snapshot_bytes(&batch);
        env.spawn_future(async move {
            // Sanctioned FFI boundary: deserialising a request that arrived as JS bytes
            let request: core::BatchRequest = rmp_serde::from_slice(&batch)
                .map_err(|error| Error::from_reason(format!("invalid batch payload: {error}")))?;
            let result: std::result::Result<core::BatchResponse, core::ClientError> =
                run_request(&state, &mut shutdown, deadline, timeout, |client| {
                    Box::pin(async move { client.execute(&db, request).await })
                })
                .await?;
            match result {
                Ok(response) => encode_buffer(&response, "encode response"),
                Err(core::ClientError::Db { code, message }) => encode_db_error(code, message),
                Err(error) => Err(infra_error(error)),
            }
        })
    }

    /// Close the connection. Idempotent — second call is a no-op.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn close<'env>(&mut self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let deadline = self.request_deadline();
        self.shutdown.send_replace(true);
        let timeout = self.request_timeout;
        let state = Arc::clone(&self.inner);
        env.spawn_future(async move {
            let closing = async move {
                let client = { state.write().await.take() };
                if let Some(client) = client {
                    client.close().await;
                }
                Ok::<(), core::ClientError>(())
            };
            match core::bound_until(deadline, closing).await {
                Ok(Some(Ok(()))) => Ok(()),
                Ok(Some(Err(error))) => Err(infra_error(error)),
                Ok(None) => Err(request_timeout_error(timeout)),
                Err(error) => Err(infra_error(error)),
            }
        })
    }
}

impl ShamirTokenHttpClient {
    fn request_parts(
        &self,
    ) -> (
        SharedHttpState,
        watch::Receiver<bool>,
        Option<tokio::time::Instant>,
        Option<Duration>,
    ) {
        (
            Arc::clone(&self.inner),
            self.shutdown.subscribe(),
            self.request_deadline(),
            self.request_timeout,
        )
    }

    fn request_deadline(&self) -> Option<tokio::time::Instant> {
        self.request_timeout
            .map(|d| tokio::time::Instant::now() + d)
    }
}
