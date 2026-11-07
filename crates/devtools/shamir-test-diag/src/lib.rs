//! Failure-only diagnostics for flaky tests (design:
//! `docs/dev-artifacts/design/flake-failure-logs.md`).
//!
//! [`FlakeGuard`] lives at the top of a test and records a wall-clock
//! timeline ([`FlakeGuard::mark`]) plus state facts ([`FlakeGuard::note`]),
//! captures the process's `log` records — and, behind the `tracing`
//! feature, `tracing` events — into one bounded in-memory ring, and prints
//! the whole report to stderr ONLY while the test is panicking: a green run
//! prints nothing. [`FlakeGuard::with_watchdog`] additionally dumps the
//! same report once if the guard is still alive after a chosen duration,
//! so a hung test leaves evidence before the runner kills it.
//!
//! Dev-only by design: never a normal (production) dependency.

pub mod guard;
pub mod log_capture;
pub mod ring;
#[cfg(feature = "tracing")]
pub mod tracing_capture;

pub use guard::FlakeGuard;
pub use log_capture::LogCaptureStatus;
#[cfg(feature = "tracing")]
pub use tracing_capture::{tracing_ring_layer, try_install_global_tracing_ring};

#[cfg(test)]
mod tests;
