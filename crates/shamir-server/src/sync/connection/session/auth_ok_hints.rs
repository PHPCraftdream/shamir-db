use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::wire::RotationInProgressPayload;
use shamir_connect::server::handshake::{needs_kdf_upgrade, AuthOkView, ServerHandshake};
use shamir_connect::server::rotation::ServerIdentityState;

pub(crate) fn auth_ok_hints(
    handshake: &ServerHandshake<'_>,
    view: &AuthOkView,
    identity: &ServerIdentityState,
    current_kdf: KdfParams,
) -> (Option<RotationInProgressPayload>, Option<bool>) {
    let kdf_upgrade_required =
        needs_kdf_upgrade(handshake.effective_kdf_params(), current_kdf).then_some(true);
    let rotation = match handshake.rotation_in_progress_payload(view, identity) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::debug!(%error, "auth_ok rotation hint unavailable");
            None
        }
    };
    (rotation, kdf_upgrade_required)
}
