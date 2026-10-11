use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use shamir_tunables::instance_defaults::SLOW_CONSUMER_THRESHOLD;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::super::push::{push_required, try_push_event};

struct RejectingSink {
    reject_first: usize,
    reject_seq: Option<u64>,
    always_reject: bool,
    calls: AtomicUsize,
    attempts: mpsc::Sender<(PushEnvelope, bool)>,
}

impl PushSink for RejectingSink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        let envelope: PushEnvelope = rmp_serde::from_slice(&frame).expect("valid push envelope");
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let rejected = self.always_reject
            || call < self.reject_first
            || (envelope.push != PushKind::SlowConsumer && self.reject_seq == Some(envelope.seq));
        self.attempts
            .try_send((envelope, !rejected))
            .expect("bounded attempt recorder has room for the retry budget");
        if rejected {
            Err(PushRejected)
        } else {
            Ok(())
        }
    }
}

fn sink(
    reject_first: usize,
    reject_seq: Option<u64>,
    always_reject: bool,
) -> (Arc<dyn PushSink>, mpsc::Receiver<(PushEnvelope, bool)>) {
    let (attempts, receiver) = mpsc::channel(512);
    let push = Arc::new(RejectingSink {
        reject_first,
        reject_seq,
        always_reject,
        calls: AtomicUsize::new(0),
        attempts,
    });
    (push, receiver)
}

fn drain(receiver: &mut mpsc::Receiver<(PushEnvelope, bool)>) -> Vec<(PushEnvelope, bool)> {
    let mut attempts = Vec::new();
    while let Ok(attempt) = receiver.try_recv() {
        attempts.push(attempt);
    }
    attempts
}

// Red with the old skip-and-continue policy: the first event is never accepted.
#[tokio::test(start_paused = true)]
async fn transient_rejection_delivers_once_in_order_with_contiguous_seq() {
    let (push, mut receiver) = sink(3, None, false);
    let seq = AtomicU64::new(0);
    let start = Instant::now();
    assert!(try_push_event(&push, 7, &seq, b"first").await);
    assert!(try_push_event(&push, 7, &seq, b"second").await);
    let attempts = drain(&mut receiver);
    assert_eq!(attempts.len(), 5);
    assert!(attempts[..3].iter().all(|(frame, accepted)| {
        !accepted && frame.seq == 0 && frame.data.as_deref() == Some(b"first".as_slice())
    }));
    let accepted: Vec<_> = attempts
        .into_iter()
        .filter_map(|(frame, accepted)| accepted.then_some(frame))
        .collect();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].push, PushKind::Event);
    assert_eq!(accepted[0].sub, 7);
    assert_eq!(accepted[0].seq, 0);
    assert_eq!(accepted[0].data.as_deref(), Some(b"first".as_slice()));
    assert_eq!(accepted[1].seq, 1);
    assert_eq!(accepted[1].data.as_deref(), Some(b"second".as_slice()));
    assert_eq!(seq.load(Ordering::Relaxed), 2);
    assert_eq!(start.elapsed(), Duration::from_millis(15));
}

// Red before bounded retry: one rejection returns true, allowing a later event.
#[tokio::test(start_paused = true)]
async fn permanent_rejection_is_terminal_and_attempts_slow_consumer() {
    let (push, mut receiver) = sink(0, None, true);
    let seq = AtomicU64::new(0);
    let start = Instant::now();
    if try_push_event(&push, 7, &seq, b"blocked").await {
        try_push_event(&push, 7, &seq, b"later").await;
        panic!("rejected required event must terminate delivery");
    }
    let attempts = drain(&mut receiver);
    assert_eq!(attempts.len(), SLOW_CONSUMER_THRESHOLD as usize + 1);
    for (frame, accepted) in &attempts[..SLOW_CONSUMER_THRESHOLD as usize] {
        assert!(!accepted);
        assert_eq!(frame.push, PushKind::Event);
        assert_eq!(frame.seq, 0);
        assert_eq!(frame.data.as_deref(), Some(b"blocked".as_slice()));
    }
    let (terminal, accepted) = attempts.last().expect("slow consumer attempt");
    assert!(!accepted);
    assert_eq!(terminal.push, PushKind::SlowConsumer);
    assert_eq!(terminal.sub, 7);
    assert_eq!(terminal.seq, 0);
    assert_eq!(terminal.data, None);
    assert_eq!(terminal.gap_at, None);
    assert_eq!(start.elapsed(), Duration::from_millis(495));
}

