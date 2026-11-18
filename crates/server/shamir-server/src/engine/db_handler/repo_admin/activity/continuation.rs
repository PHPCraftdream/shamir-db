//! [`ActivityContinuation`] — the opaque `ListActivity` continuation token
//! (R4b-1, operational-views D4).
//!
//! Token layout (20 bytes, base64 URL-safe no-pad on the wire — bounded,
//! ~27 characters): version byte, request `scope`, request `kind` filter,
//! section kind (which row kind the walk resumes in), last-seen numeric key
//! (the keyset boundary within that section) and the chain `sequence`.
//!
//! Design properties:
//!
//! - **Stateless**: the server holds no snapshot anywhere. The token is a
//!   pure function of (scope, filter, section, last key, sequence); a page
//!   is rendered from a fresh lock-free scan and rows with key
//!   `> last_key` are taken. A momentarily torn snapshot is acceptable and
//!   documented (D4): rows admitted/removed between pages may be missing
//!   or duplicated relative to any single linear point.
//! - **`sequence` is a chain nonce, not a snapshot promise.** The paging
//!   worker mints it from this module's monotonic counter when a listing
//!   chain starts (no incoming token) and copies it verbatim into every
//!   emitted continuation. It lets operators and tests correlate pages of
//!   one chain; it is validated only for PRESENCE (a decoded `sequence`
//!   of `0` is rejected — the counter starts at 1, so `0` is never
//!   minted), never for recency — the server promises no stable snapshot.
//! - **No forged permission**: the token re-states the request's `scope`.
//!   The permission check for `scope: All` runs BEFORE the token is
//!   parsed, and a decoded token whose scope/kind disagree with the
//!   request is rejected as `invalid_request`, so a token can never widen
//!   what the caller is allowed to see.
//! - **No credentials**: the token carries ids and a nonce only — never
//!   the bearer token, its prefix, a username or a user id.
//! - **Fixed-shape validation**: decode checks the exact byte length, the
//!   version byte and that every enum byte is a known value; every
//!   combination not produced by [`Self::new`] is rejected.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_query_types::admin::activity::{ActivityKind, ActivityScope};
use std::sync::atomic::{AtomicU64, Ordering};

/// Token format version. Bumped on any layout change; decoders reject
/// other versions as malformed rather than guessing.
pub(in crate::engine::db_handler) const TOKEN_VERSION: u8 = 1;

/// Exact decoded token length: 4 enum bytes + two `u64`s.
const TOKEN_BYTES: usize = 20;

/// Exact encoded token length: base64 URL-safe no-pad of [`TOKEN_BYTES`]
/// is exactly 27 characters.
const TOKEN_ENCODED_LEN: usize = 27;

/// Handler-chain nonce source: a monotonic counter minting the initial
/// `sequence` of every `ListActivity` continuation chain. Process-global
/// (the worker owns no mutable handler state; the counter is never
/// rendered anywhere and leaks no information — only that some listing
/// happened).
static CHAIN_NONCE: AtomicU64 = AtomicU64::new(1);

/// Mint the initial chain `sequence` for a new `ListActivity` walk.
pub(in crate::engine::db_handler) fn fresh_sequence() -> u64 {
    CHAIN_NONCE.fetch_add(1, Ordering::Relaxed)
}

/// Section order for a `ListActivity` kind filter: sessions → requests →
/// transactions → cursors (design §2.1 render order). A specific kind
/// filter walks exactly one section.
pub(in crate::engine::db_handler) fn section_order(
    filter: ActivityKind,
) -> &'static [ActivityKind] {
    match filter {
        ActivityKind::Sessions => &[ActivityKind::Sessions],
        ActivityKind::Requests => &[ActivityKind::Requests],
        ActivityKind::Transactions => &[ActivityKind::Transactions],
        ActivityKind::Cursors => &[ActivityKind::Cursors],
        ActivityKind::All => &[
            ActivityKind::Sessions,
            ActivityKind::Requests,
            ActivityKind::Transactions,
            ActivityKind::Cursors,
        ],
    }
}

/// Decoded continuation token (never serialized as itself — encode with
/// [`Self::encode`], the wire sees only the opaque base64 string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine::db_handler) struct ActivityContinuation {
    /// Scope the chain was started with (must equal the request's scope).
    pub scope: ActivityScope,
    /// Kind filter the chain was started with (must equal the request's).
    pub filter: ActivityKind,
    /// Section the walk resumes in (a specific row kind).
    pub section: ActivityKind,
    /// Keyset boundary inside `section`: only rows with a numeric key
    /// strictly greater are eligible.
    pub last_key: u64,
    /// Chain nonce (see the module docs — not a snapshot promise).
    pub sequence: u64,
}

