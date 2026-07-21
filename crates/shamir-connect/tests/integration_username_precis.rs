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
    assert_eq!(
        norm.as_str(),
        "abc",
        "full-width ABC must fold to ascii abc"
    );
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
    let a = NormalizedUsername::from_raw("café").unwrap(); // composed
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

// ---------------------------------------------------------------------------
// Canonical NFC composition/decomposition edge cases (Unicode
// NormalizationTest.txt categories). These pin the REAL output of the
// `unicode-normalization` + PRECIS pipeline for representative composition,
// canonical-ordering, and rejection cases — so a JS SDK using
// `String.normalize("NFC")` can be checked against the same expected bytes.
// ---------------------------------------------------------------------------

/// Hangul precomposed syllable U+AC00 (가) is already in NFC and stays stable
/// (NormalizationTest.txt: Hangul LVT syllables are NFC-stable).
#[test]
fn nfc_precomposed_hangul_is_stable() {
    let raw = "\u{AC00}"; // 가
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "\u{AC00}");
}

/// Conjoining Hangul jamo (L, V, T filler code points like U+1100) are NOT in
/// the PRECIS IdentifierClass and are rejected — a JS `normalize("NFC")` would
/// compose them to a syllable, but PRECIS forbids the raw jamo, so this is a
/// genuine cross-pipeline divergence point that must be pinned.
#[test]
fn nfc_hangul_conjoining_jamo_rejected() {
    // U+1100 (CHOSEONG KIYEOK) + U+1161 (JUNGSEONG A) + U+11A8 (JONGSEONG)
    let raw = "\u{1100}\u{1161}\u{11A8}";
    assert!(
        NormalizedUsername::from_raw(raw).is_err(),
        "conjoining Hangul jamo are rejected by PRECIS IdentifierClass"
    );
}

/// Canonical singleton: U+212B ANGSTROM SIGN canonically decomposes to
/// U+00C5 (Å). PRECIS rejects it (not in IdentifierClass) — pinning this
/// rejection prevents a regression where it might silently become accepted.
#[test]
fn nfc_angstrom_sign_singleton_rejected() {
    assert!(
        NormalizedUsername::from_raw("x\u{212B}").is_err(),
        "U+212B ANGSTROM SIGN is rejected by PRECIS (not IdentifierClass)"
    );
}

/// Partial composition with a surviving non-starter: `n` + U+0303 (tilde) +
/// U+0301 (acute). NFC composes n+̃ → ñ (U+00F1); the acute (U+0301) has no
/// composition with ñ and survives. Pin the exact byte output.
#[test]
fn nfc_partial_composition_with_surviving_mark() {
    let raw = "n\u{0303}\u{0301}";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "\u{00F1}\u{0301}"); // ñ + combining acute
}

/// Canonical ordering + composition: `a` + U+0300 (grave, CCC 230) + U+0316
/// (grave-below, CCC 220). Canonical reordering places CCC 220 before 230;
/// composition yields à (U+00E0) followed by the surviving U+0316. This pins
/// the interaction of reordering and composition.
#[test]
fn nfc_canonical_ordering_then_composition() {
    let raw = "a\u{0300}\u{0316}";
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "\u{00E0}\u{0316}"); // à + combining grave below
}

/// Case-fold THEN NFC compose: Cyrillic capital I (U+0418) + combining breve
/// (U+0306). Case-fold lowers to U+0438, then NFC composes with the breve to
/// U+0439 (й). Pins the casefold-before-NFC ordering.
#[test]
fn nfc_casefold_then_compose_cyrillic() {
    let raw = "\u{0418}\u{0306}"; // Capital И + combining breve
    let norm = NormalizedUsername::from_raw(raw).unwrap();
    assert_eq!(norm.as_str(), "\u{0439}"); // lowercase й
}

/// Compatibility characters are rejected by PRECIS (it uses canonical NFC,
/// not NFKC, for the normalization step; compatibility chars like U+FB01 ﬁ and
/// U+00B2 ² are not in IdentifierClass). Pin both rejections.
#[test]
fn nfc_compatibility_chars_rejected() {
    assert!(
        NormalizedUsername::from_raw("x\u{FB01}").is_err(),
        "U+FB01 LATIN SMALL LIGATURE FI is rejected"
    );
    assert!(
        NormalizedUsername::from_raw("x\u{00B2}").is_err(),
        "U+00B2 SUPERSCRIPT TWO is rejected"
    );
}
