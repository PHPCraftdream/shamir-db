use shamir_connect::server::token::{encode_token, generate_token_parts};

use crate::{ApiToken, ClientError};

fn raw_token() -> (String, String) {
    let (id, secret) = generate_token_parts().expect("token parts");
    let token = encode_token(&id, &secret);
    (token.to_string(), id.to_b64())
}

#[test]
fn valid_token_round_trips() {
    let (raw, id) = raw_token();
    let token = ApiToken::new(raw.clone()).expect("valid token");
    assert_eq!(token.reveal(), raw);
    assert_eq!(token.id(), id);
    let from_str = ApiToken::try_from(raw.as_str()).expect("try_from");
    assert_eq!(from_str.reveal(), raw);
}

#[test]
fn malformed_tokens_are_rejected_without_echo() {
    let (raw, _) = raw_token();
    let wrong_prefix = raw.replacen("shk1", "shk2", 1);
    let wrong_length = raw[..raw.len() - 1].to_string();
    let mut non_canonical = raw.clone();
    non_canonical.pop();
    non_canonical.push('_');
    for bad in [wrong_prefix, wrong_length, non_canonical] {
        let err = ApiToken::new(bad.clone()).expect_err("must be rejected");
        assert!(matches!(err, ClientError::InvalidToken));
        let text = format!("{err} {err:?}");
        assert!(!text.contains(&bad));
        assert!(!text.contains(&bad[5..]));
    }
}

#[test]
fn debug_is_redacted() {
    let (raw, id) = raw_token();
    let token = ApiToken::new(raw.clone()).expect("valid token");
    let text = format!("{token:?}");
    assert!(text.contains("<redacted>"));
    assert!(text.contains(&id));
    assert!(!text.contains(&raw[28..]));
    assert!(!text.contains(&raw));
}

#[test]
fn authorization_header_is_sensitive_bearer() {
    let (raw, _) = raw_token();
    let token = ApiToken::new(raw.clone()).expect("valid token");
    let header = token.authorization_header().expect("header");
    assert!(header.is_sensitive());
    assert_eq!(header.as_bytes(), format!("Bearer {raw}").as_bytes());
    assert_eq!(format!("{header:?}"), "Sensitive");
}
