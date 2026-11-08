//! Decoder emission, split per concern: shared root context (`root.rs`),
//! rejection plumbing (`reject.rs`), leaf payloads (`payload.rs`) and
//! container frames (`container.rs`).

mod container;
mod payload;
mod reject;
mod root;

// glob re-exports: the FAIL_* rejection-class constants are shared with the
// encoder and the named local/type constants with the codec tests through
// this module, without enumerating every name twice
pub(crate) use reject::*;
pub(crate) use root::*;
