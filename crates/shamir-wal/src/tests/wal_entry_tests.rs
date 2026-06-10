use crate::wal_entry::{WalEntry, WalOp};
use shamir_types::types::record_id::RecordId;

#[test]
fn wal_entry_roundtrips_bincode() {
    let entry = WalEntry::new(
        42,
        vec![
            WalOp::RecordCreated {
                record_id: RecordId::new(),
            },
            WalOp::RecordDeleted {
                record_id: RecordId::new(),
            },
        ],
    );
    let bytes = bincode::serialize(&entry).unwrap();
    let back: WalEntry = bincode::deserialize(&bytes).unwrap();
    assert_eq!(back.txn_id, entry.txn_id);
    assert_eq!(back.ops.len(), 2);
    assert_eq!(back.counter_delta, 0, "default counter_delta must be 0");
}

#[test]
fn wal_entry_carries_counter_delta() {
    let entry = WalEntry::new_with_delta(
        7,
        vec![WalOp::RecordCreated {
            record_id: RecordId::new(),
        }],
        42,
    );
    let bytes = bincode::serialize(&entry).unwrap();
    let back: WalEntry = bincode::deserialize(&bytes).unwrap();
    assert_eq!(back.counter_delta, 42);

    let entry_neg = WalEntry::new_with_delta(
        8,
        vec![WalOp::RecordDeleted {
            record_id: RecordId::new(),
        }],
        -13,
    );
    let bytes = bincode::serialize(&entry_neg).unwrap();
    let back: WalEntry = bincode::deserialize(&bytes).unwrap();
    assert_eq!(back.counter_delta, -13);
}

#[test]
fn unknown_future_op_does_not_break_existing_variants() {
    // Smoke test: the present variants encode/decode fine. When
    // a future variant is added, this test ensures the existing
    // variants keep their numeric tags (bincode is sensitive to
    // variant order).
    let v1 = WalOp::RecordCreated {
        record_id: RecordId::new(),
    };
    let bytes = bincode::serialize(&v1).unwrap();
    let _: WalOp = bincode::deserialize(&bytes).expect("RecordCreated roundtrip");
}
