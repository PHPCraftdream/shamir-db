//! (feature `tracing`) A `tracing_subscriber` layer that feeds the SAME
//! shared ring the `log` capture uses — for crates that emit `tracing`
//! events (server, transport-http, client).

use crate::ring::shared_ring;
use std::fmt;
use tracing::field::{Field, Visit};
use tracing::{Event, Level};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// Layer writing every `tracing` event up to `DEBUG` (design: all levels up
/// to `Debug`) into the process-wide shared ring.
#[derive(Clone, Copy, Default)]
pub struct TracingRingLayer;

impl<S: tracing::Subscriber> Layer<S> for TracingRingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Filter here, never through `max_level_hint`: a hint would cap the
        // verbosity every other layer of a composed subscriber sees (a test
        // asserting on TRACE events would lose them).
        if *event.metadata().level() > Level::DEBUG {
            return;
        }
        let mut visitor = EventFields::default();
        event.record(&mut visitor);
        shared_ring().push(
            event.metadata().level().to_string(),
            event.metadata().target().to_string(),
            visitor.render(),
        );
    }
}

/// Installs `registry().with(TracingRingLayer)` as the process-global
/// default dispatcher. Returns `false` (graceful degradation) when a global
/// default was already set — the existing subscriber stays untouched. Tests
/// that install their own global dispatch must instead compose
/// [`tracing_ring_layer`] into it.
pub fn try_install_global_tracing_ring() -> bool {
    tracing::dispatcher::set_global_default(tracing::Dispatch::new(
        tracing_subscriber::registry().with(tracing_ring_layer()),
    ))
    .is_ok()
}

/// Convenience constructor for composing the layer into a test's own
/// subscriber (`registry().with(tracing_ring_layer()).with(...)`) — the
/// required form for tests that set their own global dispatch.
pub fn tracing_ring_layer() -> TracingRingLayer {
    TracingRingLayer
}

#[derive(Default)]
struct EventFields {
    message: Option<String>,
    fields: Vec<String>,
}

impl Visit for EventFields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.fields.push(format!("{}={:?}", field.name(), value));
        }
    }
}

impl EventFields {
    fn render(self) -> String {
        let mut out = self.message.unwrap_or_default();
        for field in &self.fields {
            if out.is_empty() {
                out.push_str(field);
            } else {
                out.push(' ');
                out.push_str(field);
            }
        }
        out
    }
}
