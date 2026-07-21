//! Tests for the leader-side replication pull-API handler
//! (REPLICATION §5, R0-b).
//!
//! Covers:
//! 1. `bad_role` — a session without `replicator` is denied.
//! 2. `Hello` with the role advertises repos the caller can read.
//! 3. `Pull` returns encoded changelog events after writes.
//! 4. `denied_repo` — a `replicator` session without a grant on the repo.
//! 5. Long-poll on an empty tail returns promptly (does not hang).
//! 6. `leader_epoch` is carried on every `ReplResponse`.
//!
//! Fixtures mirror `node_mode_tests.rs`: `alice` owns the `app/main` repo;
//! `bob` owns a separate repo used for the deny case.

use std::sync::Arc;
use std::time::Instant;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse, CURRENT_REPL_PROTO_VER};

use crate::db_handler::handler::ShamirDbHandler;

// ---------------------------------------------------------------------------
// Session fixtures
// ---------------------------------------------------------------------------

/// A plain session for `alice` with only `read_write` — NO `replicator` role.
fn alice_plain_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// `alice` with the `is_replicator` capability flag set (task #621 — no
/// longer a role string; still owns the `app/main` repo).
fn alice_replicator_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, true, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// `carol` with the `is_replicator` capability flag set but NO ownership of
/// any repo — used for the `denied_repo` case.
fn carol_replicator_session() -> Session {
    Session::new(
        [0xCD; 16],
        "carol".into(),
        SessionPermissions::new(false, true, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

// ---------------------------------------------------------------------------
// DB / handler fixture
// ---------------------------------------------------------------------------

/// Build a handler over an in-memory `ShamirDb`:
///   - `app` db (owned by `alice`)
///   - `app/main` repo with an `items` table (owned by `alice`)
///   - `app/secret` repo with a `docs` table (owned by `bob`)
///
/// `alice` can read `main` but not `secret`; `carol` can read neither.
async fn build_handler() -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let alice = Actor::User(principal64([0xAB; 16]));
    let bob = Actor::User(principal64_from_username("bob"));

    shamir.create_db_as("app", alice.clone()).await;

    // alice-owned repo.
    let cfg_main =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg_main, alice)
        .await
        .expect("add main repo");

    // bob-owned repo (alice / carol have no access).
    let cfg_secret =
        RepoConfig::new("secret", BoxRepoFactory::in_memory()).add_table(TableConfig::new("docs"));
    shamir
        .add_repo_as("app", cfg_secret, bob)
        .await
        .expect("add secret repo");

    ShamirDbHandler::new(Arc::new(shamir))
}

/// Write `n` rows into `app/main/items` as `alice` (the owner) so the
/// changelog has events to pull. Uses a transactional batch (one commit →
/// one changefeed event) via the direct `ShamirDb` API, mirroring
/// `changefeed_e2e.rs`. The journal writer is asynchronous, so we poll
/// until all `n` events are durable before returning.
async fn write_rows(handler: &ShamirDbHandler, n: usize) {
    use shamir_query_builder::write::insert;
    let db = handler.db();
    let owner = Actor::User(principal64([0xAB; 16]));
    for i in 0..n {
        let key_str = format!("k{i}");
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert("items").rows([doc! {
                "id" => key_str,
                "v" => i as i64,
            }]),
        );
        let resp = db
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write should succeed");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}",
        );
    }

    // The journal writer is async (channel + background persist); poll
    // until all n events are durable, matching the pattern in
    // `changefeed_e2e.rs::commit_without_subscribers_succeeds`.
    for _ in 0..100 {
        if let Some(jr) = db.read_changelog_from_journal("app", "main", 0, 1000).await {
            if jr.events.len() >= n {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("journal did not durable-land {n} events in time");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Deny without the `replicator` role → `bad_role`.
#[tokio::test]
async fn deny_without_replicator_role() {
    let handler = build_handler().await;
    let session = alice_plain_session();

    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: 1,
                node_id: "follower-1".into(),
            },
        )
        .await;

    match resp {
        ReplResponse::Error {
            leader_epoch,
            code,
            message,
        } => {
            assert_eq!(leader_epoch, 1, "default epoch is 1");
            assert_eq!(code, "bad_role", "wrong code; message: {message}");
            assert!(
                message.contains("replicator"),
                "message should mention the role: {message}",
            );
        }
        other => panic!("expected Error(bad_role), got: {other:?}"),
    }
}

/// 2. `Hello` with the `replicator` role advertises repos the caller can
/// read — `main` (owned by alice) but NOT `secret` (owned by bob).
#[tokio::test]
async fn hello_with_role_lists_accessible_repos() {
    let handler = build_handler().await;
    let session = alice_replicator_session();

    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: 1,
                node_id: "follower-1".into(),
            },
        )
        .await;

    let repos = match resp {
        ReplResponse::Hello {
            leader_epoch,
            repos,
        } => {
            assert_eq!(leader_epoch, 1);
            repos
        }
        other => panic!("expected Hello, got: {other:?}"),
    };

    // alice can read `main` but not `secret`.
    let main = repos
        .iter()
        .find(|r| r.repo == "main")
        .expect("main should be advertised");
    assert_eq!(main.db, "app");
    assert_eq!(main.journal_floor, 0);
    // No writes yet → current_version is 0 (or whatever the empty journal reports).
    assert!(
        repos.iter().all(|r| r.repo != "secret"),
        "secret repo must NOT be advertised to alice: {repos:?}",
    );
}

