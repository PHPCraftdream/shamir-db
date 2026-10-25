//! Node.js native binding for `shamir-client`.
//!
//! Mirrors the Rust SDK 1:1 in Node API. JavaScript sees:
//!
//! ```js
//! const { ShamirClient } = require('shamir-client-node');
//!
//! const client = await ShamirClient.connect({
//!   host: '127.0.0.1',
//!   port: 3742,
//!   serverName: 'localhost',
//!   username: 'admin',
//!   password: 'correct horse battery staple',
//!   acceptNewHost: true,
//! });
//!
//! await client.ping();
//!
//! const resp = await client.execute('prod', {
//!   id: 'rw',
//!   queries: { rd: { from: 'items' } },
//! });
//!
//! await client.close();
//! ```
//!
//! All TLS / SCRAM / Argon2id / Ed25519 verification happens in the
//! native binary via `shamir-client` — JS never touches crypto. Drift
//! between server and client crypto is impossible: both built from the
//! same Rust source.

#![deny(clippy::all)]

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use shamir_client as core;
use shamir_query_types::wire::repl::ReplRequest;
use shamir_query_types::wire::DbResponse;

// --------------------------------------------------------------------------
// Connect options — JS sees a plain object with camelCase fields.
// --------------------------------------------------------------------------

/// Connection parameters passed to `ShamirClient.connect`.
#[napi(object)]
pub struct ConnectOptions {
    /// Server host (e.g. `"127.0.0.1"` or `"db.example.com"`).
    pub host: String,
    /// Server port.
    pub port: u32,
    /// SNI hostname for TLS — usually matches the cert's CN.
    pub server_name: String,
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Zeroised in the native side after the
    /// handshake completes.
    pub password: String,
    /// Trust-on-first-use: accept whatever Ed25519 pubkey the server
    /// presents on first connection. `true` for first connect; once you
    /// persist the pin, switch to `false` and pass `trustedPin`.
    pub accept_new_host: Option<bool>,
    /// Pre-pinned `SHA256(server_ed25519_pub_key)` — 32 bytes. When
    /// supplied, mismatch fails the handshake.
    pub trusted_pin: Option<Buffer>,
    /// Deadline (ms) for the TCP connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for the WHOLE TCP→TLS→SCRAM handshake — covers the
    /// TLS and SCRAM awaits that `connectTimeoutMs` does not
    /// (RV-CLIENT-NODE-1CFB3796). Omitted = unbounded (prior behaviour).
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip after connect.
    pub request_timeout_ms: Option<u32>,
}

// --------------------------------------------------------------------------
// Client wrapper — holds the connected core::Client behind an async
// Mutex so that close() can `take()` it (since core::Client::close
// consumes self).
// --------------------------------------------------------------------------

/// Connected, authenticated client over TLS 1.3 + SCRAM-Argon2id.
#[napi]
pub struct ShamirClient {
    inner: Arc<Mutex<Option<core::Client>>>,
    /// Cached at connect time so JS callers can read it without the
    /// async lock — useful for persisting the TOFU pin.
    pin: [u8; 32],
    session_id: [u8; 32],
    expires_at_ns: u64,
    resumption_ticket: Option<Vec<u8>>,
    resumption_expires_at_ns: Option<u64>,
    /// Cached at connect time: the terminal per-request deadline — bounds
    /// each JS call (lock wait + write + response wait) from the moment it
    /// starts (RV-CLIENT-NODE-1CFB3796). `None` = unbounded (prior behaviour).
    request_timeout: Option<std::time::Duration>,
}

