use serde::Serialize;
use shamir_db::types::value::QueryValue;
use shamir_tx::ChangeOp;

pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("write to String is infallible");
    }
    s
}

#[derive(Serialize)]
struct EventData<'a> {
    table: &'a str,
    op: &'a str,
    key: &'a QueryValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a QueryValue>,
    commit_version: u64,
}

pub fn make_event_data(
    change: &shamir_tx::changefeed::RecordChange,
    value_qv: Option<&QueryValue>,
    commit_version: u64,
) -> Vec<u8> {
    let op_str = match change.op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<QueryValue>(&change.key)
        .unwrap_or_else(|_| QueryValue::Str(hex_encode(&change.key)));
    let payload = EventData {
        table: &change.table,
        op: op_str,
        key: &key_value,
        value: value_qv,
        commit_version,
    };
    rmp_serde::to_vec_named(&payload).unwrap_or_default()
}

#[derive(Serialize)]
struct KeysData<'a> {
    table: &'a str,
    op: &'a str,
    key: &'a QueryValue,
    commit_version: u64,
}

pub fn make_keys_data(table: &str, op: &ChangeOp, key: &[u8], commit_version: u64) -> Vec<u8> {
    let op_str = match op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<QueryValue>(key)
        .unwrap_or_else(|_| QueryValue::Str(hex_encode(key)));
    let payload = KeysData {
        table,
        op: op_str,
        key: &key_value,
        commit_version,
    };
    rmp_serde::to_vec_named(&payload).unwrap_or_default()
}
