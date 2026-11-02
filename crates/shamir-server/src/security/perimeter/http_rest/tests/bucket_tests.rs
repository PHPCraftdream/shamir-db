use super::{BucketCache, BucketError};
use shamir_connect::server::token::{TokenChannels, TokenMode, TokenRecord, TokenState, Verified};
use std::sync::Arc;

use shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC;

const TEST_NOW: u64 = 10;

fn grant(id: u16) -> Verified {
    let mut token_id = [0; 16];
    token_id[14..].copy_from_slice(&id.to_be_bytes());
    let record = TokenRecord {
        id: token_id,
        owner_user_id: [1; 16],
        label: "bucket test".into(),
        mac: [0; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: u64::MAX,
        mode: TokenMode::ReadWrite,
        channels: TokenChannels::Rest,
        databases: None,
        roles: Vec::new(),
        revoked_at_ns: None,
    };
    Verified {
        state: Arc::new(TokenState::new(record)),
    }
}

fn failing_sid() -> Result<[u8; 32], ()> {
    Err(())
}

fn zero_sid() -> Result<[u8; 32], ()> {
    Ok([0; 32])
}

#[test]
fn existing_cached_token_works_when_capacity_is_full() {
    let cache = BucketCache::with_cap(1);
    let first = grant(1);
    assert!(cache.session_for(&first, "owner".into(), TEST_NOW).is_ok());
    assert!(cache.session_for(&first, "owner".into(), TEST_NOW).is_ok());
    assert!(matches!(
        cache.session_for(&grant(2), "owner".into(), TEST_NOW),
        Err(BucketError::Unavailable)
    ));
}

#[test]
fn idle_bucket_is_evicted_to_admit_new_token() {
    let cache = BucketCache::with_cap(1);
    assert!(cache
        .session_for(&grant(1), "owner".into(), TEST_NOW)
        .is_ok());
    assert!(cache
        .session_for(&grant(2), "owner".into(), TEST_NOW + 1_000_000_001)
        .is_ok());
    for _ in 1..POST_AUTH_RATE_LIMIT_PER_SEC {
        assert!(cache
            .session_for(&grant(2), "owner".into(), TEST_NOW + 1_000_000_002)
            .is_ok());
    }
    assert!(matches!(
        cache.session_for(&grant(2), "owner".into(), TEST_NOW + 1_000_000_002),
        Err(BucketError::RateLimited(_))
    ));
    assert_eq!(cache.entries.load(std::sync::atomic::Ordering::Acquire), 1);
    assert!(cache
        .buckets
        .contains_key(&shamir_connect::server::token::TokenId({
            let mut id = [0; 16];
            id[14..].copy_from_slice(&2u16.to_be_bytes());
            id
        })));
    assert!(!cache
        .buckets
        .contains_key(&shamir_connect::server::token::TokenId({
            let mut id = [0; 16];
            id[14..].copy_from_slice(&1u16.to_be_bytes());
            id
        })));
}

#[test]
fn full_cache_evicts_idle_entry_and_admits_new_token_without_growth() {
    let cache = BucketCache::with_cap(4096);
    for id in 1..=4096u16 {
        assert!(cache
            .session_for(&grant(id), "owner".into(), TEST_NOW)
            .is_ok());
    }
    assert_eq!(
        cache.entries.load(std::sync::atomic::Ordering::Acquire),
        4096
    );
    assert!(cache
        .session_for(&grant(4097), "owner".into(), TEST_NOW + 1_000_000_001)
        .is_ok());
    assert_eq!(cache.entries.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(cache.buckets.len(), 1);
}

#[test]
fn entropy_failure_and_zero_sid_are_unavailable() {
    let mut cache = BucketCache::with_cap(2);
    cache.set_sid_generator(failing_sid);
    assert!(matches!(
        cache.session_for(&grant(1), "owner".into(), TEST_NOW),
        Err(BucketError::Unavailable)
    ));
    cache.set_sid_generator(zero_sid);
    assert!(matches!(
        cache.session_for(&grant(2), "owner".into(), TEST_NOW),
        Err(BucketError::Unavailable)
    ));
}

#[test]
fn actual_post_auth_budget_returns_rate_limited() {
    let cache = BucketCache::with_cap(2);
    let token = grant(3);
    let mut saw_rate_limited = false;
    for _ in 0..=POST_AUTH_RATE_LIMIT_PER_SEC {
        match cache.session_for(&token, "owner".into(), TEST_NOW) {
            Err(BucketError::RateLimited(_)) => {
                saw_rate_limited = true;
                break;
            }
            Err(BucketError::Unavailable) => panic!("session bucket unavailable"),
            Ok(_) => {}
        }
    }
    assert!(
        saw_rate_limited,
        "post-auth budget did not rate-limit within burst"
    );
}

#[test]
fn parallel_distinct_ids_never_exceed_capacity() {
    let cache = Arc::new(BucketCache::with_cap(8));
    let workers: Vec<_> = (1..=32)
        .map(|id| {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                let token = grant(id);
                let _ = cache.session_for(&token, "owner".into(), TEST_NOW);
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("worker");
    }
    assert!(cache.entries.load(std::sync::atomic::Ordering::Acquire) <= cache.cap);
    assert!(cache.buckets.len() <= cache.cap);
}
