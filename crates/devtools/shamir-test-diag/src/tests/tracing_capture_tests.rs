//! Unit tests for the tracing ring layer: `tracing` events land in the same
//! shared ring the `log` capture uses.

use tracing_subscriber::layer::SubscriberExt;

use crate::ring::shared_ring;
use crate::tracing_capture::tracing_ring_layer;

/// A layer added to a scoped dispatch records events (level, target,
/// message and structured fields) into the shared ring, alongside `log`
/// records.
#[test]
fn tracing_ring_layer_captures_events_into_the_shared_ring() {
    let subscriber = tracing_subscriber::registry().with(tracing_ring_layer());
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!(target: "tracing_capture_tests", "layer captures this message");
        tracing::warn!(target: "tracing_capture_tests", answer = 42, "structured event");
    });
    let records = shared_ring().snapshot();
    assert!(
        records.iter().any(|r| r.level == "INFO"
            && r.target == "tracing_capture_tests"
            && r.message.contains("layer captures this message")),
        "plain message event missing from the ring ({} records)",
        records.len()
    );
    assert!(
        records.iter().any(|r| r.level == "WARN"
            && r.message.contains("structured event")
            && r.message.contains("answer=42")),
        "structured event missing from the ring ({} records)",
        records.len()
    );
}

/// The layer must not cap the verbosity of a composed subscriber: another
/// layer of the same dispatch (a test asserting on TRACE events) still sees
/// them, while the ring itself keeps only DEBUG and below.
#[test]
fn tracing_ring_layer_does_not_cap_the_composed_subscribers_verbosity() {
    let layer = tracing_ring_layer();
    assert!(
        <crate::tracing_capture::TracingRingLayer as tracing_subscriber::Layer<
            tracing_subscriber::Registry,
        >>::max_level_hint(&layer)
        .is_none(),
        "the ring layer must not install a max level hint"
    );
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::trace!(target: "tracing_capture_tests", "trace-level-event-not-kept");
        tracing::debug!(target: "tracing_capture_tests", "debug-level-event-kept");
    });
    let records = shared_ring().snapshot();
    assert!(records
        .iter()
        .any(|r| r.message.contains("debug-level-event-kept")));
    assert!(!records
        .iter()
        .any(|r| r.message.contains("trace-level-event-not-kept")));
}
