//! Tests for [`session_actor`] — the per-session [`Actor`] resolver.
//!
//! Specifically: the identity-inheritance-on-recreate regression (design doc
//! §1.1 finding 3). Under the OLD model `session_actor` read
//! `Session::principal_id()`, a `fxhash::hash64(username)` — so two different
//! accounts that happened to reuse the same username resolved to the SAME
//! `Actor::User(id)`, silently inheriting every grant/ownership the prior
//! account had. After the fix `session_actor` projects the directory-minted
//! 16-byte `session.user_id` via `principal64`, so a dropped-and-recreated
//! account gets a fresh id even when it reuses the same name.

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};

use crate::db_handler::handler::session_actor;

/// Build a non-superuser session with a given `user_id` byte array and username.
fn user_session_with(user_id: [u8; 16], username: &str) -> Session {
    Session::new(
        user_id,
        username.into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// Two sessions with the SAME username but DIFFERENT directory-minted
/// `user_id` byte arrays (simulating: drop the account, recreate one with
/// the same name — the directory mints fresh bytes either way) must resolve
/// to DIFFERENT actors.
///
/// This is the direct regression test for the identity-inheritance-on-recreate
/// bug. Under the OLD code (`session_actor` reading `session.principal_id()`,
/// i.e. a username hash) this test FAILS — both sessions would resolve to the
/// same `Actor::User(id)` because the hash only depends on the username.
#[test]
fn recreate_same_username_gets_different_actor() {
    let session_a = user_session_with([0xAA; 16], "alice");
    let session_b = user_session_with([0xBB; 16], "alice");

    assert_ne!(
        session_actor(&session_a),
        session_actor(&session_b),
        "two accounts reusing the same username but with different directory-\
         minted user_id bytes must resolve to different actors — otherwise a \
         dropped-and-recreated account silently inherits the old account's \
         grants/ownership"
    );
}
