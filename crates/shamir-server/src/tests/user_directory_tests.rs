//! Internal unit tests for [`crate::user_directory`].
//!
//! These exercise logic that needs `pub(crate)` visibility into the
//! directory's internals:
//!   - the pure principal64 projection step (the fail-closed collision /
//!     zero-projection branch — real collisions are cryptographically
//!     near-impossible, so a deterministic fixture is the only practical
//!     way to cover the branch),
//!   - `#[serde(default)]` backward-compat for pre-#556 persisted blobs
//!     that lack the `superuser` field entirely,
//!   - the relocated #556 normalization-idempotence test (task #557: the
//!     `"superuser"` role string is now reserved at the `update_roles`
//!     write boundary, so the legacy pre-migration shape can no longer be
//!     seeded through the public API; this in-crate module can construct
//!     `PersistedUser` directly via `pub(crate)` to seed the legacy blob).
//!
//! Behavioural coverage of `remove()`, the `UserStateLookup` fix, and the
//! `set_superuser` / reservation behaviours lives in `tests/user_directory.rs`
//! (public API).

use serde::Serialize;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::user_directory::{project_user_ids_to_principal64, FjallUserDirectory, PersistedUser};

/// `principal64` takes the first 8 bytes big-endian and clears the high bit
/// (`& i64::MAX`). Two user ids differing ONLY in that high bit therefore
/// collide — a deterministic fixture for the otherwise-cryptographically-
/// improbable collision branch of boot-time normalization.
#[test]
fn projection_fails_closed_on_collision_naming_both_usernames() {
    let mut a = [0u8; 16];
    a[0..8].copy_from_slice(&(0x0102_0304_0506_0708u64).to_be_bytes());
    a[8] = 0xAA;
    let mut b = [0u8; 16];
    // Same projection as `a` (high bit set → cleared by the i64::MAX mask),
    // distinct user_id bytes, distinct username.
    b[0..8].copy_from_slice(&(0x8102_0304_0506_0708u64).to_be_bytes());
    b[8] = 0xBB;
    assert_eq!(
        shamir_types::access::principal64(a),
        shamir_types::access::principal64(b),
        "fixture precondition: the two ids must project identically"
    );

    let err =
        project_user_ids_to_principal64(vec![(a, "alice".to_string()), (b, "bob".to_string())])
            .expect_err("a genuine projection collision must fail closed");
    assert!(
        err.contains("collision"),
        "error must say 'collision': {err}"
    );
    assert!(
        err.contains("alice") && err.contains("bob"),
        "error must name BOTH conflicting usernames: {err}"
    );
}

/// A zero projection is reserved for `OWNER_SYSTEM` / `Actor::System` and
/// must fail `open()` closed (an operator must drop/recreate the account).
/// A user id whose first 8 bytes are all zero projects to zero.
#[test]
fn projection_fails_closed_on_zero_projection() {
    let zero_id = [0u8; 16];
    assert_eq!(
        shamir_types::access::principal64(zero_id),
        0,
        "fixture precondition: all-zero id projects to the reserved 0"
    );
    // 0x80… in the high byte also projects to 0 after the high-bit mask.
    let mut masked_zero = [0u8; 16];
    masked_zero[0] = 0x80;
    assert_eq!(shamir_types::access::principal64(masked_zero), 0);

    let err = project_user_ids_to_principal64(vec![(zero_id, "ops".to_string())])
        .expect_err("a zero projection must fail closed");
    assert!(err.contains("zero"), "error must mention zero: {err}");
    assert!(
        err.contains("ops"),
        "error must name the offending username: {err}"
    );
}

/// A clean set of distinct, non-zero projections projects without error,
/// preserving usernames and producing one entry per input.
#[test]
fn projection_succeeds_for_distinct_nonzero_ids() {
    let mut ids = Vec::new();
    for i in 1..=3u8 {
        let mut id = [0u8; 16];
        id[0] = i;
        ids.push((id, format!("user{i}")));
    }
    let out = project_user_ids_to_principal64(ids.clone()).expect("distinct ids project cleanly");
    assert_eq!(out.len(), 3);
    // Projections are distinct and non-zero.
    let mut seen = shamir_collections::new_fx_set::<u64>();
    for (p, name) in &out {
        assert_ne!(*p, 0, "no zero projection");
        assert!(seen.insert(*p), "projection {p} duplicated");
        assert!(name.starts_with("user"));
    }
}

