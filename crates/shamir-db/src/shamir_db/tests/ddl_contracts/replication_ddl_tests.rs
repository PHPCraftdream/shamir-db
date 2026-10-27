//! 386-a: server-side execution of replication DDL — round-trip CRUD + list
//! against an in-memory `ShamirDb`. Queries are built ONLY through the
//! `shamir_query_builder::ddl::replication::*` builders (builder-only rule).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_publication, drop_replication_profile, drop_subscription,
    list_publications, list_subscriptions, publication, repl_scope, replication_profile,
    replication_status, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;

use crate::types::value::QueryValue;
use crate::ShamirDb;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // `execute` resolves the target db before dispatching admin ops; the
    // repl catalogue itself lives in the `system` repo, but the call needs a
    // valid db context.
    shamir.create_db("testdb").await.expect("create_db");
    shamir
}

/// Extract the single admin-result record value for `alias`.
fn result_value(resp: &crate::query::batch::BatchResponse, alias: &str) -> QueryValue {
    resp.results[alias].records[0].as_value().into_owned()
}

#[tokio::test]
async fn create_publication_then_list_contains_it() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        publication("pub_a").scope(repl_scope("app").repo("main").build()),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let val = result_value(&resp, "lp");
    let pubs = val.get("publications").and_then(|v| v.as_array()).unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].get("name").and_then(|v| v.as_str()), Some("pub_a"));
}

#[tokio::test]
async fn create_subscription_stores_fields_and_lists() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    // S5c-X PR8b-2: creation binds the profile's CONCRETE Pull destinations,
    // so the profile must exist (and be concrete) before the subscription.
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let val = result_value(&resp, "ls");
    let subs = val.get("subscriptions").and_then(|v| v.as_array()).unwrap();
    assert_eq!(subs.len(), 1);
    let s = &subs[0];
    assert_eq!(s.get("name").and_then(|v| v.as_str()), Some("sub_a"));
    assert_eq!(
        s.get("upstream").and_then(|v| v.as_str()),
        Some("tcp://leader:9000")
    );
    assert_eq!(s.get("publication").and_then(|v| v.as_str()), Some("pub_a"));
    assert_eq!(s.get("profile").and_then(|v| v.as_str()), Some("profile_a"));
    assert_eq!(s.get("state").and_then(|v| v.as_str()), Some("active"));
}

#[tokio::test]
async fn alter_subscription_pause_resume_set_profile() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    // profile_b must have the SAME concrete Pull destination set as
    // profile_a: SetProfile is refused when it would change the binding.
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "crp2",
        replication_profile("profile_b").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // pause → state paused
    let mut b = Batch::new();
    b.id(2);
    b.try_op("alt", alter_subscription("sub_a").pause())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("paused".into())
    );

    // resume → state active
    let mut b = Batch::new();
    b.id(3);
    b.try_op("alt", alter_subscription("sub_a").resume())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("active".into())
    );

    // set_profile → profile updated
    let mut b = Batch::new();
    b.id(4);
    b.try_op("alt", alter_subscription("sub_a").set_profile("profile_b"))
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "profile").await,
        Some("profile_b".into())
    );
}