// The old threshold path consumes a sequence even when SlowConsumer is rejected.
#[tokio::test(start_paused = true)]
async fn rejected_event_and_terminal_notification_do_not_advance_seq() {
    let (push, mut receiver) = sink(0, None, true);
    let seq = AtomicU64::new(42);
    assert!(!try_push_event(&push, 7, &seq, b"blocked").await);
    assert_eq!(seq.load(Ordering::Relaxed), 42);
    assert!(drain(&mut receiver)
        .iter()
        .all(|(frame, accepted)| frame.seq == 42 && !accepted));
}

// Red with the former single try_push control sites: Ready/Gap are lost.
#[tokio::test(start_paused = true)]
async fn ready_and_both_gap_shapes_retry_without_consuming_rejected_seq() {
    for (kind, gap_at) in [
        (PushKind::Ready, None),
        (PushKind::Gap, Some(19)),
        (PushKind::Gap, None),
    ] {
        let (push, mut receiver) = sink(2, None, false);
        let seq = AtomicU64::new(4);
        let envelope = PushEnvelope {
            push: kind,
            sub: 7,
            seq: 4,
            data: None,
            gap_at,
        };
        let frame = rmp_serde::to_vec_named(&envelope).expect("serialize control envelope");
        assert!(push_required(&push, 7, &seq, frame).await);
        let attempts = drain(&mut receiver);
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0], (envelope.clone(), false));
        assert_eq!(attempts[1], (envelope.clone(), false));
        assert_eq!(attempts[2], (envelope.clone(), true));
        assert_eq!(seq.load(Ordering::Relaxed), 5);
        assert!(try_push_event(&push, 7, &seq, b"next").await);
        assert_eq!(drain(&mut receiver)[0].0.seq, 5);

        let (blocked_push, mut blocked_receiver) = sink(0, None, true);
        let blocked_seq = AtomicU64::new(4);
        let frame = rmp_serde::to_vec_named(&envelope).expect("serialize control envelope");
        assert!(!push_required(&blocked_push, 7, &blocked_seq, frame).await);
        assert_eq!(blocked_seq.load(Ordering::Relaxed), 4);
        let attempts = drain(&mut blocked_receiver);
        assert_eq!(attempts.len(), SLOW_CONSUMER_THRESHOLD as usize + 1);
        assert!(attempts[..SLOW_CONSUMER_THRESHOLD as usize]
            .iter()
            .all(|(frame, accepted)| frame == &envelope && !accepted));
        assert_eq!(
            attempts.last().expect("terminal attempt").0.push,
            PushKind::SlowConsumer,
        );
    }
}

// Mirrors the bridge's per-change return-on-false call pattern without a DB fixture.
// Red with skip-and-continue: the third change is delivered after losing the second.
#[tokio::test(start_paused = true)]
async fn second_change_rejection_stops_before_third_change() {
    let (push, mut receiver) = sink(0, Some(1), false);
    let seq = AtomicU64::new(0);
    let mut continued = true;
    for data in [
        b"first".as_slice(),
        b"second".as_slice(),
        b"third".as_slice(),
    ] {
        if !try_push_event(&push, 7, &seq, data).await {
            continued = false;
            break;
        }
    }
    assert!(!continued);
    let attempts = drain(&mut receiver);
    assert_eq!(attempts.len(), SLOW_CONSUMER_THRESHOLD as usize + 2);
    assert_eq!(attempts[0].0.data.as_deref(), Some(b"first".as_slice()));
    assert!(attempts[0].1);
    for (frame, accepted) in &attempts[1..=SLOW_CONSUMER_THRESHOLD as usize] {
        assert!(!accepted);
        assert_eq!(frame.seq, 1);
        assert_eq!(frame.data.as_deref(), Some(b"second".as_slice()));
    }
    let (terminal, accepted) = attempts.last().expect("slow consumer attempt");
    assert!(*accepted);
    assert_eq!(terminal.push, PushKind::SlowConsumer);
    assert_eq!(terminal.seq, 1);
    assert_eq!(seq.load(Ordering::Relaxed), 2);
    assert!(attempts
        .iter()
        .all(|(frame, _)| frame.data.as_deref() != Some(b"third".as_slice())));
}
