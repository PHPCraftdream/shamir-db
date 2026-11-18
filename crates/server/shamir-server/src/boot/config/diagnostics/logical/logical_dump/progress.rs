use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Shared writer byte counter (`Send`; the job future may move between
/// threads, so this is an atomic, never a `Cell`).
pub(super) type ByteCounter = Arc<AtomicU64>;

pub(super) fn byte_counter() -> ByteCounter {
    Arc::new(AtomicU64::new(0))
}

/// Check cancellation/deadline on every codec I/O boundary, including frames
/// that have no items. Synchronous offline I/O is deliberately joined in the
/// caller, never a detached spawn_blocking task holding the directory lock.
pub(super) struct CheckedIo<T> {
    inner: T,
    cancel: CancellationToken,
    deadline: Instant,
    bytes: ByteCounter,
}

impl<T> CheckedIo<T> {
    pub(super) fn new(inner: T, cancel: &CancellationToken, deadline: Instant) -> Self {
        Self::with_counter(inner, cancel, deadline, byte_counter())
    }

    pub(super) fn with_counter(
        inner: T,
        cancel: &CancellationToken,
        deadline: Instant,
        bytes: ByteCounter,
    ) -> Self {
        Self {
            inner,
            cancel: cancel.clone(),
            deadline,
            bytes,
        }
    }

    pub(super) fn deadline() -> Instant {
        // Leave margin inside the engine's 600-second freeze deadline.
        Instant::now() + Duration::from_secs(590)
    }

    /// The inner sink plus total bytes written through this wrapper.
    pub(super) fn into_parts(self) -> (T, u64) {
        (self.inner, self.bytes.load(Ordering::Relaxed))
    }

    pub(super) fn into_inner(self) -> T {
        self.into_parts().0
    }

    fn check(&self) -> io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("logical-export cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "logical-export deadline exceeded",
            ));
        }
        Ok(())
    }
}

impl<T: Read> Read for CheckedIo<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.inner.read(buffer)
    }
}

impl<T: Write> Write for CheckedIo<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.check()?;
        let written = self.inner.write(buffer)?;
        self.bytes.fetch_add(written as u64, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}