/// 3. `Pull` returns encoded changelog events after writes.
#[tokio::test]
async fn pull_returns_events_after_writes() {
    let handler = build_handler().await;
    write_rows(&handler, 3).await;

    let session = alice_replicator_session();
    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: 0,
                limit: 100,
                wait_ms: None,
            },
        )
        .await;

    let (events, current_version, gap_at) = match resp {
        ReplResponse::Pull {
            leader_epoch,
            events,
            gap_at,
            current_version,
        } => {
            assert_eq!(leader_epoch, 1);
            (events, current_version, gap_at)
        }
        other => panic!("expected Pull, got: {other:?}"),
    };

    // Decode the msgpack-encoded events.
    let decoded: Vec<shamir_db::engine::ChangelogEvent> =
        rmp_serde::from_slice(&events).expect("events should decode");
    assert!(!decoded.is_empty(), "pull should return at least one event",);
    assert_eq!(decoded[0].repo, "main");
    assert!(
        current_version > 0,
        "current_version should be > 0 after writes, got {current_version}",
    );
    // No gap on a fresh journal.
    assert!(gap_at.is_none(), "no gap expected: {gap_at:?}");
}

/// 4. `Pull` deny without a grant — `carol` has the `replicator` role but
/// does not own `app/main` → `denied_repo`.
#[tokio::test]
async fn pull_denied_without_grant() {
    let handler = build_handler().await;
    let session = carol_replicator_session();

    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: 0,
                limit: 100,
                wait_ms: None,
            },
        )
        .await;

    match resp {
        ReplResponse::Error {
            leader_epoch, code, ..
        } => {
            assert_eq!(leader_epoch, 1);
            assert_eq!(code, "denied_repo", "carol has no grant on main");
        }
        other => panic!("expected Error(denied_repo), got: {other:?}"),
    }
}

