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
        let s = serde_json::to_string(lvl).unwrap();
        let back: IsolationLevel = serde_json::from_str(&s).unwrap();
        assert_eq!(*lvl, back);
    }
    // Wire format check
    assert_eq!(
        serde_json::to_string(&IsolationLevel::Snapshot).unwrap(),
        r#""snapshot""#
    );
    assert_eq!(
        serde_json::to_string(&IsolationLevel::Serializable).unwrap(),
        r#""serializable""#
    );
    assert_eq!(
        serde_json::to_string(&IsolationLevel::Pessimistic).unwrap(),
        r#""pessimistic""#
    );
}

#[test]
fn isolation_level_default_is_snapshot() {
    assert_eq!(IsolationLevel::default(), IsolationLevel::Snapshot);
}
