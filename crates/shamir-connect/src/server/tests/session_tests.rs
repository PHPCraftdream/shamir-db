//! Tests for [`SessionPermissions`] role helpers.

use crate::server::session::SessionPermissions;

#[test]
fn has_role_true_for_present_role() {
    let perms = SessionPermissions::from_roles(vec!["replicator".into(), "read_write".into()]);
    assert!(perms.has_role("replicator"));
    assert!(perms.has_role("read_write"));
}

#[test]
fn has_role_false_for_absent_role() {
    let perms = SessionPermissions::from_roles(vec!["read_write".into()]);
    assert!(!perms.has_role("replicator"));
}

#[test]
fn has_role_false_for_empty_roles() {
    let perms = SessionPermissions::from_roles(vec![]);
    assert!(!perms.has_role("replicator"));
    assert!(!perms.is_superuser);
}

#[test]
fn has_role_is_case_sensitive() {
    let perms = SessionPermissions::from_roles(vec!["Replicator".into()]);
    assert!(!perms.has_role("replicator"));
    assert!(perms.has_role("Replicator"));
}

#[test]
fn has_role_finds_superuser_role_by_name() {
    let perms = SessionPermissions::from_roles(vec!["superuser".into()]);
    assert!(perms.has_role("superuser"));
    assert!(perms.is_superuser);
}
