//! 386-a: server-side execution of replication DDL — round-trip CRUD + list
//! against an in-memory `ShamirDb`. Queries are built ONLY through the
//! `shamir_query_builder::ddl::replication::*` builders (builder-only rule).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_publication, drop_subscription, list_publications, list_subscriptions,
    publication, repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::types::value::QueryValue;
use crate::ShamirDb;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // `execute` resolves the target db before dispatching admin ops; the
    // repl catalogue itself lives in the `system` repo, but the call needs a
    // valid db context.
    shamir.create_db("testdb").await;
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
    b.op("alt", alter_subscription("sub_a").pause());
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
    b.op("alt", alter_subscription("sub_a").resume());
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
    b.op("alt", alter_subscription("sub_a").set_profile("profile_b"));
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
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("dp", drop_publication("pub_a"));
    b.op("ds", drop_subscription("sub_a"));
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
    let interner = table.interner().get().await.unwrap();
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
/// `state` (no new endpoint was added). The existing `Resume` admin action
/// still recovers it back to `"active"`.
#[tokio::test]
async fn resync_required_round_trips_through_status_and_list_then_resume_recovers() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
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

    // The existing `Resume` admin action still recovers it back to `active`
    // — no new admin action is needed for this new state value.
    let mut b = Batch::new();
    b.id(4);
    b.op("alt", alter_subscription("sub_a").resume());
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("active".into())
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

/// Read one field of the stored subscription record from the system store.
async fn sub_field(shamir: &ShamirDb, name: &str, field: &str) -> Option<String> {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner().get().await.unwrap();
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
