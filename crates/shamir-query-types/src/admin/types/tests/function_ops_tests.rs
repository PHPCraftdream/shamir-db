use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::admin::{
    AlterFunctionOp, CheckFunctionOp, CreateFunctionOp, DescribeFunctionOp, ExportFunctionOp,
};
use crate::batch::BatchOp;

// Serde round-trip tests: the wire format is the subject, so JSON literals are used directly.

fn assert_round_trip<T: Serialize + DeserializeOwned>(wire: Value) {
    let op: T = serde_json::from_value(wire.clone()).expect("decode op");
    assert_eq!(serde_json::to_value(&op).expect("encode op"), wire);
}

fn assert_rejected<T: DeserializeOwned>(wire: Value) {
    assert!(serde_json::from_value::<T>(wire).is_err());
}

#[test]
fn describe_function_round_trips() {
    assert_round_trip::<DescribeFunctionOp>(json!({
        "describe_function": "my_fn"
    }));
}

#[test]
fn export_function_round_trips() {
    assert_round_trip::<ExportFunctionOp>(json!({
        "export_function": "my_fn"
    }));
}

#[test]
fn check_function_round_trips() {
    assert_round_trip::<CheckFunctionOp>(json!({
        "check_function": {
            "wasm": "AGFzbQ=="
        }
    }));
    assert_round_trip::<CheckFunctionOp>(json!({
        "check_function": {
            "lang": "rust",
            "source": "pub fn shamir_call() {}",
            "optimize": "now"
        }
    }));
    assert_round_trip::<CheckFunctionOp>(json!({
        "check_function": {}
    }));
}

#[test]
fn alter_function_round_trips() {
    assert_round_trip::<AlterFunctionOp>(json!({
        "alter_function": "my_fn",
        "optimize": "never"
    }));
}

#[test]
fn new_function_ops_reject_unknown_fields() {
    assert_rejected::<DescribeFunctionOp>(json!({
        "describe_function": "my_fn",
        "typo": true
    }));
    assert_rejected::<ExportFunctionOp>(json!({
        "export_function": "my_fn",
        "typo": true
    }));
    assert_rejected::<CheckFunctionOp>(json!({
        "check_function": {},
        "typo": true
    }));
    assert_rejected::<AlterFunctionOp>(json!({
        "alter_function": "my_fn",
        "optimize": "now",
        "typo": true
    }));
}

#[test]
fn check_function_spec_rejects_unknown_fields() {
    assert_rejected::<CheckFunctionOp>(json!({
        "check_function": {
            "wasm": "AGFzbQ==",
            "typo": true
        }
    }));
}

#[test]
fn alter_function_requires_optimize() {
    assert_rejected::<AlterFunctionOp>(json!({
        "alter_function": "my_fn"
    }));
}

#[test]
fn create_function_lang_and_optimize_round_trip() {
    assert_round_trip::<CreateFunctionOp>(json!({
        "create_function": "my_fn",
        "wasm": "AGFzbQ==",
        "replace": false,
        "lang": "wasm",
        "optimize": "never"
    }));
}

#[test]
fn create_function_without_lang_and_optimize_omits_them() {
    let op: CreateFunctionOp = serde_json::from_value(json!({
        "create_function": "my_fn",
        "wasm": "AGFzbQ=="
    }))
    .expect("decode create_function");
    assert_eq!(op.lang, None);
    assert_eq!(op.optimize, None);
    let encoded = serde_json::to_value(&op).expect("encode create_function");
    let map = encoded.as_object().expect("object");
    assert!(!map.contains_key("lang"));
    assert!(!map.contains_key("optimize"));
}

#[test]
fn batch_op_picks_the_variant_for_each_new_key() {
    let cases = [
        json!({
            "describe_function": "my_fn"
        }),
        json!({
            "export_function": "my_fn"
        }),
        json!({
            "check_function": {
                "wasm": "AGFzbQ=="
            }
        }),
        json!({
            "alter_function": "my_fn",
            "optimize": "now"
        }),
    ];
    for wire in cases {
        let op: BatchOp = serde_json::from_value(wire.clone()).expect("decode batch op");
        let key = wire.as_object().unwrap().keys().next().unwrap().as_str();
        let matched = matches!(
            (&op, key),
            (BatchOp::DescribeFunction(_), "describe_function")
                | (BatchOp::ExportFunction(_), "export_function")
                | (BatchOp::CheckFunction(_), "check_function")
                | (BatchOp::AlterFunction(_), "alter_function")
        );
        assert!(matched, "wrong variant for {wire}");
        assert_eq!(serde_json::to_value(&op).expect("encode batch op"), wire);
    }
}

#[test]
fn new_function_ops_classify_write_and_admin() {
    let read_like = [
        json!({
            "describe_function": "my_fn"
        }),
        json!({
            "export_function": "my_fn"
        }),
        json!({
            "check_function": {}
        }),
    ];
    for wire in read_like {
        let op: BatchOp = serde_json::from_value(wire).expect("decode batch op");
        assert!(op.is_admin());
        assert!(!op.is_write());
    }
    let alter: BatchOp = serde_json::from_value(json!({
        "alter_function": "my_fn",
        "optimize": "now"
    }))
    .expect("decode alter_function");
    assert!(alter.is_admin());
    assert!(alter.is_write());
}
