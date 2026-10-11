//! Duration precision at the actual Authorized -> root scope seam.

use std::time::{Duration, Instant};

use shamir_query_builder::batch::Batch;
use shamir_query_types::batch::{BatchLimits, ExecPolicy};
use shamir_types::access::Actor;

use crate::query::batch::{Authorized, BatchError};

#[test]
fn fractional_cap_is_exact_and_does_not_change_wire_limits() {
    let request = Batch::new().build();
    let cap = Duration::from_nanos(123_456_789);
    let known_now = Instant::now();
    let auth = Authorized::unchecked(&request, Actor::System, "test")
        .with_deadline_cap_at_for_test(known_now, cap);
    let scope = auth.root_scope();
    // This is the deadline checked by the scope, not its error-budget metadata.
    assert_eq!(
        scope.cap_deadline(),
        Some(known_now.checked_add(cap).expect("small cap fits Instant")),
    );
    assert_eq!(scope.capped_budget(), Some(cap));
    assert_eq!(scope.limits(), &request.limits);
}

#[test]
fn request_and_policy_clamp_budget_metadata_with_independent_time_enforcement() {
    for (request_secs, policy_secs, expected) in [(2, 7, 2), (7, 2, 2), (0, 7, 1)] {
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_execution_time_secs: request_secs,
            ..BatchLimits::default()
        });
        let request = batch.build();
        let policy = ExecPolicy {
            caps: BatchLimits {
                max_execution_time_secs: policy_secs,
                ..BatchLimits::CEILING
            },
            enforce_result_size: false,
        };
        let auth = Authorized::unchecked(&request, Actor::System, "test")
            .with_policy(policy)
            .with_deadline_cap(Duration::from_secs(5));
        assert_eq!(
            auth.root_scope().capped_budget(),
            Some(Duration::from_secs(expected)),
        );
    }
}

#[test]
fn zero_duration_expires_immediately_but_uncapped_zero_secs_keeps_legacy_minimum() {
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_execution_time_secs: 0,
        ..BatchLimits::default()
    });
    let request = batch.build();
    let uncapped = Authorized::unchecked(&request, Actor::System, "test").root_scope();
    assert_eq!(uncapped.capped_budget(), None);
    let capped = Authorized::unchecked(&request, Actor::System, "test")
        .with_deadline_cap(Duration::ZERO)
        .root_scope();
    assert_eq!(capped.capped_budget(), Some(Duration::ZERO));
    assert!(matches!(
        capped.check(),
        Err(BatchError::ExecutionTimedOut { budget_secs: 0 }),
    ));
}
