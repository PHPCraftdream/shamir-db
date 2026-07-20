# Compliance & Ops 5d — implement the audit.retention_days sweep (dead config knob)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fourth item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03
(`docs/dev-artifacts/research/2026-07-17-release-audit/03-compliance-data-governance.md`,
capability-matrix row **1e**):

> Audit log age-based retention — **Absent (config knob is dead)**.
> `config.rs:184-187` parses `audit.retention_days` ("Delete rotated audit
> files older than this … Default 30 days") but grep shows **zero
> consumers** outside `config.rs` — no cleanup task exists.
> `07-operations.md:342` claims "устаревшие файлы удаляются автоматически"
> — false today.

This is a REAL CODE fix (unlike 5a/5b/5c, which were docs-only). The work
plan gives two options: implement a real sweep, or remove the dead knob.
**This brief chooses "implement the sweep"** — investigation below shows
the natural hook point is small and low-risk. If, after reading the code
yourself, you find this assessment wrong (the hook point is riskier than
described), fall back to removing the knob + its doc claim instead, and
say so explicitly in your summary.

## Investigation already done (verify yourself too)

- **Config**: `crates/shamir-server/src/config.rs` ~lines 183-204 —
  `AuditConfig { max_file_size_mb, retention_days }`, `retention_days: u32`
  with `#[serde(default = "default_audit_retention_days")]` → `30`. The
  field's own doc comment already specifies the "off switch" convention:
  `0` disables cleanup (operator manages retention out-of-band). PRESERVE
  this convention exactly — `retention_days == 0` must mean "never sweep."
- **Rotation is the natural, ALREADY-EXISTING hook point**:
  `crates/shamir-server/src/audit_appender.rs`'s `rotate_locked` (~lines
  508-533) is called SYNCHRONOUSLY, inline, whenever a write pushes the
  active log past `max_size_bytes` (see `append_batched`/`append_strict`
  at ~lines 465-483 and ~485-506 — both check `new_size >=
  rot.max_size_bytes` and call `self.rotate_locked(&mut f)`). `rotate_locked`
  renames the active file to `audit_log.log.<unix_nanos>` (a fresh
  timestamped file, ~line 524) and opens a new active file. **This is
  exactly the moment a retention sweep naturally belongs** — right after a
  new rotated file is created, sweep the directory for OTHER rotated files
  older than the retention window and delete them. No new background
  task/timer infrastructure is needed; the sweep piggybacks on rotation,
  which already happens synchronously on the write path.
- **`RotationPolicy`** (~lines 199-202) currently has only
  `max_size_bytes`/`current_size` — no retention field. You will need to
  add one.
- **Both constructors need updating**: `open_strict_with_rotation` (~lines
  255-279) and `open_batched_with_rotation` (~lines 288-...) each build a
  `RotationPolicy { .. }` inline (grep `RotationPolicy {` in this file —
  two call sites) from a `max_size_bytes: Option<u64>` parameter. Add a
  parallel `retention_days: u32` (or similar) parameter to both, threaded
  the same way.
