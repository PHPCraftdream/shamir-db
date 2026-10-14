use crate::meta::repl::{
    decode_receipt, encode_receipt, receipt_id, ReceiptBinding, ReceiptCounterDelta, ReceiptOp,
    ReceiptPhase, ReceiptTimestamp, ReceiptVersionMapping, ReplicationReceipt, MAX_RECEIPT_EVENTS,
    MAX_RECEIPT_OPS,
};

fn full_receipt() -> ReplicationReceipt {
    ReplicationReceipt {
        lineage_id: [3; 16],
        upstream_epoch: 7,
        predecessor_bookmark: 40,
        upstream_start: 41,
        upstream_end: 43,
        mapping: vec![
            ReceiptVersionMapping {
                upstream_version: 41,
                local_version: 101,
            },
            ReceiptVersionMapping {
                upstream_version: 43,
                local_version: 102,
            },
        ],
        ops: vec![
            ReceiptOp {
                table: "users".into(),
                key: vec![1, 2, 3],
                value: Some(vec![9, 8]),
            },
            ReceiptOp {
                table: "users".into(),
                key: vec![4, 5],
                value: None,
            },
        ],
        bindings: vec![ReceiptBinding {
            interned_id: 12,
            name: "name".into(),
        }],
        downstream_events: vec![vec![1; 8], vec![2; 8]],
        timestamps: vec![
            ReceiptTimestamp {
                local_version: 101,
                ts_ms: 1_000,
            },
            ReceiptTimestamp {
                local_version: 102,
                ts_ms: 2_000,
            },
        ],
        counters: vec![ReceiptCounterDelta {
            table: "row_count".into(),
            before: 10,
            after: 11,
        }],
        phase: ReceiptPhase::Prepared,
    }
}

#[test]
fn receipt_round_trips_all_fields() {
    for phase in [
        ReceiptPhase::Prepared,
        ReceiptPhase::Materialized,
        ReceiptPhase::Checkpointed,
    ] {
        let receipt = ReplicationReceipt {
            phase,
            ..full_receipt()
        };
        let id = receipt_id(&receipt);
        let bytes = encode_receipt(&receipt).unwrap();
        assert_eq!(decode_receipt(&id, &bytes).unwrap(), receipt);
    }
}

#[test]
fn receipt_id_is_deterministic_and_separating() {
    let receipt = full_receipt();
    assert_eq!(receipt_id(&receipt), receipt_id(&receipt));

    let mut mutated = receipt.clone();
    mutated.lineage_id = [4; 16];
    assert_ne!(receipt_id(&receipt), receipt_id(&mutated));

    let mut mutated = receipt.clone();
    mutated.upstream_epoch += 1;
    assert_ne!(receipt_id(&receipt), receipt_id(&mutated));

    let mut mutated = receipt.clone();
    mutated.mapping.push(ReceiptVersionMapping {
        upstream_version: 99,
        local_version: 999,
    });
    assert_ne!(receipt_id(&receipt), receipt_id(&mutated));

    let mut mutated = receipt.clone();
    mutated.ops.push(ReceiptOp {
        table: "extra".into(),
        key: vec![7],
        value: Some(vec![7]),
    });
    assert_ne!(receipt_id(&receipt), receipt_id(&mutated));

    let mut mutated = receipt.clone();
    mutated.phase = ReceiptPhase::Materialized;
    assert_ne!(receipt_id(&receipt), receipt_id(&mutated));
}

#[test]
fn decode_rejects_wrong_id() {
    let receipt = full_receipt();
    let bytes = encode_receipt(&receipt).unwrap();
    let mut other_id = receipt_id(&receipt);
    other_id[0] ^= 0xff;
    assert!(decode_receipt(&other_id, &bytes).is_err());
}

#[test]
fn decode_rejects_garbage_and_truncation() {
    let receipt = full_receipt();
    let bytes = encode_receipt(&receipt).unwrap();
    let id = receipt_id(&receipt);

    let mut garbage = vec![0u8; 64];
    for (i, byte) in garbage.iter_mut().enumerate() {
        *byte = (i * 37 % 251) as u8;
    }
    assert!(decode_receipt(&id, &garbage).is_err());
    assert!(decode_receipt(&id, &bytes[..bytes.len() / 2]).is_err());
    assert!(decode_receipt(&id, &[]).is_err());
}

#[test]
fn decode_rejects_trailing_bytes() {
    let receipt = full_receipt();
    let mut bytes = encode_receipt(&receipt).unwrap();
    bytes.push(0);
    assert!(decode_receipt(&receipt_id(&receipt), &bytes).is_err());
}

#[test]
fn decode_rejects_unknown_version() {
    let receipt = full_receipt();
    let bytes = crate::meta::MetaEnvelope::new((7u16, receipt.clone()))
        .encode()
        .unwrap();
    assert!(decode_receipt(&receipt_id(&receipt), &bytes).is_err());
}

#[test]
fn decode_rejects_oversized_receipt() {
    let mut receipt = full_receipt();
    receipt.mapping = (0..=MAX_RECEIPT_EVENTS)
        .map(|i| ReceiptVersionMapping {
            upstream_version: i as u64,
            local_version: i as u64,
        })
        .collect();
    assert!(encode_receipt(&receipt).is_err());
}

#[test]
fn receipt_with_invalid_range_rejected() {
    let mut receipt = full_receipt();
    receipt.upstream_end = receipt.upstream_start - 1;
    assert!(encode_receipt(&receipt).is_err());
}

#[test]
fn size_bound_constants_are_sane() {
    const { assert!(MAX_RECEIPT_OPS >= MAX_RECEIPT_EVENTS) };
    const { assert!(MAX_RECEIPT_EVENTS >= 1) };
}
