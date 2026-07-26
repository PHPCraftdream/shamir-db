# Wave F post-wave independent review (@oh)

Scope: F-1..F-13 (#791-#803), F-14 (#804, CHANGELOG), F-15 (#805, release gate)
+ follow-up (#808). Every commit's full diff was read (not just messages),
current source of touched modules was read directly, and
`KNOWN_LIMITATIONS.md`/`CHANGELOG.md` were checked for consistency. Read-only
— no code changed.

## Per-task status

| Task | Commit | Status | Comment |
|---|---|---|---|
| #807 (pre-wave flake) | — | Correctly closed as non-bug | `vr5_cofilter` TIMEOUT documented as CPU-contention flakiness across 5 independent runs; good test-infra discipline. |
| F-1 (#792) | 220c8b10 | Closed, but see **N-1** | Gate is well-built for "schema declared, then rows written" but silently assumes homogeneity holds for the WHOLE table, including rows written *before* the schema was bound. Not enforced anywhere and not tested. |
| F-2 (#791) | 5a7b65b9 | Fully closed | `numeric_cmp.rs` derivation verified by hand; consolidation is genuine, non-cosmetic; cross-path tests are non-vacuous. |
| F-3 (#793) | 29f80622 | Fully closed | Rotate→consume→delete reorder is correct; checked the "no username" edge case in the boot sweep — confirmed unreachable (`bootstrap_username`/`bootstrap_token_hash` always set/cleared together). |
| F-4 (#794) | 76b487b8 | Fully closed | validate→precompile→persist→activate + rollback, serialized end-to-end by the pre-existing per-table `lock_schema_rmw` guard — no lost-update race. |
| F-5 (#795) | f2ffa1b1 | Fully closed for scope | Gate correctly placed before authz; all 5 named unfixed gaps honestly restated in code + `KNOWN_LIMITATIONS.md` §2. |
| F-6 (#796) | 0b81913d | Fully closed | Minimal, correct gate addition; test asserts both fixed and still-topk cases. |
| F-7 (#797) | 594da439 | Fully closed, composes correctly with F-6/F-10 | Traced `read_exec.rs` end-to-end: `id_acc`/`paged_ids` threading correct; `use_topk` independently excludes `count_total` and `with_version`; F-10's corrupt-skip correctly keeps `id_acc`/`rec_acc` in lockstep. No inter-task conflict despite three fixes touching the same function. |
| F-8 (#798) | ce42157a | Fully closed | Matches `visit_u64`'s bound check exactly; differential test against old msgpack round-trip is the right shape. |
| F-9 (#799) | 28d70e9b | Fully closed, residual under-documented | `try_lock()` fix is correct and minimal; verified only two lock sites exist in `cursor_handlers.rs`, both short. Residual reap-race window is real and correctly small, but **not recorded in `KNOWN_LIMITATIONS.md`** (see N-3). |
| F-10 (#800) | b75bb746 | Fully closed for scope | Verified `read_index_scan.rs`/`read_temporal.rs` got only mechanical field additions (no real detection), matching the "out of scope" claim. `table_manager_index_mgmt.rs`'s cited site is actually a rename key-migration guard, not a comparable read-path skip — minor cross-reference imprecision (N-4). |
| F-11 (#801) | 774b452e | Closed for shipped profiles | Math checks out. `server.example.ktav` follow-up gap only in commit/CHANGELOG, not `KNOWN_LIMITATIONS.md` (N-3). |
| F-12 (#802) | d1ad2e9b | Closed for scope; adjacent gap not flagged | Rename-fsync logic correct, mirrors `wal_segment.rs`. But `backup()`'s own new-file/manifest creation has no parent-directory fsync anywhere — only `restore()`'s renames get `fsync_dir` (N-2). |
| F-13 (#803) | 1c87708a | Fully closed | Cross-checked Rust `try_build()` vs TS `build()` — genuine parity, not just similar-looking code. `Option<QueryValue>` fix correctly closes the Null-sentinel ambiguity. |
| F-14 (#804) | 1c4b58fd | Accurate, one nit | CHANGELOG bullets cross-checked against diffs — accurate. "~800" vs commit's "~822" call sites is cosmetic. |
| F-15 (#805)+follow-up (#808) | 29e3e700, 5270aa92 | Fully closed | Good example of the process working: release gate's own e2e run surfaced a real gap, closed same-session with brief+tests+CHANGELOG. |

## New findings

**N-1 (P1)** — F-1's schema-typed keyset gate
(`crates/shamir-server/src/db_handler/cursor_handlers.rs::order_by_column_is_schema_typed_scalar`)
assumes a schema-enforced type proves the WHOLE column homogeneous, but
schema validators only gate future writes (confirmed via
`table_manager_validators.rs::run_validators_qv/view` and
`boot_compile_schemas` — neither backfills nor scans existing data). Ordinary
workflow: insert mixed-type rows into a schemaless table, then
`set_table_schema`/`add_schema_rule` to declare the field `Int` — the gate now
returns `true` and the cursor enters Keyset mode, silently dropping the
pre-existing non-Int rows past page 1: the exact bug F-1 was written to
close. Needs either a real existing-data check at schema-activation time or
an honest re-scoping of the "CLOSED" claim in the doc comment and
`KNOWN_LIMITATIONS.md` §6.

**N-2 (P2)** — `backup()` (`crates/shamir-server/src/backup.rs`) fsyncs
copied files' *content* and the manifest's *content*, but never fsyncs the
containing directory after creating them — only `restore()`'s renames get
the new `fsync_dir` helper. Same "fsync'd the file, not the directory" bug
class F-12 fixed for renames, unaddressed for `create`. Lower severity than
N-1 (source data untouched; only the backup artifact's completeness after a
crash mid-backup is at risk).

**N-3 (P2)** — F-9's collect-then-remove reap-race residual and F-11's
`server.example.ktav` gap are documented only in commit
messages/CHANGELOG/session checkpoints, not in `KNOWN_LIMITATIONS.md` —
breaking the pattern every other Wave F residual (F-1, F-5, F-10, F-12)
followed. Cheap fix: two bullets.

**N-4 (P2/nit)** — F-10's commit message and `KNOWN_LIMITATIONS.md` group
`table_manager_index_mgmt.rs`'s `continue; // malformed; skip` (line 854, a
table-rename key-migration guard) with `table_manager_streaming.rs`'s
genuinely comparable `Err(_) => false` sites — the former isn't really the
same class of gap. Documentation-accuracy only.

## Prioritized plan

1. N-1 (P1) — decide fix-vs-rescope for F-1's homogeneity claim; not a
   one-line patch, needs a design call first.
2. N-3 (P2) — add the two missing `KNOWN_LIMITATIONS.md` bullets (cheap, do
   before next release cut).
3. N-2 (P2) — extend `backup()` with parent-directory fsync, reusing
   `restore.rs`'s `fsync_dir`.
4. N-4 (P2/nit) — fold into the same PR as #2.

## Candidate tasks (no cross-blocking; all can start in parallel)

1. **Fix or document: schema-typed keyset gate assumes homogeneity for
   pre-schema rows** — investigate a cheap "schema active since version N"
   epoch stamp in the table catalogue vs. narrowing the existing "CLOSED"
   claim; either close the gap for real or accurately re-scope it. (N-1)
2. **Document F-9 and F-11 residuals in KNOWN_LIMITATIONS.md** — add the
   cursor-reaper race window and the `server.example.ktav` gap as
   citation-backed bullets; fold in the N-4 cross-reference correction while
   touching the section. (N-3, N-4)
3. **fsync parent directory for newly-created files in backup()** — extend
   `copy_dir_recursive`/`write_manifest` to fsync `dest_dir`, mirroring
   `restore.rs`'s convention; update `KNOWN_LIMITATIONS.md` §9. (N-2)

Overall: 1 P1 finding (real recurrence of the silent-row-loss bug class this
wave targeted, under a realistic workflow), 3 P2 findings (one adjacent
durability gap, one documentation-completeness gap covering two residuals,
one minor cross-reference nit). No P0s — the wave's actual fixes are all
sound and its own test suites are non-vacuous; the gaps found are at the
boundaries of what each task's brief scoped in, not inside it.
