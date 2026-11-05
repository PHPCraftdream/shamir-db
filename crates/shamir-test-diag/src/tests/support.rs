//! Shared helpers for this crate's unit tests: report-sink capture and a
//! no-op logger used to occupy the process logger slot.

use crate::guard::set_report_sink;
use std::sync::{Arc, Mutex};

/// In-memory capture of everything the guards emitted.
pub type Capture = Arc<Mutex<Vec<String>>>;

/// Routes every guard report into an in-memory capture instead of stderr.
pub fn install_capture() -> Capture {
    let capture: Capture = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&capture);
    set_report_sink(Some(Arc::new(move |text: &str| {
        sink.lock().expect("capture lock").push(text.to_string());
    })));
    capture
}

/// Restores stderr as the report destination.
pub fn clear_capture() {
    set_report_sink(None);
}

/// The captured reports joined by newlines.
pub fn captured(capture: &Capture) -> String {
    capture.lock().expect("capture lock").join("\n")
}

/// A logger that swallows every record — occupies the process logger slot
/// for the degraded-mode test.
pub struct NoopLogger;

impl log::Log for NoopLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        false
    }

    fn log(&self, _record: &log::Record) {}

    fn flush(&self) {}
}
