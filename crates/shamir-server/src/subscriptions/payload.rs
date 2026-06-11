use shamir_tx::ChangeOp;

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn make_event_data(
    change: &shamir_tx::changefeed::RecordChange,
    value_json: Option<&serde_json::Value>,
    commit_version: u64,
) -> Vec<u8> {
    let op_str = match change.op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<serde_json::Value>(&change.key)
        .unwrap_or_else(|_| serde_json::Value::String(hex_encode(&change.key)));
    let mut obj = serde_json::json!({
        "table": change.table,
        "op": op_str,
        "key": key_value,
        "commit_version": commit_version
    });
    if let Some(val) = value_json {
        obj["value"] = val.clone();
    }
    serde_json::to_vec(&obj).unwrap_or_default()
}

pub(super) fn make_keys_data(
    table: &str,
    op: &ChangeOp,
    key: &[u8],
    commit_version: u64,
) -> Vec<u8> {
    let op_str = match op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<serde_json::Value>(key)
        .unwrap_or_else(|_| serde_json::Value::String(hex_encode(key)));
    serde_json::to_vec(&serde_json::json!({
        "table": table,
        "op": op_str,
        "key": key_value,
        "commit_version": commit_version
    }))
    .unwrap_or_default()
}
