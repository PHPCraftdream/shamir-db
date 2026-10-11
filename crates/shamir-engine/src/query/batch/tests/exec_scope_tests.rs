//! Contract tests for root construction and policy clamping, not runtime bugs.
//! The ExecScope/policy API did not exist before W2. No clock or sleeps needed.

use shamir_query_builder::batch::Batch;
use shamir_query_types::batch::{BatchLimits, ExecPolicy};
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};

use crate::query::batch::{AccessGate, Authorized, ExecScope};

struct AllowAccess;

#[async_trait::async_trait]
impl AccessGate for AllowAccess {
    async fn check(
        &self,
        _actor: &Actor,
        _path: &ResourcePath,
        _action: Action,
    ) -> Result<(), AccessError> {
        Ok(())
    }
}

#[test]
fn root_preserves_request_limits_without_applying_policy() {
    let limits = BatchLimits {
        max_execution_time_secs: u64::MAX,
        max_iterations: 0,
        ..BatchLimits::CEILING
    };
    let scope = ExecScope::root(&limits);

    // Root itself must not substitute defaults or clamp to EMBEDDED caps.
    assert_eq!(scope.limits(), &limits);
    assert_eq!(scope.work(), 0);
    assert_eq!(scope.bytes(), 0);
}

#[test]
fn authorized_root_clamps_every_field_without_changing_request() {
    let limits = BatchLimits::CEILING;
    let caps = BatchLimits {
        max_queries: 7,
        max_dependency_depth: 3,
        max_execution_time_secs: 2,
        max_result_size: 1024,
        max_nesting_depth: 1,
        max_iterations: 5,
        max_total_queries: 11,
        max_total_work: 13,
        max_page_size: 17,
        max_offset: 19,
    };
    let mut batch = Batch::new();
    batch.limits(limits.clone());
    let request = batch.build();
    let auth = Authorized::unchecked(&request, Actor::System, "test").with_policy(ExecPolicy {
        caps: caps.clone(),
        enforce_result_size: true,
    });

    // This is the actual root-building path shared by both execution entries.
    // Removing clamping or omitting any field must change the expected result.
    let scope = auth.root_scope();
    assert_eq!(scope.limits(), &caps);
    assert_eq!(auth.request().limits, limits);
    assert_eq!(scope.work(), 0);
    assert_eq!(scope.bytes(), 0);

    // Clamping must not raise a smaller request to the policy cap either.
    let lower = BatchLimits {
        max_queries: 1,
        max_dependency_depth: 0,
        max_execution_time_secs: 0,
        max_result_size: 0,
        max_nesting_depth: 0,
        max_iterations: 0,
        max_total_queries: 0,
        max_total_work: 0,
        max_page_size: 0,
        max_offset: 0,
    };
    batch.limits(lower.clone());
    let request = batch.build();
    let auth =
        Authorized::unchecked(&request, Actor::System, "test").with_policy(auth.policy().clone());
    assert_eq!(auth.root_scope().limits(), &lower);
}

#[tokio::test]
async fn default_policy_preserves_default_and_in_ceiling_requests_in_both_constructors() {
    for limits in [BatchLimits::default(), BatchLimits::CEILING] {
        let mut batch = Batch::new();
        batch.limits(limits.clone());
        let request = batch.build();
        let unchecked = Authorized::unchecked(&request, Actor::System, "test");
        let authorized = Authorized::authorize(&request, Actor::System, "test", &AllowAccess)
            .await
            .expect("allowing gate must authorize the empty builder request");

        // Catch accidental replacement with defaults and non-EMBEDDED ctor policy.
        assert_eq!(unchecked.policy(), &ExecPolicy::EMBEDDED);
        assert_eq!(authorized.policy(), &ExecPolicy::EMBEDDED);
        assert_eq!(unchecked.root_scope().limits(), &limits);
        assert_eq!(authorized.root_scope().limits(), &limits);
    }
}
