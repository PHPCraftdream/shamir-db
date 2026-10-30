//! Protected buffers and deserialization for resume handshakes.

use std::fmt;
use std::ops::{Deref, DerefMut};

use serde::de::{SeqAccess, Visitor};
use serde::{Deserializer, Serialize};
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_transport_tcp::framing::FrameError;
use tokio::io::{AsyncRead, AsyncReadExt};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
thread_local! {
    static RESUME_WIPES: std::cell::RefCell<Vec<(usize, bool, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_resume_wipes() -> Vec<(usize, bool, bool)> {
    RESUME_WIPES.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

#[derive(Default)]
pub(crate) struct ProtectedResumeBuffer(pub(crate) Zeroizing<Vec<u8>>);

impl ProtectedResumeBuffer {
    pub(crate) fn into_inner(mut self) -> Zeroizing<Vec<u8>> {
        std::mem::take(&mut self.0)
    }
}

/// The caller must reserve enough for the complete serialized value; otherwise
/// `Vec` growth could free an unwiped earlier allocation containing secrets.
/// Production passes the fixed ResumeInit overhead plus its ticket length.
pub(crate) fn serialize_resume<T: Serialize>(
    value: &T,
    capacity: usize,
) -> Result<ProtectedResumeBuffer, rmp_serde::encode::Error> {
    let mut buf = ProtectedResumeBuffer(Zeroizing::new(Vec::with_capacity(capacity)));
    rmp_serde::encode::write(&mut *buf, value)?;
    Ok(buf)
}

impl Deref for ProtectedResumeBuffer {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ProtectedResumeBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ProtectedResumeBuffer {
    fn drop(&mut self) {
        // Observe only initialized, still-live storage, before Zeroizing frees it.
        let len = self.0.len();
        #[cfg(test)]
        let had_nonzero = self.0.iter().any(|b| *b != 0);
        self.0.as_mut_slice().zeroize();
        #[cfg(test)]
        RESUME_WIPES.with(|events| {
            if len != 0 {
                events
                    .borrow_mut()
                    .push((len, had_nonzero, self.0.iter().all(|b| *b == 0)));
            }
        });
        #[cfg(not(test))]
        let _ = len;
    }
}

/// Read a bounded resume response directly into protected storage. A partial
/// header or body remains owned by zeroizing guards across every await.
pub(crate) async fn read_resume_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<ProtectedResumeBuffer, FrameError> {
    let mut header = Zeroizing::new([0u8; 4]);
    let n = reader.read(&mut header[..1]).await?;
    if n == 0 {
        return Err(FrameError::PeerEof);
    }
    reader.read_exact(&mut header[1..]).await?;
    let len = u32::from_be_bytes(*header) as usize;
    if len == 0 {
        return Err(FrameError::PeerClose);
    }
    if len > MAX_PRE_AUTH_FRAME {
        return Err(FrameError::TooLarge {
            actual: len,
            max: MAX_PRE_AUTH_FRAME,
        });
    }
    let mut body = ProtectedResumeBuffer(Zeroizing::new(vec![0; len]));
    reader.read_exact(&mut body[..]).await?;
    Ok(body)
}

/// Deserialize bytes directly into an allocation protected on all error paths.
pub(crate) fn deserialize_resume_ticket<'de, D>(
    deserializer: D,
) -> Result<ProtectedResumeBuffer, D::Error>
where
    D: Deserializer<'de>,
{
    struct TicketVisitor;
    impl<'de> Visitor<'de> for TicketVisitor {
        type Value = ProtectedResumeBuffer;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a byte string or byte sequence")
        }

        fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
            let mut result = ProtectedResumeBuffer(Zeroizing::new(Vec::with_capacity(bytes.len())));
            result.extend_from_slice(bytes);
            Ok(result)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            // Deserializers may provide the bytes as an arbitrarily long
            // sequence, including outside the bounded handshake reader.
            let mut result =
                ProtectedResumeBuffer(Zeroizing::new(Vec::with_capacity(MAX_PRE_AUTH_FRAME)));
            while let Some(byte) = seq.next_element()? {
                if result.len() == result.capacity() {
                    let next_capacity = result.capacity().saturating_mul(2).max(1);
                    let mut next =
                        ProtectedResumeBuffer(Zeroizing::new(Vec::with_capacity(next_capacity)));
                    next.extend_from_slice(&result);
                    result = next;
                }
                result.push(byte);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_bytes(TicketVisitor)
}
