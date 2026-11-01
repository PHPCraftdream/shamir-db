use crate::common::types::{BindingMode, TransportKind};
use crate::server::auth::token::TokenId;
use crate::server::auth::token::Verified;
use crate::server::state::origin::SessionOrigin;
use crate::server::state::session::{Session, SessionPermissions};
use crate::server::state::token_session::{TokenAdmitReject, TokenSocketCaps};
use crate::server::token::{TokenChannels, TokenMode, TokenState};
use crate::server::TokenSessionStore;
use std::sync::{Arc, Barrier};
fn session_for(id: u8, owner: [u8; 16], roles: Vec<String>, expires: u64) -> Session {
    let record = crate::server::token::TokenRecord {
        id: [id; 16],
        owner_user_id: owner,
        label: "label".into(),
        mac: [0; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: expires,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Ws,
        databases: Some(vec!["db".into()]),
        roles,
        revoked_at_ns: None,
    };
    let state = Arc::new(TokenState::new(record));
    Session::for_token(&Verified { state }, "fixture-owner".into(), 2, [9; 32])
}
fn session(id: u8, owner: [u8; 16]) -> Session {
    session_for(id, owner, vec![], 100)
}
#[test]
fn origin_preserves_scope_principal_filler_fields_and_live_state() {
    let roles = vec!["superuser".into(), "replicator".into()];
    let token = session_for(21, [6; 16], roles.clone(), 100);
    assert_eq!(token.session_id, [9; 32]);
    assert_eq!(token.created_at_ns, 2);
    assert_eq!(token.user_id, [6; 16]);
    assert_eq!(token.username, "fixture-owner");
    assert_eq!(
        token
            .last_activity_ns
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(token.permissions.roles, roles);
    assert!(!token.permissions.is_superuser);
    assert!(!token.permissions.is_replicator);
    assert_eq!(token.transport_kind, TransportKind::WebSocket);
    assert_eq!(token.binding_mode, BindingMode::None);
    assert_eq!(token.channel_binding_at_auth, [0; 32]);
    let origin = match &token.origin {
        SessionOrigin::Token(origin) => origin,
        _ => panic!("token session must retain token origin"),
    };
    assert_eq!(origin.token_id(), TokenId([21; 16]));
    let (mode, channels, databases) = origin.scope();
    assert_eq!(mode, TokenMode::ReadOnly);
    assert_eq!(channels, TokenChannels::Ws);
    assert_eq!(databases, Some(&["db".to_owned()][..]));
    assert!(origin.is_live(99));
    assert!(!origin.is_live(100));
    let state = Arc::clone(&origin.state);
    state.revoke(33);
    assert!(!origin.is_live(1));
    assert_eq!(state.revoked_at_ns(), Some(33));
    let scram = Session::new(
        [6; 16],
        "fixture-owner".into(),
        SessionPermissions::new(false, false, vec![]),
        TransportKind::WebSocket,
        BindingMode::None,
        [0; 32],
        2,
    );
    assert!(matches!(scram.origin, SessionOrigin::Scram));
}

#[test]
fn caps_reject_zero_and_cleanup_allows_readmission() {
    let store = TokenSessionStore::new();
    assert!(matches!(
        store.admit(
            [1; 32],
            session(1, [4; 16]),
            TokenSocketCaps {
                per_token: 0,
                per_owner: 3
            }
        ),
        Err(TokenAdmitReject::PerTokenCap)
    ));
    assert_eq!(store.count_for_token(&TokenId([1; 16])), 0);
    assert!(store
        .admit(
            [1; 32],
            session(1, [4; 16]),
            TokenSocketCaps {
                per_token: 1,
                per_owner: 1
            }
        )
        .is_ok());
    assert!(store.remove(&[1; 32]));
    assert!(!store.remove(&[1; 32]));
    assert!(store.get(&[1; 32]).is_none());
    assert!(store.sessions_of_token(&TokenId([1; 16])).is_empty());
    assert_eq!(store.count_for_token(&TokenId([1; 16])), 0);
    assert_eq!(store.count_for_owner(&[4; 16]), 0);
    assert!(store
        .admit(
            [2; 32],
            session(1, [4; 16]),
            TokenSocketCaps {
                per_token: 1,
                per_owner: 1
            }
        )
        .is_ok());
}
#[test]
fn cross_token_owner_cap_rolls_back_and_zero_owner_cap_rejects() {
    let store = TokenSessionStore::new();
    let caps = TokenSocketCaps {
        per_token: 2,
        per_owner: 1,
    };
    assert!(store.admit([11; 32], session(11, [8; 16]), caps).is_ok());
    assert!(matches!(
        store.admit([12; 32], session(12, [8; 16]), caps),
        Err(TokenAdmitReject::PerOwnerCap)
    ));
    assert_eq!(store.count_for_token(&TokenId([12; 16])), 0);
    assert_eq!(store.count_for_owner(&[8; 16]), 1);
    assert!(matches!(
        store.admit(
            [13; 32],
            session(13, [9; 16]),
            TokenSocketCaps {
                per_token: 1,
                per_owner: 0
            }
        ),
        Err(TokenAdmitReject::PerOwnerCap)
    ));
    assert_eq!(store.count_for_token(&TokenId([13; 16])), 0);
}
#[test]
fn duplicate_sid_different_owner_rolls_back_both_counters() {
    let store = TokenSessionStore::new();
    let sid = [22; 32];
    assert!(store
        .admit(
            sid,
            session(22, [1; 16]),
            TokenSocketCaps {
                per_token: 2,
                per_owner: 2
            }
        )
        .is_ok());
    assert!(matches!(
        store.admit(
            sid,
            session_for(23, [2; 16], vec![], 100),
            TokenSocketCaps {
                per_token: 2,
                per_owner: 2
            }
        ),
        Err(TokenAdmitReject::DuplicateSid)
    ));
    assert_eq!(store.count_for_token(&TokenId([23; 16])), 0);
    assert_eq!(store.count_for_owner(&[2; 16]), 0);
    assert_eq!(store.sessions_of_token(&TokenId([22; 16])).len(), 1);
}
#[test]
fn duplicate_sid_rolls_back_and_does_not_disturb_existing_fanout() {
    let store = TokenSessionStore::new();
    let caps = TokenSocketCaps {
        per_token: 3,
        per_owner: 3,
    };
    let sid = [1; 32];
    let first = store.admit(sid, session(2, [4; 16]), caps).unwrap();
    assert!(matches!(
        store.admit(sid, session(2, [4; 16]), caps),
        Err(TokenAdmitReject::DuplicateSid)
    ));
    assert_eq!(store.count_for_token(&TokenId([2; 16])), 1);
    assert_eq!(store.count_for_owner(&[4; 16]), 1);
    assert!(Arc::ptr_eq(&first, &store.get(&sid).unwrap()));
    assert_eq!(store.sessions_of_token(&TokenId([2; 16])).len(), 1);
}
#[test]
fn barrier_concurrent_admissions_count_successes_and_fanout_exactly() {
    let store = Arc::new(TokenSessionStore::new());
    let barrier = Arc::new(Barrier::new(9));
    let results: Vec<Result<[u8; 32], TokenAdmitReject>> = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for index in 0..8 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            workers.push(scope.spawn(move || {
                let sid = [u8::try_from(index + 1).expect("index fits"); 32];
                barrier.wait();
                store
                    .admit(
                        sid,
                        session(3, [5; 16]),
                        TokenSocketCaps {
                            per_token: 3,
                            per_owner: 8,
                        },
                    )
                    .map(|_| sid)
            }));
        }
        barrier.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.len(), 8);
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 3);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(TokenAdmitReject::PerTokenCap)))
            .count(),
        5
    );
    let admitted: Vec<[u8; 32]> = results.into_iter().filter_map(Result::ok).collect();
    assert_eq!(store.count_for_token(&TokenId([3; 16])), 3);
    assert_eq!(store.count_for_owner(&[5; 16]), 3);
    assert_eq!(
        store.sessions_of_token(&TokenId([3; 16])).len(),
        admitted.len()
    );
    for sid in &admitted {
        assert!(store.get(sid).is_some());
    }
    for sid in admitted {
        assert!(store.remove(&sid));
    }
    assert_eq!(store.count_for_token(&TokenId([3; 16])), 0);
    assert_eq!(store.count_for_owner(&[5; 16]), 0);
}
#[test]
fn barrier_owner_cap_spans_distinct_token_ids() {
    let store = Arc::new(TokenSessionStore::new());
    let barrier = Arc::new(Barrier::new(9));
    let results: Vec<Result<u8, TokenAdmitReject>> = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for index in 0..8 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            workers.push(scope.spawn(move || {
                let ix = u8::try_from(index + 1).expect("index fits");
                barrier.wait();
                store
                    .admit(
                        [ix; 32],
                        session(ix, [10; 16]),
                        TokenSocketCaps {
                            per_token: 8,
                            per_owner: 3,
                        },
                    )
                    .map(|_| ix)
            }));
        }
        barrier.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    assert_eq!(results.len(), 8);
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 3);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(TokenAdmitReject::PerOwnerCap)))
            .count(),
        5
    );
    let admitted: Vec<u8> = results.into_iter().filter_map(Result::ok).collect();
    assert_eq!(admitted.len(), 3);
    assert_eq!(store.count_for_owner(&[10; 16]), 3);
    let fanout: usize = admitted
        .iter()
        .map(|id| store.sessions_of_token(&TokenId([*id; 16])).len())
        .sum();
    assert_eq!(fanout, 3);
    for id in &admitted {
        assert!(store.get(&[*id; 32]).is_some());
    }
    for id in admitted {
        assert!(store.remove(&[id; 32]));
    }
    assert_eq!(store.count_for_owner(&[10; 16]), 0);
    assert_eq!(store.count_for_token(&TokenId([1; 16])), 0);
}
#[test]
fn token_admission_does_not_consume_scram_session_store() {
    let token_store = TokenSessionStore::new();
    let scram_store = crate::server::state::session::SessionStore::new();
    let admitted = token_store
        .admit(
            [6; 32],
            session(6, [7; 16]),
            TokenSocketCaps {
                per_token: 1,
                per_owner: 1,
            },
        )
        .unwrap();
    assert!(matches!(admitted.origin, SessionOrigin::Token(_)));
    let scram = Session::new(
        [1; 16],
        "scram-user".into(),
        SessionPermissions::new(false, false, vec![]),
        TransportKind::WebSocket,
        BindingMode::None,
        [0; 32],
        2,
    );
    assert!(matches!(
        token_store.admit(
            [7; 32],
            scram,
            TokenSocketCaps {
                per_token: 1,
                per_owner: 1
            }
        ),
        Err(TokenAdmitReject::NotTokenSession)
    ));
    assert_eq!(token_store.count_for_token(&TokenId([6; 16])), 1);
    assert_eq!(token_store.count_for_owner(&[7; 16]), 1);
    assert!(scram_store.lookup_at(&[6; 32], 2).is_none());
}

