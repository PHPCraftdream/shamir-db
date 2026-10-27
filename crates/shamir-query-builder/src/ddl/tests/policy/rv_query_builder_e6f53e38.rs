//! RV-QUERY-BUILDER-E6F53E38 — HMAC-gated policy builders must reject a
//! missing tag in `build()` (fallible `Result`, not advisory). Each no-tag
//! case below fails on the pre-fix code, where `build()` returned a `BatchOp`
//! with `hmac: None` and no error.

use shamir_query_types::admin::{GroupRef, PurgeScope, ResourceRef, Retention};
use shamir_query_types::batch::BatchOp;

use crate::ddl;
use crate::write::BuilderError;

#[test]
fn rv_query_builder_e6f53e38_no_tag_rejected_for_all_required_policy_builders() {
    let user = GroupRef::Name {
        name: "devs".into(),
    };
    let db = ResourceRef::Database {
        database: "app".into(),
    };
    let cases: Vec<(Result<BatchOp, BuilderError>, &str)> = vec![
        (ddl::create_user("alice", "pw").build(), "create_user"),
        (ddl::drop_user("alice").build(), "drop_user"),
        (ddl::grant_role("admin", "alice").build(), "grant_role"),
        (ddl::revoke_role("admin", "alice").build(), "revoke_role"),
        (ddl::chmod(db.clone(), 0o750).build(), "chmod"),
        (ddl::chown(db.clone(), 7).build(), "chown"),
        (ddl::chgrp(db.clone(), Some(3)).build(), "chgrp"),
        (ddl::create_group("devs").build(), "create_group"),
        (ddl::drop_group(user.clone()).build(), "drop_group"),
        (
            ddl::rename_group(user.clone(), "eng").build(),
            "rename_group",
        ),
        (
            ddl::add_group_member(user.clone(), 42).build(),
            "add_group_member",
        ),
        (
            ddl::remove_group_member(user, 42).build(),
            "remove_group_member",
        ),
        (
            ddl::start_migration("events", "archive", "fjall").build(),
            "start_migration",
        ),
        (ddl::commit_migration("mig-1").build(), "commit_migration"),
        (
            ddl::rollback_migration("mig-1").build(),
            "rollback_migration",
        ),
        (
            ddl::set_retention("users", Retention::default()).build(),
            "set_retention",
        ),
        (
            ddl::purge_history("users", PurgeScope::OlderThanAge { age_secs: 86_400 }).build(),
            "purge_history",
        ),
        (ddl::drop_db("app").build(), "drop_db"),
        (ddl::drop_repo("cold").build(), "drop_repo"),
        (ddl::drop_table("users").build(), "drop_table"),
        (ddl::drop_index("email_idx", "users").build(), "drop_index"),
    ];
    for (res, name) in cases {
        assert_eq!(
            res.err(),
            Some(BuilderError::MissingHmac),
            "{name} must reject a missing HMAC tag"
        );
    }
}

#[test]
fn rv_query_builder_e6f53e38_signed_tag_builds_same_shape_for_policy_builders() {
    assert_eq!(
        ddl::grant_role("admin", "alice").build().err(),
        Some(BuilderError::MissingHmac)
    );
    assert_eq!(
        ddl::drop_db("app").build().err(),
        Some(BuilderError::MissingHmac)
    );
    let op = ddl::grant_role("admin", "alice")
        .hmac("beef")
        .build()
        .unwrap();
    match op {
        BatchOp::GrantRole(g) => {
            assert_eq!(g.grant_role, "admin");
            assert_eq!(g.user, "alice");
            assert_eq!(g.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected GrantRole"),
    }
    let op = ddl::drop_db("app").hmac("beef").build().unwrap();
    match op {
        BatchOp::DropDb(d) => {
            assert_eq!(d.drop_db, "app");
            assert!(!d.cascade);
            assert_eq!(d.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected DropDb"),
    }
}
