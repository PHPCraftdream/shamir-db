//! Client-side bootstrap flow (spec §11).
//!
//! Verifies the server's `bootstrap_challenge` against the operator-supplied
//! pin BEFORE deriving and sending the password material — so even a MITM
//! cannot harvest the token + password by impersonating the server.

use crate::common::bootstrap_message::build_bootstrap_input;
use crate::common::crypto::{constant_time_eq, sha256, Ed25519Keypair};
use crate::common::error::{Error, Result};
use crate::common::identity::verify_identity;
use crate::common::kdf_params::KdfParams;
use crate::common::scram::DerivedKeys;
use crate::common::types::{limits, TransportKind};
use crate::common::username::NormalizedUsername;
use crate::server::bootstrap::{BootstrapChallenge, BootstrapHello, BootstrapRequest};
use zeroize::Zeroize;

/// Maximum allowed clock skew between client and server during bootstrap.
/// Per spec §11.3.4 (d).
pub const BOOTSTRAP_CLOCK_SKEW_NS: u64 = 60 * crate::common::time::ns::SECOND;

/// Generate a `bootstrap_hello` — caller transmits this as the first wire frame.
pub fn build_hello() -> BootstrapHello {
    BootstrapHello {
        client_nonce: crate::common::crypto::random_array::<32>(),
    }
}

/// Verify a [`BootstrapChallenge`] received from the server.
///
/// Performs (in order, fail-closed) per spec §11.3.4:
/// (a) `SHA256(server_pub_key) == pinned_hash` (constant-time)
/// (b) Ed25519 strict verify of `identity_sig_bootstrap`
/// (c) `client_nonce` echoed in payload matches what we sent (anti-replay)
/// (d) `abs(now_ns - server_time_ns) ≤ 60s` (clock anomaly)
///
/// On any failure: returns [`Error::BootstrapFailed`] — caller MUST disconnect
/// without transmitting the token.
pub fn verify_challenge(
    pinned_hash: &[u8; 32],
    transport_kind: TransportKind,
    tls_exporter: &[u8; 32],
    hello: &BootstrapHello,
    challenge: &BootstrapChallenge,
    now_ns: u64,
) -> Result<()> {
    // (a) pin
    let received_hash = sha256(&challenge.server_pub_key);
    if !constant_time_eq(pinned_hash, &received_hash) {
        return Err(Error::BootstrapFailed);
    }

    // (b) signature — note we recompute the payload locally with the SAME
    // client_nonce we sent (so (c) is implicit in (b) once payload matches).
    let payload = build_bootstrap_input(
        &challenge.server_pub_key,
        transport_kind,
        tls_exporter,
        &hello.client_nonce,
        challenge.server_time_ns,
    );
    if !verify_identity(
        &challenge.server_pub_key,
        &payload,
        &challenge.identity_sig_bootstrap,
    ) {
        return Err(Error::BootstrapFailed);
    }

    // (d) clock anomaly
    let skew = now_ns.abs_diff(challenge.server_time_ns);
    if skew > BOOTSTRAP_CLOCK_SKEW_NS {
        return Err(Error::BootstrapFailed);
    }

    Ok(())
}

/// Build a [`BootstrapRequest`] — derives material from `password` locally
/// (spec §11.3.5). `password` is zeroized after Argon2id.
pub fn build_request(
    token: [u8; 32],
    user: NormalizedUsername,
    password: &mut [u8],
    kdf_params: KdfParams,
) -> Result<BootstrapRequest> {
    // Spec §3.2: enforce password policy BEFORE running Argon2id (server
    // cannot validate; this is the client's only chance).
    crate::common::password::validate_password(password)?;
    kdf_params.validate_client_limits()?;
    let salt = crate::common::crypto::random_array::<{ limits::SALT_BYTES }>();
    let derived = DerivedKeys::derive(password, &salt, &kdf_params)?;
    password.zeroize();

    let mut server_key_bytes = [0u8; 32];
    server_key_bytes.copy_from_slice(&derived.server_key[..]);

    Ok(BootstrapRequest {
        token,
        user,
        salt,
        stored_key: derived.stored_key.0,
        server_key: server_key_bytes,
        kdf_params,
    })
}

/// Lightweight helper for testing & convenience: wrap a complete bootstrap
/// flow (hello → challenge → request) into one call. Intended for in-process
/// integration tests; production caller wires each step to its own transport.
pub fn run_local_bootstrap_with(
    keypair: &Ed25519Keypair,
    pinned_hash: &[u8; 32],
    transport_kind: TransportKind,
    tls_exporter: &[u8; 32],
    token: [u8; 32],
    user: NormalizedUsername,
    password: &mut [u8],
    kdf_params: KdfParams,
    now_ns: u64,
) -> Result<BootstrapRequest> {
    let hello = build_hello();
    let challenge = crate::server::bootstrap::make_bootstrap_challenge(
        keypair,
        transport_kind,
        tls_exporter,
        &hello,
    );
    verify_challenge(
        pinned_hash,
        transport_kind,
        tls_exporter,
        &hello,
        &challenge,
        now_ns,
    )?;
    build_request(token, user, password, kdf_params)
}
