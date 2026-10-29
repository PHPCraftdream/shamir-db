# 252 — audit-fix-wal-io (P1 audit follow-up: F6 typed I/O causes)

Source: post-merge audit of PR #324 (P1 F6), finding RV-WAL-5839FF0D (partial). Verified on master: append, fsync and replay I/O boundaries still stringify `io::Error`, destroying `ErrorKind`: `crates/shamir-wal/src/segment/wal_segment.rs` ~lines 338, 398, 659, 664. Preserving the shared `Arc<DbError::Storage>` cannot restore the lost type. The stage acceptance (brief `docs/dev-artifacts/prompts/remediation/192-p1-wal-errors-hygiene.md`, lines ~5 and ~19) requires typed I/O causes at the WAL boundaries; only the open path got a typed error.

## Change
1. Read 192-p1-wal-errors-hygiene.md and the existing typed open-error (`WalOpenError` or equivalent) first and extend THAT mechanism (same style) to append / fsync / replay: the original `io::ErrorKind` (and the source error) must survive up to the ticket failure cause and be matchable by callers (e.g. `NotFound`, `PermissionDenied`, storage-full classification used by retry/poison logic).
2. Where a `DbError`/storage error type must carry it, prefer an EXISTING variant; if a new minimal variant or field is unavoidable, add it in the owning crate, keep downstream matches compiling and list every touched crate in the report. No string parsing of messages anywhere.
3. Tests (WAL test dirs per layout rules): injected failures at append and at fsync (the crate already has injection seams from F6 - reuse) assert the classified kind; replay-read failure asserts the kind; the existing open-error test stays untouched.

## Rules
Only `shamir-wal` unless an additional crate is strictly required (then minimal and reported). No new dependencies, no `#[allow]`, layout limits, imports at top, `mod.rs` re-exports only.

## Local gates (only these)
`cargo fmt -p shamir-wal -- --check`; `cargo clippy -p shamir-wal --all-targets -- -D warnings`; `cargo check -p <crate> --all-targets` for every dependent of any crate whose public type changed; `python -I scripts/ci/check_layout.py`. Real exit codes.
