use crate::common::error::Error;
use crate::common::password::validate_password;
use crate::common::types::limits::PASSWORD_MAX_CHARS;

#[test]
fn accepts_typical_strong_password() {
    assert!(validate_password(b"correct horse battery staple").is_ok());
}

#[test]
fn rejects_short_password_per_spec_3_2() {
    // 11 chars — one below the 12-char min.
    assert!(matches!(
        validate_password(b"shortpass11"),
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn accepts_exactly_min_length() {
    // 12 chars exactly.
    assert!(validate_password(b"twelvechars1").is_ok());
}

#[test]
fn rejects_too_long_password() {
    let long = vec![b'a'; PASSWORD_MAX_CHARS + 1];
    assert!(matches!(
        validate_password(&long),
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn rejects_empty_password() {
    assert!(matches!(
        validate_password(b""),
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn rejects_whitespace_only_password() {
    assert!(matches!(
        validate_password(b"            "), // 12 spaces
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn rejects_single_repeated_char_password() {
    assert!(matches!(
        validate_password(b"aaaaaaaaaaaa"), // 12 'a's
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn rejects_invalid_utf8() {
    assert!(matches!(
        validate_password(&[0xff, 0xfe, 0xfd]),
        Err(Error::InvalidPassword(_))
    ));
}

#[test]
fn counts_code_points_not_bytes_for_min_length() {
    // 4 multi-byte chars (each emoji is 4 bytes) — 4 chars < 12 → reject.
    let four_emoji = "🎯🎯🎯🎯";
    assert!(matches!(
        validate_password(four_emoji.as_bytes()),
        Err(Error::InvalidPassword(_))
    ));

    // 12 distinct emoji = 12 chars → accept (and not single-repeated).
    let twelve_distinct = "🎯🦀🌊🎲🍀🚀🎨🎵🎭🎬🎮🎲";
    assert!(validate_password(twelve_distinct.as_bytes()).is_ok());
}
