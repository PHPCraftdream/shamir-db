use crate::api::{CompileLimits, Position};
use crate::front::lexer::{
    scan, IntegerRadix, IntegerSuffix, Keyword, Operator, Punctuation, TokenKind,
};

fn kinds(source: &str) -> Vec<TokenKind> {
    scan(source, &CompileLimits::default())
        .expect("valid lexer fixture")
        .into_iter()
        .map(|token| token.kind)
        .collect()
}

fn is_code(source: &str, code: &str) -> bool {
    scan(source, &CompileLimits::default())
        .err()
        .is_some_and(|diagnostic| diagnostic.code == code)
}

#[test]
fn identifiers_keywords_annotation_and_symbol_inventory_have_exact_kinds() {
    assert_eq!(
        kinds("scalar validator procedure fn let const if else while for in return break continue throw true false null i32 i64 f64 bool string bytes Validation Batch Request Response name_7 @entry"),
        vec![
            TokenKind::Keyword(Keyword::Scalar), TokenKind::Keyword(Keyword::Validator),
            TokenKind::Keyword(Keyword::Procedure), TokenKind::Keyword(Keyword::Fn),
            TokenKind::Keyword(Keyword::Let), TokenKind::Keyword(Keyword::Const),
            TokenKind::Keyword(Keyword::If), TokenKind::Keyword(Keyword::Else),
            TokenKind::Keyword(Keyword::While), TokenKind::Keyword(Keyword::For),
            TokenKind::Keyword(Keyword::In), TokenKind::Keyword(Keyword::Return),
            TokenKind::Keyword(Keyword::Break), TokenKind::Keyword(Keyword::Continue),
            TokenKind::Keyword(Keyword::Throw), TokenKind::Keyword(Keyword::True),
            TokenKind::Keyword(Keyword::False), TokenKind::Keyword(Keyword::Null),
            TokenKind::Keyword(Keyword::I32), TokenKind::Keyword(Keyword::I64),
            TokenKind::Keyword(Keyword::F64), TokenKind::Keyword(Keyword::Bool),
            TokenKind::Keyword(Keyword::String), TokenKind::Keyword(Keyword::Bytes),
            TokenKind::Keyword(Keyword::Validation), TokenKind::Keyword(Keyword::Batch),
            TokenKind::Keyword(Keyword::Request), TokenKind::Keyword(Keyword::Response),
            TokenKind::Identifier("name_7".to_owned()), TokenKind::AnnotationEntry, TokenKind::Eof,
        ]
    );
    assert!(is_code("@entryx", "L001"));
    assert_eq!(
        kinds("= + - * / % ! == != < <= > >= && || .. -> ( ) { } [ ] , : ; . ?"),
        vec![
            TokenKind::Operator(Operator::Assign),
            TokenKind::Operator(Operator::Plus),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Operator(Operator::Star),
            TokenKind::Operator(Operator::Slash),
            TokenKind::Operator(Operator::Percent),
            TokenKind::Operator(Operator::Bang),
            TokenKind::Operator(Operator::EqualEqual),
            TokenKind::Operator(Operator::BangEqual),
            TokenKind::Operator(Operator::Less),
            TokenKind::Operator(Operator::LessEqual),
            TokenKind::Operator(Operator::Greater),
            TokenKind::Operator(Operator::GreaterEqual),
            TokenKind::Operator(Operator::AndAnd),
            TokenKind::Operator(Operator::OrOr),
            TokenKind::Operator(Operator::Range),
            TokenKind::Operator(Operator::Arrow),
            TokenKind::Punctuation(Punctuation::LeftParen),
            TokenKind::Punctuation(Punctuation::RightParen),
            TokenKind::Punctuation(Punctuation::LeftBrace),
            TokenKind::Punctuation(Punctuation::RightBrace),
            TokenKind::Punctuation(Punctuation::LeftBracket),
            TokenKind::Punctuation(Punctuation::RightBracket),
            TokenKind::Punctuation(Punctuation::Comma),
            TokenKind::Punctuation(Punctuation::Colon),
            TokenKind::Punctuation(Punctuation::Semicolon),
            TokenKind::Punctuation(Punctuation::Dot),
            TokenKind::Punctuation(Punctuation::Question),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn integers_floats_ranges_and_negative_min_candidates_are_distinguished() {
    assert_eq!(
        kinds("0 42 -2147483648i32 -9223372036854775808 -0x80000000i32 -0x8000000000000000i64 2147483647i32 9223372036854775807i64 0x2Ai64 1.25 3e+2f64 4..5"),
        vec![
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 0, suffix: None, radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 42, suffix: None, radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 2_147_483_648, suffix: Some(IntegerSuffix::I32), radix: IntegerRadix::Decimal, negative_min_candidate: true }),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 9_223_372_036_854_775_808, suffix: None, radix: IntegerRadix::Decimal, negative_min_candidate: true }),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 2_147_483_648, suffix: Some(IntegerSuffix::I32), radix: IntegerRadix::Hexadecimal, negative_min_candidate: true }),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 9_223_372_036_854_775_808, suffix: Some(IntegerSuffix::I64), radix: IntegerRadix::Hexadecimal, negative_min_candidate: true }),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 2_147_483_647, suffix: Some(IntegerSuffix::I32), radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 9_223_372_036_854_775_807, suffix: Some(IntegerSuffix::I64), radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 42, suffix: Some(IntegerSuffix::I64), radix: IntegerRadix::Hexadecimal, negative_min_candidate: false }),
            TokenKind::Float(1.25), TokenKind::Float(300.0),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 4, suffix: None, radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Operator(Operator::Range),
            TokenKind::Integer(crate::front::IntegerLiteral { magnitude: 5, suffix: None, radix: IntegerRadix::Decimal, negative_min_candidate: false }),
            TokenKind::Eof,
        ]
    );
    for malformed in [
        "01",
        "0x",
        "1i",
        "1i33",
        "2147483648i32",
        "9223372036854775808",
        "0x80000000i32",
        "0x8000000000000000i64",
        "2147483649i32",
        "9223372036854775809",
        "1e",
        "1e+",
        "1.0e9999",
        "1e9999",
        "01.2",
    ] {
        assert!(is_code(malformed, "L002"), "{malformed}");
    }
    for maximum in [
        "2147483647i32",
        "9223372036854775807i64",
        "0x7fffffffi32",
        "0x7fffffffffffffffi64",
    ] {
        assert!(
            scan(maximum, &CompileLimits::default()).is_ok(),
            "{maximum}"
        );
    }
}