#[tokio::test]
async fn drop_publication_and_subscription_remove_from_list() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op("cp", publication("pub_a").scope(repl_scope("app").build()));
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op(
        "dp",
        drop_publication("pub_a")
            .hmac("dummy")
            .build()
            .expect("drop publication builder"),
    );
    b.op(
        "ds",
        drop_subscription("sub_a")
            .hmac("dummy")
            .build()
            .expect("drop subscription builder"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op("lp", list_publications());
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let pubs = result_value(&resp, "lp");
    assert!(pubs
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());
    let subs = result_value(&resp, "ls");
    assert!(subs
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn create_replication_profile_persists_streams() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("cluster").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Read the stored record directly from the system store.
    let table = shamir
        .system_store()
        .replication_profiles_table()
        .await
        .unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let query = crate::query::read::ReadQuery::new("replication_profiles").filter(
        crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String("cluster".to_string()),
        },
    );
    let result = table.read(&query, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 1);
    let rec = result.records[0].as_value();
    assert_eq!(rec.get("name").and_then(|v| v.as_str()), Some("cluster"));
    let streams = rec.get("streams").and_then(|v| v.as_array()).unwrap();
    assert_eq!(streams.len(), 1);
}

/// RI-10 Part 2: `mark_subscription_resync_required` (called by the
/// follower-loop supervisor on a terminal `JournalGap`) persists
/// `state = "resync_required"` on the subscription's row, and that value
/// round-trips verbatim through BOTH `ListSubscriptions` and
/// `ReplicationStatus` — the pre-existing admin surface that already echoes
/// `state` (no new endpoint was added). Every admin action on the
/// quarantined row is refused (typed `resync_required`), and — S5c-X
/// PR8b-2 — so is DROP: dropping is not evidence of consistency.
#[tokio::test]
async fn resync_required_round_trips_through_status_and_list_then_refuses_recovery() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Simulate what the supervisor does when `run_follower_loop` returns a
    // terminal `ReplError::JournalGap`.
    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("resync_required".into())
    );

    // `ListSubscriptions` echoes it verbatim.
    let mut b = Batch::new();
    b.id(2);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let subs = result_value(&resp, "ls");
    let subs = subs
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(
        subs[0].get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );

    // `ReplicationStatus` echoes it verbatim too.
    let mut b = Batch::new();
    b.id(3);
    b.op("rs", replication_status());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let status = result_value(&resp, "rs");
    let entries = status
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].get("name").and_then(|v| v.as_str()),
        Some("sub_a")
    );
    assert_eq!(
        entries[0].get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );

    // S5c-X PR8b (revised): the quarantine is a durable marker — ANY admin
    // action on the quarantined row is refused until a replacement seed is
    // installed: Resume, Pause and SetProfile all fail with the typed code.
    for (alias, action) in [
        ("alt_resume", alter_subscription("sub_a").resume()),
        ("alt_pause", alter_subscription("sub_a").pause()),
    ] {
        let mut b = Batch::new();
        b.id(4);
        b.try_op(alias, action).unwrap();
        let error = shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("resync_required"), "{error}");
        assert_eq!(
            sub_field(&shamir, "sub_a", "state").await,
            Some("resync_required".into()),
            "refusal must not mutate the row"
        );
    }

    // S5c-X PR8b-2: dropping a quarantined subscription is REFUSED —
    // dropping is not evidence that its data became consistent; follow the
    // reseed runbook instead. Round 6, traced: `ensure_drop_allowed` returns
    // the typed quarantine error and the admin dispatch maps it to the
    // DEDICATED `replication_destination_quarantined` code — the read-path
    // `resync_required` above is only for reads and the Resume/Pause gate.
    // COUNTERFACTUAL: `resync_required` here is the stale round-5 oracle.
    let mut b = Batch::new();
    b.id(5);
    b.op(
        "drop",
        drop_subscription("sub_a")
            .hmac("dummy")
            .build()
            .expect("drop subscription builder"),
    );
    let error = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        Some("replication_destination_quarantined"),
        "{error}"
    );
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("resync_required".into()),
        "refused DROP must not mutate the row"
    );
}

/// `mark_subscription_resync_required` on a subscription name that does not
/// exist is a no-op (does not error) — the subscription may have been
/// dropped concurrently by an admin command racing with gap detection.
#[tokio::test]
async fn mark_resync_required_on_missing_subscription_is_a_noop() {
    let shamir = setup().await;
    shamir
        .mark_subscription_resync_required("does_not_exist", 100, 42)
        .await
        .expect("missing subscription must not error the caller");
}

