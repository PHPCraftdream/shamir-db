use crate::api::{CompileLimits, Position};
use crate::front::{scan, Keyword, TokenKind};

#[test]
fn tokens_track_utf8_bytes_and_unicode_scalar_columns() {
    let source = "\"é🙂\" name";
    let tokens = scan(source, &CompileLimits::default()).expect("valid string and identifier");
    let name = tokens.get(1).expect("identifier token");
    assert_eq!(name.kind, TokenKind::Identifier("name".to_owned()));
    assert_eq!(name.span.start_byte, 9);
    assert_eq!(name.span.end_byte, 13);
    assert_eq!(name.span.start, Position { line: 1, column: 6 });
    assert_eq!(
        name.span.end,
        Position {
            line: 1,
            column: 10
        }
    );
}

#[test]
fn line_comments_block_comments_crlf_and_lone_carriage_return_keep_positions() {
    let source = "// line\r\n/* block\rcontinued */\rlet\ntrue";
    let tokens = scan(source, &CompileLimits::default()).expect("comments and newline forms");
    let let_token = tokens.first().expect("let token");
    assert_eq!(let_token.kind, TokenKind::Keyword(Keyword::Let));
    assert_eq!(let_token.span.start, Position { line: 4, column: 1 });
    let true_token = tokens.get(1).expect("true token");
    assert_eq!(true_token.kind, TokenKind::Keyword(Keyword::True));
    assert_eq!(true_token.span.start, Position { line: 5, column: 1 });
    let eof = tokens.get(2).expect("EOF token");
    assert_eq!(eof.span.start, Position { line: 5, column: 5 });
}
