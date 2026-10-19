//! `discovery` — compiled test-inventory diff between two
//! `cargo nextest list --message-format json` outputs (CI-03b).
//!
//! A test's IDENTITY is the pair (binary id, test name): the same test
//! name under two different binaries is two different identities, and a
//! binary that disappears is itself a finding (`TARGET_LOST`).
//!
//! Delta mapping: the conservation delta parser
//! (`conservation_cmd::delta::Delta`, `consume_pair`) is reused
//! unchanged. A `[[moved]]` or `[[renamed]]` entry pairs a BASE test
//! with a HEAD test by reading `path` + `item` as the BASE identity
//! (binary id, test name) and `to_path` + `to_item` as the HEAD
//! identity — exactly the `to_path::to_item` head-side semantics of the
//! conservation gate, transplanted onto (binary, test) identities. When
//! `to_path`/`to_item` are absent they default to `path`/`item`.
//! A base test paired through a consumed entry is OK; every other base
//! test without a head counterpart is `TEST_LOST` (never excusable by
//! any other entry kind). Entries that matched nothing surface as
//! `UNUSED_ALLOWANCE`. The delta's `base` SHA is required by the loader
//! but cannot be VERIFIED here: this subcommand reads two list files
//! and never sees a revision — the CI job pins the base list to the
//! merge base instead.
//!
//! Known limitation — newly-ignored tests are NOT detected:
//! `cargo nextest list --message-format json` does not carry the
//! `#[ignore]` status of a test, so `TEST_IGNORED`-style drift between
//! the lists is invisible to this gate by construction. It is
//! deliberately NOT faked (no `[[changed]]` reinterpretation); detecting
//! ignore-status changes stays the job of the `conservation` gate's
//! `TEST_IGNORED`/`TEST_UNIGNORED` findings, which read the sources.
//!
//! Finding codes: `TEST_LOST`, `TEST_DUPLICATE`, `TARGET_LOST`,
//! `EMPTY_LIST`, `UNUSED_ALLOWANCE`, `BLOCKED_COMPARISON`.
//!
//! Re-exports only; logic lives in the sibling files.

pub mod lists;
pub mod run;

pub use run::{run, DiscoveryArgs};
