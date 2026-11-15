# Brief: where do the other ~5 s of a Windows system-store open go? (BOOT-COST-2, measurement test)

## Why

`docs/dev-artifacts/perf/boot-cost.md` section 7 measured on CI that the raw Fjall work of one open (one `Database` open
plus 16 keyspace opens) costs about 1.2 s on windows-latest (43 ms on ubuntu), while `win-test-runtime.md` attributes about
6.4 s to ONE open of the system store on Windows. Idle flusher persists and `SyncAll` of an empty store were measured at
about zero. So about 5 s of a system-store open on Windows are NOT explained. Candidates: the work done while the
catalogue is created on a fresh store (the DDL path persists with `SyncAll` per statement, section 2 of the document),
the construction around the store (MemBuffer wrapping, seed and registry writes), and the close and directory removal.

## What (a plain measurement test, NO production change)

1. Read the real open path: `crates/engine/shamir-db/src/shamir_db/system_store/store.rs` (open and the catalogue
   creation around its lines 171-184), what `ShamirDb::init` or the server boot calls before and after it, and every
   `persist` or `flush` it reaches. Write down the sequence of phases with file and line.
2. Add a plain test (a `tests/` file under the crate that owns the code, so it may use `pub(crate)` items if it lives
   inside `src/.../tests/`; follow the root CLAUDE.md test layout, multi-line literals, no `#[ignore]`, no new dependency)
   that performs THE SAME CALLS IN THE SAME ORDER as the real open, each individually timed with `Instant`, and prints
   one `BOOT-COST2` line per phase to stdout: wall time and the value of `FJALL_PERSIST_SYNC_ALL_COUNT` after the phase.
   Also time the real open as a whole (the public entry point) in the same test, so the sum of the phases can be
   cross-checked against the total. If a phase cannot be separated without changing production code, say so and time the
   smallest enclosing public call.
3. Do not change production code, `.rust-guard/` or any workflow. The orchestrator runs the test on the three OS in CI
   (the output of a passing nextest test is hidden, so a throwaway branch makes the test fail on purpose at the end;
   you do NOT do that).
4. Append a section 8 to `docs/dev-artifacts/perf/boot-cost.md` that lists the phases with file and line, the
   hypothesis for each, and what the numbers must show to confirm or reject it. The orchestrator fills the numbers.

## Observable acceptance

`cargo fmt -p <crate>` clean, `cargo clippy -p <crate> --all-targets -- -D warnings` clean for the crate that gets the test,
layout OK. Final report: the phase list, the test file, what the CI run must show.
