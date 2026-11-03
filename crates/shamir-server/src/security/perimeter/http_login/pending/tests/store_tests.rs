use super::super::store::{PendingEntry, PendingError, PendingId, PendingStore};
use crate::http_login::admission::{LoginClock, Source};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use zeroize::Zeroizing;

struct Clock {
    now: Instant,
    seconds: AtomicU64,
}
impl LoginClock for Clock {
    fn now(&self) -> Instant {
        self.now + std::time::Duration::from_secs(self.seconds.load(Ordering::SeqCst))
    }
}
fn fixture() -> (Arc<Clock>, PendingStore) {
    let clock = Arc::new(Clock {
        now: Instant::now(),
        seconds: AtomicU64::new(0),
    });
    let store = PendingStore::with_selector(clock.clone(), |_| Some(0));
    (clock, store)
}
fn source(address: [u8; 4]) -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::from(address)))
}
fn entry(id: PendingId, address: [u8; 4]) -> PendingEntry {
    let listener_id = [3; 16];
    let scope = [0, 1, b'x', 0, 0, 0xff];
    let transcript = shamir_connect::server::http_login::build_transcript(
        &shamir_connect::server::http_login::HttpLoginTranscriptInput {
            protocol_version: 1,
            binding_mode: 3,
            security: 1,
            listener_id: &listener_id,
            handshake_id: &id.0,
            user: "a",
            client_nonce: &[1; 32],
            server_nonce: &[2; 32],
            salt: &[4; 16],
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 19,
            scope_bytes: &scope,
        },
    )
    .unwrap();
    PendingEntry {
        transcript: Zeroizing::new(transcript),
        user_id: None,
        stored_key: Zeroizing::new([1; 32]),
        server_key: Zeroizing::new([2; 32]),
        tickets_invalid_before_ns: 0,
        begin_time_ns: 0,
        eligible: true,
        listener_id,
        security_class: 1,
        source: source(address),
    }
}
fn id(number: u16) -> PendingId {
    let mut bytes = [0; 32];
    bytes[..2].copy_from_slice(&number.to_be_bytes());
    PendingId(bytes)
}
fn entry_v6(key: PendingId, source: Source) -> PendingEntry {
    let mut item = entry(key, [10, 2, 3, 4]);
    item.source = source;
    item
}

#[tokio::test]
async fn reclaim_released_work_is_proportional_to_release_count_at_capacity() {
    let (_, store) = fixture();
    let mut claims = Vec::with_capacity(1_024);
    for number in 0..1_024u16 {
        let key = id(number);
        let address = [10, (number / 16) as u8, (number % 16) as u8, 1];
        store.insert(key, entry(key, address)).await.unwrap();
        claims.push(store.claim(key, source(address), [3; 16], 1).await.unwrap());
    }
    for claim in claims.drain(..512) {
        drop(claim);
    }
    store.reset_reclaim_visits().await;
    let extra = id(2_000);
    store
        .insert(extra, entry(extra, [11, 2, 3, 4]))
        .await
        .unwrap();
    assert_eq!(store.count().await, 513);
    assert_eq!(store.reclaim_visits().await, 512);
    for claim in claims.drain(..) {
        drop(claim);
    }
    store.reset_reclaim_visits().await;
    let reused = id(2_001);
    store
        .insert(reused, entry(reused, [11, 2, 4, 4]))
        .await
        .unwrap();
    assert_eq!(store.count().await, 2);
    assert_eq!(store.reclaim_visits().await, 512);
    assert!(store.contains_id(extra).await);
    assert!(store.contains_id(reused).await);
}

#[tokio::test]
async fn pending_id_debug_is_redacted_and_live_collision_is_atomic() {
    let (_, store) = fixture();
    let key = id(44);
    let first = entry(key, [10, 2, 3, 4]);
    assert_eq!(format!("{key:?}"), "PendingId(<redacted>)");
    store.insert(key, first).await.unwrap();

    let collision = store.insert(key, entry(key, [10, 2, 3, 5])).await;
    assert_eq!(collision, Err(PendingError::Collision));
    assert_eq!(store.count().await, 1);
    assert!(store.contains_id(key).await);
    let mut claim = store
        .claim(key, source([10, 2, 3, 4]), [3; 16], 1)
        .await
        .unwrap();
    assert_eq!(
        claim.take().map(|value| value.source),
        Some(source([10, 2, 3, 4]))
    );
    drop(claim);
}

