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
