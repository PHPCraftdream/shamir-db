use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Per-connection heartbeat correlation state shared between WS halves.
#[derive(Clone, Default)]
pub struct HeartbeatState(Arc<State>);

#[derive(Default)]
struct State {
    last_sent: AtomicU64,
    last_acked: AtomicU64,
    reader_parked: AtomicBool,
}

impl HeartbeatState {
    pub fn next_ping(&self) -> ([u8; 8], u64) {
        let sequence = self
            .0
            .last_sent
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);
        self.0.last_sent.store(sequence, Ordering::Release);
        (sequence.to_be_bytes(), sequence)
    }

    pub fn observe_pong(&self, payload: &[u8]) -> bool {
        if payload.len() != 8 {
            return false;
        }
        let sequence = u64::from_be_bytes(payload.try_into().expect("8-byte payload"));
        let sent = self.0.last_sent.load(Ordering::Acquire);
        if sequence == 0 || sequence != sent {
            return false;
        }
        let mut acked = self.0.last_acked.load(Ordering::Acquire);
        loop {
            if sequence <= acked {
                return false;
            }
            match self.0.last_acked.compare_exchange_weak(
                acked,
                sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => acked = observed,
            }
        }
    }

    pub fn pending(&self) -> bool {
        let sent = self.0.last_sent.load(Ordering::Acquire);
        sent != 0 && self.0.last_acked.load(Ordering::Acquire) < sent
    }
    pub fn reader_parked(&self, parked: bool) {
        self.0.reader_parked.store(parked, Ordering::Release);
    }
    pub fn is_reader_parked(&self) -> bool {
        self.0.reader_parked.load(Ordering::Acquire)
    }
}

pub fn is_matched_heartbeat_pong(state: Option<&HeartbeatState>, payload: &[u8]) -> bool {
    state.is_some_and(|state| state.observe_pong(payload))
}
