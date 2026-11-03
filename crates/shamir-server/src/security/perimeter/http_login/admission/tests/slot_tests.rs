use super::super::{AdmissionError, AdmissionState, LoginClock, Source};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Instant;

struct FixedClock(Instant);
impl LoginClock for FixedClock {
    fn now(&self) -> Instant {
        self.0
    }
}

fn v4(a: u8, b: u8, c: u8, d: u8) -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::new(a, b, c, d)))
}

fn state() -> AdmissionState {
    AdmissionState::new(Arc::new(FixedClock(Instant::now())))
}

#[tokio::test]
async fn exhausted_credit_pin_budget_fails_closed_without_leaking_arrival_pin() {
    let state = state();
    let mut permits = Vec::with_capacity(1_024);
    for index in 0..1_024u16 {
        let source = Source::from_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, index, 0, 0, 0, 0, 1,
        )));
        let (arrival, credit) = state.pin_source(source).await.expect("initial pins");
        permits.push((arrival, credit));
    }
    assert!(matches!(
        state.pin_source(v4(11, 1, 1, 1)).await,
        Err(AdmissionError::ServerBusy)
    ));
    drop(permits);
    assert!(state.pin_source(v4(11, 1, 1, 1)).await.is_ok());
}

#[tokio::test]
async fn denied_slot_does_not_refund_admission_budget() {
    let state = state();
    let source = v4(10, 1, 2, 3);
    let mut permits = Vec::new();
    for _ in 0..16 {
        permits.push(state.admit_slot_for_test(source).await.expect("slot"));
    }
    assert!(matches!(
        state.admit(source).await,
        Err(AdmissionError::SourceSlots)
    ));
    drop(permits);
    for _ in 0..9 {
        assert!(state.admit(source).await.is_ok());
    }
    assert!(matches!(
        state.admit(source).await,
        Err(AdmissionError::RateLimited { .. })
    ));
}
#[tokio::test]
async fn base_prefix_caps_at_sixteen_live_slots() {
    let state = state();
    let source = v4(10, 1, 2, 3);
    let mut permits = Vec::new();
    for _ in 0..16 {
        permits.push(state.admit_slot_for_test(source).await.expect("slot"));
    }
    assert!(matches!(
        state.admit_slot_for_test(source).await,
        Err(AdmissionError::SourceSlots)
    ));
}

#[tokio::test]
async fn parent_prefix_allows_four_base_prefixes_then_caps_at_sixty_four() {
    let state = state();
    let mut permits = Vec::new();
    for base in 0..4 {
        for host in 1..=16 {
            permits.push(
                state
                    .admit_slot_for_test(v4(10, 1, base, host))
                    .await
                    .expect("slot"),
            );
        }
    }
    assert!(matches!(
        state.admit_slot_for_test(v4(10, 1, 4, 1)).await,
        Err(AdmissionError::SourceSlots)
    ));
}

#[tokio::test]
async fn global_capacity_rejects_after_sixteen_parents_reach_sixty_four() {
    let state = state();
    let mut permits = Vec::new();
    for parent in 0..16 {
        for base in 0..4 {
            for host in 1..=16 {
                permits.push(
                    state
                        .admit_slot_for_test(v4(10, parent, base, host))
                        .await
                        .expect("slot"),
                );
            }
        }
    }
    assert!(matches!(
        state.admit_slot_for_test(v4(11, 0, 0, 1)).await,
        Err(AdmissionError::ServerBusy)
    ));
}

#[tokio::test]
async fn dropping_permit_immediately_frees_its_slot() {
    let state = state();
    let source = v4(10, 1, 2, 3);
    let mut permits = Vec::new();
    for _ in 0..16 {
        permits.push(state.admit_slot_for_test(source).await.expect("slot"));
    }
    drop(permits.pop());
    assert!(state.admit_slot_for_test(source).await.is_ok());
}

#[tokio::test]
async fn cancelled_waiter_does_not_reserve_a_slot() {
    let state = Arc::new(state());
    let source = v4(10, 1, 2, 3);
    let permits = state.admit_slot_for_test(source).await.expect("slot");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let waiting = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let permit = state.admit_slot_for_test(source).await.expect("slot");
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
            drop(permit);
        })
    };
    ready_rx.await.expect("task acquired permit");
    waiting.abort();
    assert!(waiting.await.expect_err("aborted task").is_cancelled());
    drop(permits);
    let mut permits = Vec::new();
    for _ in 0..16 {
        permits.push(state.admit_slot_for_test(source).await.expect("slot"));
    }
}
