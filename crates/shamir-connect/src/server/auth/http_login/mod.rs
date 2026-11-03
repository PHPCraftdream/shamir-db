//! HTTP login transcript and proof primitives, independent of HTTP framing.

mod proof;
mod scope_bytes;
mod transcript;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

pub use proof::{build_server_signature, verify_http_login_proof};
pub use scope_bytes::{encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope};
pub use transcript::{build_transcript, HttpLoginTranscriptInput};
