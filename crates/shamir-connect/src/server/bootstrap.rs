//! Server-side bootstrap state machine (spec §11).
//!
//! Bootstrap creates the **first** superuser. It is gated by the
//! `superuser_ever_existed` invariant in `__system__/server_meta`. Once a
//! bootstrap has succeeded the flag stays `true` forever (defending against
//! "silent re-bootstrap on corrupted backup", spec §11.1).
//!
//! Wire flow (per spec §11.3):
//! ```text
//! 1. client → server: bootstrap_hello { client_nonce(32) }
//! 2. server → client: bootstrap_challenge { server_pub_key, server_time, identity_sig_bootstrap }
//! 3. client verifies pin + sig + client_nonce + server_time freshness
//! 4. client → server: bootstrap { token, user, salt, stored_key, server_key, kdf_params }
//! 5. server CAS-validates and creates the user
//! ```

use crate::common::bootstrap_message::build_bootstrap_input;
use crate::common::crypto::{constant_time_eq, sha256, Ed25519Keypair, StoredKey};
use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use crate::common::time::UnixNanos;
use crate::common::types::{limits, TransportKind};
use crate::common::username::NormalizedUsername;
use crate::server::user_record::UserRecord;
use parking_lot::Mutex;
use zeroize::Zeroizing;

/// Server-side bootstrap meta — held in `__system__/server_meta`.
///
/// Only the bootstrap-related fields are modeled here; the larger
/// `server_meta` schema lives elsewhere in the application.
///
/// Custom [`Debug`] impl redacts `bootstrap_token_hash` (spec IMPL §4 —
/// the hash itself is sensitive: equality lets an attacker confirm a
/// guessed token).
pub struct BootstrapState {
    inner: Mutex<BootstrapInner>,
}

impl core::fmt::Debug for BootstrapState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let g = self.inner.lock();
        f.debug_struct("BootstrapState")
            .field(
                "bootstrap_token_hash",
                &if g.bootstrap_token_hash.is_some() {
                    "<REDACTED:32 (active)>"
                } else {
                    "<None>"
                },
            )
            .field("bootstrap_token_expires_at_ns", &g.bootstrap_token_expires_at_ns)
            .field("superuser_ever_existed", &g.superuser_ever_existed)
            .finish()
    }
}

struct BootstrapInner {
    /// SHA256 of the active bootstrap token (or None when consumed/expired).
    bootstrap_token_hash: Option<[u8; 32]>,
    /// Wall-clock expiry of the active token.
    bootstrap_token_expires_at_ns: Option<u64>,
    /// Persistent flag: once a bootstrap has succeeded, this stays true.
    superuser_ever_existed: bool,
}

impl BootstrapState {
    /// Construct empty (fresh server first start).
    pub fn empty() -> Self {
        Self {
            inner: Mutex::new(BootstrapInner {
                bootstrap_token_hash: None,
                bootstrap_token_expires_at_ns: None,
                superuser_ever_existed: false,
            }),
        }
    }

    /// Construct from existing meta — useful when rehydrating from disk.
    pub fn from_meta(
        bootstrap_token_hash: Option<[u8; 32]>,
        bootstrap_token_expires_at_ns: Option<u64>,
        superuser_ever_existed: bool,
    ) -> Self {
        Self {
            inner: Mutex::new(BootstrapInner {
                bootstrap_token_hash,
                bootstrap_token_expires_at_ns,
                superuser_ever_existed,
            }),
        }
    }

    /// Whether bootstrap is currently allowed (spec §11.1 trigger).
    pub fn is_bootstrap_allowed(&self) -> bool {
        let g = self.inner.lock();
        !g.superuser_ever_existed && g.bootstrap_token_hash.is_none()
    }

    /// Issue a fresh bootstrap token. Returns the **plaintext** token bytes
    /// for the operator to deliver out-of-band.
    ///
    /// Caller is responsible for outputting the token via the configured
    /// channel (TTY / file / command), per spec §11.2.3.
    pub fn issue_token(
        &self,
        ttl_ns: u64,
        now_ns: u64,
    ) -> Result<Zeroizing<[u8; 32]>> {
        let mut g = self.inner.lock();
        if g.superuser_ever_existed {
            return Err(Error::BootstrapFailed);
        }
        if g.bootstrap_token_hash.is_some() {
            return Err(Error::BootstrapFailed);
        }
        let token: [u8; 32] = crate::common::crypto::random_array::<32>();
        g.bootstrap_token_hash = Some(sha256(&token));
        g.bootstrap_token_expires_at_ns = Some(now_ns.saturating_add(ttl_ns));
        Ok(Zeroizing::new(token))
    }

