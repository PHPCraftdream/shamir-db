//! Async TLS+SCRAM client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tokio::io::{split, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{
    HandshakeBuilder, ServerAuthOk, ServerChallenge,
};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use crate::error::ClientError;
use crate::wire_frames::{WireAuthInit, WireAuthOk, WireChallenge, WireClientProof};

/// Connection parameters.
pub struct ConnectOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS — usually the same hostname the server
    /// generated its self-signed certificate for (`localhost` in tests).
    pub server_name: String,
    /// Username (raw — will be NFC + UsernameCaseMapped normalised).
    pub username: String,
    /// Plaintext password. Zeroized after handshake completes.
    pub password: Zeroizing<Vec<u8>>,
    /// Trust-on-first-use: if `trusted_pin` is `None`, accept whatever
    /// Ed25519 public key the server presents on this first connection.
    /// Set to `false` once the pin is known and persisted.
    pub accept_new_host: bool,
    /// Pre-pinned `SHA256(server_ed25519_pub_key)`. When `Some`, the
    /// SDK validates against this hash and refuses on mismatch (spec
    /// `ServerIdentityChanged`). When `None`, requires
    /// `accept_new_host = true` and captures the pin during handshake;
    /// retrieve it via [`Client::server_pub_key_pin`] for persistence.
    pub trusted_pin: Option<[u8; 32]>,
}

/// Connected, authenticated client. One TLS stream + one session.
///
/// Roundtrips are serialized via internal mutexes — call sites can
/// share `&Client` across tasks; each `execute`/`ping` runs to
/// completion before the next starts. (Multi-in-flight pipelining
/// would require either request_id-keyed dispatcher or a per-call
/// stream split; not needed today.)
pub struct Client {
    write: tokio::sync::Mutex<WriteHalf<TlsStream<TcpStream>>>,
    read: tokio::sync::Mutex<ReadHalf<TlsStream<TcpStream>>>,
    session_id: [u8; 32],
    pinned_hash: [u8; 32],
    expires_at_ns: u64,
    resumption_ticket: Option<Vec<u8>>,
    resumption_expires_at_ns: Option<u64>,
    next_request_id: AtomicU32,
}

impl Client {
    /// Run the full TCP→TLS→SCRAM handshake and return a ready client.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ClientError> {
        // Install rustls crypto provider once. `install_default` is a
        // no-op on second call.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ---- TLS ----
        let client_cfg = make_client_config_no_ca();
        let connector = TlsConnector::from(client_cfg);
        let server_name = rustls::pki_types::ServerName::try_from(opts.server_name.clone())
            .map_err(|e| ClientError::Tls(e.to_string()))?;
        let tcp = TcpStream::connect(opts.addr).await?;
        let tls = connector.connect(server_name, tcp).await?;
        let exporter = extract_tls_exporter(&tls)
            .ok_or_else(|| ClientError::Handshake("TLS exporter unavailable".into()))?;

        // ---- SCRAM ----
        let mut hb = HandshakeBuilder::new(
            username,
            TransportKind::Tcp,
            BindingMode::TlsExporter,
        )
        .tls_exporter(exporter);
        hb = match opts.trusted_pin {
            Some(pin) => hb.pinned_hash(pin),
            None => hb.accept_new_host(opts.accept_new_host),
        };
        let hs = hb.build().map_err(|e| ClientError::Handshake(e.to_string()))?;

        let (mut r, mut w) = split(tls);

        // Step 1: auth_init
        let init = hs.auth_init();
        let init_wire = WireAuthInit {
            user: init.user,
            client_nonce: init.client_nonce.to_vec(),
            binding_mode: init.binding_mode,
            version: init.version,
        };
        write_frame(&mut w, &rmp_serde::to_vec(&init_wire)?)
            .await
            .map_err(|e| ClientError::Transport(format!("send auth_init: {e}")))?;

