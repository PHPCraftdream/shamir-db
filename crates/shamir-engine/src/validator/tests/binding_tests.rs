use crate::validator::{PersistedValidators, ValidatorBinding, WriteOp};
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;

#[test]
fn binding_serde_round_trip() {
    let binding = ValidatorBinding {
        validator_id: RecordId::system("test_val"),
        ops: smallvec![WriteOp::Insert, WriteOp::Update],
        priority: 1000,
    };

    let bytes = bincode::serialize(&binding).unwrap();
    let got: ValidatorBinding = bincode::deserialize(&bytes).unwrap();

    assert_eq!(got.validator_id, binding.validator_id);
    assert_eq!(got.ops.as_slice(), binding.ops.as_slice());
    assert_eq!(got.priority, 1000);
}

#[test]
fn persisted_validators_serde_round_trip() {
    let pv = PersistedValidators {
        bindings: vec![
            ValidatorBinding {
                validator_id: RecordId::system("val_a"),
                ops: smallvec![WriteOp::Insert],
                priority: 2000,
            },
            ValidatorBinding {
                validator_id: RecordId::system("val_b"),
                ops: smallvec![WriteOp::Update, WriteOp::Delete],
                priority: 3000,
            },
        ],
    };

    let bytes = bincode::serialize(&pv).unwrap();
    let got: PersistedValidators = bincode::deserialize(&bytes).unwrap();

    assert_eq!(got.bindings.len(), 2);
    assert_eq!(got.bindings[0].validator_id, RecordId::system("val_a"));
    assert_eq!(got.bindings[0].priority, 2000);
    assert_eq!(
        got.bindings[1].ops.as_slice(),
        &[WriteOp::Update, WriteOp::Delete]
    );
}

#[test]
fn persisted_validators_empty() {
    let pv = PersistedValidators {
        bindings: Vec::new(),
    };
    let bytes = bincode::serialize(&pv).unwrap();
    let got: PersistedValidators = bincode::deserialize(&bytes).unwrap();
    assert!(got.bindings.is_empty());
}
