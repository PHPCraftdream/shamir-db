//! Regression coverage for RV-CONNECT-FED6910E: the client flow must not
//! depend on `server`-gated items. The shared wire views now live in
//! `common::wire`; this test exercises them from the client path.
//!
//! The actual oracle is a compile-configuration one: pre-fix this crate
//! failed to build with `cargo check --no-default-features --features
//! client` because `client::{bootstrap,changepw,handshake,rotation}`
//! imported `crate::server::*` types. A runtime assertion cannot
//! distinguish the configurations, so this file pins the client-only
//! surface (it compiles under BOTH configurations, including
//! client-only, only after the fix).

use shamir_connect::client::{
    HandshakeBuilder, RotationInProgressPayload, ServerAuthOk, ServerChallenge,
};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::common::wire::{
    ChangePwRequest, IdentityRotationEvent, RotationInProgressPayload as WireRotationPayload,
};

#[test]
fn rv_feature_fed6910e_client_surface_independent_of_server() {
    // Client handshake type aliases the shared (common) rotation payload.
    let payload: Option<RotationInProgressPayload> = None;
    let _wire: Option<WireRotationPayload> = payload;

    // TOFU client builds without any server feature involvement.
    let client = HandshakeBuilder::new(
        NormalizedUsername::from_raw("alice").unwrap(),
        TransportKind::Tcp,
        BindingMode::None,
    )
    .accept_new_host(true)
    .build()
    .unwrap();
    let init = client.auth_init();
    assert_eq!(init.user, "alice");

    // Shared wire views exist at the common path.
    let _ = core::mem::size_of::<ServerChallenge>();
    let _ = core::mem::size_of::<ServerAuthOk>();
    let _ = core::mem::size_of::<ChangePwRequest>();
    let _ = core::mem::size_of::<IdentityRotationEvent>();
}
