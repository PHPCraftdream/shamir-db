use crate::hmac::{compute_tag_hex, derive_session_hmac_key, verify_tag_hex};
use crate::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use crate::wire::DbRequest;

#[test]
fn token_admin_canonical_encoding_binds_optional_values_and_ignores_hmac() {
    let make = |label: &str, ttl_secs, mode, channels, databases, for_user, hmac| {
        DbRequest::CreateApiToken {
            label: label.into(),
            ttl_secs,
            mode,
            channels,
            databases,
            for_user,
            hmac,
        }
    };
    let created = make(
        "svc",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["app".into()]),
        Some("svc".into()),
        None,
    );
    for changed in [
        make(
            "other",
            Some(60),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Both),
            Some(vec!["app".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(61),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Both),
            Some(vec!["app".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(60),
            Some(ApiTokenMode::ReadWrite),
            Some(ApiTokenChannels::Both),
            Some(vec!["app".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(60),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Rest),
            Some(vec!["app".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(60),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Both),
            Some(vec!["app".into(), "other".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(60),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Both),
            Some(vec!["other".into(), "app".into()]),
            Some("svc".into()),
            None,
        ),
        make(
            "svc",
            Some(60),
            Some(ApiTokenMode::ReadOnly),
            Some(ApiTokenChannels::Both),
            Some(vec!["app".into()]),
            Some("other".into()),
            None,
        ),
    ] {
        assert_ne!(
            crate::hmac::canonical_db_request(&created).unwrap(),
            crate::hmac::canonical_db_request(&changed).unwrap()
        );
    }
    let same = make(
        "svc",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["app".into()]),
        Some("svc".into()),
        Some("ignored".into()),
    );
    assert_eq!(
        crate::hmac::canonical_db_request(&created).unwrap(),
        crate::hmac::canonical_db_request(&same).unwrap()
    );
    let prefix = b"shamir-db hmac canonical v2\0";
    let populated = make(
        "svc",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["app".into()]),
        Some("svc".into()),
        None,
    );
    let mut populated_expected = prefix.to_vec();
    populated_expected.extend_from_slice(&[0, 0x20]);
    populated_expected.extend_from_slice(&[0, 0, 0, 3, b's', b'v', b'c']);
    populated_expected.extend_from_slice(&[1]);
    populated_expected.extend_from_slice(&60u64.to_be_bytes());
    populated_expected.extend_from_slice(&[1, 0]);
    populated_expected.extend_from_slice(&[1, 2]);
    populated_expected.extend_from_slice(&[1]);
    populated_expected.extend_from_slice(&[0, 0, 0, 1]);
    populated_expected.extend_from_slice(&[0, 0, 0, 3, b'a', b'p', b'p']);
    populated_expected.extend_from_slice(&[1, 0, 0, 0, 3, b's', b'v', b'c']);
    assert_eq!(
        crate::hmac::canonical_db_request(&populated).unwrap(),
        populated_expected
    );
    let reordered = make(
        "svc",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["other".into(), "app".into()]),
        Some("svc".into()),
        None,
    );
    let ordered = make(
        "svc",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["app".into(), "other".into()]),
        Some("svc".into()),
        None,
    );
    assert_ne!(
        crate::hmac::canonical_db_request(&ordered).unwrap(),
        crate::hmac::canonical_db_request(&reordered).unwrap()
    );
    let cases = [
        (
            make("x", None, None, None, None, None, None),
            [
                &prefix[..],
                &[0, 0x20],
                &[0, 0, 0, 1, b'x'],
                &[0, 0, 0, 0, 0],
            ]
            .concat(),
        ),
        (
            DbRequest::ListApiTokens {
                for_user: None,
                hmac: None,
            },
            [&prefix[..], &[0, 0x21, 0]].concat(),
        ),
        (
            DbRequest::RevokeApiToken {
                id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                hmac: None,
            },
            [&prefix[..], &[0, 0x22], &[0, 0, 0, 22], &[b'A'; 22]].concat(),
        ),
        (
            DbRequest::RevokeUserTokens {
                user: None,
                hmac: None,
            },
            [&prefix[..], &[0, 0x23, 0]].concat(),
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(
            crate::hmac::canonical_db_request(&request).unwrap(),
            expected
        );
    }
    let key = derive_session_hmac_key(&[0x51; 32]);
    let canonical = crate::hmac::canonical_db_request(&created).unwrap();
    let tag = compute_tag_hex(&key, &canonical);
    assert!(verify_tag_hex(&key, &canonical, &tag));
    let wrong_key = derive_session_hmac_key(&[0x52; 32]);
    assert!(!verify_tag_hex(&wrong_key, &canonical, &tag));
    let mut modified_tag = tag.clone().into_bytes();
    modified_tag[0] = if modified_tag[0] == b'0' { b'1' } else { b'0' };
    let modified_tag = String::from_utf8(modified_tag).unwrap();
    assert!(!verify_tag_hex(&key, &canonical, &modified_tag));
    assert!(!verify_tag_hex(&key, &canonical, &tag[..tag.len() - 1]));
    assert!(!verify_tag_hex(&key, &canonical, ""));
    let changed_message = crate::hmac::canonical_db_request(&make(
        "other",
        Some(60),
        Some(ApiTokenMode::ReadOnly),
        Some(ApiTokenChannels::Both),
        Some(vec!["app".into()]),
        Some("svc".into()),
        None,
    ))
    .unwrap();
    assert!(!verify_tag_hex(&key, &changed_message, &tag));
    assert!(!verify_tag_hex(
        &key,
        &canonical[..canonical.len() - 1],
        &tag
    ));
    assert!(!verify_tag_hex(&key, &canonical, "bad"));
}
