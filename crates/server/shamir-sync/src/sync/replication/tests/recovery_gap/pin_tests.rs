use std::sync::{Arc, Barrier};

use super::{check_pin, parse_upstream_addr, PinPolicy, UpstreamPins};
use crate::replication::ReplError;
use shamir_collections::THasher;

#[test]
fn rv_server_e8e1d11b_tofu_rejects_changed_key_for_normalized_address() {
    let policy = PinPolicy {
        configured: UpstreamPins::default(),
        learned: scc::HashMap::with_hasher(THasher::default()),
    };
    let first = parse_upstream_addr("tcp://127.0.0.1:7331").unwrap();
    let second = parse_upstream_addr("tls://127.0.0.1:7331").unwrap();
    assert!(policy.check(first, [1; 32]).is_ok());
    assert!(policy.check(second, [1; 32]).is_ok());
    assert!(matches!(
        policy.check(second, [2; 32]),
        Err(ReplError::UpstreamKeyMismatch { .. })
    ));
}

#[test]
fn rv_server_e8e1d11b_competing_first_connections_enroll_only_one_key() {
    let policy = Arc::new(PinPolicy {
        configured: UpstreamPins::default(),
        learned: scc::HashMap::with_hasher(THasher::default()),
    });
    let barrier = Arc::new(Barrier::new(2));
    let addr = parse_upstream_addr("127.0.0.1:7331").unwrap();
    let handles: Vec<_> = [1, 2]
        .into_iter()
        .map(|byte| {
            let policy = policy.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                policy.check(addr, [byte; 32]).is_ok()
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_ne!(results[0], results[1]);
}

#[test]
fn rv_server_e8e1d11b_rotation_accepts_either_pin_and_rejects_other() {
    let addr = parse_upstream_addr("127.0.0.1:7331").unwrap();
    let pins = [[1; 32], [2; 32]];
    assert!(check_pin(addr, [1; 32], &pins).is_ok());
    assert!(check_pin(addr, [2; 32], &pins).is_ok());
    assert!(matches!(
        check_pin(addr, [3; 32], &pins),
        Err(ReplError::UpstreamKeyMismatch { .. })
    ));
}

#[test]
fn rv_server_e8e1d11b_rejects_invalid_config() {
    for value in [
        r#"{
            "not-an-address": ["0000000000000000000000000000000000000000000000000000000000000000"]
        }"#,
        r#"{
            "127.0.0.1:7331": ["zz00000000000000000000000000000000000000000000000000000000000000"]
        }"#,
        r#"{
            "127.0.0.1:7331": ["00"]
        }"#,
        r#"{
            "127.0.0.1:7331": []
        }"#,
    ] {
        assert!(
            serde_json::from_str::<UpstreamPins>(value).is_err(),
            "{value}"
        );
    }
}
