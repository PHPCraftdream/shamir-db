//! Token WebSocket session binding and lifetime arithmetic.

use serde::Serialize;

use shamir_connect::server::session::Session;
use shamir_connect::server::token::Verified;

const NS_PER_HOUR: u64 = 3_600_000_000_000;

/// Build a token session, absolute deadline, and fresh nonzero session id.
pub(crate) fn new_binding(
    grant: &Verified,
    username: String,
    opened_at_ns: u64,
    session_max_age_hours: u64,
) -> Result<(Session, u64, Vec<u8>), ()> {
    let sid = generate_sid()?;
    let deadline = session_deadline(
        opened_at_ns,
        grant.state.record().expires_at_ns(),
        session_max_age_hours,
    );
    let session = Session::for_token(grant, username, opened_at_ns, sid);
    let binding = encode_binding(&sid, deadline)?;
    Ok((session, deadline, binding))
}

/// Compute the earlier token/session expiry, saturating overflow to token expiry.
pub(crate) fn session_deadline(opened_at_ns: u64, token_expiry_ns: u64, hours: u64) -> u64 {
    hours
        .checked_mul(NS_PER_HOUR)
        .and_then(|duration| opened_at_ns.checked_add(duration))
        .map_or(token_expiry_ns, |session| session.min(token_expiry_ns))
}

/// Generate a 32-byte, OS-backed, nonzero session identifier.
pub(crate) fn generate_sid() -> Result<[u8; 32], ()> {
    for _ in 0..4 {
        let sid = shamir_connect::common::crypto::try_random_array::<32>().map_err(|_| ())?;
        if sid.iter().any(|byte| *byte != 0) {
            return Ok(sid);
        }
    }
    Err(())
}

/// Encode the exact three-field token WebSocket binding envelope.
pub(crate) fn encode_binding(sid: &[u8; 32], deadline: u64) -> Result<Vec<u8>, ()> {
    #[derive(Serialize)]
    struct Binding<'a> {
        ok: bool,
        #[serde(with = "serde_bytes")]
        sid: &'a [u8],
        expires_at_ns: u64,
    }
    rmp_serde::to_vec_named(&Binding {
        ok: true,
        sid,
        expires_at_ns: deadline,
    })
    .map_err(|_| ())
}
