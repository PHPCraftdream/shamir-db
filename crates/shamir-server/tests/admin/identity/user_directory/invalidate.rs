use super::common::{fixture_record, fresh_dir, reopen};
use shamir_connect::server::admin::UserDirectory;
use tempfile::TempDir;

// ----------------------------------------------------------------------------
// RI-11: invalidate_all_tickets
// ----------------------------------------------------------------------------

/// `invalidate_all_tickets` is a no-op on an empty directory: returns
/// `Ok(0)`, not an error.
#[test]
fn invalidate_all_tickets_noop_on_empty_directory() {
    let (_tmp, store) = fresh_dir();
    let bumped = store
        .invalidate_all_tickets(1_000_000)
        .expect("empty directory is not an error");
    assert_eq!(bumped, 0);
}

/// Every user whose current `tickets_invalid_before_ns` is older than
/// `now_ns` gets bumped to `now_ns`; users already at/after `now_ns` are
/// left untouched. The return value is the count of users ACTUALLY bumped.
#[test]
fn invalidate_all_tickets_bumps_older_leaves_newer_returns_count() {
    let (_tmp, store) = fresh_dir();

    let uid_a = store.insert("alice".to_string(), fixture_record()).unwrap();
    let uid_b = store.insert("bob".to_string(), fixture_record()).unwrap();
    let uid_c = store.insert("carol".to_string(), fixture_record()).unwrap();

    // alice: never bumped (starts at 0).
    // bob: bumped to a value BELOW the upcoming invalidate_all_tickets call.
    store.bump_tickets_invalid("bob", 500).unwrap();
    // carol: bumped to a value AT/ABOVE the upcoming call — must be left alone.
    store.bump_tickets_invalid("carol", 2_000_000).unwrap();

    let now_ns = 1_000_000u64;
    let bumped = store
        .invalidate_all_tickets(now_ns)
        .expect("invalidate_all_tickets succeeds");

    // alice (0 -> now_ns) and bob (500 -> now_ns) are bumped; carol
    // (2_000_000, already newer than now_ns) is left untouched.
    assert_eq!(bumped, 2, "exactly alice and bob should be bumped");

    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid_a), now_ns);
    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid_b), now_ns);
    assert_eq!(
        store.tickets_invalid_before_ns_by_user_id(&uid_c),
        2_000_000,
        "carol's newer value must be preserved, not overwritten with an older now_ns"
    );

    // Second call with the SAME now_ns is a full no-op: everyone is already
    // at or past now_ns.
    let bumped_again = store
        .invalidate_all_tickets(now_ns)
        .expect("second call succeeds");
    assert_eq!(
        bumped_again, 0,
        "re-invalidating at the same instant bumps nobody"
    );
}

/// The bump must survive a restart (durable, not just cached) — reopen the
/// directory and confirm the persisted value, not just the in-memory cache.
#[test]
fn invalidate_all_tickets_persists_across_restart() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let uid = {
        let store = reopen(&path);
        store.insert("alice".to_string(), fixture_record()).unwrap()
    };
    {
        let store = reopen(&path);
        let bumped = store.invalidate_all_tickets(777_777).unwrap();
        assert_eq!(bumped, 1);
    }
    {
        let store = reopen(&path);
        assert_eq!(
            store.tickets_invalid_before_ns_by_user_id(&uid),
            777_777,
            "bump must be durable across reopen, not just cached"
        );
    }
}