#[napi]
impl ShamirClient {
    /// Run the full TCP→TLS→SCRAM handshake; resolves to a connected
    /// client.
    #[napi(factory)]
    pub async fn connect(opts: ConnectOptions) -> Result<ShamirClient> {
        let port = u16::try_from(opts.port)
            .map_err(|_| Error::from_reason(format!("port out of range: {}", opts.port)))?;
        let req_timeout = opts
            .request_timeout_ms
            .map(|ms| std::time::Duration::from_millis(u64::from(ms)));
        let hs_timeout = opts
            .handshake_timeout_ms
            .map(|ms| std::time::Duration::from_millis(u64::from(ms)));

        let trusted_pin = match opts.trusted_pin {
            None => None,
            Some(b) => {
                let bytes: &[u8] = &b;
                if bytes.len() != 32 {
                    return Err(Error::from_reason(format!(
                        "trustedPin must be 32 bytes, got {}",
                        bytes.len()
                    )));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                Some(arr)
            }
        };

        // Single production hostname path: resolution + connect live in the
        // core (`connect_host`); the Node layer parses nothing.
        let host = opts.host.clone();
        let core_opts = core::ConnectOptions {
            // Overwritten by `connect_host` from host:port — never used.
            addr: std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0,
            ),
            server_name: opts.server_name,
            username: opts.username,
            password: Zeroizing::new(opts.password.into_bytes()),
            accept_new_host: opts.accept_new_host.unwrap_or(false),
            trusted_pin,
            connect_timeout: opts
                .connect_timeout_ms
                .map(|ms| std::time::Duration::from_millis(u64::from(ms))),
            handshake_timeout: hs_timeout,
            request_timeout: req_timeout,
        };
        let client = core::Client::connect_host(&host, port, core_opts)
            .await
            .map_err(infra_error)?;

        let pin = client.server_pub_key_pin();
        let session_id = client.session_id();
        let expires_at_ns = client.expires_at_ns();
        let resumption_ticket = client.resumption_ticket().map(|s| s.to_vec());
        let resumption_expires_at_ns = client.resumption_expires_at_ns();

        Ok(ShamirClient {
            inner: Arc::new(Mutex::new(Some(client))),
            pin,
            session_id,
            expires_at_ns,
            resumption_ticket,
            resumption_expires_at_ns,
            request_timeout: req_timeout,
        })
    }

    /// `SHA256(server_ed25519_pub_key)` — persist this and pass back
    /// as `trustedPin` on subsequent connections.
    #[napi]
    pub fn server_pub_key_pin(&self) -> Buffer {
        Buffer::from(self.pin.to_vec())
    }

    /// 32-byte session id assigned by the server.
    #[napi]
    pub fn session_id(&self) -> Buffer {
        Buffer::from(self.session_id.to_vec())
    }

    /// Absolute session expiry (unix nanoseconds). Returned as `BigInt`
    /// in JS because nanoseconds since epoch overflow `Number`.
    #[napi]
    pub fn expires_at_ns(&self) -> BigInt {
        BigInt::from(self.expires_at_ns)
    }

    /// Resumption ticket bytes (if the server issued one).
    #[napi]
    pub fn resumption_ticket(&self) -> Option<Buffer> {
        self.resumption_ticket
            .as_ref()
            .map(|b| Buffer::from(b.clone()))
    }

    /// Resumption expiry (paired with [`Self::resumption_ticket`]).
    #[napi]
    pub fn resumption_expires_at_ns(&self) -> Option<BigInt> {
        self.resumption_expires_at_ns.map(BigInt::from)
    }