#[test]
fn string_and_byte_escapes_decode_to_exact_payloads() {
    assert_eq!(
        kinds(r#""\u{0}" "\u{000000}" "\u{10FFFF}" b"\x00\x7f\xFF\0""#),
        vec![
            TokenKind::String("\0".to_owned()),
            TokenKind::String("\0".to_owned()),
            TokenKind::String("\u{10ffff}".to_owned()),
            TokenKind::Bytes(vec![0, 127, 255, 0]),
            TokenKind::Eof,
        ]
    );
    for control in (0u8..=31).chain(std::iter::once(127)) {
        let character = char::from(control);
        for prefix in ["\"", "b\""] {
            let source = format!("{prefix}{character}\"");
            assert!(is_code(&source, "L002"), "raw control {control}");
        }
    }
    for malformed in [r#""\u{110000}""#, r#""\u{-1}""#] {
        assert!(is_code(malformed, "L002"), "{malformed:?}");
    }
    for byte_escape in [
        r#"b"\"""#,
        r#"b"\\""#,
        r#"b"\n""#,
        r#"b"\r""#,
        r#"b"\t""#,
        r#"b"\0""#,
        r#"b"\xAf""#,
    ] {
        assert!(
            scan(byte_escape, &CompileLimits::default()).is_ok(),
            "{byte_escape}"
        );
    }
    for float in ["0.01", "1e+2", "1E-02f64", "12.00e003", "0.5f64"] {
        assert!(scan(float, &CompileLimits::default()).is_ok(), "{float}");
    }
    assert_eq!(
        kinds(
            r#""quote:\" slash:\/ backslash:\\ b:\b f:\f n:\n r:\r t:\t u:\u{1F642}" b"\"\\\n\r\t\0\x41" "é🙂""#
        ),
        vec![
            TokenKind::String(
                "quote:\" slash:/ backslash:\u{005c} b:\u{0008} f:\u{000c} n:\n r:\r t:\t u:🙂"
                    .to_owned()
            ),
            TokenKind::Bytes(vec![b'"', b'\\', b'\n', b'\r', b'\t', 0, b'A']),
            TokenKind::String("é🙂".to_owned()),
            TokenKind::Eof,
        ]
    );

    for malformed in [
        r#""\q""#,
        r#""\u{}""#,
        r#""\u{1234567}""#,
        r#""\u{D800}""#,
        r#""\u{1é}""#,
        "b\"é\"",
        "b\"\\u\"",
        "\"\u{0001}\"",
        "b\"\u{007f}\"",
        "b\"\\b\"",
        "b\"\\/\"",
        "\"\\0\"",
        "\"\\x41\"",
    ] {
        assert!(is_code(malformed, "L002"), "{malformed:?}");
    }
    assert!(is_code("\"unfinished", "L003"));
    assert!(is_code("b\"unfinished", "L003"));
    assert!(is_code("/* unfinished", "L003"));
    assert!(is_code("\"\\u{12", "L003"));
    assert!(is_code("b\"\\x1", "L003"));
    assert!(is_code("\"\\u{12\"", "L002"));
    assert!(is_code("b\"\\x1\"", "L002"));
    let rendered =
        scan("\r\nλ", &CompileLimits::default()).expect_err("non-ASCII character after CRLF");
    assert_eq!(rendered.to_string(), "L001:2:1: invalid character");
    assert!(is_code("@", "L001"));
}

#[test]
fn malformed_unicode_escape_spans_stay_on_utf8_boundaries() {
    let source = "\"\\u{é}\"";
    let diagnostic = scan(source, &CompileLimits::default()).expect_err("malformed escape");
    assert_eq!(diagnostic.code, "L002");
    assert!(source.is_char_boundary(diagnostic.primary.start_byte as usize));
    assert!(source.is_char_boundary(diagnostic.primary.end_byte as usize));
    assert!(diagnostic.primary.start_byte <= diagnostic.primary.end_byte);
}

#[test]
fn comments_crlf_lone_cr_and_unicode_positions_are_exact() {
    let source = "// hi\r\n/* λ */\rname\n🙂";
    let diagnostic =
        scan(source, &CompileLimits::default()).expect_err("non-ASCII outside literals");
    assert_eq!(diagnostic.code, "L001");
    assert_eq!(diagnostic.primary.start, Position { line: 4, column: 1 });

    let tokens = scan(
        "// first\r\n/* multi\rline */\rlet",
        &CompileLimits::default(),
    )
    .expect("comments and CRLF");
    let keyword = tokens.first().expect("keyword token");
    assert_eq!(keyword.kind, TokenKind::Keyword(Keyword::Let));
    assert_eq!(keyword.span.start, Position { line: 4, column: 1 });
    let eof = tokens.get(1).expect("EOF token");
    assert_eq!(eof.span.start, Position { line: 4, column: 4 });
}