#[tokio::test]
async fn claimed_entries_release_capacity_for_repeated_identifier_reuse() {
    let (_, store) = fixture();
    let key = id(700);
    for _ in 0..1_025 {
        store.insert(key, entry(key, [10, 2, 3, 4])).await.unwrap();
        let claim = store
            .claim(key, source([10, 2, 3, 4]), [3; 16], 1)
            .await
            .unwrap();
        drop(claim);
    }
    assert_eq!(store.count().await, 0);
}

#[tokio::test]
async fn ordered_expiry_keeps_future_entries_and_removes_elapsed_entries() {
    let (clock, store) = fixture();
    store
        .insert(id(51), entry(id(51), [10, 2, 3, 4]))
        .await
        .unwrap();
    clock.seconds.store(30, Ordering::SeqCst);
    store
        .insert(id(52), entry(id(52), [10, 2, 4, 4]))
        .await
        .unwrap();
    clock.seconds.store(60, Ordering::SeqCst);

    assert_eq!(store.count().await, 1);
    assert!(!store.contains_id(id(51)).await);
    assert!(store.contains_id(id(52)).await);
}

#[tokio::test]
async fn transcript_payload_accounting_and_ttl_claim_lifecycle() {
    let item = entry(id(1), [10, 2, 3, 4]);
    assert_eq!(item.transcript.len(), 173);
    let nominal_entry_bytes = std::mem::size_of::<PendingEntry>() + item.transcript.len();
    assert!(nominal_entry_bytes <= std::mem::size_of::<PendingEntry>() + 933);
    let (clock, store) = fixture();
    assert!(item.validate().is_ok());
    store.insert(id(1), item).await.unwrap();
    assert!(matches!(
        store.claim(id(1), source([10, 2, 4, 5]), [3; 16], 1).await,
        Err(PendingError::WrongScope)
    ));
    assert_eq!(store.count().await, 1);
    assert!(matches!(
        store.claim(id(1), source([10, 2, 3, 5]), [4; 16], 1).await,
        Err(PendingError::WrongScope)
    ));
    assert!(matches!(
        store.claim(id(1), source([10, 2, 3, 5]), [3; 16], 2).await,
        Err(PendingError::WrongScope)
    ));
    let claim = store
        .claim(id(1), source([10, 2, 3, 5]), [3; 16], 1)
        .await
        .unwrap();
    assert_eq!(store.count().await, 1);
    clock.seconds.store(60, Ordering::SeqCst);
    assert_eq!(store.count().await, 1);
    drop(claim);
    assert_eq!(store.count().await, 0);
    store
        .insert(id(2), entry(id(2), [10, 2, 3, 4]))
        .await
        .unwrap();
    clock.seconds.store(120, Ordering::SeqCst);
    assert!(matches!(
        store.claim(id(2), source([10, 2, 3, 4]), [3; 16], 1).await,
        Err(PendingError::NotFound)
    ));
}

#[tokio::test]
async fn ipv4_prefix_cap_rejects_seventeenth_entry() {
    let (_, store) = fixture();
    for number in 0..16u16 {
        let key = id(number);
        store
            .insert(key, entry(key, [10, 2, 3, number as u8]))
            .await
            .unwrap();
    }
    assert_eq!(store.count().await, 16);
    let key = id(16);
    assert_eq!(
        store
            .insert(key, entry(key, [10, 2, 3, 16]))
            .await
            .unwrap_err(),
        PendingError::Full
    );
    assert_eq!(store.count().await, 16);
}

