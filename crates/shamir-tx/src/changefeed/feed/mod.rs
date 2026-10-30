//! Core changefeed machinery split by concern: `events` holds the changelog
//! event types, key/metadata codecs, and projection helpers; `core` holds the
//! `RepoChangefeed` runtime (live broadcast + durable journal writer).

mod batch;
pub(crate) mod core;
pub(crate) mod error;
pub(crate) mod events;

pub use core::{ChangelogStore, RepoChangefeed};
pub use error::{ChangelogError, ChangelogResult};
