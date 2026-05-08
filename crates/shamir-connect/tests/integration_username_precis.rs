//! Spec §15.3 NORMATIVE — PRECIS UsernameCaseMapped (RFC 8265).
//!
//! Validates that `NormalizedUsername::from_raw` follows RFC 8265 byte-
//! exactly and matches reference behaviour for cross-language consistency:
//!
//! - **Width mapping** (half-width ↔ full-width).
//! - **Case mapping** (RFC 8265 case-fold, NOT `String::to_lowercase`).
//! - **NFC normalization** after case mapping.
//! - **Banned categories**: control, surrogate, private-use, non-character,
//!   incompatible-with-IdentifierClass.
//!
//! Cross-language vectors below are taken from RFC 8265 examples + common
//! edge cases that distinguish PRECIS from `to_lowercase`.

use shamir_connect::common::username::NormalizedUsername;

/// RFC 8265 §3.3 — width mapping converts full-width to half-width.
#[test]
fn full_width_ascii_folded_to_half_width() {
    // U+FF21 FULLWIDTH LATIN CAPITAL LETTER A
    let raw = "\u{FF21}\u{FF22}\u{FF23}";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "abc", "full-width ABC must fold to ascii abc");
}

/// RFC 8265 §3.3: PRECIS UsernameCaseMapped uses `Lowercase_Mapping` (NOT
/// `Default_Case_Folding`). German ß is PRESERVED (lowercase ß stays ß).
/// This matches `String::to_lowercase` for ASCII + ß; the full distinction
/// between PRECIS and `to_lowercase` lies in width-mapping and bidi rules.
#[test]
fn german_eszett_preserved_via_lowercase_mapping() {
    let raw = "Straße";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(
        norm.as_str(),
        "straße",
        "German ß is preserved by Lowercase_Mapping (not folded to 'ss')"
    );
}

/// RFC 8265 case-fold for Greek capital sigma Σ → σ (NOT final-sigma ς).
#[test]
fn greek_capital_sigma_casefolds_to_lowercase_sigma() {
    let raw = "ΟΔΥΣΣΕΥΣ"; // ODYSSEUS in capitals
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    // Each Σ becomes σ (NOT ς) per RFC 8265 default case-fold (no
    // context-sensitive final-sigma rule).
    assert_eq!(
        norm.as_str(),
        "οδυσσευσ",
        "all Σ should casefold to σ (not final-sigma ς)"
    );
}

/// NFC composition + case-folding round-trip.
#[test]
fn nfc_composes_combining_marks() {
    // NFD form of "café": e + combining acute → after NFC → é.
    let raw = "Cafe\u{0301}";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "café");
}

/// RFC 8265 rejects ZERO WIDTH JOINER (U+200D) in Username (it's allowed
/// only in JoinControl context, which UsernameCaseMapped excludes).
#[test]
fn rejects_zero_width_joiner() {
    let raw = "alice\u{200D}bob";
    let result = NormalizedUsername::from_raw(raw);
    assert!(result.is_err(), "ZWJ MUST be rejected in usernames");
}

/// RFC 8265 rejects bidi format characters (RIGHT-TO-LEFT MARK).
#[test]
fn rejects_bidi_format_chars() {
    let raw = "alice\u{200E}";
    let result = NormalizedUsername::from_raw(raw);
    assert!(result.is_err(), "RLM MUST be rejected");
}

/// RFC 8265 rejects ASCII control chars.
#[test]
fn rejects_ascii_control_chars() {
    for c in [0x00u8, 0x01, 0x07, 0x09, 0x1f, 0x7f] {
        let s = format!("alice{}bob", c as char);
        let result = NormalizedUsername::from_raw(&s);
        assert!(result.is_err(), "control 0x{:02x} must be rejected", c);
    }
}

/// RFC 8265 rejects private-use plane (U+E000..U+F8FF basic, plus higher planes).
#[test]
fn rejects_private_use_chars() {
    let raw = "alice\u{E000}";
    assert!(NormalizedUsername::from_raw(raw).is_err());

    let raw2 = "alice\u{F8FF}";
    assert!(NormalizedUsername::from_raw(raw2).is_err());
}

/// Empty input rejected.
#[test]
fn rejects_empty() {
    assert!(NormalizedUsername::from_raw("").is_err());
}

/// Whitespace-only rejected.
#[test]
fn rejects_whitespace_only() {
    assert!(NormalizedUsername::from_raw("   ").is_err());
}

/// Length cap enforced AFTER PRECIS normalization.
#[test]
fn rejects_oversized_after_normalization() {
    let raw = "a".repeat(256);
    assert!(NormalizedUsername::from_raw(&raw).is_err());
}

/// Boundary: exactly USERNAME_MAX_BYTES (255) is accepted.
#[test]
fn accepts_at_max_length() {
    let raw = "a".repeat(255);
    assert!(NormalizedUsername::from_raw(&raw).is_ok());
}

/// Idempotence: applying from_raw twice gives the same result (i.e., the
/// canonical form is a fixed point of the normalization function).
#[test]
fn idempotent_normalization() {
    let raw = "Café\u{FF21}";
    let n1 = NormalizedUsername::from_raw(raw).unwrap();
    let n2 = NormalizedUsername::from_raw(n1.as_str()).unwrap();
    assert_eq!(n1.as_str(), n2.as_str());
}

/// Equality of two semantically-identical inputs (NFC-equivalent).
#[test]
fn nfc_equivalent_inputs_normalize_equal() {
    let a = NormalizedUsername::from_raw("café").unwrap();        // composed
    let b = NormalizedUsername::from_raw("cafe\u{0301}").unwrap(); // decomposed
    assert_eq!(a.as_str(), b.as_str());
}

/// Cross-language consistency vector: simple ASCII case-fold matches what a
/// JS reference (`s.normalize("NFC").toLowerCase()`) would produce for
/// pure ASCII (PRECIS and JS happen to agree on this trivial subset).
#[test]
fn ascii_case_fold_matches_javascript_default() {
    let raw = "Alice";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "alice");
}
