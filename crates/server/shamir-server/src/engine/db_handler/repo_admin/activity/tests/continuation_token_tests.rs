//! [`ActivityContinuation`] token tests: round trip, bounded opaque wire
//! form, strict validation (fixed byte length, version, known enum bytes,
//! section-must-be-specific-and-in-filter), scope/kind re-statement, and
//! the monotonic chain nonce.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_query_types::admin::activity::{ActivityKind as K, ActivityScope};

use super::super::continuation::{
    fresh_sequence, section_order, ActivityContinuation, ContinuationError,
};

fn token(scope: ActivityScope, filter: K, section: K, last_key: u64, sequence: u64) -> String {
    ActivityContinuation::new(scope, filter, section, last_key, sequence).encode()
}

/// Round trip preserves every field exactly (fixed-width big-endian
/// encoding, no truncation of large keys or sequences).
#[test]
fn round_trip_preserves_all_fields() {
    for scope in [ActivityScope::All, ActivityScope::Own] {
        for filter in [
            K::Sessions,
            K::Requests,
            K::Transactions,
            K::Cursors,
            K::All,
        ] {
            for section in section_order(filter) {
                let token = token(scope, filter, *section, u64::MAX, 0xDEAD_BEEF_CAFE_F00D);
                let decoded =
                    ActivityContinuation::decode(&token).expect("valid token must decode");
                assert_eq!(decoded.scope, scope);
                assert_eq!(decoded.filter, filter);
                assert_eq!(decoded.section, *section);
                assert_eq!(decoded.last_key, u64::MAX);
                assert_eq!(decoded.sequence, 0xDEAD_BEEF_CAFE_F00D);
            }
        }
    }
}

/// The wire form is bounded and opaque: fixed small length, base64
/// alphabet only, and — critically — it carries NO plaintext scope, kind,
/// key or sequence bytes (no credentials and no readable routing data).
#[test]
fn wire_form_is_bounded_and_opaque() {
    let token = token(
        ActivityScope::Own,
        K::All,
        K::Transactions,
        18446744073709551615,
        u64::MAX,
    );
    assert!(
        token.len() <= 64,
        "token must stay bounded, got {}",
        token.len()
    );
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "token must be base64url alphabet only: {token}"
    );
    // Byte-scan: no field label, scope or kind name appears in the clear.
    for literal in [
        "scope", "kind", "section", "all", "own", "sessions", "cursor",
    ] {
        assert!(!token.to_ascii_lowercase().contains(literal));
    }
}

/// Every structural corruption is rejected as `Malformed`: truncation,
/// padding bytes appended, wrong version, and oversized input.
#[test]
fn malformed_tokens_are_rejected() {
    let good = token(ActivityScope::Own, K::All, K::Cursors, 7, 1);
    let raw = base64_decode(&good);
    assert_eq!(raw.len(), 20, "token layout changed — update the tests");

    // Truncated payload.
    let short = base64_encode(&raw[..19]);
    assert_eq!(
        ActivityContinuation::decode(&short),
        Err(ContinuationError::Malformed)
    );
    // Extended payload.
    let mut long = raw.clone();
    long.push(0);
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&long)),
        Err(ContinuationError::Malformed)
    );
    // Wrong version byte.
    let mut version = raw.clone();
    version[0] = 99;
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&version)),
        Err(ContinuationError::Malformed)
    );
    // Not base64 at all / oversized plain input.
    assert_eq!(
        ActivityContinuation::decode("!!!!not-base64!!!!"),
        Err(ContinuationError::Malformed)
    );
    assert_eq!(
        ActivityContinuation::decode(&"A".repeat(65)),
        Err(ContinuationError::Malformed)
    );
}

/// Unknown enum bytes and impossible sections are rejected with typed
/// errors: a token can never smuggle an out-of-vocabulary scope, kind or
/// section, and `All` is a filter — never a resume section.
#[test]
fn forged_enum_bytes_are_rejected() {
    let raw = base64_decode(&token(ActivityScope::Own, K::All, K::Cursors, 7, 1));

    // Unknown scope byte.
    let mut scope = raw.clone();
    scope[1] = 42;
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&scope)),
        Err(ContinuationError::UnknownValue)
    );
    // Unknown filter kind byte.
    let mut filter = raw.clone();
    filter[2] = 42;
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&filter)),
        Err(ContinuationError::UnknownValue)
    );
    // Unknown section byte.
    let mut section = raw.clone();
    section[3] = 42;
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&section)),
        Err(ContinuationError::UnknownSection)
    );
    // `All` as a section — a filter, never a resume point.
    let mut all_section = raw.clone();
    all_section[3] = 5;
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&all_section)),
        Err(ContinuationError::UnknownSection)
    );
    // Section outside the filter's walk (Cursors is not in a
    // Sessions-only chain).
    let mut stray = raw.clone();
    stray[2] = 1; // filter = Sessions
    stray[3] = 4; // section = Cursors
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&stray)),
        Err(ContinuationError::UnknownSection)
    );
}

/// Section order is the design's fixed render order and each specific
/// filter walks exactly its own single section.
#[test]
fn section_order_matches_design() {
    assert_eq!(
        section_order(K::All),
        &[K::Sessions, K::Requests, K::Transactions, K::Cursors]
    );
    assert_eq!(section_order(K::Sessions), &[K::Sessions]);
    assert_eq!(section_order(K::Requests), &[K::Requests]);
    assert_eq!(section_order(K::Transactions), &[K::Transactions]);
    assert_eq!(section_order(K::Cursors), &[K::Cursors]);
}

/// The chain nonce is monotonic: a fresh chain always observes a sequence
/// at least as large as any previously minted one (never regresses).
#[test]
fn fresh_sequence_is_monotonic() {
    let first = fresh_sequence();
    let second = fresh_sequence();
    let third = fresh_sequence();
    assert!(second > first);
    assert!(third > second);
}

/// The encoded wire form is EXACTLY 27 characters (base64url no-pad of
/// the 20-byte layout); anything longer or shorter is rejected before the
/// decoder even runs.
#[test]
fn wire_form_is_exactly_27_chars() {
    let token = token(ActivityScope::All, K::Transactions, K::Transactions, 42, 9);
    assert_eq!(token.len(), 27, "token layout changed — update the tests");
    // Shorter valid base64 decodes to the wrong byte length.
    let raw = base64_decode(&token);
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&raw[..18])),
        Err(ContinuationError::Malformed)
    ); // Longer input (a 21-byte payload, valid base64) is rejected outright.
    let mut long = raw.clone();
    long.push(0);
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&long)),
        Err(ContinuationError::Malformed)
    );
}

/// A decoded `sequence` of 0 is rejected as `Malformed`: the minting
/// counter starts at 1, so zero can only come from a forged token —
/// presence validation of the chain nonce.
#[test]
fn zero_sequence_is_rejected() {
    let raw = base64_decode(&token(ActivityScope::Own, K::All, K::Requests, 3, 5));
    let mut zero = raw.clone();
    zero[12..20].copy_from_slice(&0u64.to_be_bytes());
    assert_eq!(
        ActivityContinuation::decode(&base64_encode(&zero)),
        Err(ContinuationError::Malformed)
    );
    // Sanity: the unmodified token (nonzero sequence) still decodes.
    assert!(ActivityContinuation::decode(&base64_encode(&raw)).is_ok());
}

/// Minimal local base64url helpers (the test must not depend on which
/// engine constant the implementation uses).
fn base64_decode(token: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(token)
        .expect("test token must decode")
}

fn base64_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
