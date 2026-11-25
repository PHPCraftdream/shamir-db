use std::fs::{self, File};
use std::io::{Read, Write};

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use shamir_query_builder::doc;
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::{encode_canonical, IdentityKind, PortableItem};
use shamir_types::access::principal64;
use shamir_types::types::value::QueryValue;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::super::progress::CheckedIo;
use super::super::projection::{resource, Projection};

#[test]
fn identity_projection_contains_names_not_credentials_or_admin_roles() {
    let root = tempfile::tempdir().unwrap();
    let directory = FjallUserDirectory::open(root.path().join("users")).unwrap();
    // Not an authentication fixture: deliberately recognizable stored bytes.
    let id = directory
        .insert(
            "alice".into(),
            UserRecord {
                salt: [0x61; 16],
                stored_key: StoredKey([0x72; 32]),
                server_key: Zeroizing::new([0x73; 32]),
                kdf_params: KdfParams {
                    memory_kb: 19_456,
                    time: 2,
                    parallelism: 1,
                    argon2_version: 0x13,
                },
                tickets_invalid_before_ns: 123,
            },
        )
        .unwrap();
    drop(directory);
    let mut projection = Projection::new(root.path()).unwrap();
    let owner = i64::try_from(principal64(id)).unwrap();
    let record: QueryValue = doc! { "owner" => owner, "mode" => 0o700_i64 }.into();
    let (owner, group, mode) = projection
        .meta(&record, resource("app", None, None, None))
        .unwrap();
    assert_eq!(owner.as_deref(), Some("alice"));
    assert_eq!(group, None);
    assert_eq!(mode, 0o700);
    let identity = projection.identities.get("alice").unwrap();
    assert_eq!(identity.kind, IdentityKind::User);
    assert!(identity.disabled_on_import);
    let bytes = encode_canonical(&PortableItem::Identity(identity.clone())).unwrap();
    for secret in [
        vec![0x61; 16],
        vec![0x72; 32],
        vec![0x73; 32],
        b"kdf_params".to_vec(),
        b"superuser".to_vec(),
        b"tickets_invalid_before_ns".to_vec(),
    ] {
        assert!(!bytes.windows(secret.len()).any(|part| part == secret));
    }
    let unresolved: QueryValue = doc! { "owner" => 999_i64 }.into();
    assert!(projection
        .meta(&unresolved, resource("other", None, None, None))
        .is_err());
}

#[test]
fn verifier_io_checks_cancellation_instead_of_reading_successfully() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    fs::write(&path, b"content").unwrap();
    let cancel = CancellationToken::new();
    let mut reader = CheckedIo::new(
        File::open(&path).unwrap(),
        &cancel,
        CheckedIo::<File>::deadline(),
    );
    cancel.cancel();
    assert!(reader.read(&mut [0; 1]).is_err());
    let mut writer = CheckedIo::new(Vec::new(), &cancel, CheckedIo::<File>::deadline());
    assert!(writer.write_all(b"partial frame").is_err());
    let (inner, _) = writer.into_parts();
    assert!(inner.is_empty());
}
