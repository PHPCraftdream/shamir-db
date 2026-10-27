//! RU8 (RV-TYPES-2EB39DFD): the typed interner quota error must map to the
//! `interner_quota_exceeded` wire code - both directly and through codec
//! call chains (`CodecError::Interner`).

use crate::core::error::DbError;
use shamir_types::codecs::CodecError;
use shamir_types::core::interner::{InternerError, InternerQuotaKind};

#[test]
fn rv_types_2eb39dfd_quota_error_maps_to_wire_code() {
    let err = DbError::from(InternerError::QuotaExceeded {
        label: "repo-17".to_string(),
        kind: InternerQuotaKind::Names,
        limit: 7,
    });
    assert!(matches!(
        &err,
        DbError::InternerQuotaExceeded {
            repo,
            kind: InternerQuotaKind::Names,
            limit: 7
        } if repo == "repo-17"
    ));
    assert_eq!(err.code(), Some("interner_quota_exceeded"));
    let message = err.to_string();
    assert!(message.contains("repo-17"));
    assert!(message.contains("Names"));
    assert!(message.contains("7"));
    assert!(message.contains("SHAMIR_INTERNER_MAX_NAMES"));
    assert!(message.contains("SHAMIR_INTERNER_MAX_NAME_BYTES"));

    let via_codec = DbError::from(CodecError::Interner(InternerError::QuotaExceeded {
        label: "repo-17".to_string(),
        kind: InternerQuotaKind::Bytes,
        limit: 9,
    }));
    assert_eq!(via_codec.code(), Some("interner_quota_exceeded"));
}
