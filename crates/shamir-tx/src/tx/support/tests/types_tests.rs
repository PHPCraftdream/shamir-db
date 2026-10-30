use crate::types::{IsolationLevel, TxId};

#[test]
fn tx_id_display() {
    assert_eq!(TxId::new(42).to_string(), "tx#42");
}

#[test]
fn isolation_level_serde_roundtrip() {
    let levels = [
        IsolationLevel::Snapshot,
        IsolationLevel::Serializable,
        IsolationLevel::Pessimistic,
    ];
    for lvl in &levels {
        let bytes = rmp_serde::to_vec_named(lvl).unwrap();
        let back: IsolationLevel = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(*lvl, back);
    }
    // Wire-name check: rmp_serde encodes unit enum variants as their serde name
    // (string). Deserializing each payload as String reveals the wire identifier
    // used by shamir-db::execute::db_tx match arms.
    let wire_name = |lvl: &IsolationLevel| -> String {
        let bytes = rmp_serde::to_vec_named(lvl).unwrap();
        rmp_serde::from_slice::<String>(&bytes).unwrap()
    };
    assert_eq!(wire_name(&IsolationLevel::Snapshot), "snapshot");
    assert_eq!(wire_name(&IsolationLevel::Serializable), "serializable");
    assert_eq!(wire_name(&IsolationLevel::Pessimistic), "pessimistic");
}

#[test]
fn isolation_level_default_is_snapshot() {
    assert_eq!(IsolationLevel::default(), IsolationLevel::Snapshot);
}
