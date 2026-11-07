use crate::tests::wasm::admission::support::{admit, limits_with, module};
use crate::ModuleHash;
use xxhash_rust::xxh3::xxh3_128;

fn expected_hex(bytes: &[u8]) -> String {
    let h = xxh3_128(bytes);
    format!("{:016x}{:016x}", (h >> 64) as u64, h as u64)
}

const SAMPLE_WORDS: [u64; 2] = [0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];

#[test]
fn hash_layout_is_high_word_then_low_word_of_xxh3_128() {
    let bytes = b"layout sample bytes";
    let h = xxh3_128(bytes);
    let hash = ModuleHash::of(bytes);
    assert_eq!(hash.to_hex(), expected_hex(bytes));
    assert_eq!(hash.words(), [(h >> 64) as u64, h as u64]);
}

#[test]
fn words_round_trip_with_high_word_first() {
    let hash = ModuleHash::from_words(SAMPLE_WORDS);
    assert_eq!(hash.words(), SAMPLE_WORDS);
    assert_eq!(hash.to_hex(), "0123456789abcdeffedcba9876543210");
}

#[test]
fn to_hex_is_32_lowercase_hex_characters() {
    let hex = ModuleHash::of(b"hex shape").to_hex();
    assert_eq!(hex.len(), 32);
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "{hex}"
    );
}

#[test]
fn to_hex_pads_small_words_with_zeros() {
    let hex = ModuleHash::from_words([1, 2]).to_hex();
    assert_eq!(hex, "00000000000000010000000000000002");
}

#[test]
fn from_hex_round_trips_to_hex() {
    for hash in [
        ModuleHash::of(b"round trip"),
        ModuleHash::from_words(SAMPLE_WORDS),
        ModuleHash::from_words([0, 0]),
        ModuleHash::from_words([u64::MAX, u64::MAX]),
    ] {
        assert_eq!(ModuleHash::from_hex(&hash.to_hex()), Ok(hash));
    }
}

#[test]
fn from_hex_rejects_uppercase() {
    let upper = ModuleHash::from_words(SAMPLE_WORDS).to_hex().to_uppercase();
    assert!(ModuleHash::from_hex(&upper).is_err());
}

#[test]
fn from_hex_rejects_wrong_lengths() {
    let valid = ModuleHash::from_words(SAMPLE_WORDS).to_hex();
    assert!(ModuleHash::from_hex(&valid[..31]).is_err());
    assert!(ModuleHash::from_hex(&format!("{valid}0")).is_err());
    assert!(ModuleHash::from_hex("").is_err());
}

#[test]
fn from_hex_rejects_leading_plus_sign() {
    let valid = ModuleHash::from_words(SAMPLE_WORDS).to_hex();
    let signed = format!("+{}", &valid[1..]);
    assert_eq!(signed.len(), 32);
    assert!(ModuleHash::from_hex(&signed).is_err());
}

#[test]
fn from_hex_rejects_non_hex_character() {
    let valid = ModuleHash::from_words(SAMPLE_WORDS).to_hex();
    let bad = format!("g{}", &valid[1..]);
    assert!(ModuleHash::from_hex(&bad).is_err());
}

#[test]
fn from_hex_rejects_whitespace() {
    let valid = ModuleHash::from_words(SAMPLE_WORDS).to_hex();
    assert!(ModuleHash::from_hex(&format!(" {}", &valid[1..])).is_err());
    assert!(ModuleHash::from_hex(&format!("{} ", &valid[..31])).is_err());
}

#[test]
fn one_bit_flip_changes_the_hash() {
    let bytes = module("");
    let mut flipped = bytes.clone();
    flipped[10] ^= 1;
    assert_ne!(ModuleHash::of(&bytes), ModuleHash::of(&flipped));
}

#[test]
fn empty_input_hash_is_stable() {
    let hash = ModuleHash::of(b"");
    assert_eq!(hash.to_hex(), expected_hex(b""));
    assert_eq!(hash, ModuleHash::of(&[]));
}

#[test]
fn report_hash_equals_hash_of_the_bytes() {
    let bytes = module("");
    let report = admit(&bytes, limits_with(|_| {})).unwrap();
    assert_eq!(report.hash, ModuleHash::of(&bytes));
}
