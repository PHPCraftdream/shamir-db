use bytes::Bytes;
use http::StatusCode;

/// Exact application-owned MessagePack login response.
#[derive(Clone)]
pub struct LoginResponse {
    status: StatusCode,
    body: Bytes,
    retry_after: Option<u32>,
}

impl LoginResponse {
    /// Constructs a response without altering the supplied body bytes.
    pub fn new(status: StatusCode, body: Bytes) -> Self {
        Self {
            status,
            body,
            retry_after: None,
        }
    }

    /// Constructs a response carrying a retry delay in seconds.
    pub fn with_retry_after(status: StatusCode, body: Bytes, seconds: u32) -> Self {
        Self {
            status,
            body,
            retry_after: Some(seconds),
        }
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }
    pub(crate) fn body(&self) -> Bytes {
        self.body.clone()
    }
    pub(crate) fn retry_after(&self) -> Option<u32> {
        self.retry_after
    }
}
