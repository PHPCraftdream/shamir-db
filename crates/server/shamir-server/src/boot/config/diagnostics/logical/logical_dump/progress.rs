use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Check cancellation/deadline on every codec I/O boundary, including frames
/// that have no items. Synchronous offline I/O is deliberately joined in the
/// caller, never a detached spawn_blocking task holding the directory lock.
pub(super) struct CheckedIo<T> {
    inner: T,
    cancel: CancellationToken,
    deadline: Instant,
}

impl<T> CheckedIo<T> {
    pub(super) fn new(inner: T, cancel: &CancellationToken, deadline: Instant) -> Self {
        Self {
            inner,
            cancel: cancel.clone(),
            deadline,
        }
    }

    pub(super) fn deadline() -> Instant {
        // Leave margin inside the engine's 600-second freeze deadline.
        Instant::now() + Duration::from_secs(590)
    }

    pub(super) fn into_inner(self) -> T {
        self.inner
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
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}