        // Step 2: challenge
        let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("read challenge: {e}")))?;
        let ch_wire: WireChallenge = rmp_serde::from_slice(&ch_bytes)?;
        let salt: [u8; 16] = ch_wire
            .salt
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol(format!("salt size {}", ch_wire.salt.len())))?;
        let server_nonce: [u8; 32] = ch_wire.server_nonce.as_slice().try_into().map_err(|_| {
            ClientError::Protocol(format!("server_nonce size {}", ch_wire.server_nonce.len()))
        })?;
        let challenge = ServerChallenge {
            salt,
            kdf_params: KdfParams {
                memory_kb: ch_wire.memory_kb,
                time: ch_wire.time,
                parallelism: ch_wire.parallelism,
                argon2_version: ch_wire.argon2_version,
            },
            server_nonce,
        };

        // Step 3: derive proof (Argon2id ~50ms-2s on the client thread)
        let mut password_buf = opts.password.to_vec();
        let (proof, derived, auth_message) = hs
            .process_challenge(&challenge, &mut password_buf)
            .map_err(|e| ClientError::Handshake(e.to_string()))?;
        // process_challenge zeroizes password_buf; the original
        // Zeroizing<Vec<u8>> in opts is dropped at function return.

        // Step 4: send proof
        let proof_wire = WireClientProof {
            client_proof: proof.to_vec(),
        };
        write_frame(&mut w, &rmp_serde::to_vec(&proof_wire)?)
            .await
            .map_err(|e| ClientError::Transport(format!("send proof: {e}")))?;

        // Step 5: auth_ok
        let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("read auth_ok: {e}")))?;
        let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes)?;

        let server_signature: [u8; 32] = ok_wire
            .server_signature
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("server_signature size".into()))?;
        let server_pub_key: [u8; 32] = ok_wire
            .server_pub_key
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("server_pub_key size".into()))?;
        let identity_sig: [u8; 64] = ok_wire
            .identity_sig
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("identity_sig size".into()))?;
        let session_id: [u8; 32] = ok_wire
            .session_id
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("session_id size".into()))?;

        let resumption_ticket = if ok_wire.resumption_ticket.is_empty() {
            None
        } else {
            Some(ok_wire.resumption_ticket.clone())
        };
        let resumption_expires_at_ns = if ok_wire.resumption_expires_at_ns > 0 {
            Some(ok_wire.resumption_expires_at_ns)
        } else {
            None
        };

        let auth_ok = ServerAuthOk {
            server_signature,
            server_pub_key,
            identity_sig,
            session_id,
            expires_at_ns: ok_wire.expires_at_ns,
            resumption_ticket: resumption_ticket.clone(),
            resumption_expires_at_ns,
            rotation_in_progress: None,
            kdf_upgrade_required: None,
        };

        // TOFU pin capture: if user supplied trusted_pin we already
        // pre-loaded it; otherwise the callback fires once with the
        // discovered pin.
        let pin_capture: Arc<std::sync::Mutex<Option<[u8; 32]>>> =
            Arc::new(std::sync::Mutex::new(opts.trusted_pin));
        let pin_for_cb = pin_capture.clone();

        let success = hs
            .process_auth_ok(&auth_ok, &derived, &auth_message, |pin| {
                *pin_for_cb.lock().unwrap() = Some(*pin);
            })
            .map_err(|e| ClientError::Handshake(e.to_string()))?;

        let pinned_hash = pin_capture
            .lock()
            .unwrap()
            .expect("either trusted_pin pre-set or TOFU callback fired");

        Ok(Self {
            write: tokio::sync::Mutex::new(w),
            read: tokio::sync::Mutex::new(r),
            session_id: success.session_id,
            pinned_hash,
            expires_at_ns: success.expires_at_ns,
            resumption_ticket,
            resumption_expires_at_ns,
            next_request_id: AtomicU32::new(1),
        })
    }

    /// 32-byte session id assigned by the server.
    pub fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    /// `SHA256(server_ed25519_pub_key)` — persist this for subsequent
    /// connections via `ConnectOptions.trusted_pin`.
    pub fn server_pub_key_pin(&self) -> [u8; 32] {
        self.pinned_hash
    }

    /// Absolute session expiry (unix nanoseconds).
    pub fn expires_at_ns(&self) -> u64 {
        self.expires_at_ns
    }

    /// Resumption ticket bytes (if the server issued one). Persist
    /// per-server to skip Argon2id on the next connection.
    pub fn resumption_ticket(&self) -> Option<&[u8]> {
        self.resumption_ticket.as_deref()
    }

    /// Resumption expiry (paired with [`Self::resumption_ticket`]).
    pub fn resumption_expires_at_ns(&self) -> Option<u64> {
        self.resumption_expires_at_ns
    }

    /// Health check.
    pub async fn ping(&self) -> Result<(), ClientError> {
        match self.roundtrip(&DbRequest::Ping).await? {
            DbResponse::Pong => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected Pong, got {other:?}"
            ))),
        }
    }

    /// Execute a [`BatchRequest`] against the named database.
    pub async fn execute(
        &self,
        db: &str,
        batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.to_string(),
            batch,
        };
        match self.roundtrip(&req).await? {
            DbResponse::Batch { response } => Ok(response),
            other => Err(ClientError::Protocol(format!(
                "expected Batch, got {other:?}"
            ))),
        }
    }

    /// Create a SCRAM-authenticatable user. Requires the current
    /// session to be a superuser (server enforces). Returns the
    /// stable 16-byte `user_id`.
    pub async fn create_scram_user(
        &self,
        name: &str,
        password: &str,
        roles: Vec<String>,
    ) -> Result<Vec<u8>, ClientError> {
        let req = DbRequest::CreateScramUser {
            name: name.to_string(),
            password: password.to_string(),
            roles,
        };
        match self.roundtrip(&req).await? {
            DbResponse::UserCreated { user_id, .. } => Ok(user_id),
            other => Err(ClientError::Protocol(format!(
                "expected UserCreated, got {other:?}"
            ))),
        }
    }

    /// Send a request and read its matching response. Holds both
    /// halves' locks for the duration — i.e. requests serialize per
    /// `Client` instance.
    async fn roundtrip(&self, req: &DbRequest) -> Result<DbResponse, ClientError> {
        let req_bytes = rmp_serde::to_vec_named(req)?;
        let rid = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let envelope = RequestEnvelope::new(self.session_id, Some(rid), req_bytes);
        let envelope_bytes = envelope
            .to_msgpack()
            .map_err(|e| ClientError::Protocol(format!("envelope encode: {e}")))?;

        let mut write = self.write.lock().await;
        let mut read = self.read.lock().await;

        write_frame(&mut *write, &envelope_bytes)
            .await
            .map_err(|e| ClientError::Transport(format!("send: {e}")))?;

        let resp_bytes = read_frame(&mut *read, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("recv: {e}")))?;

        let resp_envelope = ResponseEnvelope::from_msgpack(&resp_bytes)
            .map_err(|e| ClientError::Protocol(format!("envelope decode: {e}")))?;

        if resp_envelope.request_id != Some(rid) {
            return Err(ClientError::RequestIdMismatch {
                sent: Some(rid),
                got: resp_envelope.request_id,
            });
        }

        let response: DbResponse = rmp_serde::from_slice(&resp_envelope.res)?;
        if let DbResponse::Error { code, message } = &response {
            return Err(ClientError::Db {
                code: code.clone(),
                message: message.clone(),
            });
        }
        Ok(response)
    }

    /// Close the TLS write half cleanly. The read half drops with the
    /// `Client`. After this, the session is dead on the server too
    /// (TCP close → session evicted).
    pub async fn close(self) {
        let mut w = self.write.lock().await;
        let _ = AsyncWriteExt::shutdown(&mut *w).await;
    }
}
