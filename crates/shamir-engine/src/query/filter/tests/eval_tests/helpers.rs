use crate::query::read::QueryResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, new_set, TMap};
use shamir_types::types::value::InnerValue;

/// Build a test record: {name: "Alice", age: 30, status: "active"}
pub(super) fn make_alice_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_age = interner.touch_ind("age").unwrap().into_key();
    let k_status = interner.touch_ind("status").unwrap().into_key();
    map.insert(k_name, InnerValue::Str("Alice".to_string()));
    map.insert(k_age, InnerValue::Int(30));
    map.insert(k_status, InnerValue::Str("active".to_string()));
    InnerValue::Map(map)
}

/// Build a record with nested fields: {user: {name: "Bob", score: 85}}
pub(super) fn make_nested_record(interner: &Interner) -> InnerValue {
    let mut inner = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_score = interner.touch_ind("score").unwrap().into_key();
    inner.insert(k_name, InnerValue::Str("Bob".to_string()));
    inner.insert(k_score, InnerValue::Int(85));

    let mut outer = new_map();
    let k_user = interner.touch_ind("user").unwrap().into_key();
    outer.insert(k_user, InnerValue::Map(inner));
    InnerValue::Map(outer)
}

/// Build a record with nullable field: {name: "Carol", deleted_at: Null}
pub(super) fn make_nullable_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_deleted = interner.touch_ind("deleted_at").unwrap().into_key();
    map.insert(k_name, InnerValue::Str("Carol".to_string()));
    map.insert(k_deleted, InnerValue::Null);
    InnerValue::Map(map)
}

/// Build a record with start/end dates: {start_date: 100, end_date: 200}
pub(super) fn make_date_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_start = interner.touch_ind("start_date").unwrap().into_key();
    let k_end = interner.touch_ind("end_date").unwrap().into_key();
    map.insert(k_start, InnerValue::Int(100));
    map.insert(k_end, InnerValue::Int(200));
    InnerValue::Map(map)
}

/// Build a record with a list field: {name: "Test", tags: ["rust", "db", "query"]}
pub(super) fn make_list_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_tags = interner.touch_ind("tags").unwrap().into_key();
    map.insert(k_name, InnerValue::Str("Test".to_string()));
    map.insert(
        k_tags,
        InnerValue::List(vec![
            InnerValue::Str("rust".to_string()),
            InnerValue::Str("db".to_string()),
            InnerValue::Str("query".to_string()),
        ]),
    );
    InnerValue::Map(map)
}

/// Build a record with a set field: {name: "Test", roles: {"admin", "user"}}
pub(super) fn make_set_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_roles = interner.touch_ind("roles").unwrap().into_key();
    let mut roles = new_set();
    roles.insert(InnerValue::Str("admin".to_string()));
    roles.insert(InnerValue::Str("user".to_string()));
    map.insert(k_name, InnerValue::Str("Test".to_string()));
    map.insert(k_roles, InnerValue::Set(roles));
    InnerValue::Map(map)
}

pub(super) fn make_body_record(interner: &Interner, body: &str) -> InnerValue {
    let mut map = new_map();
    let k_body = interner.touch_ind("body").unwrap().into_key();
    map.insert(k_body, InnerValue::Str(body.to_string()));
    InnerValue::Map(map)
}

pub(super) fn make_email_record(interner: &Interner, email: &str) -> InnerValue {
    let mut map = new_map();
    let k_email = interner.touch_ind("email").unwrap().into_key();
    map.insert(k_email, InnerValue::Str(email.to_string()));
    InnerValue::Map(map)
}

/// Build a record {a: "alice", b: "ALICE"} for FnCall dispatch tests.
pub(super) fn make_ab_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_a = interner.touch_ind("a").unwrap().into_key();
    let k_b = interner.touch_ind("b").unwrap().into_key();
    map.insert(k_a, InnerValue::Str("alice".into()));
    map.insert(k_b, InnerValue::Str("ALICE".into()));
    InnerValue::Map(map)
}

pub(super) fn empty_refs() -> TMap<String, QueryResult> {
    new_map()
}
