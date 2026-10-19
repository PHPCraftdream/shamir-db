# S5c-X PR8b-2 W2 — binding-authoritative, generation-aware supervisor (shamir-server)

Master brief: `docs/dev-artifacts/prompts/remediation/120-s5cx-scoped-quarantine.md` (authoritative design, appendix §1 "enforced write boundary" and §4 "Rebind and profile replacement"). Stage W1 (already landed in the working tree) added `shamir_db::SubscriptionBinding` (crate-root re-export), `mark_subscription_terminal_generation(name, state, message, Option<u64>)` and `mark_subscription_resync_required_generation(name, gap_at, from_version, u64)` on `ShamirDb`, and binding fields on every subscription row. READ the W1 code in the working tree before starting.

## Rules (binding)

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO compile/test/lint commands of any kind. Only `cargo fmt -p shamir-server` as your LAST step. CI is the gate; resolve every name by reading real definitions first.
- Never silence a lint with `#[allow]`.
- Layout ratchet: ≤7 entries per folder, ≤1000 lines per file; over-limit files/folders must not grow. `crates/shamir-server/src/replication/supervisor.rs` is 618 lines (headroom to 1000); `crates/shamir-server/src/replication/follower_loop.rs` is 571; `src/replication/tests/` is ALREADY OVER the 7-entry limit (8 entries) — do NOT add files there, edit existing files in place.
- Imports at file tops. No machine-specific absolute paths. Queries only through the builder.

## Work items

### 1. `crates/shamir-server/src/replication/supervisor.rs`
- `Subscription` gains `pub binding_generation: u64` and `pub binding_destinations: Vec<(String, String)>` (doc: the DURABLE authority for follower targets and quarantine coverage — the profile is not).
- `read_subscriptions`: parse each row with `shamir_db::SubscriptionBinding::from_row(&r)`; a row that fails validation → return `Err` naming the subscription (reconcile logs a warning and leaves running loops alone — fail-safe, never fail-open). Build the two new fields from the parsed binding (drop the `unwrap_or("")` permissiveness for these fields only; keep the rest of the mapping as is).
- `start_subscription`: the pull targets are `sub.binding_destinations` (the enforced boundary), NOT profile-derived `pull_target` filtering. Still resolve the profile streams (existing code) and REQUIRE that every binding destination is covered by a concrete Pull stream of the resolved profile (a binding destination not covered → `ReplError::Start` "profile does not cover bound destination ..." → no start; this keeps the existing admission/hello machinery honest). Keep the duplicate-owner and reopen-required per-target checks and their `mark_subscription_terminal(..., None)` startup markings (they run under the reconcile lock against the current row). The `pull_target` helper becomes unused — remove it and its doc, or keep it only if some other call site remains (grep first).
- `SubHandle` gains `generation: u64`.
- `reconcile` keep-condition (registry stop pass): keep a loop only when the row has the same name, `is_active()`, same profile, AND the same `binding_generation` as the handle; otherwise stop + join (a generation change means the binding was replaced — an old task must never survive to mark or clear the replacement).
- `FollowerLoopConfig` construction: pass the subscription's `binding_generation` (see item 2), and capture it in the spawned task closures.
- Terminal callbacks in the spawn closures: the journal-gap path calls `local.mark_subscription_resync_required_generation(&sub_name, gap_at, from_version, generation)`; the identity-fence quarantine path calls `local.mark_subscription_terminal_generation(&sub_name, "resync_required", &e.to_string(), Some(generation))`. The `ReopenRequired` and generic-terminal arms keep the generation-less entries (they are startup/operational markings, still safe under the reconcile lock). Update the comments that describe reconciliation by "name + active + profile" to include the binding generation.

### 2. `crates/shamir-server/src/replication/follower_loop.rs`
- `FollowerLoopConfig` gains `pub binding_generation: u64` (documented: the generation this loop was started under; forwarded to terminal marking so a stale task cannot mutate a replacement binding). Set it in `new()` to 0 and have the supervisor assign it explicitly (or add a `with_binding_generation` builder mirroring the existing builder style — match the file's conventions).

### 3. Compile-compat sweep across `crates/shamir-server` (edit in place only)
- Grep for `Subscription {` struct literals (tests construct it directly, e.g. supervisor tests) and fill the two new fields (`binding_generation: 1`, `binding_destinations: vec![...]` matching the fixture's scope).
- Grep for `FollowerLoopConfig {` literals / `FollowerLoopConfig::new(` in tests and set the new field where structs are built literally.
- Grep for `read_subscriptions` stub rows in tests: any test that seeds a subscription row by hand must write the binding fields (`binding_version`, `binding_generation`, `binding_destinations`) — mirror what W1's `SubscriptionBinding::write_to_row` produces (check the exact field names/shapes in `crates/shamir-db/src/repl_binding/binding.rs` and reuse `shamir_db::SubscriptionBinding` where the test has access to it).
- `replication/seed/terminal.rs::persist_terminal` calls `mark_subscription_terminal` — signature unchanged, leave it, but confirm it still compiles conceptually (same args).
- Check `replication/tests/supervisor_tests.rs`, `pull_bounds_tests.rs`, `seed/tests/loop_tests.rs`, `subscriptions/tests/quarantine/quarantine_termination_tests.rs` for wildcard (bare-db) Pull scopes in fixtures — W1 made creation refuse them; convert those fixtures to concrete repos IN PLACE (the tests folder is over the entry limit; no new files).

## Acceptance (self-review — no compiling)

- Every binding destination a loop can target comes from the durable binding (grep: no remaining profile-derived target construction on the start path).
- A stale generation can reach only `..._generation(..., Some/its own g)` entries, and W1's mismatch check makes those no-ops on a replaced binding.
- `wc -l` printed for every touched file (all ≤1000); no folder over 7 entries grew; imports at tops; no `allow`s.
- Final step: `cargo fmt -p shamir-server`.
- Report: files changed with line counts, decisions taken where the brief was silent, unresolved call sites.
