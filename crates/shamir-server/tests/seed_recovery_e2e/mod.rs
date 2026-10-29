//! SEED-5 (task #1329) — end-to-end crash-recovery certification for the
//! offline follower seed (R2-lite).
//!
//! # What these tests prove
//!
//! leader's data directory reconciles events from its surviving WAL without
//! an unclean-stop blanket gap. Seed scenarios explicitly persist a residual
//! gap, then verify a fresh follower stops with `resync_required` and applies
//! NO payload; the stopped-source seed CLI installs and resumes a new follower
//! repository, whose subsequent writes converge row-for-row.
//!
//! # Honesty boundary (do not overstate)
//!
//! `Child::kill` establishes PROCESS-CRASH behaviour only, not power-loss
//! guarantees: the OS page cache may still hold data that would survive a
//! process death but not a machine power cut. Durability ORDERING claims
//! (history before bookmark before Complete before publication) are proven
//! by the deterministic SEED-3 boundary-injection seam, not by killing
//! processes. See `docs/guide-docs/guide/09-seed-recovery.md`.

#![cfg(unix)]

mod harness;
mod scenarios;
