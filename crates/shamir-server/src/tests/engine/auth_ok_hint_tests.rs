use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use shamir_connect::common::auth_message::{AuthMessage, AuthMessageInputs};
use shamir_connect::common::crypto::sha256;
use shamir_connect::common::identity::build_identity_input;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, ProtocolVersion, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::config::{ListenerPolicy, ServerSecrets};
use shamir_connect::server::handshake::{AuthInitView, AuthOkView, ServerHandshake};
use shamir_connect::server::rotation::ServerIdentityState;

use crate::connection::auth_ok_hints::auth_ok_hints;
use crate::connection::wire::AuthOk;

#[derive(Serialize)]
struct LegacyAuthOk<'a> {
    #[serde(with = "serde_bytes")]
    server_signature: &'a [u8],
    #[serde(with = "serde_bytes")]
    server_pub_key: &'a [u8],
    #[serde(with = "serde_bytes")]
    identity_sig: &'a [u8],
    #[serde(with = "serde_bytes")]
    session_id: &'a [u8],
    expires_at_ns: u64,
    #[serde(with = "serde_bytes")]
    resumption_ticket: &'a [u8],
    resumption_expires_at_ns: u64,
    server_query_version: u8,
}

fn auth_ok(
    rotation: Option<shamir_connect::common::wire::RotationInProgressWire>,
    kdf: Option<bool>,
) -> AuthOk {
    AuthOk {
        server_signature: vec![1; 32],
        server_pub_key: vec![2; 32],
        identity_sig: vec![3; 64],
        session_id: vec![4; 32],
        expires_at_ns: 7,
        resumption_ticket: Vec::new(),
        resumption_expires_at_ns: 0,
        server_query_version: 1,
        rotation_in_progress: rotation,
        kdf_upgrade_required: kdf,
    }
}

#[derive(Deserialize)]
struct ClientAuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    server_query_version: u8,
    #[serde(default)]
    rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
    #[serde(default)]
    kdf_upgrade_required: Option<bool>,
}

#[test]
fn no_hints_preserve_legacy_eight_field_encoding() {
    let value = auth_ok(None, None);
    let encoded = rmp_serde::to_vec(&value).expect("encode auth_ok");
    let legacy = LegacyAuthOk {
        server_signature: &value.server_signature,
        server_pub_key: &value.server_pub_key,
        identity_sig: &value.identity_sig,
        session_id: &value.session_id,
        expires_at_ns: value.expires_at_ns,
        resumption_ticket: &value.resumption_ticket,
        resumption_expires_at_ns: value.resumption_expires_at_ns,
        server_query_version: value.server_query_version,
    };
    let slots: Vec<IgnoredAny> = rmp_serde::from_slice(&encoded).expect("decode array");
    assert_eq!(slots.len(), 8);
    assert_eq!(encoded, rmp_serde::to_vec(&legacy).expect("encode legacy"));
}

#[test]
fn kdf_hint_uses_typed_client_shape_and_preserves_other_fields() {
    let value = auth_ok(None, Some(true));
    let bytes = rmp_serde::to_vec(&value).expect("encode auth_ok");
    let slots: Vec<IgnoredAny> = rmp_serde::from_slice(&bytes).expect("decode array");
    assert_eq!(slots.len(), 10);
    let client: ClientAuthOk = rmp_serde::from_slice(&bytes).expect("decode client shape");
    assert!(client.rotation_in_progress.is_none());
    assert_eq!(client.kdf_upgrade_required, Some(true));
    assert_eq!(client.server_signature, value.server_signature);
    assert_eq!(client.server_pub_key, value.server_pub_key);
    assert_eq!(client.identity_sig, value.identity_sig);
    assert_eq!(client.session_id, value.session_id);
    assert_eq!(client.expires_at_ns, value.expires_at_ns);
    assert_eq!(client.resumption_ticket, value.resumption_ticket);
    assert_eq!(
        client.resumption_expires_at_ns,
        value.resumption_expires_at_ns
    );
    assert_eq!(client.server_query_version, value.server_query_version);
}

#[test]
fn rotation_hint_uses_typed_client_shape_and_has_ten_fields() {
    let value = auth_ok(
        Some(shamir_connect::common::wire::RotationInProgressWire {
            previous_pub: vec![1; 32],
            identity_sig_previous: vec![2; 64],
            transition_until_ns: 9,
            rotation_proof: vec![3; 64],
        }),
        None,
    );
    let bytes = rmp_serde::to_vec(&value).expect("encode auth_ok");
    let slots: Vec<IgnoredAny> = rmp_serde::from_slice(&bytes).expect("decode array");
    assert_eq!(slots.len(), 10);
    let client: ClientAuthOk = rmp_serde::from_slice(&bytes).expect("decode client shape");
    let rotation = client.rotation_in_progress.expect("rotation hint");
    assert_eq!(rotation.previous_pub, vec![1; 32]);
    assert_eq!(rotation.identity_sig_previous, vec![2; 64]);
    assert_eq!(rotation.transition_until_ns, 9);
    assert_eq!(rotation.rotation_proof, vec![3; 64]);
    assert_eq!(client.kdf_upgrade_required, None);
    assert_eq!(client.server_signature, value.server_signature);
    assert_eq!(client.server_pub_key, value.server_pub_key);
    assert_eq!(client.identity_sig, value.identity_sig);
    assert_eq!(client.session_id, value.session_id);
    assert_eq!(client.expires_at_ns, value.expires_at_ns);
    assert_eq!(client.resumption_ticket, value.resumption_ticket);
    assert_eq!(
        client.resumption_expires_at_ns,
        value.resumption_expires_at_ns
    );
    assert_eq!(client.server_query_version, value.server_query_version);
}

