//! Internal unit test for the [`RedbUserStateLookup`] adapter.
//!
//! The adapter is `pub(crate)` (production use is internal to the
//! connection handshake), so its fail-closed behaviour (unknown user →
//! `None` vs known-but-zero → `Some(0)`) is exercised here rather than
//! from the external `tests/` directory.

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::resume::UserStateLookup;
use shamir_connect::server::user_record::UserRecord;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::connection::user_state_lookup::RedbUserStateLookup;
use crate::user_directory::FjallUserDirectory;

fn fixture_record() -> UserRecord {
    let salt = [0xa1u8; 16];
    let stored = StoredKey([0xc3u8; 32]);
    let mut server_key = Zeroizing::new([0u8; 32]);
    for (i, b) in server_key.iter_mut().enumerate() {
        *b = i as u8;
    }
    UserRecord {
        salt,
        stored_key: stored,
        server_key,
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

/// **The fail-open fix.** Before task #556, `RedbUserStateLookup::lookup`
/// ALWAYS returned `Some(tib)` — even for an unknown user_id — so resume
/// never rejected a ticket carrying a removed/never-existed user_id. The
/// rewrite routes through `state_by_user_id`, which returns `None` for an
/// unknown user_id and `Some(tib)` for a known one (even when tib == 0).
#[test]
fn redb_user_state_lookup_returns_none_for_unknown_some_for_known() {
    let dir = TempDir::new().expect("tempdir");
    let store = FjallUserDirectory::open(dir.path().join("u.redb")).expect("open");
    let uid = store
        .insert("alice".to_string(), fixture_record())
        .expect("insert");

    let lookup = RedbUserStateLookup(&store);

    // Known user with tib == 0 (default) → Some(0), NOT None.
    assert_eq!(
        lookup.lookup(&uid),
        Some(0),
        "a known user with tib=0 must yield Some(0), not None"
    );

    // Unknown user_id → None (resume must reject).
    assert_eq!(
        lookup.lookup(&[0xFFu8; 16]),
        None,
        "an unknown user_id must yield None — the old impl returned Some(0) \
         (fail-open)"
    );

    // Bumping tib is reflected through the adapter.
    store.bump_tickets_invalid("alice", 42_000).expect("bump");
    assert_eq!(
        lookup.lookup(&uid),
        Some(42_000),
        "adapter must reflect a bumped tib"
    );
}
