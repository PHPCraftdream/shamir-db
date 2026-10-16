# 120 — S5c-X PR8b-2: scope resync_required quarantine to the bound destinations

## Context

PR8b (#181) excludes `resync_required` destinations from downstream serving
and ordinary reads. When a subscription's profile cannot be resolved, a
global sentinel quarantines reads of ALL databases, because the subscription
row does not durably record which destinations it writes. The owner decided
(2026-10-16): minimise blocking as far as possible with ZERO loss of
reliability. The product is unreleased: the subscription catalogue format
may change without migration; rows lacking the new binding are refused at
open with a typed error (operator replaces/reseeds).

The binding design is the reviewer verdict in the appendix. Implement it in
full (sections 1–6), including its merge criteria and the CI evidence table.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other
  compile or test command, by you or any child agent (repeat this rule in
  every child brief). Only `cargo fmt -p <crate>` for touched crates as the
  last step. CI is the gate; tests run only through `./scripts/test.sh` in
  CI. Clippy runs with `-D warnings` (`await_holding_lock`,
  `type_complexity`, `needless_borrow`, `clone_on_copy`, unused imports —
  gate test-only imports with `#[cfg(test)]`).
- Never silence a lint with `allow`.
- Lock-free read path: `arc_swap::ArcSwap` snapshot of an Fx-hashed map
  (`shamir_collections::THasher`), no `DashMap` guard, no allocation on the
  healthy path.
- Layout ratchet: at most 7 entries per folder, at most 1000 lines per file;
  files and folders already over a limit must not grow (new code goes to a
  new file or subfolder).
- Imports at the top of the file; `mod.rs` files only re-export; tests in
  `tests/` folders, one topic per file, every new test file wired into its
  `tests/mod.rs`; queries only through the builder (Rust and TS clients).
- No machine-specific absolute paths in any file you write.
- Futures on deep inline chains (nested Batch, admin dispatch, ALTER
  SUBSCRIPTION RESUME, seed install) must not grow: heap-pin new heavy
  awaits (the admin replication arms already sit behind a boxed
  `dispatch_repl`).
- Every `impl Trait` function returns one concrete type on every branch.
- Before finishing, one verifier child agent (same no-compile rule) reads in
  full every changed file plus direct callers in all crates (benches,
  examples, tests, TS client) and checks name resolution, privacy, imports,
  `cfg(test)` gating, signatures, `Send` across awaits and the clippy lints
  above.

## Final message

Per merge criterion and per row of the CI evidence table: what was built,
file:line, the test name; measured line counts for every touched file above
900 lines.

## Appendix — binding design (reviewer verdict)

## Verdict: APPROVE the direction, with mandatory binding and lifecycle safeguards

**PR8b-2 should make the subscription row—not the current replication profile—the durable authority for quarantine coverage.** Remove the global sentinel for valid new-format rows. Refuse legacy/invalid rows at open with a typed error; do not guess their destination from the database executing the admin command.

Merely adding `destination_db` and `destination_repo` and changing the lookup is **not sufficient**: profile replacement, in-flight followers, destination rename, and subscription replacement could otherwise detach the quarantine from data already written.

**Evidence boundary:** OBSERVED below means observed in source, not runtime-verified. Recommendations are design requirements. No builds or tests were run.

## 1. Durable subscription binding

### OBSERVED

- Creation persists `name`, `upstream`, `publication`, `profile`, and `state`, with optional seed binding: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:314–332`.
- There can be **multiple Pull destinations**, including destinations in different databases: `crates/shamir-server/src/replication/supervisor.rs:293–313`.
- Current quarantine coverage is recomputed from the **current profile**, not recorded destination evidence: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:965–995`.
- Missing profile resolution is currently accepted during subscription creation: `crates/shamir-db/src/shamir_db/execute/replication_seed/control.rs:182–191`.
- Seed binding currently writes only `seed_receipt`: `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:97–100`.

### Required design

Each row must contain a versioned, validated binding:

- `binding_version`.
- A canonical, deduplicated **destination set**, each entry containing explicit local database and repository names. A singular pair is insufficient unless the PR also rejects multi-target subscriptions.
- A durable **binding generation**, changed whenever the executable binding changes.
- Existing upstream endpoint and publication.
- Existing seed receipt/source identity evidence where required by the receipt-based apply protocol.

**Upstream identity is not a quarantine lookup key.** A local destination remains blocked even when the upstream endpoint, source identity, or profile is missing or wrong. Preserve the existing lineage/history-epoch/schema fences; those are separate reliability checks implemented at `crates/shamir-db/src/shamir_db/execute/replication_seed/control.rs:21–38`.

The destination set must be an **enforced write boundary**, not descriptive metadata: follower startup and apply must never target a destination outside the durable binding. Missing or changed profiles must not silently change that boundary.

**Choose concrete repositories only for new Pull bindings.** Current followers skip database-only scopes rather than expanding them (`crates/shamir-server/src/replication/supervisor.rs:582–590`), whereas quarantine currently expands them to a whole database (`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:985–988`). Reject unsupported database-wide Pull scopes at binding time instead of introducing new wildcard execution semantics. Do not expand a wildcard once into today’s repository list and claim that covers future repositories.

## 2. Legacy or malformed rows: typed refusal at open

**Choose refusal at open, not “all destinations in the database they live in.”**

### OBSERVED

Subscriptions are read from the shared system catalogue, not a catalogue owned by the destination database: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:761–767`. The admin request’s database therefore cannot establish a safe containment boundary.

### Required behavior

- Validate **every subscription row**, including active and paused rows—not only already-quarantined rows.
- Missing fields, unsupported binding versions, malformed destination entries, or ambiguous bindings cause a dedicated typed catalogue error identifying the subscription.
- Return that error before the initialized `ShamirDb` becomes available. The existing initialization seam already refreshes quarantine before returning: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:525–529`.
- Do not silently discard old rows or automatically reconstruct historical destination coverage from today’s profile. Neither proves where an older follower wrote.
- With no released format to migrate, require offline replacement/reseed or a separately justified rewrite with authoritative destination evidence.

This can refuse the whole instance when an old row is unknowable. **That is unavoidable uncertainty, not justification for a permanent global read sentinel in the supported format.** Database-scoped guessing would have a smaller apparent blast radius but is not fail-closed.

## 3. Read-path lookup: constant-time and lock-free

### OBSERVED

- The cache currently uses `DashMap`, with exact, database-wildcard, and global-sentinel lookups: `crates/shamir-db/src/shamir_db/execute/repo_admin/quarantine_cache.rs:6–16`, `39–53`. It is not a strictly lock-free lookup implementation.
- Ordinary batch reads use the shared request gate: `crates/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:76–91`.
- The visitor checks nested read operations by destination: `crates/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:131–151`.
- Interactive transactions reuse that coverage: `crates/shamir-db/src/shamir_db/execute/db_tx.rs:60–95`.
- The typed storage error already carries the requested database, repository, and reason: `crates/shamir-storage/src/error.rs:136–158`; its wire code is `resync_required`: `198`.

### Required design

Publish an immutable, Fx-hashed quarantine map through **`ArcSwap`**, keyed by exact `(destination_db, destination_repo)`.

- Per destination: one snapshot load and one expected-O(1) hash lookup; no catalogue read, profile resolution, scan, or blocking map guard.
- Healthy path remains allocation-free.
- Whole-request work remains proportional to the query tree, not the subscription count.
- Build the map from quarantined rows’ durable destination sets, using the existing predicate: `state == resync_required OR quarantined == true` (`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:955–962`).
- Aggregate overlapping subscriptions: clearing one must not unblock a destination still covered by another.
- Make downstream serving consume the same authority. Its current coverage helper rebuilds and scans catalogue-derived entries: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:732–743`.

**No global sentinel is needed for valid rows.** Catalogue read/validation failure must fail initialization or retain the last safe runtime snapshot; it must never publish an empty replacement.

## 4. Ordering, durability, and lifecycle

### Creation and seed installation

Write the destination set, binding generation, profile reference, upstream/publication, state, and receipt binding in the **same subscription-row transaction**. Persist associated field-name metadata and complete the required durability barrier **before follower startup is allowed**.

Existing creation performs transaction, interner persistence, and catalogue flush in sequence (`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:340–358`); the flush explicitly flushes buffers, history, and metadata (`crates/shamir-db/src/shamir_db/shamir_db/seed_catalogue.rs:63–70`). These are implementation seams, not proof of crash ordering. Both ordinary creation and offline seed installation must use the new binding invariant; the latter independently constructs rows at `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:176–191`.

### Terminal quarantine

Preserve this ordering:

1. Publish affected destination keys.
2. Revoke and drain affected live delivery gates.
3. Persist and flush terminal state.
4. Reconcile the cache without losing prepublished protection on persistence failure.

The current terminal path already prepublishes and revokes before persistence (`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:890–903`, `938–950`). Do not regress it.

### Rebind and profile replacement

**For this narrow PR, refuse destination-changing rebinds and same-name subscription overwrites.** Permit profile changes only when their executable destination set remains identical and receipt constraints still hold.

Reason: reconciliation currently keeps a running follower based on subscription name, active state, and profile name—not a durable binding generation (`crates/shamir-server/src/replication/supervisor.rs:232–248`). A profile can change under an unchanged name. Generation-aware startup, stopping, and terminal callbacks are required so an old task cannot mark or clear a replacement binding.

Do not release an old inconsistent destination merely because a profile now points elsewhere.

### Destination DROP/RENAME

**Refuse DROP/RENAME of a destination database or repository while referenced by a binding or unresolved quarantine.** Apply this at the underlying facade APIs, not only admin dispatch.

Current rename moves live data under new names (`crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:165–174`, `542–556`); name-keyed quarantine must not remain behind under the old name. Supporting transactional rename propagation is unnecessary scope for PR8b-2.

### Subscription DROP

Current DROP can lift quarantine (`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:392–398`). **Dropping a subscription is not evidence that its data became consistent.** To meet the owner’s reliability requirement, refuse DROP of quarantined subscriptions in this PR; do not add a tombstone subsystem merely to support it. A future explicit recovery/destruction workflow can define safe removal.

## 5. Required CI evidence

Extend the existing quarantine/read and replication lifecycle suites; do not replace production-path tests with cache-only assertions.

| Required case | Acceptance result |
|---|---|
| Quarantine `A/main`; read `A/side` and populated `B/main` | Only `A/main` refuses; unrelated reads return their actual data |
| One subscription has multiple destinations across databases | Every bound destination refuses; everything outside the set remains readable |
| Delete, corrupt, or recreate the profile after binding | Original bound destinations remain blocked, including after reopen |
| Missing new fields; malformed or unsupported binding | Typed open refusal, including active/paused legacy rows |
| Profile update, SetProfile, same-name creation, stale terminal callback | No destination escapes the durable binding; no replacement binding is mutated by an old generation |
| Terminal persistence/flush failure | Affected live reads remain blocked; unrelated destinations stay readable |
| Two quarantined subscriptions overlap | Clearing one does not clear the other’s protection |
| Quarantined destination DROP/RENAME and subscription DROP | Refused before any live or durable mutation |
| Valid replacement seed and Resume | Only destinations proven recovered become readable; stale receipt/state flips do not suffice |

Retain coverage for nested batches, interactive transactions opened before quarantine, direct table acquisition, journal reads, cursors/live receivers, downstream Hello/Pull, and seed export. Existing entry points include `crates/shamir-db/src/shamir_db/execute/db_tx.rs:32–38`, `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:367–379`, and receiver revocation at `crates/shamir-db/src/shamir_db/execute/repo_admin/revocation/registry.rs:9–31`.

Replace the old **global-fallback expectation**, not its safety obligation: `crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs:522–608`. Its replacement must first create a valid durable binding, then make the profile unresolvable, and prove the original destinations still refuse.

## 6. Scope and merge criteria

**In scope:** versioned destination bindings; all row writers; typed open validation; enforced follower binding/generation; profile-mutation guards; lock-free destination cache; existing read/serve/revocation consumers; narrow lifecycle guards; CI regression coverage.

**Merge only when:**

- Valid rows never need profile resolution to establish quarantine coverage.
- No follower can write outside its durable destination binding.
- Missing profiles never unblock affected data or block unrelated destinations.
- Restart and failed persistence preserve protection.
- Rebind, rename, drop, and stale callbacks cannot detach quarantine from inconsistent data.
- CI demonstrates both refusal of affected reads and successful reads of real unrelated data.

**Non-goals:** legacy migration, wildcard Pull expansion, transactional destination rename, general replication redesign, revocation of previously acquired raw table handles, or unrelated cleanup. The existing raw-handle limitation is explicit at `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:367–371`; this PR must not claim to solve it.

**Files changed:** none. **Builds/tests run:** none; verification reserved for GitHub CI as requested.