/// Why a continuation string was rejected. Rendered into the
/// `invalid_request` message only — the token itself is never echoed
/// back (it may be attacker-controlled garbage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine::db_handler) enum ContinuationError {
    /// Not valid base64, wrong decoded length, or wrong version byte.
    Malformed,
    /// An enum byte is not a known value.
    UnknownValue,
    /// The section byte is not a specific row kind.
    UnknownSection,
}

impl ContinuationError {
    /// Fixed diagnostic string for the `invalid_request` message.
    pub(in crate::engine::db_handler) fn message(self) -> &'static str {
        match self {
            Self::Malformed => "continuation token is malformed",
            Self::UnknownValue => "continuation token carries an unknown scope or kind",
            Self::UnknownSection => "continuation token carries an unknown section",
        }
    }
}

impl ActivityContinuation {
    /// A fresh token starting (or resuming) at the top of `section`.
    pub(in crate::engine::db_handler) fn new(
        scope: ActivityScope,
        filter: ActivityKind,
        section: ActivityKind,
        last_key: u64,
        sequence: u64,
    ) -> Self {
        Self {
            scope,
            filter,
            section,
            last_key,
            sequence,
        }
    }

    /// Encode to the opaque wire form (bounded base64 URL-safe no-pad).
    pub(in crate::engine::db_handler) fn encode(&self) -> String {
        let mut bytes = [0u8; TOKEN_BYTES];
        bytes[0] = TOKEN_VERSION;
        bytes[1] = scope_byte(self.scope);
        bytes[2] = filter_byte(self.filter);
        bytes[3] = filter_byte(self.section);
        bytes[4..12].copy_from_slice(&self.last_key.to_be_bytes());
        bytes[12..20].copy_from_slice(&self.sequence.to_be_bytes());
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Decode and fully validate the wire form. Every failure mode is a
    /// typed [`ContinuationError`] rendered as `invalid_request` — the
    /// token is untrusted input, not a credential or a capability.
    pub(in crate::engine::db_handler) fn decode(token: &str) -> Result<Self, ContinuationError> {
        // Fixed-shape input: the encode form of [`TOKEN_BYTES`] is
        // exactly [`TOKEN_ENCODED_LEN`] characters; anything else is
        // rejected before it reaches the decoder.
        if token.len() != TOKEN_ENCODED_LEN {
            return Err(ContinuationError::Malformed);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .map_err(|_| ContinuationError::Malformed)?;
        if bytes.len() != TOKEN_BYTES || bytes[0] != TOKEN_VERSION {
            return Err(ContinuationError::Malformed);
        }
        let scope = scope_from_byte(bytes[1]).ok_or(ContinuationError::UnknownValue)?;
        let filter = filter_from_byte(bytes[2]).ok_or(ContinuationError::UnknownValue)?;
        let section = filter_from_byte(bytes[3]).ok_or(ContinuationError::UnknownSection)?;
        // A resume section is always a specific kind — `All` is a filter,
        // never a section.
        if section == ActivityKind::All {
            return Err(ContinuationError::UnknownSection);
        }
        // The section must be one the filter actually walks.
        if !section_order(filter).contains(&section) {
            return Err(ContinuationError::UnknownSection);
        }
        let sequence = u64::from_be_bytes(bytes[12..20].try_into().expect("fixed slice"));
        // Presence validation of the chain nonce: `fresh_sequence` mints
        // from a counter starting at 1, so a decoded `sequence` of 0 is
        // never genuine.
        if sequence == 0 {
            return Err(ContinuationError::Malformed);
        }
        Ok(Self {
            scope,
            filter,
            section,
            last_key: u64::from_be_bytes(bytes[4..12].try_into().expect("fixed slice")),
            sequence,
        })
    }
}

fn scope_byte(scope: ActivityScope) -> u8 {
    match scope {
        ActivityScope::All => 1,
        ActivityScope::Own => 2,
    }
}

fn scope_from_byte(raw: u8) -> Option<ActivityScope> {
    match raw {
        1 => Some(ActivityScope::All),
        2 => Some(ActivityScope::Own),
        _ => None,
    }
}

fn filter_byte(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Sessions => 1,
        ActivityKind::Requests => 2,
        ActivityKind::Transactions => 3,
        ActivityKind::Cursors => 4,
        ActivityKind::All => 5,
    }
}

fn filter_from_byte(raw: u8) -> Option<ActivityKind> {
    match raw {
        1 => Some(ActivityKind::Sessions),
        2 => Some(ActivityKind::Requests),
        3 => Some(ActivityKind::Transactions),
        4 => Some(ActivityKind::Cursors),
        5 => Some(ActivityKind::All),
        _ => None,
    }
}
