use crate::meta::envelope::{MetaEnvelope, MetaError};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    a: u32,
    b: String,
}

#[test]
fn round_trip() {
    let payload = Sample {
        a: 42,
        b: "hello".into(),
    };
    let env = MetaEnvelope::new(Sample {
        a: 42,
        b: "hello".into(),
    });
    let bytes = env.encode().unwrap();
    let got: Sample = MetaEnvelope::open(&bytes).unwrap();
    assert_eq!(got, payload);
}

#[test]
fn bad_magic_rejected() {
    let mut env = MetaEnvelope::new(Sample {
        a: 1,
        b: "x".into(),
    });
    env.magic = *b"XXXX";
    let bytes = bincode::serialize(&env).unwrap();
    let err = MetaEnvelope::<Sample>::open(&bytes).unwrap_err();
    assert!(matches!(err, MetaError::BadMagic { .. }));
}

#[test]
fn version_mismatch_rejected() {
    let mut env = MetaEnvelope::new(Sample {
        a: 1,
        b: "x".into(),
    });
    env.version = 999;
    let bytes = bincode::serialize(&env).unwrap();
    let err = MetaEnvelope::<Sample>::open(&bytes).unwrap_err();
    assert!(matches!(err, MetaError::UnsupportedVersion(999)));
}
