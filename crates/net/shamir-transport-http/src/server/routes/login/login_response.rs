use bytes::Bytes;
use http::StatusCode;
use std::sync::Arc;

/// Exact application-owned MessagePack login response.
#[derive(Clone)]
pub struct LoginResponse {
    status: StatusCode,
    body: Bytes,
    retry_after: Option<u32>,
    hold: Option<Arc<dyn Send + Sync>>,
}

impl LoginResponse {
    /// Constructs a response without altering the supplied body bytes.
    pub fn new(status: StatusCode, body: Bytes) -> Self {
        Self {
            status,
            body,
            retry_after: None,
            hold: None,
        }
    }

    /// Constructs a response carrying a retry delay in seconds.
    pub fn with_retry_after(status: StatusCode, body: Bytes, seconds: u32) -> Self {
        Self {
            status,
            body,
            retry_after: Some(seconds),
            hold: None,
        }
    }

    /// Retains an application permit until response writing completes or the connection drops.
    pub fn with_hold(mut self, hold: Arc<dyn Send + Sync>) -> Self {
        self.hold = Some(hold);
        self
    }
    /// Internal response hold, moved into the connection's write owner.
    pub(crate) fn take_hold(&mut self) -> Option<Arc<dyn Send + Sync>> {
        self.hold.take()
    }

    /// Response status.
    pub fn status(&self) -> StatusCode {
        self.status
    }
    /// Exact MessagePack body bytes.
    pub fn body(&self) -> &Bytes {
        &self.body
    }
    /// Retry-After seconds, when set.
    pub fn retry_after(&self) -> Option<u32> {
        self.retry_after
    }
}

impl core::fmt::Debug for LoginResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LoginResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}
