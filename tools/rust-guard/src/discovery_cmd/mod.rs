//! `discovery` — compiled test-inventory diff between two
//! `cargo nextest list --message-format json` outputs (CI-03b).
//!
//! A test's IDENTITY is the pair (binary id, test name): the same test
//! name under two different binaries is two different identities, and a
//! binary that disappears is itself a finding (`TARGET_LOST`).
//!
//! Delta mapping: the conservation delta parser
//! (`conservation_cmd::delta::Delta`) is reused. A `[[moved]]` or
//! `[[renamed]]` entry pairs a BASE test with a HEAD test by reading
//! `path` + `item` as the BASE identity (binary id, test name) and
//! `to_path` + `to_item` as the HEAD identity — which MUST exist in
//! the head list and is consumed by at most one declaration (a second
//! claimant is a finding). An entry whose head fields are missing, or
//! whose target identity is absent from head, never silently passes:
//! the base test is `TEST_LOST` and the malformed entry surfaces as
//! `UNUSED_ALLOWANCE`. Identities present in BOTH lists match
//! directly, no delta needed; head-only identities are additions and
//! never findings. Every other base test without a head counterpart
//! is `TEST_LOST` (never excusable by any other entry kind). Entries
//! that matched nothing surface as `UNUSED_ALLOWANCE`. The delta's
//! `base` SHA is required by the loader but cannot be VERIFIED here:
//! this subcommand reads two list files and never sees a revision —
//! the CI job pins the base list to the merge base instead.
//!
//! Newly-ignored tests ARE detected: the nextest list document carries
//! each testcase's `ignored` flag, so a base test that is not ignored
//! while its directly matched head counterpart is ignored is a
//! `TEST_IGNORED` finding (compared on direct matches only, never
//! through a declared move/rename). The reverse flip (un-ignoring) is
//! allowed.
//!
//! Finding codes: `TEST_LOST`, `TEST_DUPLICATE`, `TEST_IGNORED`,
//! `TARGET_LOST`, `EMPTY_LIST`, `UNUSED_ALLOWANCE`,
//! `BLOCKED_COMPARISON`.
//!
//! Re-exports only; logic lives in the sibling files.

pub mod lists;
pub mod run;

pub use run::{run, DiscoveryArgs};
