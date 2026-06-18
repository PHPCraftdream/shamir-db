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

use std::net::SocketAddr;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use shamir_client as core;

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
}

#[napi]
impl ShamirClient {
    /// Run the full TCP→TLS→SCRAM handshake; resolves to a connected
    /// client.
    #[napi(factory)]
    pub async fn connect(opts: ConnectOptions) -> Result<ShamirClient> {
        let port = u16::try_from(opts.port).map_err(|_| {
            Error::from_reason(format!("port out of range: {}", opts.port))
        })?;
        let addr: SocketAddr = format!("{}:{}", opts.host, port)
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                Error::from_reason(format!("invalid host:port: {e}"))
            })?;

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

        let core_opts = core::ConnectOptions {
            addr,
            server_name: opts.server_name,
            username: opts.username,
            password: Zeroizing::new(opts.password.into_bytes()),
            accept_new_host: opts.accept_new_host.unwrap_or(false),
            trusted_pin,
        };

        let client = core::Client::connect(core_opts)
            .await
            .map_err(to_napi)?;

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
        let guard = self.inner.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| Error::from_reason("client closed"))?;
        client.ping().await.map_err(to_napi)
    }

    /// Execute a `BatchRequest` (passed as a MessagePack-encoded `Buffer`)
    /// against the named database. Returns the full `BatchResponse` as a
    /// MessagePack-encoded `Buffer`.
    #[napi]
    pub async fn execute(&self, db: String, batch: Buffer) -> Result<Buffer> {
        let batch_req: core::BatchRequest = rmp_serde::from_slice(&batch[..])
            .map_err(|e| Error::from_reason(format!("invalid batch payload: {e}")))?;
        let guard = self.inner.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| Error::from_reason("client closed"))?;
        let response = client.execute(&db, batch_req).await.map_err(to_napi)?;
        let bytes = rmp_serde::to_vec_named(&response)
            .map_err(|e| Error::from_reason(format!("encode response: {e}")))?;
        Ok(Buffer::from(bytes))
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
        let guard = self.inner.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| Error::from_reason("client closed"))?;
        let user_id = client
            .create_scram_user(&name, Zeroizing::new(password), roles)
            .await
            .map_err(to_napi)?;
        Ok(Buffer::from(user_id))
    }

    /// Close the TLS write half cleanly. Idempotent — second call is
    /// a no-op.
    #[napi]
    pub async fn close(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(client) = guard.take() {
            client.close().await;
        }
        Ok(())
    }
}

fn to_napi(e: core::ClientError) -> Error {
    Error::from_reason(e.to_string())
}
