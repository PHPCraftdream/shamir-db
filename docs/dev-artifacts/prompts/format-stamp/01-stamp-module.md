# Brief: the format-stamp module in `shamir-sync` (release-1 R1a, step 1)

Design: `docs/dev-artifacts/design/format-stamp.md`, sections 2 (the stamp: D1-D7), 3 (startup rules, the decision table and
the mode per subcommand), 4 (base epoch and the change log), 6 (tests and how to fake an older epoch) and 7 "Step 1". Read
them in full. The open questions of section 8 all have defaults; the defaults apply, do not ask.

## Why

Nothing stamps a data directory today. Step 1 builds the stamp, its codec, its atomic write, the data-present test, the
total decision function and the change log as a self-contained module of `shamir-sync` with NO behaviour change anywhere
else: nothing calls it yet (step 2, a separate change, wires it into `launch()`, the token CLI, `doctor`, `access-tree`,
the seed and backup/restore). This step must land first so that step 2 is a small, reviewable wiring change.

## What

1. **The module tree exactly as in design section 7, step 1**: `crates/server/shamir-sync/src/format_stamp/` with `mod.rs`,
   `model/`, `codec/`, `io/`, `check/`, `change_log/`, `tests/` and the files listed there (`FormatStamp`, `EpochPolicy`,
   `BASE_FORMAT_EPOCH`, `CURRENT_FORMAT_EPOCH`, `StampError`, `read_header`, encode/decode, `write_stamp` with the atomic
   write of D4 (strict directory fsync on Unix, file re-sync and read-back on Windows), `inspect`, `IgnoreSet` and
   `data_present`, `check_data_dir` with `StampMode`/`StampOutcome` implementing the whole decision table of 3.4, `FormatChange`
   and the const change table whose first entry is `pre-release-absorbed`). One primary export per file, `mod.rs` re-exports
   only, at most 7 entries per folder, at most 1000 lines per file. `lib.rs` gets `pub mod format_stamp;`.
2. **No `Cargo.toml` change** (`serde`, `serde_json`, `sha2`, `thiserror`, `tracing` are present). Do not add a dependency.
3. **Encoding** exactly D3: fixed header (magic, layout version, epoch), a JSON body with named fields that preserves unknown
   keys, and a SHA-256 trailer. Reading a stamp with a newer layout version is refused with its own error (decision table).
4. **Tests** (`format_stamp/tests/`, one tests folder per module, `mod.rs` manifests only, no inline test modules, JSON
   literals multi-line): every row of the 3.4 table gets a named test; codec round trip, unknown-key preservation, torn and
   corrupt stamps (D5), the atomic-write behaviour (crash simulated by leaving the tmp file; tmp is never trusted),
   `data_present` with the ignore set, `last_opened_by` rewritten only when the binary version changes (D6). Use the way of
   faking an older epoch given in design 6.1 (no new `#[allow]`, no visibility widening). Use `tempfile` or the helper that
   `shamir-sync`'s existing tests already use for directories. Tests that need a Unix-only or Windows-only behaviour carry the
   matching `cfg` and are re-read for unused imports (CI compiles each OS; locally only this one).
5. **No rust-guard campaign.** No `shamir-engine` source changes, so do not touch `.rust-guard/`. This overrides the
   corresponding bullet of the common tail that follows the brief.
6. **Nothing else changes**: no edit in `shamir-server`, `shamir-db`, `shamir-engine`, `shamir-wal`; no guide edit; no
   version change. `CLAUDE.md` gets no edit (no new crate).

## Non-goals

No wiring (step 2), no `format-migration` command (R1b), no engine-embedded protection (Q3), no per-repo stamps (Q1), no
retry on Windows sharing violations (Q13).

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-sync --all-targets`) and layout clean; the new tests green on Linux, macOS and Windows; every row
of the decision table has a test that names the row. State in the final report which rows or behaviours you could not
check locally and what CI must show.
