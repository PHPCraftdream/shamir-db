//! Tests for QueryReference parsing using JSON-like test cases
//!
//! Tests are organized as JSON-like test cases for documentation purposes.

use crate::query::batch::{QueryPath, QueryReference};

fn parse(
    s: &str,
) -> Result<QueryReference, crate::query::batch::ReferenceParseError> {
    QueryReference::parse(s)
}

#[test]
fn test_parse_simple_alias() {
    let ref_str = "@users";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::Root);
}

#[test]
fn test_parse_with_index() {
    let ref_str = "@users[0]";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::Index(0));
}

#[test]
fn test_parse_with_large_index() {
    let ref_str = "@items[42]";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "items");
    assert_eq!(r.path, QueryPath::Index(42));
}

#[test]
fn test_parse_all_items() {
    let ref_str = "@users[]";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::All);
}

#[test]
fn test_parse_field() {
    let ref_str = "@users.name";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::Field("name".to_string()));
}

#[test]
fn test_parse_count() {
    let ref_str = "@users.count";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::Count);
}

#[test]
fn test_parse_length() {
    let ref_str = "@users.length";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    assert_eq!(r.path, QueryPath::Count);
}

#[test]
fn test_parse_nested_field() {
    let ref_str = "@users.address.city";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0], QueryPath::Field("address".to_string()));
            assert_eq!(segments[1], QueryPath::Field("city".to_string()));
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_index_then_field() {
    let ref_str = "@users[0].name";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0], QueryPath::Index(0));
            assert_eq!(segments[1], QueryPath::Field("name".to_string()));
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_all_then_field() {
    let ref_str = "@users[].id";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0], QueryPath::All);
            assert_eq!(segments[1], QueryPath::Field("id".to_string()));
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_deep_path() {
    let ref_str = "@user[0].profile.address.city";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "user");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 4);
            assert_eq!(segments[0], QueryPath::Index(0));
            assert_eq!(segments[1], QueryPath::Field("profile".to_string()));
            assert_eq!(segments[2], QueryPath::Field("address".to_string()));
            assert_eq!(segments[3], QueryPath::Field("city".to_string()));
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_missing_at() {
    let ref_str = "users";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::MissingAt
    ));
}

#[test]
fn test_parse_empty_alias() {
    let ref_str = "@";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::EmptyAlias
    ));
}

#[test]
fn test_parse_invalid_alias() {
    let ref_str = "@user-name";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::InvalidAlias(_)
    ));
}

#[test]
fn test_parse_unclosed_bracket() {
    let ref_str = "@users[0";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::UnclosedBracket
    ));
}

#[test]
fn test_parse_invalid_index() {
    let ref_str = "@users[abc]";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::InvalidIndex(_)
    ));
}

#[test]
fn test_parse_trailing_dot() {
    let ref_str = "@users.";
    let err = parse(ref_str).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::ReferenceParseError::TrailingDot
    ));
}

#[test]
fn test_display_roundtrip() {
    let cases = [
        "@users",
        "@users[0]",
        "@users[]",
        "@users.name",
        "@users.count",
        "@users[0].name",
        "@users[].id",
        "@users[0].address.city",
    ];

    for case in cases {
        let r = parse(case).unwrap();
        let displayed = format!("{}", r);
        assert_eq!(displayed, case);
    }
}

#[test]
fn test_parse_with_underscore() {
    let ref_str = "@user_profile";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "user_profile");
}

#[test]
fn test_parse_with_numbers() {
    let ref_str = "@query1";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "query1");
}

#[test]
fn test_parse_complex_chain() {
    let ref_str = "@data[0].items[].value";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "data");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 4);
            assert_eq!(segments[0], QueryPath::Index(0));
            assert_eq!(segments[1], QueryPath::Field("items".to_string()));
            assert_eq!(segments[2], QueryPath::All);
            assert_eq!(segments[3], QueryPath::Field("value".to_string()));
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_count_in_chain() {
    let ref_str = "@users[].id.count";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "users");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 3);
            assert_eq!(segments[0], QueryPath::All);
            assert_eq!(segments[1], QueryPath::Field("id".to_string()));
            assert_eq!(segments[2], QueryPath::Count);
        }
        _ => panic!("Expected Chain"),
    }
}

#[test]
fn test_parse_mixed_index_and_all() {
    let ref_str = "@matrix[0][]";
    let r = parse(ref_str).unwrap();
    assert_eq!(r.alias, "matrix");
    match r.path {
        QueryPath::Chain(segments) => {
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0], QueryPath::Index(0));
            assert_eq!(segments[1], QueryPath::All);
        }
        _ => panic!("Expected Chain"),
    }
}