#[test]
fn counter_cells_are_reclaimed_after_rejection_removal_and_churn() {
    let store = TokenSessionStore::new();
    for id in 1..=64 {
        let owner = [id; 16];
        assert!(matches!(
            store.admit(
                [id; 32],
                session(id, owner),
                TokenSocketCaps {
                    per_token: 0,
                    per_owner: 1
                }
            ),
            Err(TokenAdmitReject::PerTokenCap)
        ));
        assert_eq!(store.counter_cells_hint(), (0, 0));
        let rejected_owner = [id.wrapping_add(64); 16];
        assert!(matches!(
            store.admit(
                [id; 32],
                session(id, rejected_owner),
                TokenSocketCaps {
                    per_token: 1,
                    per_owner: 0
                }
            ),
            Err(TokenAdmitReject::PerOwnerCap)
        ));
        assert_eq!(store.counter_cells_hint(), (0, 0));
        assert!(store
            .admit(
                [id; 32],
                session(id, owner),
                TokenSocketCaps {
                    per_token: 1,
                    per_owner: 1
                }
            )
            .is_ok());
        assert_eq!(store.counter_cells_hint(), (1, 1));
        assert!(store.remove(&[id; 32]));
        assert_eq!(store.counter_cells_hint(), (0, 0));
        assert!(!store.remove(&[id; 32]));
        assert_eq!(store.counter_cells_hint(), (0, 0));
        assert_eq!(store.count_for_token(&TokenId([rejected_owner[0]; 16])), 0);
        assert_eq!(store.count_for_owner(&rejected_owner), 0);
        assert_eq!(store.counter_cells_hint(), (0, 0));
    }
}

#[test]
fn scram_origin_is_rejected_by_token_session_store() {
    let store = TokenSessionStore::new();
    let scram = Session::new(
        [6; 16],
        "fixture-owner".into(),
        SessionPermissions::new(false, false, vec![]),
        TransportKind::WebSocket,
        BindingMode::None,
        [0; 32],
        2,
    );
    assert!(matches!(
        store.admit(
            [8; 32],
            scram,
            TokenSocketCaps {
                per_token: 1,
                per_owner: 1
            }
        ),
        Err(TokenAdmitReject::NotTokenSession)
    ));
    assert_eq!(store.count_for_token(&TokenId([0; 16])), 0);
    assert_eq!(store.count_for_owner(&[6; 16]), 0);
}
