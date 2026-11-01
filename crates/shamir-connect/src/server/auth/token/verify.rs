use super::{
    format::parse_token,
    mac::{compute_mac, dummy_mac_work, mac_eq},
    mirror::{TokenMirror, TokenState},
    record::Endpoint,
};
use std::sync::Arc;
/// The server supplies its synchronous cached owner floor. `None` denotes an unknown owner.
pub trait OwnerFloors {
    fn floor_if_present(&self, user_id: &[u8; 16]) -> Option<u64>;
}
/// Secret verification key is always redacted in Debug.
pub struct VerifyCtx<'a> {
    pub mirror: &'a TokenMirror,
    pub key: &'a [u8; 32],
    pub key_id: u8,
    pub owners: &'a dyn OwnerFloors,
    pub now_ns: u64,
    pub endpoint: Endpoint,
}
impl std::fmt::Debug for VerifyCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyCtx")
            .field("key", &"<REDACTED>")
            .field("key_id", &self.key_id)
            .field("now_ns", &self.now_ns)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyFailure {
    #[error("invalid token")]
    Invalid,
    #[error("token revoked")]
    Revoked,
    #[error("token expired")]
    Expired,
    #[error("token owner invalid")]
    OwnerInvalid,
    #[error("token channel denied")]
    ChannelDenied,
}
pub struct Verified {
    pub state: Arc<TokenState>,
}
impl std::fmt::Debug for Verified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verified")
            .field("state", &self.state)
            .finish()
    }
}
const FORMAT_VERSION: u8 = 1;
#[cfg(test)]
thread_local! {
    static MAC_COMPUTATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub fn reset_mac_count() {
    MAC_COMPUTATIONS.with(|count| count.set(0));
}
#[cfg(test)]
pub fn mac_count() -> usize {
    MAC_COMPUTATIONS.with(std::cell::Cell::get)
}
#[cfg(test)]
pub(super) fn record_mac_computation() {
    MAC_COMPUTATIONS.with(|count| count.set(count.get() + 1));
}
/// Decision order is normative TOKEN_API.md §5. Unknown ID and incorrect secret each compute one HMAC.
pub fn verify_token(ctx: &VerifyCtx<'_>, input: &[u8]) -> Result<Verified, VerifyFailure> {
    let parsed = parse_token(input).map_err(|_| VerifyFailure::Invalid)?;
    let Some(state) = ctx.mirror.get(&parsed.id) else {
        dummy_mac_work(ctx.key, ctx.key_id, FORMAT_VERSION);
        return Err(VerifyFailure::Invalid);
    };
    let record = state.record();
    if record.key_id != ctx.key_id || record.format_version != FORMAT_VERSION {
        return Err(VerifyFailure::Invalid);
    }
    let computed = compute_mac(
        ctx.key,
        ctx.key_id,
        FORMAT_VERSION,
        &parsed.id,
        &parsed.secret,
    );
    if !mac_eq(&computed, &record.mac) {
        return Err(VerifyFailure::Invalid);
    }
    if state.is_revoked() {
        return Err(VerifyFailure::Revoked);
    }
    if ctx.now_ns >= record.expires_at_ns {
        return Err(VerifyFailure::Expired);
    }
    let Some(floor) = ctx.owners.floor_if_present(&record.owner_user_id) else {
        return Err(VerifyFailure::OwnerInvalid);
    };
    if record.created_at_ns <= floor {
        return Err(VerifyFailure::OwnerInvalid);
    }
    if !record.channels.allows(ctx.endpoint) {
        return Err(VerifyFailure::ChannelDenied);
    }
    Ok(Verified { state })
}
