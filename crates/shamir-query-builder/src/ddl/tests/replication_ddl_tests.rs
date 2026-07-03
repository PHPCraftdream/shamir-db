//! Tests for the replication DDL builders (10 ops + `repl_scope` helper).

use shamir_query_types::admin::{ReplDirection, ReplMode, SubAction};
use shamir_query_types::batch::BatchOp;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// repl_scope helper
// ============================================================================

#[test]
fn repl_scope_db_only() {
    let s = ddl::repl_scope("app").build();
    assert_eq!(s.db, "app");
    assert_eq!(s.repo, None);
    assert_eq!(s.table, None);
}

#[test]
fn repl_scope_db_repo_table() {
    let s = ddl::repl_scope("app").repo("main").table("users").build();
    assert_eq!(s.db, "app");
    assert_eq!(s.repo.as_deref(), Some("main"));
    assert_eq!(s.table.as_deref(), Some("users"));
}

#[test]
fn repl_scope_db_repo_only() {
    let s = ddl::repl_scope("system").repo("edge_42").build();
    assert_eq!(s.db, "system");
    assert_eq!(s.repo.as_deref(), Some("edge_42"));
    assert_eq!(s.table, None);
}

// ============================================================================
// create_replication_profile
// ============================================================================

#[test]
fn replication_profile_single_stream() {
    let op = ddl::replication_profile("cluster")
        .stream(
            ddl::repl_scope("app").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        )
        .build();

    match op {
        BatchOp::CreateReplicationProfile(c) => {
            assert_eq!(c.create_replication_profile, "cluster");
            assert_eq!(c.streams.len(), 1);
            let s = &c.streams[0];
            assert_eq!(s.scope.db, "app");
            assert_eq!(s.direction, ReplDirection::Pull);
            assert_eq!(s.mode, ReplMode::ReadOnly);
        }
        _ => panic!("expected CreateReplicationProfile"),
    }
}

#[test]
fn replication_profile_multiple_streams() {
    let op = ddl::replication_profile("multi")
        .stream(
            ddl::repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        )
        .stream(
            ddl::repl_scope("edge").repo("collect").build(),
            ReplDirection::Push,
            ReplMode::ReadWrite,
        )
        .build();

    match op {
        BatchOp::CreateReplicationProfile(c) => {
            assert_eq!(c.streams.len(), 2);
            assert_eq!(c.streams[0].direction, ReplDirection::Pull);
            assert_eq!(c.streams[1].direction, ReplDirection::Push);
            assert_eq!(c.streams[1].mode, ReplMode::ReadWrite);
        }
        _ => panic!("expected CreateReplicationProfile"),
    }
}

#[test]
fn replication_profile_empty_streams() {
    let op = ddl::replication_profile("empty").build();
    match op {
        BatchOp::CreateReplicationProfile(c) => {
            assert_eq!(c.create_replication_profile, "empty");
            assert!(c.streams.is_empty());
        }
        _ => panic!("expected CreateReplicationProfile"),
    }
}

// ============================================================================
// drop_replication_profile
// ============================================================================

#[test]
fn drop_replication_profile_basic() {
    let op = ddl::drop_replication_profile("cluster");
    match op {
        BatchOp::DropReplicationProfile(d) => {
            assert_eq!(d.drop_replication_profile, "cluster");
        }
        _ => panic!("expected DropReplicationProfile"),
    }
}

// ============================================================================
// create_publication
// ============================================================================

#[test]
fn publication_single_scope() {
    let op = ddl::publication("pub_app")
        .scope(ddl::repl_scope("app").build())
        .build();

    match op {
        BatchOp::CreatePublication(c) => {
            assert_eq!(c.create_publication, "pub_app");
            assert_eq!(c.scopes.len(), 1);
            assert_eq!(c.scopes[0].db, "app");
        }
        _ => panic!("expected CreatePublication"),
    }
}

