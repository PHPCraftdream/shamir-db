use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::user_record::UserRecord;
use shamir_server::user_directory::FjallUserDirectory;
use tempfile::TempDir;
use zeroize::Zeroizing;

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

pub(crate) fn fixture_record() -> UserRecord {
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

pub(crate) fn fresh_dir() -> (TempDir, FjallUserDirectory) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let store = FjallUserDirectory::open(&path).expect("open redb user dir");
    (dir, store)
}

pub(crate) fn assert_records_equal(actual: &UserRecord, expected: &UserRecord) {
    assert_eq!(actual.salt, expected.salt, "salt mismatch");
    assert_eq!(
        actual.stored_key.0, expected.stored_key.0,
        "stored_key mismatch"
    );
    assert_eq!(
        actual.server_key.as_slice(),
        expected.server_key.as_slice(),
        "server_key mismatch"
    );
    assert_eq!(
        actual.kdf_params, expected.kdf_params,
        "kdf_params mismatch"
    );
    assert_eq!(
        actual.tickets_invalid_before_ns, expected.tickets_invalid_before_ns,
        "tickets_invalid_before_ns mismatch"
    );
}
/// Helper: open the directory at `path` (mirrors the test-local reopen idiom).
pub(crate) fn reopen(path: &std::path::Path) -> FjallUserDirectory {
    FjallUserDirectory::open(path).expect("reopen user dir")
}
