//! Tests for username PRECIS UsernameCaseMapped + NFC (spec §15.3).

use crate::common::username::NormalizedUsername;

#[test]
fn ascii_lowercase_passes() {
    let n = NormalizedUsername::from_raw("alice").unwrap();
    assert_eq!(n.as_str(), "alice");
}

#[test]
fn ascii_mixed_case_is_lowercased() {
    let n = NormalizedUsername::from_raw("Alice").unwrap();
    assert_eq!(n.as_str(), "alice");
}

#[test]
fn empty_rejected() {
    assert!(NormalizedUsername::from_raw("").is_err());
}

#[test]
fn whitespace_only_rejected() {
    assert!(NormalizedUsername::from_raw("   ").is_err());
    assert!(NormalizedUsername::from_raw("\t\n").is_err());
}

#[test]
fn control_characters_rejected() {
    assert!(NormalizedUsername::from_raw("alice\0bob").is_err());
    assert!(NormalizedUsername::from_raw("\x07").is_err()); // BEL
    assert!(NormalizedUsername::from_raw("a\x1bb").is_err()); // ESC
}

#[test]
fn bidi_format_chars_rejected() {
    // U+202E RIGHT-TO-LEFT OVERRIDE — common homograph/spoofing tool
    assert!(NormalizedUsername::from_raw("a\u{202E}b").is_err());
    // U+200E LEFT-TO-RIGHT MARK
    assert!(NormalizedUsername::from_raw("a\u{200E}b").is_err());
    // U+FEFF ZERO WIDTH NO-BREAK SPACE (BOM)
    assert!(NormalizedUsername::from_raw("a\u{FEFF}b").is_err());
}

#[test]
fn private_use_chars_rejected() {
    // U+E000 PUA
    assert!(NormalizedUsername::from_raw("a\u{E000}").is_err());
}

#[test]
fn over_max_length_rejected() {
    let too_long = "a".repeat(256);
    assert!(NormalizedUsername::from_raw(&too_long).is_err());
}

#[test]
fn at_max_length_accepted() {
    let exactly_max = "a".repeat(255);
    assert!(NormalizedUsername::from_raw(&exactly_max).is_ok());
}

#[test]
fn nfc_combining_marks_normalized() {
    // "café" can be encoded as either:
    //   precomposed: U+00E9 (é)
    //   decomposed:  U+0065 + U+0301 (e + combining acute)
    // After NFC both should produce the same byte sequence.
    let precomposed = "caf\u{00E9}";
    let decomposed = "cafe\u{0301}";

    let n1 = NormalizedUsername::from_raw(precomposed).unwrap();
    let n2 = NormalizedUsername::from_raw(decomposed).unwrap();

    assert_eq!(
        n1.as_bytes(),
        n2.as_bytes(),
        "NFC normalization must collapse precomposed and decomposed forms"
    );
}

#[test]
fn cyrillic_lookalike_distinguishable() {
    // Cyrillic 'а' (U+0430) ≠ Latin 'a' (U+0061) even after case-folding —
    // homograph attack is *not* automatically blocked, but produces
    // different byte sequences so each is its own user.
    let latin_a = NormalizedUsername::from_raw("alice").unwrap();
    let cyrillic_a = NormalizedUsername::from_raw("\u{0430}lice").unwrap();
    assert_ne!(latin_a.as_bytes(), cyrillic_a.as_bytes());
}

#[test]
fn lossless_byte_access_after_normalize() {
    let n = NormalizedUsername::from_raw("alice").unwrap();
    assert_eq!(n.as_bytes(), b"alice");
}