    /// Health check.
    #[napi]
    pub async fn ping(&self) -> Result<()> {
        let deadline = self.request_deadline();
        match core::bound_until(deadline, async {
            let guard = self.inner.lock().await;
            let client = guard
                .as_ref()
                .ok_or_else(|| core::ClientError::Transport("client closed".into()))?;
            client.ping().await
        })
        .await
        {
            Ok(Some(Ok(()))) => Ok(()),
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }

    /// Execute a `BatchRequest` (passed as a MessagePack-encoded `Buffer`)
    /// against the named database. Returns the full `BatchResponse` as a
    /// MessagePack-encoded `Buffer`.
    #[napi]
    pub async fn execute(&self, db: String, batch: Buffer) -> Result<Buffer> {
        let deadline = self.request_deadline();
        // Domain-level DB errors (ClientError::Db) are routed through
        // [`encode_db_error`] as a marker Buffer instead of thrown as a plain
        // napi Error — the JS wrapper (js/wrapper.js) decodes the marker and throws
        // a typed ShamirDbError with `.code` / `.retryable`. See task #519.
        match core::bound_until(deadline, async {
            // Decoding sits INSIDE the deadline: the budget covers the whole call.
            let batch_req: core::BatchRequest = rmp_serde::from_slice(&batch[..])
                .map_err(|e| core::ClientError::Transport(format!("invalid batch payload: {e}")))?;
            let guard = self.inner.lock().await;
            let client = guard
                .as_ref()
                .ok_or_else(|| core::ClientError::Transport("client closed".into()))?;
            client.execute(&db, batch_req).await
        })
        .await
        {
            Ok(Some(Ok(response))) => {
                let bytes = rmp_serde::to_vec_named(&response)
                    .map_err(|e| Error::from_reason(format!("encode response: {e}")))?;
                Ok(Buffer::from(bytes))
            }
            Ok(Some(Err(core::ClientError::Db { code, message }))) => {
                encode_db_error(code, message)
            }
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }

    /// Privileged replication pull-API (REPLICATION §5). Takes a msgpack
    /// `ReplRequest` Buffer, returns a msgpack `ReplResponse` Buffer. The
    /// session must hold the `replicator` role (or be superuser).
    #[napi]
    pub async fn repl(&self, req: Buffer) -> Result<Buffer> {
        // FFI boundary — raw serde is the sanctioned exception (CLAUDE.md):
        // we deserialize a request that ARRIVED as bytes, not construct a query.
        let deadline = self.request_deadline();
        // FFI boundary — raw serde is the sanctioned exception (CLAUDE.md):
        // we deserialize a request that ARRIVED as bytes, not construct a query.
        match core::bound_until(deadline, async {
            let repl_req: ReplRequest = rmp_serde::from_slice(&req[..])
                .map_err(|e| core::ClientError::Transport(format!("invalid repl payload: {e}")))?;
            let guard = self.inner.lock().await;
            let client = guard
                .as_ref()
                .ok_or_else(|| core::ClientError::Transport("client closed".into()))?;
            client.repl(repl_req).await
        })
        .await
        {
            Ok(Some(Ok(resp))) => {
                let bytes = rmp_serde::to_vec_named(&resp)
                    .map_err(|e| Error::from_reason(format!("encode repl response: {e}")))?;
                Ok(Buffer::from(bytes))
            }
            Ok(Some(Err(core::ClientError::Db { code, message }))) => {
                encode_db_error(code, message)
            }
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }

    /// Create a new SCRAM-authenticatable user. Requires the current
    /// session to belong to a superuser. Returns the stable 16-byte
    /// `user_id` as a Buffer.
    #[napi]
    pub async fn create_scram_user(
        &self,
        name: String,
        password: String,
        roles: Vec<String>,
    ) -> Result<Buffer> {
        let deadline = self.request_deadline();
        match core::bound_until(deadline, async {
            let guard = self.inner.lock().await;
            let client = guard
                .as_ref()
                .ok_or_else(|| core::ClientError::Transport("client closed".into()))?;
            client
                .create_scram_user(&name, Zeroizing::new(password), roles)
                .await
        })
        .await
        {
            Ok(Some(Ok(user_id))) => Ok(Buffer::from(user_id)),
            Ok(Some(Err(core::ClientError::Db { code, message }))) => {
                encode_db_error(code, message)
            }
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }

    /// Grant or revoke the `replicator` role on an existing SCRAM user via
    /// the dedicated `SetReplicator` wire op (the `replicator` pseudo-role is
    /// reserved — it cannot be attached through `create_scram_user`'s generic
    /// `roles` array). Requires the current session to belong to a superuser.
    ///
    /// Domain-level DB errors (e.g. `permission_denied`, `hmac_required`,
    /// `not_found`) surface through the same `DbResponse::Error`-marker
    /// convention as `create_scram_user` / `repl` — the JS wrapper decodes the
    /// returned `Buffer` and throws a typed `ShamirDbError`.
    #[napi]
    pub async fn set_replicator(&self, user: String, on: bool) -> Result<Buffer> {
        let deadline = self.request_deadline();
        match core::bound_until(deadline, async {
            let guard = self.inner.lock().await;
            let client = guard
                .as_ref()
                .ok_or_else(|| core::ClientError::Transport("client closed".into()))?;
            client.set_replicator(&user, on).await
        })
        .await
        {
            Ok(Some(Ok(()))) => {
                // Success: encode the echoed `ReplicatorSet { user, on }` so
                // the JS wrapper can `decodeOrThrow` for the error marker,
                // matching `create_scram_user`'s `Result<Buffer>` convention.
                // The server echoes exactly these values on a confirmed
                // grant/revoke — the core client already verified the
                // `ReplicatorSet` variant before returning.
                let resp = DbResponse::ReplicatorSet { user, on };
                let bytes = rmp_serde::to_vec_named(&resp).map_err(|e| {
                    Error::from_reason(format!("encode set_replicator response: {e}"))
                })?;
                Ok(Buffer::from(bytes))
            }
            Ok(Some(Err(core::ClientError::Db { code, message }))) => {
                encode_db_error(code, message)
            }
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }

    /// Close the TLS write half cleanly. Idempotent — second call is
    /// a no-op.
    #[napi]
    pub async fn close(&self) -> Result<()> {
        let deadline = self.request_deadline();
        match core::bound_until(deadline, async {
            let mut guard = self.inner.lock().await;
            if let Some(client) = guard.take() {
                client.close().await;
            }
            Ok::<(), core::ClientError>(())
        })
        .await
        {
            Ok(Some(Ok(()))) => Ok(()),
            // `close()`'s future is infallible; the arm exists only for
            // exhaustiveness.
            Ok(Some(Err(e))) => Err(infra_error(e)),
            Ok(None) => Err(self.request_timeout_error()),
            Err(e) => Err(infra_error(e)),
        }
    }
}

impl ShamirClient {
    /// Absolute per-request deadline, captured at the FIRST statement of
    /// every async method so the budget covers decoding, the client-mutex
    /// wait, the write and the response wait (RV-CLIENT-NODE-1CFB3796).
    fn request_deadline(&self) -> Option<tokio::time::Instant> {
        self.request_timeout
            .map(|d| tokio::time::Instant::now() + d)
    }

    fn request_timeout_error(&self) -> Error {
        Error::from_reason(format!(
            "request timed out after {:?}",
            self.request_timeout.unwrap_or_default()
        ))
    }
}

// -----------------------------------------------------------------------
// Error mapping
// -----------------------------------------------------------------------
//
// napi-rs 3.x async `#[napi]` fns hard-pin the Result error type to
// `napi::Error<Status>`: the codegen funnels through
// `execute_tokio_future_with_finalize_callback` whose `Future` bound is
// `Output = Result<Data, impl Into<Error>>` where `Error = Error<Status>`.
// A custom `Error<S>` (where `S: AsRef<str>` carries the DB error code as the
// JS `.code` property) has no `From<Error<S>> for Error<Status>` impl, and the
// orphan rule blocks adding one — so the Rust-side custom-error-type approach
// does NOT compile for async fns (verified by real compilation, task #519).
//
// Instead, domain-level DB errors (`ClientError::Db { code, message }`) are
// NOT thrown at the napi boundary. The Rust method returns `Ok(Buffer)` where
// the buffer is a msgpack-encoded `DbResponse::Error { kind: "error", code,
// message }`. The JS wrapper in `js/wrapper.js` decodes every response buffer and,
// if it detects the `kind: "error"` marker, throws a typed `ShamirDbError`
// (mirrors the TS SDK's `shamir-client-ts/src/core/support/errors.ts`) with `.code`
// and `.retryable` properties.
//
// Infrastructure errors (connection closed, TLS, transport, timeout, …) stay
// as plain napi `Error<Status>` → JS `Error` with only `.message`, which is
// appropriate — these are not the `ShamirDbError`-parity case.

/// Map an **infrastructure** [`core::ClientError`] into a napi [`Error`].
///
/// Callers that return `Buffer` (`execute`, `repl`, `create_scram_user`)
/// route `ClientError::Db` through [`encode_db_error`] instead — this
/// function is only reached for non-Db variants or from `connect` / `ping`
/// where no structured-error surface is possible.
fn infra_error(e: core::ClientError) -> Error {
    Error::from_reason(e.to_string())
}

/// Encode a domain-level DB error as a `DbResponse::Error` msgpack Buffer.
///
/// Returns `Ok(Buffer)` so the JS wrapper can decode `{ kind: "error",
/// code, message }`, detect the marker, and throw a `ShamirDbError` with
/// typed `.code` / `.retryable` — instead of receiving it as a thrown napi
/// `Error` that only carries `.message`.
fn encode_db_error(code: String, message: String) -> Result<Buffer> {
    let err = DbResponse::Error { code, message };
    let bytes = rmp_serde::to_vec_named(&err)
        .map_err(|e| Error::from_reason(format!("encode db error: {e}")))?;
    Ok(Buffer::from(bytes))
}