/// S5c-X PR8b-2 binding contract: after a VALID durable binding exists and
/// the subscription is marked `resync_required`, deleting the replication
/// profile must not unblock anything — the bound destination stays refused
/// with the typed `resync_required` code while an unrelated populated repo
/// still returns its actual data. Coverage comes from the row's durable
/// binding, never from profile resolution.
#[tokio::test]
async fn resync_marking_keeps_bound_destinations_blocked_without_profile() {
    let shamir = setup().await;

    // testdb/main (the bound destination) + testdb/side (unrelated,
    // populated): both must exist with a `users` table.
    let instance = shamir.get_db("testdb").unwrap();
    for repo in ["main", "side"] {
        instance
            .add_repo(
                RepoConfig::new(repo, BoxRepoFactory::in_memory())
                    .add_table(crate::engine::table::TableConfig::new("users")),
            )
            .await
            .unwrap();
    }

    // Valid durable binding via the builder: profile_a is CONCRETE.
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .expect("marking must succeed: the row's binding is valid");

    // Persisted state + durable marker were written.
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("resync_required".into())
    );
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let row = table
        .read(
            &crate::query::read::ReadQuery::new("subscriptions").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String("sub_a".to_string()),
                },
            ),
            &ctx,
        )
        .await
        .unwrap()
        .records
        .remove(0)
        .as_value()
        .into_owned();
    assert_eq!(row.get("quarantined"), Some(&QueryValue::Bool(true)));
    assert!(row.get("binding_version").is_some(), "row must be bound");

    // DELETE the profile: the binding, not the profile, is the authority.
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "drp",
        drop_replication_profile("profile_a")
            .hmac("dummy")
            .build()
            .expect("drop replication profile builder"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // The bound destination is STILL refused with the typed code.
    let outcome = shamir.read_changelog_bounded("testdb", "main", 0, 10).await;
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("bound destination must stay quarantined after the profile is deleted"),
    };
    assert_eq!(
        error.code(),
        Some("resync_required"),
        "read of testdb/main must be refused with the typed quarantine code"
    );

    // An unrelated populated repo still returns its actual data.
    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ins",
        shamir_query_builder::write::Insert::with_repo("side", "users")
            .row(doc! { "name" => "Bob", "age" => 7 }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(4);
    b.query("r", Query::with_repo("side", "users"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(resp.results["r"].records.len(), 1);

    // status/list show the quarantined state.
    let mut b = Batch::new();
    b.id(5);
    b.op("ls", list_subscriptions());
    b.op("rs", replication_status());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    for alias in ["ls", "rs"] {
        let value = result_value(&resp, alias);
        let entry = &value
            .get("subscriptions")
            .and_then(|v| v.as_array())
            .unwrap()[0];
        assert_eq!(
            entry.get("state").and_then(|v| v.as_str()),
            Some("resync_required")
        );
    }
}

/// S5c-X 2b: publishing an EPHEMERAL backend (in-memory repo, no persistent
/// WAL) requires an explicit `replication_serve: completed_reseed` durability
/// override. A plain in-memory repo + publication fails with the typed code
/// `ephemeral_backend_requires_completed_reseed` and leaves NO publication
/// row (the check runs before any store write); a second in-memory repo with
/// the explicit override publishes fine.
#[tokio::test]
async fn publication_over_ephemeral_backend_requires_completed_reseed() {
    use shamir_query_builder::ddl::create_repo;
    use shamir_query_types::durability::{DurabilityOverride, ReplicationServe};

    let shamir = setup().await;
    // A fn item (not a closure) so the returned `&str` borrows from the argument.
    fn err_code(error: &crate::query::batch::BatchError) -> Option<&str> {
        match error {
            crate::query::batch::BatchError::QueryError { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    // Plain in-memory repo under the default durable policy.
    let mut b = Batch::new();
    b.id(1);
    b.op("cr", create_repo("r1"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Publication covering it → typed refusal.
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "cp",
        publication("pub_x").scope(repl_scope("testdb").repo("r1").build()),
    );
    let error = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err_code(&error),
        Some("ephemeral_backend_requires_completed_reseed"),
        "error: {error}"
    );
    assert!(error.to_string().contains("testdb/r1"), "{error}");
    assert!(
        error.to_string().contains("completed_reseed"),
        "message must name the required explicit value: {error}"
    );

    // No partial write: the catalogue has no publication row.
    let mut b = Batch::new();
    b.id(3);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let pubs = result_value(&resp, "lp");
    assert!(pubs
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());

    // Second in-memory repo with the EXPLICIT completed_reseed override.
    let mut b = Batch::new();
    b.id(4);
    b.op(
        "cr",
        create_repo("r2").durability(DurabilityOverride {
            replication_serve: Some(ReplicationServe::CompletedReseed),
            ..Default::default()
        }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(5);
    b.op(
        "cp",
        publication("pub_y").scope(repl_scope("testdb").repo("r2").build()),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("explicit completed_reseed override must allow publication");

    let mut b = Batch::new();
    b.id(6);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let pubs = result_value(&resp, "lp")
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .clone();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].get("name").and_then(|v| v.as_str()), Some("pub_y"));
}

/// S5c-X 2e: `has_resync_required_subscription_covering` — a subscription in
/// `state = "resync_required"` whose profile has a Pull stream over the repo
/// makes the predicate true for the covered (db, repo) and false for an
/// uncovered one; with no subscriptions at all it is false (the leader serve
/// path must NOT refuse subscription-free nodes).
#[tokio::test]
async fn resync_required_subscription_covering_predicate() {
    let shamir = setup().await;

    // No subscriptions → false for anything (regression guard).
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("cluster").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "cluster"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // An ACTIVE subscription does not block serving.
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());

    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();

    // Covered repo → true; uncovered repo/db → false.
    assert!(shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "other")
        .await
        .unwrap());
    assert!(!shamir
        .has_resync_required_subscription_covering("otherdb", "main")
        .await
        .unwrap());
}

/// Read one field of the stored subscription record from the system store.
async fn sub_field(shamir: &ShamirDb, name: &str, field: &str) -> Option<String> {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let query = crate::query::read::ReadQuery::new("subscriptions").filter(
        crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String(name.to_string()),
        },
    );
    let result = table.read(&query, &ctx).await.unwrap();
    result.records.first().and_then(|r| {
        r.as_value()
            .get(field)
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

/// S5c-X PR8b-2 (evidence rows 3 + 5, recreate-profile leg; round 2 C11
/// reorder): the REAL recreate-after-delete flow — once a row is bound, the
/// DURABLE binding outlives everything: deleting `profile_a`, recreating it
/// with DIFFERENT scopes (which now succeeds — no profile row exists to
/// guard; the upsert-mismatch refusal while the profile is still referenced
/// lives in `binding_lifecycle_tests::binding_profile_replacement_…`), and
/// recreating it with the SAME scopes never lifts the quarantine; the row
/// keeps its ORIGINAL binding and the original destinations stay blocked.
#[tokio::test]
async fn recreating_profile_with_different_scopes_keeps_original_binding() {
    let shamir = setup().await;
    let instance = shamir.get_db("testdb").unwrap();
    for repo in ["main", "side"] {
        instance
            .add_repo(
                RepoConfig::new(repo, BoxRepoFactory::in_memory())
                    .add_table(crate::engine::table::TableConfig::new("users")),
            )
            .await
            .unwrap();
    }

    // Valid durable binding: profile_a over testdb/main, sub_a active.
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("profile_a").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();

    // S5c-X round 2 (C11): the REAL recreate-after-delete flow. DELETE the
    // referenced profile FIRST — no profile row exists to guard anymore, so
    // recreating it with DIFFERENT scopes SUCCEEDS now.
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "drp",
        drop_replication_profile("profile_a")
            .hmac("dummy")
            .build()
            .expect("drop replication profile builder"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op(
        "crp2",
        replication_profile("profile_a").stream(
            repl_scope("testdb").repo("side").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("recreating an UNREFERENCED profile with new scopes must succeed");

    // The row keeps its ORIGINAL binding: testdb/main only.
    let row = sub_row(&shamir, "sub_a").await;
    let destinations = row
        .get("binding_destinations")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(destinations.len(), 1);
    assert_eq!(
        destinations[0].get("repo").and_then(|v| v.as_str()),
        Some("main")
    );

    // The ORIGINAL destination stays blocked (the binding outlived both the
    // profile and its different-scopes recreation), and the proposed new
    // scope is NOT quarantined.
    let outcome = shamir.read_changelog_bounded("testdb", "main", 0, 10).await;
    assert!(
        outcome.is_err(),
        "original bound destination must stay quarantined"
    );
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("original bound destination must stay quarantined after reopen"),
    };
    assert_eq!(error.code(), Some("resync_required"));
    // `side` was never bound: writes AND reads work with actual data.
    let mut b = Batch::new();
    b.id(4);
    b.insert(
        "ins",
        shamir_query_builder::write::Insert::with_repo("side", "users")
            .row(doc! { "name" => "Side", "age" => 2 }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(5);
    b.query("r", Query::with_repo("side", "users"));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(served.get("name").and_then(|v| v.as_str()), Some("Side"));

    // Drop again, then recreate with the SAME scopes: the original gate
    // STILL holds (same-scopes recreation never lifts a quarantine).
    let mut b = Batch::new();
    b.id(6);
    b.op(
        "drp2",
        drop_replication_profile("profile_a")
            .hmac("dummy")
            .build()
            .expect("drop replication profile builder"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(7);
    b.op(
        "same",
        replication_profile("profile_a").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let outcome = shamir.read_changelog_bounded("testdb", "main", 0, 10).await;
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("same-scopes recreation must keep the quarantine gate"),
    };
    assert_eq!(error.code(), Some("resync_required"));
}

/// Raw subscription-row read (status/list only project a subset of fields).
async fn sub_row(shamir: &ShamirDb, name: &str) -> QueryValue {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    table
        .read(
            &crate::query::read::ReadQuery::new("subscriptions").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String(name.to_string()),
                },
            ),
            &ctx,
        )
        .await
        .unwrap()
        .records
        .remove(0)
        .as_value()
        .into_owned()
}
