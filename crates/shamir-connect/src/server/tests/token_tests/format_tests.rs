use crate::server::token::format::{
    encode_token, generate_token_parts, parse_token, TokenId, TokenParseError,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
#[test]
fn encode_parse_round_trip_and_secret_redaction() {
    let canonical = b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let parsed = parse_token(canonical).unwrap();
    let wire = encode_token(&parsed.id, &parsed.secret);
    assert_eq!(parse_token(wire.as_bytes()).unwrap().id, parsed.id);
    let encoded_secret = wire.rsplit('.').next().unwrap();
    assert!(!format!("{parsed:?}").contains(encoded_secret));
    assert!(!format!("{:?}", parsed.secret).contains(encoded_secret));
    assert_eq!(wire.as_bytes(), canonical.as_slice());

    let id_bytes = [1_u8; 16];
    let secret_bytes = [2_u8; 32];
    let nonzero_canonical = format!(
        "shk1.{}.{}",
        URL_SAFE_NO_PAD.encode(id_bytes),
        URL_SAFE_NO_PAD.encode(secret_bytes)
    );
    let parsed_nonzero = parse_token(nonzero_canonical.as_bytes()).unwrap();
    assert_eq!(parsed_nonzero.id, TokenId(id_bytes));
    assert_eq!(parsed_nonzero.secret.as_bytes(), &secret_bytes);
    let reencoded = encode_token(&parsed_nonzero.id, &parsed_nonzero.secret);
    assert_eq!(reencoded.as_bytes(), nonzero_canonical.as_bytes());
    let reparsed = parse_token(reencoded.as_bytes()).unwrap();
    assert_eq!(reparsed.id, TokenId(id_bytes));
    assert_eq!(reparsed.secret.as_bytes(), &secret_bytes);
}

#[test]
fn generation_entropy_failures_are_reported() {
    assert!(crate::common::crypto::with_entropy_failure(generate_token_parts).is_err());
    assert!(crate::common::crypto::with_entropy_failure_after(1, generate_token_parts).is_err());
    assert!(generate_token_parts().is_ok());
}
#[test]
fn malformed_inputs_and_canonical_tail_bits() {
    let canonical = b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let mut malformed = vec![
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/".to_vec(),
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+AA".to_vec(),
        vec![],
        vec![b'a'; 70],
        vec![b'a'; 72],
        b"bad1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_vec(),
        b"shk1/AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_vec(),
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_vec(),
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_vec(),
        {
            let mut value = canonical.to_vec();
            value[30] = b'/';
            value
        },
        {
            let mut value = canonical.to_vec();
            value[30] = b'=';
            value
        },
        {
            let mut value = canonical.to_vec();
            value[70] = 0xff;
            value
        },
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/".to_vec(),
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.AAA".to_vec(),
        b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            .iter()
            .copied()
            .chain("é".bytes())
            .collect(),
        vec![0xff],
        vec![b'x'; 1024 * 1024],
    ];
    for input in malformed.drain(..) {
        assert_eq!(parse_token(&input).unwrap_err(), TokenParseError);
    }
    for (index, replacement) in [
        (4, b'/'),
        (27, b'/'),
        (28, b'/'),
        (49, b'='),
        (10, b'.'),
        (10, b'+'),
        (70, b'B'),
    ] {
        let mut altered = canonical.to_vec();
        altered[index] = replacement;
        assert_eq!(parse_token(&altered).unwrap_err(), TokenParseError);
    }
    let mut extra_separator = canonical.to_vec();
    extra_separator[70] = b'.';
    assert_eq!(parse_token(&extra_separator).unwrap_err(), TokenParseError);
    let mut non_ascii = canonical.to_vec();
    non_ascii[10] = 0xc3;
    assert_eq!(parse_token(&non_ascii).unwrap_err(), TokenParseError);
    let mut non_utf8 = canonical.to_vec();
    non_utf8[50] = 0xff;
    assert_eq!(parse_token(&non_utf8).unwrap_err(), TokenParseError);
    for index in [26usize, 70] {
        let mut altered = canonical.to_vec();
        altered[index] = if altered[index] == b'A' { b'B' } else { b'A' };
        assert_eq!(parse_token(&altered).unwrap_err(), TokenParseError);
    }
    assert!(parse_token(canonical).is_ok());
}
#[test]
fn token_id_base64_is_canonical_for_every_last_byte() {
    for value in 0u16..=255 {
        let mut raw = [0u8; 16];
        raw[15] = value as u8;
        let id = TokenId(raw);
        let encoded = id.to_b64();
        assert_eq!(TokenId::from_b64(&encoded).unwrap(), id);

        let mut altered = encoded.into_bytes();
        let last = altered.len() - 1;
        let alphabet_index = match altered[last] {
            b'A'..=b'Z' => altered[last] - b'A',
            b'a'..=b'z' => altered[last] - b'a' + 26,
            b'0'..=b'9' => altered[last] - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => unreachable!("encoder emits only base64url alphabet"),
        };
        altered[last] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            [(alphabet_index ^ 1) as usize];
        assert_eq!(
            TokenId::from_b64(std::str::from_utf8(&altered).unwrap()).unwrap_err(),
            TokenParseError
        );
    }
}
