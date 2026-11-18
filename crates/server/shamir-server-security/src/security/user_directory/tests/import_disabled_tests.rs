//! Tests for the release-1 offline logical-import identity change:
//! disabled imported identities (CSPRNG credential fill) and the
//! backward-compatible `disabled` marker on `PersistedUser`.

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::error::Error;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use shamir_types::access::principal64;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::user_directory::record::USERS_KEYSPACE;
use crate::user_directory::{FjallUserDirectory, PersistedUser};

fn user_record() -> UserRecord {
    UserRecord {
        salt: [7; 16],
        stored_key: StoredKey([8; 32]),
        server_key: Zeroizing::new([9; 32]),
        kdf_params: shamir_connect::common::kdf_params::KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

/// Test 1: a disabled imported user cannot start or complete
/// authentication — the directory's auth-snapshot surface
/// (`UserDirectory::lookup_by_name`, consumed by login BEGIN, login FINISH
/// proof verification, and the replication handshake) must fail it with
/// EXACTLY the unknown-user result: equal `None`, which collapses into the
/// fake-verifier challenge / unknown-user verification failure.
#[test]
fn disabled_import_fails_auth_exactly_like_unknown_user() {
    let temp = TempDir::new().expect("tempdir");
    let directory = FjallUserDirectory::open(temp.path()).expect("open directory");
    directory
        .insert_disabled_import("imported".to_string(), None)
        .expect("import user");
    directory
        .insert("known".to_string(), user_record())
        .expect("insert enabled user");

    // Same failure as an unknown user — assert EQUALITY of the error, not
    // just failure.
    assert_eq!(
        directory.lookup_by_name("imported").map(|r| r.salt),
        directory.lookup_by_name("never-existed").map(|r| r.salt)
    );
    // An enabled user still resolves through the same surface.
    assert!(directory.lookup_by_name("known").is_some());
    // Re-importing the same name is still refused (name collision).
    assert!(matches!(
        directory.insert_disabled_import("imported".to_string(), None),
        Err(Error::InvalidInput("username exists"))
    ));
}

/// Test 2: a persisted record WITHOUT the `disabled` marker decodes as
/// enabled and re-encodes to byte-identical bytes (the marker is omitted
/// when `false`, so pre-existing records and reference-catalogue bytes are
/// untouched).
#[test]
fn missing_disabled_marker_decodes_enabled_round_trip_identical() {
    let user_id = [0x11u8; 16];
    let mut user = PersistedUser::from_record(
        user_id,
        &user_record(),
        Vec::new(),
        false,
        Some("db".to_string()),
    );
    assert!(!user.disabled);

    let encoded = rmp_serde::to_vec_named(&user).expect("encode");
    let decoded: PersistedUser = rmp_serde::from_slice(&encoded).expect("decode");
    assert!(!decoded.disabled);
    let re_encoded = rmp_serde::to_vec_named(&decoded).expect("re-encode");
    assert_eq!(encoded, re_encoded);

    // Flipping the marker on changes the bytes; flipping it back restores
    // them exactly (omission when false).
    user.disabled = true;
    let disabled_encoded = rmp_serde::to_vec_named(&user).expect("encode disabled");
    assert_ne!(encoded, disabled_encoded);
    user.disabled = false;
    let restored = rmp_serde::to_vec_named(&user).expect("encode restored");
    assert_eq!(encoded, restored);
}

/// Test 3: the CSPRNG credential fill differs between two imports of the
/// same username (nothing constant or guessable is stored).
#[test]
fn imported_credential_material_differs_between_imports() {
    fn import_and_read_material() -> (TempDir, [u8; 16], [u8; 32], [u8; 32]) {
        let temp = TempDir::new().expect("tempdir");
        {
            // Scope the handle: the directory is exclusive, so it must be
            // closed before the raw reopen below.
            let directory = FjallUserDirectory::open(temp.path()).expect("open directory");
            directory
                .insert_disabled_import("imported".to_string(), None)
                .expect("import user");
        }
        let blob = read_imported_blob(temp.path(), "imported");
        (
            temp,
            blob.0,
            blob.1.try_into().expect("stored_key 32 bytes"),
            blob.2.try_into().expect("server_key 32 bytes"),
        )
    }

    let (_temp_a, salt_a, stored_a, server_a) = import_and_read_material();
    let (_temp_b, salt_b, stored_b, server_b) = import_and_read_material();

    assert_ne!(salt_a, salt_b);
    assert_ne!(stored_a, stored_b);
    assert_ne!(server_a, server_b);
    // Never zeros or placeholders.
    assert!(salt_a.iter().any(|&b| b != 0));
    assert!(stored_a.iter().any(|&b| b != 0));
    assert!(server_a.iter().any(|&b| b != 0));
}

/// Test 4: ACL/owner-style lookup still finds a disabled imported user —
/// the ownership/principal-resolution surfaces (`user_id`,
/// `state_by_user_id`, `resolve_by_principal64`) stay OPEN for the import's
/// ownership mapping.
#[test]
fn ownership_lookup_resolves_disabled_imported_user() {
    let temp = TempDir::new().expect("tempdir");
    let directory = FjallUserDirectory::open(temp.path()).expect("open directory");
    let user_id = directory
        .insert_disabled_import("imported".to_string(), Some("db".to_string()))
        .expect("import user");

    assert_eq!(directory.user_id("imported"), Some(user_id));
    let state = directory
        .state_by_user_id(&user_id)
        .expect("state_by_user_id resolves disabled import");
    assert_eq!(state.username, "imported");
    assert_eq!(state.database.as_deref(), Some("db"));
    let resolved = directory
        .resolve_by_principal64(principal64(user_id))
        .expect("resolve_by_principal64 resolves disabled import");
    assert_eq!(resolved.username, "imported");
}

/// Read the persisted blob of an imported user directly from the
/// `users_v1` keyspace (the auth surface refuses disabled users by design,
/// so the raw record is read at the storage layer after closing the
/// directory).
fn read_imported_blob(path: &std::path::Path, username: &str) -> ([u8; 16], Vec<u8>, Vec<u8>) {
    drop(FjallUserDirectory::open(path).expect("reopen directory"));
    let db = fjall::Database::builder(path).open().expect("open raw db");
    let users = db
        .keyspace(USERS_KEYSPACE, fjall::KeyspaceCreateOptions::default)
        .expect("users keyspace");
    let entry = users
        .get(username.as_bytes())
        .expect("get imported blob")
        .expect("imported blob present");
    let user: PersistedUser = rmp_serde::from_slice(entry.as_ref()).expect("decode import");
    assert!(user.disabled);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&user.salt);
    (salt, user.stored_key.clone(), user.server_key.clone())
}