/// Pre-#556 persisted blobs have NO `superuser` field. `#[serde(default)]`
/// must let them deserialize as `superuser == false` so the boot-time
/// normalization pass can then re-encode the legacy role string into the
/// flag. This is the on-disk backward-compat guarantee.
#[test]
fn legacy_blob_without_superuser_field_deserializes_as_false() {
    // A blob in the EXACT pre-#556 shape (no `superuser` key at all).
    #[derive(Serialize)]
    struct LegacyUser {
        #[serde(with = "serde_bytes")]
        user_id: Vec<u8>,
        #[serde(with = "serde_bytes")]
        salt: Vec<u8>,
        #[serde(with = "serde_bytes")]
        stored_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        server_key: Vec<u8>,
        kdf_params: LegacyKdf,
        roles: Vec<String>,
        tickets_invalid_before_ns: u64,
    }
    #[derive(Serialize)]
    struct LegacyKdf {
        memory_kb: u32,
        time: u32,
        parallelism: u32,
        argon2_version: u8,
    }

    let legacy = LegacyUser {
        user_id: vec![0x42; 16],
        salt: vec![0xa1; 16],
        stored_key: vec![0xc3; 32],
        server_key: (0..32).collect(),
        kdf_params: LegacyKdf {
            memory_kb: 64,
            time: 1,
            parallelism: 1,
            argon2_version: 0x13,
        },
        roles: vec!["superuser".to_string()],
        tickets_invalid_before_ns: 0,
    };
    let bytes = rmp_serde::to_vec_named(&legacy).expect("encode legacy blob");

    let decoded: PersistedUser =
        rmp_serde::from_slice(&bytes).expect("legacy blob must still deserialize");
    assert!(
        !decoded.superuser,
        "missing `superuser` field must default to false (#[serde(default)])"
    );
    // The legacy role string is still there — normalization re-encodes it
    // later; here we only assert deserialization does not lose it.
    assert!(decoded.roles.iter().any(|r| r == "superuser"));
}

/// A present `superuser: true` field round-trips through the on-disk format.
#[test]
fn superuser_flag_round_trips_through_msgpack() {
    let mut user = sample_persisted_user();
    user.superuser = true;
    let bytes = rmp_serde::to_vec_named(&user).expect("encode");
    let back: PersistedUser = rmp_serde::from_slice(&bytes).expect("decode");
    assert!(back.superuser, "superuser=true must round-trip");
}

fn sample_persisted_user() -> PersistedUser {
    let record = sample_record();
    PersistedUser::from_record([0x42; 16], &record, Vec::new(), false)
}

