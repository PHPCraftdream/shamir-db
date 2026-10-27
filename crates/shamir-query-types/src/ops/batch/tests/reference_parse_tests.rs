//! Owner-local oracles for `QueryReference::parse` error and path shapes.

use crate::batch::{QueryPath, QueryReference, ReferenceParseError};

#[test]
fn rv_query_types_b53ec5ce_reference_parse_shapes() {
    let root = QueryReference::parse("@users").unwrap();
    assert_eq!(root.alias, "users");
    assert_eq!(root.path, QueryPath::Root);

    let idx = QueryReference::parse("@users[0]").unwrap();
    assert_eq!(idx.path, QueryPath::Index(0));

    let all = QueryReference::parse("@users[]").unwrap();
    assert_eq!(all.path, QueryPath::All);

    let field = QueryReference::parse("@users.name").unwrap();
    assert_eq!(field.path, QueryPath::Field("name".to_string()));
}

#[test]
fn rv_query_types_b53ec5ce_reference_parse_errors() {
    assert!(matches!(
        QueryReference::parse("users"),
        Err(ReferenceParseError::MissingAt)
    ));
    assert!(matches!(
        QueryReference::parse("@"),
        Err(ReferenceParseError::EmptyAlias)
    ));
    assert!(matches!(
        QueryReference::parse("@us-ers"),
        Err(ReferenceParseError::InvalidAlias(alias)) if alias == "us-ers"
    ));
    assert!(matches!(
        QueryReference::parse("@users[0"),
        Err(ReferenceParseError::UnclosedBracket)
    ));
    assert!(matches!(
        QueryReference::parse("@users[x]"),
        Err(ReferenceParseError::InvalidIndex(inner)) if inner == "x"
    ));
    assert!(matches!(
        QueryReference::parse("@users."),
        Err(ReferenceParseError::TrailingDot)
    ));
    assert!(matches!(
        QueryReference::parse("@users[0]x"),
        Err(ReferenceParseError::UnexpectedChar('x'))
    ));
}
