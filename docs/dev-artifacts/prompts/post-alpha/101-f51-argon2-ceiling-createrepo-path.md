# Brief for F-51 (#862, P1) — base deploy profile Argon2 ceiling + remove CreateRepo::path builder method

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. A 2026-07-28 review flagged three
"truthfulness" findings — docs/config claiming something the code doesn't
actually deliver. This session independently re-verified all three against
the current tree:

- **Finding 3 (KNOWN_LIMITATIONS.md FK/RI-barrier overclaim) is ALREADY
  RESOLVED** — it was correctly updated as part of this session's own F-46
  commit (`57382bab`). An Explore-agent verification confirmed the current
  `docs/guide-docs/KNOWN_LIMITATIONS.md` FK section (lines ~90-215)
  accurately reflects the fixed state, with no remaining overclaim. **Do
  NOT touch this file for finding 3 — nothing to do there.**
- **Findings 1 and 2 are still open** — this brief scopes ONLY those two.

## Finding 1 — `deploy/server.example.ktav`'s Argon2 ceiling is undocumented and disproportionate

Confirmed current state:
- `deploy/server.example.ktav` (the BASE profile — the first thing a new
  operator copies, per its own framing vs. `server.large.example.ktav`'s
  explicit "opt-in only" framing) has `kdf_defaults.memory_kb: 131072`
  (:18) and `argon2_concurrent_max: 64` (:28), with its own comment
  admitting "64 × 128 MB = 8 GB worst case" (:26-27) — but states NO target
  RAM for this profile anywhere, unlike its siblings.
- `deploy/server.small.example.ktav` and `server.medium.example.ktav`
  **already do this correctly** — each documents a target RAM ("1–2 GiB
  container", "4–8 GiB container") in both its own header comment AND
  `deploy/README.md`'s profile table (`README.md:13-14,23-24`), and sizes
  `argon2_concurrent_max` (6 and 12 respectively) via the documented sizing
  formula ("`argon2_concurrent_max × memory_kb ≤ ~25% of your host RAM`").
  **Mirror this existing convention for the base profile — do not invent a
  new one.**
- `deploy/server.large.example.ktav` has the SAME sizing-formula comment
  (:14-17) as small/medium, but ships `argon2_concurrent_max: 64` (:43)
  with NO stated target RAM anywhere (not in its own header, not in
  `README.md`'s table, which only lists small/medium — `README.md:13-14`,
  confirmed via grep, "large" does not appear in that table at all). Its
  own sizing formula's worked example ("a 4 GiB container ... allows
  argon2_concurrent_max up to ~8") is inconsistent with its actual shipped
  value of 64 by 8x, with no explanation of what host size 64 is actually
  sized for.

**What to fix:**

1. **`server.example.ktav`**: lower `argon2_concurrent_max` from 64 to a
   conservative default consistent with a "safe to copy without reading
   further" base profile — the task's own investigation suggested 4-8;
   given `small.example.ktav` already uses 6 for a 1-2 GiB target, use the
   same value (6) OR add the same sizing-formula comment block the
   small/medium/large files already carry and pick a value consistent with
   it for whatever target RAM you document. Add a target-RAM statement to
   this file's header comment, matching the small/medium/large convention,
   and add this profile to `README.md`'s profile table too (it's
   conspicuously the one profile currently undocumented there — confirm
   this by re-reading the whole table before adding).
2. **`server.large.example.ktav`**: add an explicit target-RAM statement
   ("sized for hosts with N+ GiB RAM" — pick a number consistent with
   64 × 128 MiB = 8 GiB actually being ≤~25% of that host's RAM per the
   file's own formula, i.e. roughly 32 GiB+) to its header comment and to
   `README.md`'s table, resolving the current internal inconsistency
   between its worked example (~8 for 4 GiB) and its shipped value (64).
3. Do NOT invent a NEW sizing table or profile tier — small/medium/large
   already exist and are the correct existing structure; this task only
   brings the base and large profiles' documentation in line with what
   small/medium already do correctly.
4. **Do NOT add runtime config validation/RAM-detection logic** (e.g.
   reading host RAM at server startup to WARN if the ceiling exceeds a
   budget) — this would need a new dependency (no RAM-detection crate is
   currently in the workspace) and is a separate, larger design decision
   out of scope for a docs/config truthfulness fix. If you believe it's
   valuable, note it in your final summary as a suggestion for a future
   task — do not implement it.

## Finding 2 — `CreateRepo::path` builder method promises something the server always rejects

Confirmed current state:
- `crates/shamir-query-builder/src/ddl/create_repo.rs:81-85` still has a
  public `pub fn path(mut self, path: impl Into<String>) -> Self` with doc
  comment "Set the data path".
- `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:152-166`
  (F-43, #851) unconditionally rejects `CreateRepoOp.path.is_some()` with
  `unsupported_field` — "the server always resolves the storage location
  internally". So `.path(...)` on the builder is a promise the server
  guarantees to break every time it's used with a non-empty value.
- No call sites of `.path(` on the `CreateRepo` builder exist anywhere in
  `crates/shamir-query-builder`, `crates/shamir-client`, or
  `crates/shamir-sdk` (confirmed by grep) — removing it is a clean,
  zero-blast-radius deletion.
- The wire `CreateRepoOp.path: Option<String>` field itself
  (`crates/shamir-query-types/src/admin/types/repo_ops.rs:14`) MUST stay —
  it's the wire DTO shape the server's rejection logic reads; only the
  Rust builder's convenience method that lets a caller set it is the
  problem.

**What to fix:** remove the `path` method from
`crates/shamir-query-builder/src/ddl/create_repo.rs` entirely (this is a
pre-1.0-alpha library — no deprecated-shim needed, no back-compat
obligation per the project's own discipline against dead-code shims for
removed functionality). If the builder struct's internal `path: Option<String>`
field becomes unused after removing the setter, remove that too (check
whether it's still needed to serialize `CreateRepoOp.path` as always-`None`,
or whether the struct can omit the field and the conversion to
`CreateRepoOp` just hardcodes `path: None`).

Check for and update any doc/example code (`docs/guide-docs/`,
doctests, README snippets) that demonstrates `.path(...)` on this builder —
grep for it workspace-wide before finishing.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-query-builder -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Do NOT touch `docs/guide-docs/KNOWN_LIMITATIONS.md` — finding 3 is
  already resolved, confirmed above.
- Do NOT touch any F-50/F-46/F-47/F-48/F-49/#872 code — unrelated.
- This is a small, low-risk docs/config/builder-API task — no new tests
  should be needed for the deploy-file changes (they're plain-text
  config), but DO add/confirm a compile-time check that removing
  `CreateRepo::path` doesn't leave a dangling doc-example or unused-field
  warning.

## Verification the orchestrator will run

```
cargo fmt -p shamir-query-builder -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder --full
```

When done, give your final summary as plain text: the exact diffs to the
three deploy files (`server.example.ktav`, `server.large.example.ktav`,
`README.md`) and their new documented target-RAM/Argon2 values, the
`CreateRepo::path` removal and what (if anything) needed updating at call
sites, and confirmation fmt/clippy/tests are clean.