fn sample_record() -> UserRecord {
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

/// Sanity: `state_by_user_id` returns `None` for a user_id that was never
/// inserted (the directory-level primitive the resume adapter relies on).
/// This duplicates the public-API test but lives next to the projection
/// tests so a regression in the reverse-index read is caught here too.
#[test]
fn state_by_user_id_unknown_after_fresh_open() {
    let dir = TempDir::new().expect("tempdir");
    let store = FjallUserDirectory::open(dir.path().join("u.redb")).expect("open");
    assert!(store.state_by_user_id(&[0xFFu8; 16]).is_none());
}

/// **Red test #7 (task #557) — relocated #556 normalization-idempotence
/// test, seeded via the in-crate `pub(crate) PersistedUser` bypass.**
///
/// Task #557 reserves the literal `"superuser"` role string at the
/// `update_roles` write boundary, so the EXTERNAL integration test file
/// (`tests/user_directory.rs`, a separate compilation unit) can no longer
/// seed the legacy pre-migration on-disk shape through the public API.
/// This in-crate test module CAN see `pub(crate) PersistedUser`, so it
/// constructs the legacy blob directly:
///
///   - Persist a user whose `roles` list contains `"superuser"` AND whose
///     `superuser` flag is `false` (the exact pre-#556 on-disk shape).
///   - Reopen the directory → boot-time normalization migrates the string
///     into the flag and removes it from `roles`.
///   - Reopen again → nothing changes (idempotence).
///
/// This proves the same property the external test used to cover (boot-time
/// migration of legacy on-disk data is idempotent), just seeded through the
/// in-crate bypass now that the public seeding API is reserved.
#[test]
fn normalization_migrates_superuser_role_string_and_is_idempotent_in_crate() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    // Seed TWO users via the public insert path (so the user_id_index is
    // populated), then OVERWRITE alice's blob with a hand-rolled legacy
    // `PersistedUser` carrying the reserved role string + flag=false. The
    // overwrite uses the same `users` keyspace + msgpack encoding the
    // directory itself uses.
    let (alice_uid, bob_uid) = {
        let store = FjallUserDirectory::open(&path).expect("open");
        let alice_uid = store.insert("alice".to_string(), sample_record()).unwrap();
        let bob_uid = store.insert("bob".to_string(), sample_record()).unwrap();

        // Construct the legacy pre-migration shape directly. This is the
        // exact on-disk representation of an account created pre-#556 that
        // had the "superuser" role string: roles=["superuser"], flag=false
        // (the `superuser` field didn't exist on disk; #[serde(default)]
        // makes absence deserialise to false).
        let mut legacy = PersistedUser::from_record(alice_uid, &sample_record(), Vec::new(), false);
        legacy.roles = vec!["superuser".to_string()];
        let legacy_bytes = rmp_serde::to_vec_named(&legacy).expect("encode legacy blob");

        // Write it into the same `users` keyspace the directory owns. We
        // re-open the keyspace by name (the const is private to the crate
        // — confirm by reading user_directory.rs).
        // The directory's `users` keyspace name is `users_v1`; we reach it
        // through fjall directly via the open Database handle. Since this
        // is an in-crate test, we can reconstruct the handle by reopening.
        drop(store); // release the fjall lock before the raw write.

        // Re-open at the fjall level to get a raw keyspace handle.
        {
            let db = fjall::Database::builder(&path).open().expect("fjall open");
            let users = db
                .keyspace("users_v1", fjall::KeyspaceCreateOptions::default)
                .expect("users keyspace");
            users
                .insert("alice".as_bytes(), legacy_bytes.as_slice())
                .expect("fjall insert legacy blob");
            db.persist(fjall::PersistMode::SyncAll)
                .expect("fjall persist legacy blob");
        }
        (alice_uid, bob_uid)
    };

    // First normalization pass on reopen: the string is migrated into the
    // flag, and removed from `roles`.
    let first_alice = {
        let store = FjallUserDirectory::open(&path).expect("reopen #1");
        let alice = store
            .state_by_user_id(&alice_uid)
            .expect("alice resolves after normalization");
        assert!(
            alice.superuser,
            "the `superuser` role string must be migrated into the flag"
        );
        assert!(
            !alice.roles.iter().any(|r| r == "superuser"),
            "the `superuser` string must be removed from roles after migration; got {:?}",
            alice.roles
        );
        // Bob was never a superuser — must stay a normal account.
        let bob = store.state_by_user_id(&bob_uid).expect("bob resolves");
        assert!(!bob.superuser);
        alice
    };

    // Second normalization pass: nothing changes (idempotence).
    {
        let store = FjallUserDirectory::open(&path).expect("reopen #2");
        let alice_again = store
            .state_by_user_id(&alice_uid)
            .expect("alice still resolves after second normalization");
        assert!(alice_again.superuser, "flag must survive a second boot");
        assert!(
            !alice_again.roles.iter().any(|r| r == "superuser"),
            "roles must stay migrated after a second boot"
        );
        assert_eq!(
            first_alice.roles, alice_again.roles,
            "roles stable across boots"
        );
        assert_eq!(
            first_alice.tickets_invalid_before_ns, alice_again.tickets_invalid_before_ns,
            "tib stable across boots"
        );
        assert!(store.state_by_user_id(&bob_uid).is_some());
    }
}
