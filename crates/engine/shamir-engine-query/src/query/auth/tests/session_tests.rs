//! Unit tests for `SessionPermissions::row_filter` (defect-2 regression:
//! removing the dead no-op loop must not change observable behavior) and
//! the deny-by-default fallback used by `extract_action_resource`'s
//! data-op arms (defect-3 regression: an invariant violation must degrade
//! to "deny", never panic, in a release build).

use crate::query::auth::{Action, Effect, Permission, Resource, Role, SessionPermissions};
use crate::query::filter::{Filter, FilterValue};

fn table_resource() -> Resource {
    Resource::Table {
        database: "db".to_string(),
        repo: "main".to_string(),
        table: "users".to_string(),
    }
}

fn eq_filter(field: &str, value: &str) -> Filter {
    Filter::Eq {
        field: vec![field.to_string()],
        value: FilterValue::String(value.to_string()),
    }
}

// ============================================================================
// Defect 2 — row_filter() dead-loop removal must not change behavior
// ============================================================================

/// A single Allow grant WITHOUT a row_filter is unrestricted: `row_filter()`
/// must return `None` (no WHERE clause to AND in).
#[test]
fn row_filter_unrestricted_grant_returns_none() {
    let sp = SessionPermissions::build(&[Role {
        name: "r".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: table_resource(),
            row_filter: None,
        }],
    }]);

    assert_eq!(sp.row_filter(Action::Read, &table_resource()), None);
}

/// A single Allow grant WITH a row_filter must return that filter verbatim.
#[test]
fn row_filter_single_grant_returns_its_filter() {
    let filter = eq_filter("status", "active");
    let sp = SessionPermissions::build(&[Role {
        name: "r".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: table_resource(),
            row_filter: Some(filter.clone()),
        }],
    }]);

    assert_eq!(sp.row_filter(Action::Read, &table_resource()), Some(filter));
}

/// Two roles granting the SAME action+resource, one filtered and one
/// unrestricted, must merge to unrestricted (`None`) — an unrestricted
/// grant always wins over a filtered one for the same (action, resource).
#[test]
fn row_filter_merges_unrestricted_over_filtered() {
    let sp = SessionPermissions::build(&[
        Role {
            name: "filtered".to_string(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: table_resource(),
                row_filter: Some(eq_filter("status", "active")),
            }],
        },
        Role {
            name: "unrestricted".to_string(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: table_resource(),
                row_filter: None,
            }],
        },
    ]);

    assert_eq!(sp.row_filter(Action::Read, &table_resource()), None);
}

/// Two roles granting the SAME action+resource, both filtered on different
/// predicates, must OR the two filters together.
#[test]
fn row_filter_merges_two_filtered_grants_with_or() {
    let f1 = eq_filter("status", "active");
    let f2 = eq_filter("status", "pending");
    let sp = SessionPermissions::build(&[
        Role {
            name: "a".to_string(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: table_resource(),
                row_filter: Some(f1.clone()),
            }],
        },
        Role {
            name: "b".to_string(),
            permissions: vec![Permission {
                effect: Effect::Allow,
                actions: vec![Action::Read],
                resource: table_resource(),
                row_filter: Some(f2.clone()),
            }],
        },
    ]);

    assert_eq!(
        sp.row_filter(Action::Read, &table_resource()),
        Some(Filter::Or {
            filters: vec![f1, f2],
        })
    );
}

/// No matching grant at all → no filter (nothing to restrict; `check()`
/// separately denies unmatched access).
#[test]
fn row_filter_no_matching_grant_returns_none() {
    let sp = SessionPermissions::build(&[Role {
        name: "r".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Insert],
            resource: table_resource(),
            row_filter: Some(eq_filter("status", "active")),
        }],
    }]);

    assert_eq!(sp.row_filter(Action::Read, &table_resource()), None);
}

// ============================================================================
// Defect 3 — deny-by-default fallback for extract_action_resource
// ============================================================================

/// The release-build fallback for a data op whose `table_ref()`
/// unexpectedly returned `None` must be a safe, deny-shaped resource —
/// never a panic. Calls the fallback function directly so the assertion
/// holds independent of `cfg!(debug_assertions)`, without relying on the
/// sibling `debug_assert!` firing.
#[test]
fn deny_by_default_fallback_is_global_resource() {
    let (action, resource) = SessionPermissions::deny_by_default(Action::Read);
    assert_eq!(action, Action::Read);
    assert_eq!(resource, Resource::Global);
}

/// The fallback resource must actually DENY a normal, scoped (non-global)
/// role — proving "deny-by-default" is a real security property, not just
/// a placeholder value. A role with only a Table-scoped Allow must NOT be
/// granted access to the fallback `Resource::Global`.
#[test]
fn deny_by_default_fallback_denies_scoped_role() {
    let sp = SessionPermissions::build(&[Role {
        name: "scoped".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: table_resource(),
            row_filter: None,
        }],
    }]);

    let (action, resource) = SessionPermissions::deny_by_default(Action::Read);
    assert_eq!(
        sp.check(action, &resource),
        Effect::Deny,
        "a table-scoped grant must not cover the deny-by-default Global fallback"
    );
}

/// Sanity check: a genuinely global-scoped Allow (superadmin-shaped) DOES
/// cover the fallback resource — confirms `Resource::Global` is a real
/// resource tier, not a synthetic value that always denies regardless of
/// grants.
#[test]
fn deny_by_default_fallback_allowed_for_global_grant() {
    let sp = SessionPermissions::build(&[Role {
        name: "global".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read],
            resource: Resource::Global,
            row_filter: None,
        }],
    }]);

    let (action, resource) = SessionPermissions::deny_by_default(Action::Read);
    assert_eq!(sp.check(action, &resource), Effect::Allow);
}