    /// Inspect: peek expiry (for status / metrics).
    pub fn token_expires_at_ns(&self) -> Option<u64> {
        self.inner.lock().bootstrap_token_expires_at_ns
    }

    /// Atomic consume + create-user. Returns the new [`UserRecord`] on
    /// success, [`Error::BootstrapFailed`] otherwise (generic per spec §11.3.7).
    ///
    /// Consumed-token state is cleared and `superuser_ever_existed` is set
    /// to `true` BEFORE we hand back the record, ensuring the invariant
    /// `bootstrap_token_hash IS NULL ⇔ superuser EXISTS` holds atomically
    /// from the caller's perspective.
    #[allow(clippy::too_many_arguments)]
    pub fn consume(
        &self,
        token: &[u8; 32],
        salt: [u8; 16],
        stored_key: StoredKey,
        server_key: Zeroizing<[u8; 32]>,
        kdf_params: KdfParams,
        kdf_params_current: &KdfParams,
        now_ns: u64,
    ) -> Result<UserRecord> {
        let mut g = self.inner.lock();

        // Pre-conditions: token must be present, not expired, and match.
        let stored_hash = g.bootstrap_token_hash.ok_or(Error::BootstrapFailed)?;
        let expires_at = g
            .bootstrap_token_expires_at_ns
            .ok_or(Error::BootstrapFailed)?;
        if expires_at <= now_ns {
            // Auto-cleanup expired state.
            g.bootstrap_token_hash = None;
            g.bootstrap_token_expires_at_ns = None;
            return Err(Error::BootstrapFailed);
        }

        // Constant-time hash compare.
        let received_hash = sha256(token);
        if !constant_time_eq(&stored_hash, &received_hash) {
            return Err(Error::BootstrapFailed);
        }

        // KDF must equal current server defaults (spec §11.3.6).
        if &kdf_params != kdf_params_current {
            return Err(Error::BootstrapFailed);
        }
        // Floor check is part of bootstrap_failed surface (no leaking why).
        kdf_params
            .validate_server_floor()
            .map_err(|_| Error::BootstrapFailed)?;

        // Atomic invariant flip.
        g.bootstrap_token_hash = None;
        g.bootstrap_token_expires_at_ns = None;
        g.superuser_ever_existed = true;

        Ok(UserRecord {
            salt,
            stored_key,
            server_key,
            kdf_params,
            tickets_invalid_before_ns: 0,
        })
    }

    /// Test/utility: whether `superuser_ever_existed` is set.
    pub fn superuser_ever_existed(&self) -> bool {
        self.inner.lock().superuser_ever_existed
    }
}

/// Wire view: client → server `bootstrap_hello`.
#[derive(Debug, Clone)]
pub struct BootstrapHello {
    /// 32-byte CSPRNG nonce — anti-replay for the challenge.
    pub client_nonce: [u8; 32],
}

/// Wire view: server → client `bootstrap_challenge`.
#[derive(Debug, Clone)]
pub struct BootstrapChallenge {
    /// Current Ed25519 server public key.
    pub server_pub_key: [u8; 32],
    /// Server wall-clock unix nanos (client validates `abs(now - server_time) ≤ 60s`).
    pub server_time_ns: u64,
    /// Ed25519 signature over `build_bootstrap_input(...)`.
    pub identity_sig_bootstrap: [u8; 64],
}

/// Wire view: client → server `bootstrap` (the actual create-superuser).
#[derive(Debug, Clone)]
pub struct BootstrapRequest {
    /// 32-byte token from operator (out-of-band channel).
    pub token: [u8; 32],
    /// Username (post-NFC + UsernameCaseMapped).
    pub user: NormalizedUsername,
    /// Per-user salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// Stored key (= SHA256(client_key)).
    pub stored_key: [u8; 32],
    /// Server key (= HMAC(salted_password, "Server Key")).
    pub server_key: [u8; 32],
    /// KDF parameters used.
    pub kdf_params: KdfParams,
}

/// Server-side: build the [`BootstrapChallenge`] in response to `bootstrap_hello`.
pub fn make_bootstrap_challenge(
    server_keypair: &Ed25519Keypair,
    transport_kind: TransportKind,
    tls_exporter: &[u8; 32],
    hello: &BootstrapHello,
) -> BootstrapChallenge {
    let server_pub = server_keypair.public_bytes();
    let server_time_ns = UnixNanos::now().as_u64();
    let payload = build_bootstrap_input(
        &server_pub,
        transport_kind,
        tls_exporter,
        &hello.client_nonce,
        server_time_ns,
    );
    let sig = server_keypair.sign(&payload);
    BootstrapChallenge {
        server_pub_key: server_pub,
        server_time_ns,
        identity_sig_bootstrap: sig,
    }
}