#[test]
fn publication_multiple_scopes() {
    let op = ddl::publication("pub_multi")
        .scope(ddl::repl_scope("app").build())
        .scope(ddl::repl_scope("system").repo("accounts").build())
        .build();

    match op {
        BatchOp::CreatePublication(c) => {
            assert_eq!(c.scopes.len(), 2);
            assert_eq!(c.scopes[0].db, "app");
            assert_eq!(c.scopes[1].db, "system");
        }
        _ => panic!("expected CreatePublication"),
    }
}

#[test]
fn publication_scopes_iter() {
    let scopes = vec![
        ddl::repl_scope("a").build(),
        ddl::repl_scope("b").build(),
        ddl::repl_scope("c").build(),
    ];
    let op = ddl::publication("p").scopes(scopes).build();

    match op {
        BatchOp::CreatePublication(c) => {
            assert_eq!(c.scopes.len(), 3);
            assert_eq!(c.scopes[0].db, "a");
            assert_eq!(c.scopes[2].db, "c");
        }
        _ => panic!("expected CreatePublication"),
    }
}

// ============================================================================
// drop_publication
// ============================================================================

#[test]
fn drop_publication_basic() {
    let op = ddl::drop_publication("pub_app");
    match op {
        BatchOp::DropPublication(d) => {
            assert_eq!(d.drop_publication, "pub_app");
        }
        _ => panic!("expected DropPublication"),
    }
}

// ============================================================================
// create_subscription
// ============================================================================

#[test]
fn subscription_basic() {
    let op = ddl::subscription("sub1", "leader:7432", "pub_app", "cluster");

    match op {
        BatchOp::CreateSubscription(c) => {
            assert_eq!(c.create_subscription, "sub1");
            assert_eq!(c.upstream, "leader:7432");
            assert_eq!(c.publication, "pub_app");
            assert_eq!(c.profile, "cluster");
        }
        _ => panic!("expected CreateSubscription"),
    }
}

// ============================================================================
// drop_subscription
// ============================================================================

#[test]
fn drop_subscription_basic() {
    let op = ddl::drop_subscription("sub1");
    match op {
        BatchOp::DropSubscription(d) => {
            assert_eq!(d.drop_subscription, "sub1");
        }
        _ => panic!("expected DropSubscription"),
    }
}

// ============================================================================
// alter_subscription — all three SubAction variants
// ============================================================================

#[test]
fn alter_subscription_pause() {
    let op = ddl::alter_subscription("sub1").pause().build();
    match op {
        BatchOp::AlterSubscription(a) => {
            assert_eq!(a.alter_subscription, "sub1");
            assert_eq!(a.action, SubAction::Pause);
        }
        _ => panic!("expected AlterSubscription"),
    }
}

#[test]
fn alter_subscription_resume() {
    let op = ddl::alter_subscription("sub1").resume().build();
    match op {
        BatchOp::AlterSubscription(a) => {
            assert_eq!(a.action, SubAction::Resume);
        }
        _ => panic!("expected AlterSubscription"),
    }
}

#[test]
fn alter_subscription_set_profile() {
    let op = ddl::alter_subscription("sub1")
        .set_profile("fast_profile")
        .build();
    match op {
        BatchOp::AlterSubscription(a) => {
            assert_eq!(a.action, SubAction::SetProfile("fast_profile".to_string()));
        }
        _ => panic!("expected AlterSubscription"),
    }
}

// ============================================================================
// read-only introspection ops
// ============================================================================

#[test]
fn list_publications_basic() {
    let op = ddl::list_publications();
    assert!(matches!(op, BatchOp::ListPublications(_)));
    // Round-trips cleanly and classifies as read-only (not write).
    assert!(!op.is_write());
    assert!(op.is_admin());
    let _ = roundtrip(&op);
}

#[test]
fn list_subscriptions_basic() {
    let op = ddl::list_subscriptions();
    assert!(matches!(op, BatchOp::ListSubscriptions(_)));
    assert!(!op.is_write());
    assert!(op.is_admin());
    let _ = roundtrip(&op);
}

#[test]
fn replication_status_basic() {
    let op = ddl::replication_status();
    assert!(matches!(op, BatchOp::ReplicationStatus(_)));
    assert!(!op.is_write());
    assert!(op.is_admin());
    let _ = roundtrip(&op);
}
