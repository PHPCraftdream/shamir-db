use crate::shamir_db::curl_gateway::escape_curl_value;

#[test]
fn escape_curl_value_backslash() {
    assert_eq!(escape_curl_value(r"a\b"), r"a\\b");
}

#[test]
fn escape_curl_value_quotes() {
    assert_eq!(escape_curl_value(r#"a"b"#), r#"a\"b"#);
}

#[test]
fn escape_curl_value_plain() {
    assert_eq!(escape_curl_value("hello"), "hello");
}