/// 5. Long-poll on an empty tail returns promptly — does not hang.
///    `wait_ms = Some(200)` on a repo with no new events should return
///    within ~1s with an empty events vec.
#[tokio::test]
async fn long_poll_empty_tail_does_not_hang() {
    let handler = build_handler().await;
    let session = alice_replicator_session();

    // from_version = u64::MAX → guaranteed empty tail (no events that high).
    let start = Instant::now();
    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: u64::MAX,
                limit: 100,
                wait_ms: Some(200),
            },
        )
        .await;
    let elapsed = start.elapsed();

    let events = match resp {
        ReplResponse::Pull {
            leader_epoch,
            events,
            ..
        } => {
            assert_eq!(leader_epoch, 1);
            events
        }
        other => panic!("expected Pull, got: {other:?}"),
    };

    // Decode to confirm emptiness.
    let decoded: Vec<shamir_db::engine::ChangelogEvent> =
        rmp_serde::from_slice(&events).expect("events should decode");
    assert!(
        decoded.is_empty(),
        "no events expected at u64::MAX from_version",
    );

    // The poll budget was 200ms; allow generous headroom for scheduler
    // jitter but flag a real hang (> 5s).
    assert!(
        elapsed.as_secs() < 5,
        "long-poll took too long: {:?} (budget 200ms)",
        elapsed,
    );
}

/// 6. `leader_epoch` is carried on every `ReplResponse`. Handler built with
///    `with_leader_epoch(7)` → every reply carries `leader_epoch == 7`.
#[tokio::test]
async fn leader_epoch_carried_on_all_responses() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo_as("app", cfg, owner).await.unwrap();
    let handler = ShamirDbHandler::new(Arc::new(shamir)).with_leader_epoch(7);

    let session = alice_replicator_session();

    // Hello carries epoch 7.
    let hello = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: 1,
                node_id: "f".into(),
            },
        )
        .await;
    match hello {
        ReplResponse::Hello { leader_epoch, .. } => assert_eq!(leader_epoch, 7),
        other => panic!("expected Hello, got: {other:?}"),
    }

    // Pull carries epoch 7.
    let pull = handler
        .handle_repl(
            &session,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: 0,
                limit: 10,
                wait_ms: None,
            },
        )
        .await;
    match pull {
        ReplResponse::Pull { leader_epoch, .. } => assert_eq!(leader_epoch, 7),
        other => panic!("expected Pull, got: {other:?}"),
    }

    // Error carries epoch 7 too (carol has no grant).
    let carol = carol_replicator_session();
    let err = handler
        .handle_repl(
            &carol,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: 0,
                limit: 10,
                wait_ms: None,
            },
        )
        .await;
    match err {
        ReplResponse::Error { leader_epoch, .. } => assert_eq!(leader_epoch, 7),
        other => panic!("expected Error, got: {other:?}"),
    }
}

/// 7. `proto_ver` upper-bound rejection: a `Hello` advertising
/// `CURRENT_REPL_PROTO_VER + 1` (an unrecognized, newer protocol) is
/// rejected with `proto_ver_unsupported`; a `Hello` at (or below) the
/// current version still succeeds normally.
#[tokio::test]
async fn proto_ver_upper_bound_reject_and_accept() {
    let handler = build_handler().await;
    let session = alice_replicator_session();

    // A newer, unrecognized proto_ver is rejected.
    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: CURRENT_REPL_PROTO_VER + 1,
                node_id: "follower-1".into(),
            },
        )
        .await;
    match resp {
        ReplResponse::Error {
            leader_epoch,
            code,
            message,
        } => {
            assert_eq!(leader_epoch, 1, "default epoch is 1");
            assert_eq!(
                code, "proto_ver_unsupported",
                "wrong code; message: {message}"
            );
        }
        other => panic!("expected Error(proto_ver_unsupported), got: {other:?}"),
    }

    // The current proto_ver still succeeds normally.
    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: CURRENT_REPL_PROTO_VER,
                node_id: "follower-1".into(),
            },
        )
        .await;
    match resp {
        ReplResponse::Hello { leader_epoch, .. } => assert_eq!(leader_epoch, 1),
        other => panic!("expected Hello, got: {other:?}"),
    }

    // An OLDER proto_ver (e.g. 0) is also accepted (forward-compat).
    let resp = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: 0,
                node_id: "follower-1".into(),
            },
        )
        .await;
    match resp {
        ReplResponse::Hello { leader_epoch, .. } => assert_eq!(leader_epoch, 1),
        other => panic!("expected Hello, got: {other:?}"),
    }
}
