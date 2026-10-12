//! SEED-5 (task #1329) — end-to-end crash-recovery certification for the
//! offline follower seed (R2-lite).
//!
//! # What these tests prove
//!
//! A REAL leader process (`env!("CARGO_BIN_EXE_shamir-server")`) is killed
//! without a clean shutdown (`Child::kill` = SIGKILL on Unix). Reopening the
//! leader's data directory certifies a recovery gap `[J+1, H]`; a fresh
//! follower stops with `resync_required` and applies NO payload; a
//! stopped-source `replication-seed-export` CLI produces a verified
//! artifact; `replication-seed-install` boots a NEW follower repository with
//! a paused, receipt-bound subscription; the operator's explicit Resume
//! activates the loop, whose first pull starts at seed version `V + 1`; and
//! subsequent leader writes converge row-for-row.
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
