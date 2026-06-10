use crate::version::{
    check_handshake_proto, check_query_lang, VersionError, CURRENT_HANDSHAKE_PROTO_VERSION,
    CURRENT_QUERY_LANG_VERSION, SUPPORTED_HANDSHAKE_PROTO_VERSIONS, SUPPORTED_QUERY_LANG_VERSIONS,
};

#[test]
fn handshake_v1_accepted() {
    assert!(check_handshake_proto(1).is_ok());
}

#[test]
fn handshake_v0_or_future_rejected() {
    assert!(matches!(
        check_handshake_proto(0),
        Err(VersionError::UnsupportedHandshake { requested: 0, .. })
    ));
    assert!(matches!(
        check_handshake_proto(99),
        Err(VersionError::UnsupportedHandshake { requested: 99, .. })
    ));
}

#[test]
fn query_lang_v1_accepted() {
    assert!(check_query_lang(1).is_ok());
}

#[test]
fn query_lang_unknown_rejected() {
    assert!(matches!(
        check_query_lang(0),
        Err(VersionError::UnsupportedQueryLang { requested: 0, .. })
    ));
    assert!(matches!(
        check_query_lang(99),
        Err(VersionError::UnsupportedQueryLang { requested: 99, .. })
    ));
}

#[test]
fn current_versions_are_in_supported_lists() {
    assert!(SUPPORTED_HANDSHAKE_PROTO_VERSIONS.contains(&CURRENT_HANDSHAKE_PROTO_VERSION));
    assert!(SUPPORTED_QUERY_LANG_VERSIONS.contains(&CURRENT_QUERY_LANG_VERSION));
}
