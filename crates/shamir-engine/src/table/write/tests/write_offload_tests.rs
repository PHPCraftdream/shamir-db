use std::sync::{Arc, OnceLock};

use shamir_funclib::registry::{FnEntry, ScalarError, ScalarRegistry};
use shamir_query_types::filter::{FilterValue, FnCall};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::super::write_helpers::eval_write_value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn computed_write_argon_dispatch_offloads_worker() {
    let observed = Arc::new(OnceLock::new());
    let thread = Arc::clone(&observed);
    let mut registry = ScalarRegistry::new();
    registry.register(
        "crypto/argon2id",
        FnEntry::pure(
            move |_| {
                let _ = thread.set(std::thread::current().id());
                Ok(QueryValue::Int(7))
            },
            0,
            Some(0),
        ),
    );
    let expr = FilterValue::FnCall {
        call: FnCall::complex("crypto/argon2id", vec![]),
    };
    let caller = std::thread::current().id();
    assert_eq!(
        eval_write_value(&expr, &new_map(), &registry),
        Ok(QueryValue::Int(7))
    );
    assert!(observed.get().is_some());
    assert_ne!(observed.get(), Some(&caller));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn computed_write_argon_error_propagates() {
    let mut registry = ScalarRegistry::new();
    registry.register(
        "crypto/argon2id",
        FnEntry::pure(|_| Err(ScalarError::new("probe_error")), 0, Some(0)),
    );
    let expr = FilterValue::FnCall {
        call: FnCall::complex("crypto/argon2id", vec![]),
    };
    assert_eq!(
        eval_write_value(&expr, &new_map(), &registry),
        Err("crypto/argon2id: probe_error".to_owned())
    );
}
