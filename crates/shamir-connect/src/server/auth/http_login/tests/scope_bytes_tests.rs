use crate::server::http_login::{encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope};

fn scope<'a>(label: &'a str, databases: Option<&'a [&'a str]>) -> HttpLoginScope<'a> {
    HttpLoginScope {
        label,
        mode: HttpLoginMode::ReadOnly,
        channels: HttpLoginChannels::Rest,
        databases,
    }
}

#[test]
fn scope_rejects_control_bytes_and_invalid_database_lists() {
    let label = "x".repeat(129);
    for label in ["", "x\u{1f}", "x\u{7f}", label.as_str()] {
        assert!(
            encode_scope(&scope(label, None)).is_err(),
            "label {label:?}"
        );
    }
    let long_name = "d".repeat(256);
    let left = ["a"];
    assert!(encode_scope(&scope("x", Some(&left))).is_ok());
    let max_label = "x".repeat(128);
    assert!(encode_scope(&scope(max_label.as_str(), None)).is_ok());
    let exactly_max = "z".repeat(255);
    let encoded_max =
        encode_scope(&scope("x", Some(&[exactly_max.as_str()]))).expect("maximum database bytes");
    assert_eq!(encoded_max.len(), 263);
    let empty_db: &[&str] = &[];
    let duplicate = ["a", "a"];
    let reverse = ["b", "a"];
    let punctuated = ["bad.name"];
    let too_long = [long_name.as_str()];
    let invalid_lists: [&[&str]; 5] = [empty_db, &duplicate, &reverse, &punctuated, &too_long];
    assert!(encode_scope(&scope("x", Some(empty_db))).is_err());
    for databases in invalid_lists {
        assert!(encode_scope(&scope("x", Some(databases))).is_err());
    }
    let invalid_name = ["é"];
    assert!(encode_scope(&scope("x", Some(&invalid_name))).is_err());
    let uppercase = ["A-b_9"];
    assert!(encode_scope(&scope("x", Some(&uppercase))).is_ok());
    let too_many = ["a", "b", "c", "d", "e"];
    assert!(encode_scope(&scope("x", Some(&too_many))).is_err());
    let four_names = ["a", "b", "c", "d"];
    assert!(encode_scope(&scope("x", Some(&four_names))).is_ok());
    let exactly_max_label = "x".repeat(128);
    let one_db = "z".repeat(255);
    let max_scope =
        encode_scope(&scope(&exactly_max_label, Some(&[one_db.as_str()]))).expect("scope max");
    assert!(max_scope.len() <= 512);
    let too_large_pair = ["a".repeat(255), "b".repeat(255)];
    assert!(encode_scope(&scope(
        "x",
        Some(&[too_large_pair[0].as_str(), too_large_pair[1].as_str()]),
    ),)
    .is_err());
}

#[test]
fn scope_total_encoded_size_preflight_rejects_oversized_combination() {
    let label = "x".repeat(128);
    let db_one = "a".repeat(255);
    let db_two = "b".repeat(255);
    assert!(encode_scope(&scope(&label, Some(&[db_one.as_str(), db_two.as_str()]))).is_err());
    let max_label = encode_scope(&scope(&label, None)).expect("maximum label");
    assert_eq!(max_label.len(), 133);
    assert!(encode_scope(&scope("x\u{00}", None)).is_err());
}