- **Wiring call site**: `crates/shamir-server/src/server/server_launcher.rs`
  ~line 146-151 computes `audit_max_bytes` from `config.audit.max_file_size_mb`
  and calls `FjallAuditAppender::open_batched_with_rotation(...)` — this is
  where `config.audit.retention_days` needs to ALSO be read and passed
  through (it currently isn't referenced here at all).

## The task

1. Add a retention field to `RotationPolicy` (e.g. `retention_days: u32`,
   or convert to a `Duration`/`Option<Duration>` at construction time if
   that's cleaner for the sweep logic — your call, keep it simple).
2. Thread it through both `open_strict_with_rotation` and
   `open_batched_with_rotation`'s signatures and their inline
   `RotationPolicy { .. }` construction.
3. Update `server_launcher.rs`'s call site to read
   `config.audit.retention_days` and pass it through (mirroring exactly
   how `audit_max_bytes`/`max_file_size_mb` is already handled two lines
   above it).
4. In `rotate_locked`, after the rename+reopen succeeds, if
   `retention_days != 0`: scan the log file's parent directory for
   sibling files matching the rotated-file naming pattern
   (`{stem}.<unix_nanos>` — same pattern `rotate_locked` itself produces,
   check the exact `format!` call ~line 524 to derive the correct glob/
   prefix match), determine each candidate's age (parse the `unix_nanos`
   suffix from the filename directly — this avoids relying on filesystem
   mtime, which can be wrong after a backup/restore/copy — fall back to
   file mtime only if the filename doesn't parse), and delete (`fs::remove_file`)
   any whose age exceeds `retention_days * 86400` seconds. Log each
   deletion at `tracing::info!` (mirroring the existing `tracing::info!`
   call for rotation itself, ~line 531) and log a deletion FAILURE at
   `tracing::warn!` (mirroring the existing rotation-failure warn at ~line
   478/501) rather than propagating an `Err` — a failed retention sweep
   should never fail the write path that triggered it (the write itself
   already succeeded; retention is best-effort housekeeping, not a
   correctness requirement — mirror `rotate_locked`'s own
   already-established "log and continue" error philosophy for the SAME
   reason).
5. Update `07-operations.md`'s retention-days line (~line 342 — the one
   task 5a deliberately left untouched: "устаревшие файлы удаляются
   автоматически") to be ACCURATE now that the sweep is real — if you
   implement the sweep as described, this claim becomes TRUE and needs NO
   further change (the line already exists to make this claim!). Just
   confirm/state this, don't rewrite a line that's now correct.

## Tests

1. Configure a tiny `retention_days` (e.g. `0` explicitly meaning "never
   sweep" — regression) vs a real small value, create several
   already-rotated files with KNOWN ages (either by controlling the
   filename's embedded timestamp directly in the test, or by manipulating
   file mtimes if you go that route), trigger a NEW rotation, and confirm
   ONLY the files older than the retention window are deleted — files
   within the window survive.
2. `retention_days == 0` — confirm NO sweep occurs regardless of how old
   existing rotated files are (this is the documented off-switch,
   preserve it exactly).
3. A retention sweep failure (e.g. simulate a file that can't be deleted —
   check what's feasible on this test environment, a read-only file or a
   directory instead of a file might work, or skip this test with a
   documented reason if it's not practically simulatable) does NOT
   propagate an error that would fail the write/rotation that triggered
   it.
4. Regression: existing rotation tests
   (`crates/shamir-server/tests/audit_rotation.rs` — check the exact
   file/module name) continue to pass unchanged — rotation itself (the
   already-existing behavior) must not regress.
5. Regression: the batched vs strict mode constructors both correctly
   thread the new retention parameter (test both, or at minimum confirm
   both compile and construct correctly with a sample config).

## Out of scope

- Do NOT add a separate background timer/cron-style sweep — the
  rotation-triggered sweep described above is the intended, sufficient
  mechanism (rotated files are, by construction, the only files subject
  to retention; the ACTIVE log file is never swept).
- Do NOT touch the audit-COVERAGE line in `07-operations.md` (task 5a,
  already committed) or the Backup section (task 5b, already committed).
- Do NOT touch `data-protection.md` (task 5c, already committed).
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, or Этап 4's funclib top-up — this brief is scoped to the
  `audit.retention_days` sweep only.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including all
  new/modified tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-server`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) `retention_days == 0` still means "no sweep"
  (the documented off-switch is preserved); (b) a sweep failure never
  propagates an error to the write/rotation caller; (c) whether you
  updated `07-operations.md`'s retention-days line (should need NO change
  if the sweep is correctly implemented — the claim becomes true) or, if
  you instead chose the "remove the knob" fallback path, exactly what you
  changed there and why.
