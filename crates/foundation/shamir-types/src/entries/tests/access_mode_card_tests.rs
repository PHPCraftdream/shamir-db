use crate::access::{Actor, Mode, ResourceMeta};
use crate::mpack;
use crate::types::value::QueryValue;

#[test]
fn rv_types_15ecb3a6_malformed_present_mode_fails_closed() {
    let actor = Actor::User(42);
    let mut record = ResourceMeta::owned_by(actor.clone()).to_query_value();
    if let QueryValue::Map(ref mut map) = record {
        map.insert("mode".to_string(), QueryValue::Str("invalid".to_string()));
    }

    let meta = ResourceMeta::from_record(&record);
    assert_eq!(meta.mode, 0);
    assert_eq!(meta.owner, actor);
}

#[test]
fn rv_types_15ecb3a6_absent_mode_preserves_legacy_open_default() {
    let record = mpack!({"owner": 42});
    assert_eq!(ResourceMeta::from_record(&record).mode, Mode::OPEN);
}