#[test]
fn helper_marks_weaker_stored_kdf_only() {
    let mut weaker = KdfParams::DEFAULT;
    weaker.time -= 1;
    let (handshake, view, identity) = fixture(weaker, KdfParams::DEFAULT);
    let (rotation, kdf) = auth_ok_hints(&handshake, &view, &identity, KdfParams::DEFAULT);
    assert!(rotation.is_none());
    assert_eq!(kdf, Some(true));

    let (handshake, view, identity) = fixture(KdfParams::DEFAULT, KdfParams::DEFAULT);
    let (_, kdf) = auth_ok_hints(&handshake, &view, &identity, KdfParams::DEFAULT);
    assert_eq!(kdf, None);
}

#[test]
fn helper_rotation_payload_verifies_during_overlap() {
    let (handshake, mut view, identity) = fixture(KdfParams::DEFAULT, KdfParams::DEFAULT);
    let now_ns = UnixNanos::now().as_u64();
    let old_pub = identity.current_pub();
    identity.rotate(now_ns).expect("start identity rotation");
    view.server_pub_key = identity.current_pub();
    let challenge = handshake.challenge();
    let auth_message = AuthMessage::build(AuthMessageInputs {
        username: &NormalizedUsername::from_raw("hints-test").expect("normalized username"),
        client_nonce: &[12; 32],
        server_nonce: &challenge.server_nonce,
        salt: &challenge.salt,
        kdf_params: handshake.effective_kdf_params(),
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsNoExport,
        tls_exporter_or_zeros: &[0; 32],
        supported_version: ProtocolVersion::V1,
    })
    .expect("build handshake auth message");
    let identity_input = build_identity_input(
        &view.server_pub_key,
        TransportKind::Tcp,
        BindingMode::TlsNoExport,
        &[0; 32],
        &auth_message,
        &view.session_id,
        view.expires_at_ns,
    );

    let (rotation, kdf) = auth_ok_hints(&handshake, &view, &identity, KdfParams::DEFAULT);
    let payload = rotation.expect("rotation hint during overlap");
    assert_eq!(payload.previous_pub, old_pub);
    let pinned_old_identity = sha256(&old_pub);
    let recovered_pin = shamir_connect::client::rotation::verify_rotation_in_progress(
        &payload,
        &view.server_pub_key,
        &identity_input,
        &pinned_old_identity,
        now_ns,
        true,
    )
    .expect("client verifies emitted recovery payload");
    assert_eq!(recovered_pin, sha256(&view.server_pub_key));
    assert_ne!(view.server_pub_key, old_pub);
    assert_eq!(kdf, None);
}

#[test]
fn helper_omits_rotation_hint_without_overlap_or_for_pubkey_mismatch() {
    let (handshake, view, identity) = fixture(KdfParams::DEFAULT, KdfParams::DEFAULT);
    let (rotation, _) = auth_ok_hints(&handshake, &view, &identity, KdfParams::DEFAULT);
    assert!(rotation.is_none());

    let now_ns = UnixNanos::now().as_u64();
    identity.rotate(now_ns).expect("start identity rotation");
    let mut mismatched = view;
    mismatched.server_pub_key = [0xA5; 32];

    let (rotation, _) = auth_ok_hints(&handshake, &mismatched, &identity, KdfParams::DEFAULT);
    assert!(rotation.is_none());
}

fn fixture(
    user_kdf: KdfParams,
    current_kdf: KdfParams,
) -> (ServerHandshake<'static>, AuthOkView, ServerIdentityState) {
    let secrets: &'static ServerSecrets = Box::leak(Box::new(ServerSecrets {
        server_secret: [7; 32],
        lockout_secret: [8; 32],
    }));
    let record = shamir_connect::server::user_record::UserRecord {
        salt: [9; 16],
        stored_key: shamir_connect::common::crypto::StoredKey([10; 32]),
        server_key: zeroize::Zeroizing::new([11; 32]),
        kdf_params: user_kdf,
        tickets_invalid_before_ns: 0,
    };
    let handshake = ServerHandshake::new(
        ListenerPolicy::new(shamir_connect::common::types::BindingMode::TlsNoExport),
        shamir_connect::common::types::TransportKind::Tcp,
        secrets,
        AuthInitView {
            user: NormalizedUsername::from_raw("hints-test").expect("username"),
            client_nonce: [12; 32],
            binding_mode: shamir_connect::common::types::BindingMode::TlsNoExport,
            version: shamir_connect::common::types::ProtocolVersion::V1.as_u8(),
        },
        [0; 32],
        current_kdf,
        |_| Some(record),
    )
    .expect("construct handshake");
    let identity = ServerIdentityState::from_material(&[13; 32], None, None, 0);
    let view = AuthOkView {
        server_signature: [17; 32],
        server_pub_key: identity.current_pub(),
        identity_sig: [18; 64],
        session_id: [16; 32],
        expires_at_ns: 1_000,
        resumption_ticket: None,
        resumption_expires_at_ns: None,
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    (handshake, view, identity)
}
