//! Cancellation-checked reader wrapper for the artifact. Mirrors the
//! `logical_dump` `CheckedIo` pattern (cancellation at every I/O boundary)
//! without widening that module's `pub(super)` visibility.

use std::io::{self, Read};

use tokio_util::sync::CancellationToken;

/// Wraps a blocking reader so every `read` observes the cancellation token.
/// The DumpReader therefore aborts between frames instead of reading the
/// whole artifact after ctrl-c.
pub(crate) struct CancelledRead<R: Read> {
    inner: R,
    cancel: CancellationToken,
}

impl<R: Read> CancelledRead<R> {
    pub(crate) fn new(inner: R, cancel: CancellationToken) -> Self {
        Self { inner, cancel }
    }
}

impl<R: Read> Read for CancelledRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("logical-import cancelled"));
        }
        self.inner.read(buf)
    }
}