#[tokio::test]
async fn ipv6_prefix_caps_use_actual_64_and_56_bits() {
    let (_, store) = fixture();
    for number in 0..16u16 {
        let key = id(number);
        let address = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 0x1200, 0, 0, 0, number));
        store
            .insert(key, entry_v6(key, Source::from_ip(address)))
            .await
            .unwrap();
    }
    let key = id(16);
    let address = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 0x1200, 0, 0, 0, 16));
    assert_eq!(
        store
            .insert(key, entry_v6(key, Source::from_ip(address)))
            .await
            .unwrap_err(),
        PendingError::Full
    );

    let (_, broad_store) = fixture();
    for number in 0..64u16 {
        let key = id(number);
        let address = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 0x1200 + number, 0, 0, 0, 1));
        broad_store
            .insert(key, entry_v6(key, Source::from_ip(address)))
            .await
            .unwrap();
    }
    let key = id(64);
    let address = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 0x1200 + 64, 0, 0, 0, 1));
    assert_eq!(
        broad_store
            .insert(key, entry_v6(key, Source::from_ip(address)))
            .await
            .unwrap_err(),
        PendingError::Full
    );
}

#[tokio::test]
async fn global_full_evicts_within_fair_share_and_claimed_entries_are_held() {
    let (_, store) = fixture();
    for principal in 0..64u8 {
        for base in 0..16u8 {
            let number = u16::from(principal) * 16 + u16::from(base);
            store
                .insert(id(number), entry(id(number), [10, principal, base, 1]))
                .await
                .unwrap();
        }
    }
    let first = store
        .claim(id(0), source([10, 0, 0, 1]), [3; 16], 1)
        .await
        .unwrap();
    store
        .insert(id(1024), entry(id(1024), [11, 0, 0, 1]))
        .await
        .unwrap();
    assert_eq!(store.count().await, 1024);
    assert!(store.contains_id(id(0)).await);
    assert!(!store.contains_id(id(1)).await);
    assert!(store.contains_id(id(1024)).await);
    drop(first);
    assert_eq!(store.count().await, 1023);
}

#[test]
fn nominal_payload_budget_arithmetic_is_exact() {
    assert!(std::mem::size_of::<PendingEntry>() + 933 <= 1_536);
    assert_eq!(
        1_024usize * 2_048
            + 2 * 4_096 * 128
            + 2 * 1_024 * 64
            + 5 * 1_024 * 64
            + 65_536
            + 8_192 * 128,
        4_718_592
    );
}

#[tokio::test]
async fn claimed_capacity_cannot_be_evicted() {
    let (_, store) = fixture();
    let mut claims = Vec::with_capacity(1024);
    for n in 0..1024usize {
        let principal = (n / 16) as u8;
        let host = (n % 16) as u8;
        let key = id(n as u16);
        let mut value = key.0;
        value[0] = (n >> 8) as u8;
        value[1] = n as u8;
        let key = PendingId(value);
        store
            .insert(key, entry(key, [10, principal, host, 1]))
            .await
            .unwrap();
        claims.push(
            store
                .claim(key, source([10, principal, host, 1]), [3; 16], 1)
                .await
                .unwrap(),
        );
    }
    let mut new_id = [255; 32];
    new_id[0] = 3;
    assert_eq!(
        store
            .insert(PendingId(new_id), entry(PendingId(new_id), [11, 1, 1, 1]))
            .await
            .unwrap_err(),
        PendingError::Full
    );
    assert_eq!(store.count().await, 1024);
}

#[tokio::test]
async fn entropy_failure_at_capacity_does_not_evict() {
    let store = PendingStore::with_selector(
        Arc::new(Clock {
            now: Instant::now(),
            seconds: AtomicU64::new(0),
        }),
        |_| None,
    );
    for principal in 0..64u8 {
        for base in 0..16u8 {
            let number = u16::from(principal) * 16 + u16::from(base);
            store
                .insert(id(number), entry(id(number), [10, principal, base, 1]))
                .await
                .unwrap();
        }
    }
    assert_eq!(
        store
            .insert(id(1024), entry(id(1024), [11, 0, 0, 1]))
            .await
            .unwrap_err(),
        PendingError::Entropy
    );
    assert_eq!(store.count().await, 1024);
    assert!(store.contains_id(id(0)).await);
    assert!(!store.contains_id(id(1024)).await);
}
