use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use tempfile::TempDir;

use super::super::FjallUserDirectory;

fn user_record() -> UserRecord {
    UserRecord {
        salt: [7; 16],
        stored_key: shamir_connect::common::crypto::StoredKey([8; 32]),
        server_key: zeroize::Zeroizing::new([9; 32]),
        kdf_params: shamir_connect::common::kdf_params::KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

#[test]
fn floor_if_present_tracks_ticket_invalidation_and_unknown_users() {
    let dir = TempDir::new().expect("tempdir");
    let directory = FjallUserDirectory::open(dir.path()).expect("open directory");
    let user_id = directory
        .insert("token-floor-user".to_string(), user_record())
        .expect("insert user");

    assert_eq!(directory.floor_if_present(&[0; 16]), None);
    assert_eq!(directory.floor_if_present(&user_id), Some(0));
    assert!(directory
        .bump_tickets_invalid("token-floor-user", 41)
        .expect("bump ticket floor"));
    assert_eq!(directory.floor_if_present(&user_id), Some(41));
    assert_eq!(
        directory
            .invalidate_all_tickets(72)
            .expect("invalidate all"),
        1
    );
    assert_eq!(directory.floor_if_present(&user_id), Some(72));
    assert!(directory.remove("token-floor-user").expect("remove user"));
    assert_eq!(directory.floor_if_present(&user_id), None);
}
