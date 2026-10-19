# S5c-X PR8b-2 W1 — durable subscription binding, lock-free quarantine cache, typed open validation (shamir-db + shamir-storage)

Master brief: `docs/dev-artifacts/prompts/remediation/120-s5cx-scoped-quarantine.md` (read it first — it is the authoritative design, "Appendix" sections 1–6). This stage implements the shamir-db/shamir-storage production half of that design.

## Rules (binding)

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` / `rustc` / `cargo doc` or ANY compile/lint/test command. Only `cargo fmt -p shamir-db -p shamir-storage` as your LAST step. CI is the gate. Because you cannot compile, resolve every name by reading the real definitions before you use it; prefer mirroring existing code in the same files over inventing new patterns.
- Never silence a lint with `#[allow]` / `#[expect]`.
- Layout ratchet: ≤7 entries per folder, ≤1000 lines per file; folders/files already over a limit must not grow. Measure every file you touch with `wc -l` after the final edit and keep it under limits. `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` is at 998 lines — it MUST end ≤1000 (remove moved-out code before/while adding new code).
- Imports at the top of the file. `mod.rs` files contain re-exports only. Do not embed `#[cfg(test)] mod tests` inline in implementation files.
- No machine-specific absolute paths.
- Futures on deep inline chains must not grow: keep the quarantine gate helpers synchronous and `#[inline(never)]` (see the Round-12/14 comments in `db_tx.rs` / `db_execute.rs`); heap-pin (`Box::pin`) any new heavy await added inside admin handlers, exactly like the existing ones.
- Queries only through the query builder (the existing system-catalogue read pattern `table.read(&ReadQuery::new(...), &ctx)` is the sanctioned pattern — reuse it, do not hand-roll JSON).

## Context you need (verified against the working tree)

- The quarantine cache: `crates/shamir-db/src/shamir_db/execute/repo_admin/quarantine_cache.rs` (DashMap-based; `global_sentinel_key`, `unresolvable_profile_reason`, `publish`, `install`, `reason_for` with wildcard fallbacks, `is_empty` via `AtomicUsize`).
- Cache consumers: `revocation/registry.rs` (`refresh_resync_quarantine_cache`, `revoke_quarantined_receivers` with a `reason_for(&db, "*")` arm), `admin_replication.rs` (`resync_quarantine_entries` 758–788, `quarantined_read_reason`, `has_resync_required_subscription_covering` 732–744, `mark_subscription_terminal` 860–952, `quarantine_row_entries` 965–998, `row_is_quarantined` 955–963), `db_execute.rs` + `db_tx.rs` (read gates — they only use `is_empty()` and `reason_for(db, repo)`, keep those method names and signatures working).
- Open seam: `shamir_db/shamir_db/core.rs:525–527` calls `refresh_resync_quarantine_cache().await?` before `init_inner` returns — this is where a typed catalogue error must abort open.
- Row writers today: `admin_replication.rs::handle_create_subscription` (314–358), `handle_alter_subscription` (Pause/Resume/SetProfile arms ~489–565), `handle_drop_subscription` (364–404, currently lifts quarantine), `handle_create_replication_profile` (67–113, upsert), and `replication_seed/catalogue.rs::install_seed_subscription` (138–194, offline seed install) + `persist` (79–95).
- `replication_seed/control.rs::bind_seed_creation` (182–216) currently ACCEPTS an unresolvable profile (`let Ok(scope) = streams(...) else { return Ok(()) }`) — this must become a refusal.
- `QueryValue` is `shamir_types::types::value::Value<String>`: variants include `QueryValue::Str(String)`, `QueryValue::Int(i64)`, `QueryValue::Bool(bool)`, `QueryValue::Null`, `QueryValue::Map(TMap<String, QueryValue>)` (build with `crate::types::common::new_map()`), `QueryValue::List(Vec<QueryValue>)`; read ints with `.as_u64()`, strings with `.as_str()`. Row maps are plain `TMap`s — adding fields needs no DDL.
- `DbError` lives in `crates/shamir-storage/src/error.rs`; there is a compile-time assert `size_of::<DbError>() <= 64` — any new variant carrying Strings must box them (precedent: `ResyncRequired(Box<ResyncRequiredInfo>)` with a `DbError::resync_required(...)` constructor and a `code()` arm).
- `arc-swap = "1.7"` is NOT yet a dependency of shamir-db (shamir-server/-storage/-types already use that exact version string).
- Tests that write subscription rows or rely on old behavior live in `crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs` (the `resync_marking_with_unresolvable_profile_fails_closed_globally` test at 522–608 encodes the GLOBAL SENTINEL — it must be replaced in this stage), `execute/replication_seed/tests/quarantine_reads_tests.rs` (concrete scopes; should keep working), `shamir_db/shamir_db/seed/tests/catalogue_tests.rs` (check whether it writes raw subscription rows; if yes, give those rows binding fields).

## Work items

### 1. `crates/shamir-db/Cargo.toml`
Add `arc-swap = "1.7"` to `[dependencies]` with a one-line comment matching sibling style (RCU snapshot for the quarantine cache).

### 2. NEW module `crates/shamir-db/src/repl_binding/` (parent `crates/shamir-db/src/` has 4 entries; adding this folder keeps it at 5)
- `mod.rs` — re-exports only (`pub mod binding; pub mod coverage; pub mod validate;` plus `pub use` of the public items).
- `binding.rs` — one primary export `SubscriptionBinding`:
  - `pub const SUBSCRIPTION_BINDING_VERSION: u64 = 1;`
  - Fields: `version: u64`, `generation: u64`, `destinations: Vec<(String, String)>` (canonical: deduplicated; keep them sorted for deterministic equality).
  - `pub fn from_row(row: &QueryValue) -> Result<Self, DbError>` — strict parse: `binding_version` present and == 1 (anything else → typed error "unsupported binding version"); `binding_generation` present, integer ≥ 1; `binding_destinations` present, a list of maps each with non-empty `db` and `repo` strings, `db != "*"`, `repo != "*"`; duplicates → "ambiguous duplicate destination" error; empty list → error naming the subscription. Every failure is `DbError::subscription_catalogue(name, reason)` naming the row's `name` (fallback `"<unnamed>"`).
  - `pub fn from_streams(subscription: &str, streams: &[ReplStream]) -> Result<Vec<(String, String)>, DbError>` — the executable destination set: Pull-direction streams only; each must have an explicit, non-empty `repo` (a database-wide Pull scope is REJECTED — this is the verdict's "reject unsupported database-wide Pull scopes at binding time") and `table: None`; sorted + deduplicated; empty result → error ("profile has no concrete Pull destination"). Errors use the typed catalogue constructor.
  - `pub fn write_to_row(&self, row: &mut QueryValue)` — inserts `binding_version` (Int), `binding_generation` (Int), `binding_destinations` (List of `Map{db, repo}`) into the row map.
  - Accessors: `destinations()`, `generation()`, `version()`. `pub fn new(version, generation, destinations) -> Result<Self, DbError>` constructor used by writers (runs the same canonicalization/validation).
- `coverage.rs` — quarantine coverage from durable rows:
  - Move `row_is_quarantined` here verbatim (from admin_replication.rs:955–963) — `pub(crate)`.
  - `pub(crate) fn quarantined_row_keys(row: &QueryValue, binding: &SubscriptionBinding, reason: &str) -> Vec<((String, String), String)>` — one entry per bound destination.
  - `impl ShamirDb { pub(crate) async fn resync_quarantine_entries(&self) -> DbResult<Vec<((String, String), String)>> }` — moved from admin_replication.rs:758–788 but REWRITTEN: read all rows (helper below), for every `row_is_quarantined` row parse `SubscriptionBinding::from_row` and emit its keys; a row that fails parsing propagates the typed error (NO sentinel fallback, NO profile resolution — delete all of that). Reason = row `status_message` or `"subscription requires resync"`.
- `validate.rs` — open-time typed validation + lifecycle guard:
  - `pub(crate) async fn read_subscription_rows(shamir: &ShamirDb) -> DbResult<Vec<QueryValue>>` — the standard catalogue read (mirror `admin_replication.rs::read_all`).
  - `pub async fn validate_subscription_catalogue(shamir: &ShamirDb) -> DbResult<()>` — validates EVERY row (active, paused, quarantined): required string fields `name` (non-empty), `upstream`, `publication`, `profile` (non-empty), `state` (non-empty), plus a fully valid binding via `SubscriptionBinding::from_row`. First failure is returned as the typed error; do not continue.
  - `impl ShamirDb { pub(crate) async fn ensure_destinations_unbound(&self, db: &str, repo: Option<&str>) -> DbResult<()> }` — reads rows, parses each binding (invalid → propagate the typed error), and returns `DbError::Validation` naming the referencing subscription if any destination matches `(db, repo)` (`repo: None` = any repo in `db`). This is the DROP/RENAME guard used in work item 6.
- Wire the module in `crates/shamir-db/src/lib.rs` following its existing module-declaration style, and re-export `SubscriptionBinding` and `SUBSCRIPTION_BINDING_VERSION` at the crate root next to the existing re-exports (shamir-server needs them in stage W2).

### 3. `crates/shamir-storage/src/error.rs`
Add a boxed variant (keep the ≤64-byte assert holding):
```rust
#[error("subscription catalogue row '{}' is invalid: {}", .0.subscription, .0.reason)]
SubscriptionCatalogue(Box<SubscriptionCatalogueError>),
```
plus `pub struct SubscriptionCatalogueError { pub subscription: String, pub reason: String }` and `impl DbError { pub fn subscription_catalogue(subscription: &str, reason: impl Into<String>) -> Self }`, and a `code()` arm returning `Some("subscription_catalogue_invalid")`. Mirror the existing `ResyncRequired`/`ResyncRequiredInfo` construction pattern exactly.

### 4. Rewrite `quarantine_cache.rs` — ArcSwap snapshot, exact keys only
- `pub(crate) struct ResyncQuarantineCache { snapshot: arc_swap::ArcSwap<QuarantineSnapshot> }` with an immutable
  `struct QuarantineSnapshot { entries: HashMap<String, HashMap<String, Arc<str>>, THasher>, pending: TSet<(String, String)> }`
  (`shamir_collections::{THasher, TSet}`, `std::collections::HashMap`). Two levels keyed by db then repo so `reason_for(&str, &str)` does two borrow-only `get(&str)` lookups — zero allocation unless a quarantine hit clones the reason.
- `is_empty()` → one `load()` + `entries.is_empty()` (delete the `AtomicUsize`). `reason_for(db, repo)` → exact hit only; DELETE `global_sentinel_key`, `unresolvable_profile_reason`, and every wildcard arm (update their one import site in admin_replication.rs).
- `publish(full_entries)` (called under `replication_control_lock` before persistence): build a NEW snapshot = current entries updated with `full_entries` where EXISTING keys keep their current reason (preserves the old vacant-only semantics) and newly added keys are recorded in `pending`; swap it in.
- `install(full_entries)` (after successful persistence): fresh map from `full_entries`; `pending` shrinks by fresh keys; surviving pending keys (with their current reasons) are merged back; swap. Never publishes an empty replacement over a failure — callers simply don't call it on error, so the last safe snapshot stays.
- Keep `skip_read_quarantine_for_test` + the `SKIP_READ_QUARANTINE_FOR_TEST` task-local exactly as they are.
- Document on the struct: no global sentinel exists anymore; catalogue validation failure fails initialization or retains the last safe runtime snapshot and never clears protection.

### 5. `revocation/registry.rs`
- `refresh_resync_quarantine_cache`: FIRST `crate::repl_binding::validate::validate_subscription_catalogue(self).await?`, then entries + `install` + `revoke_quarantined_receivers`. Update the doc: validation failure → `Err` → open fails / runtime keeps the last snapshot.
- `revoke_quarantined_receivers`: exact `(db, repo)` lookup only (drop the `reason_for(&db, "*")` arm).

### 6. `admin_replication.rs` (CEILING: file must end ≤1000 lines; it starts at 998)
- DELETE `quarantine_row_entries` and `row_is_quarantined` (moved to repl_binding) and the `quarantine_cache::{global_sentinel_key, unresolvable_profile_reason}` import. Move the body of `resync_quarantine_entries` out (it now lives in `coverage.rs` as the same method name — callers keep calling `self.resync_quarantine_entries()`).
- `has_resync_required_subscription_covering`: body becomes `Ok(self.quarantined_read_reason(db_name, repo_name).is_some())`; rewrite the doc (exact bound destinations, constant time, no wildcard/db-wide scope — database-wide Pull scopes are rejected at binding time).
- `mark_subscription_terminal`: replace the profile-resolution block (~887–904) with binding-based coverage — parse the row's binding with `SubscriptionBinding::from_row` (parse failure → propagate the typed catalogue error: a quarantined row without a valid binding must fail loudly, never mark nothing), then `quarantined_row_keys` → `publish` → `revoke_quarantined_receivers`, preserving the exact existing ordering: (1) publish keys, (2) revoke+drain receivers, (3) persist + interner + flush, (4) refresh. On persist/flush failure the prepublished keys stay (refresh is not reached) — do not regress this.
- Generation awareness: add `pub async fn mark_subscription_terminal_generation(&self, name: &str, state: &str, message: &str, expected_binding_generation: Option<u64>) -> DbResult<()>` containing the (renamed) current logic; when `Some(g)` is given and the row's `binding_generation != g`, log a `warn!` and return `Ok(())` WITHOUT touching the row or the cache (a stale follower callback must never mark or clear a replacement binding). Existing `mark_subscription_terminal` delegates with `None`; existing `mark_subscription_resync_required` delegates with `None` (both keep their current public signatures — many tests/crates call them). Add `pub async fn mark_subscription_resync_required_generation(&self, name: &str, gap_at: u64, from_version: u64, expected_binding_generation: u64) -> DbResult<()>` delegating with `Some(gen)` for the supervisor (stage W2).
- `handle_create_subscription`: (a) refuse same-name overwrite — read the existing row by name first and return a typed coded error (use `BatchError::query_coded("", "already_exists", ...)` naming the subscription) when it exists; (b) resolve the profile via `super::replication_seed::catalogue::streams` (map `SeedRefusal` through `super::replication_seed::control::batch_error` — an unresolvable profile is now a REFUSAL) and build the binding via `SubscriptionBinding::from_streams(&name, &streams)` → `SubscriptionBinding::new(SUBSCRIPTION_BINDING_VERSION, 1, destinations)` → `write_to_row(&mut m)` so the binding lands in the SAME subscription-row `SetOp` transaction (interner persist + flush follow as today, before any follower may start). Heap-pin any new await chain on this path the way the file already does.
- `handle_alter_subscription` SetProfile arm: resolve the proposed profile's streams (missing profile → typed refusal — the destination set is unknowable); compute `from_streams` for it and require EXACT equality with the row's existing binding destinations; mismatch → coded error `"binding_destination_mismatch"` naming both sets. Equal → insert the new profile name; binding and generation stay unchanged.
- `handle_create_replication_profile` (upsert): when the profile name already exists AND at least one subscription row references it, the proposed streams' Pull destination set must equal each referencing row's binding destinations, else coded `"binding_destination_mismatch"`. Rows not referencing the profile are unaffected. (`validate_seed_profile_change` keeps running.)
- `handle_drop_subscription`: BEFORE the delete, read the row; if `row_is_quarantined` → typed coded refusal (code `"resync_required"`, message: dropping a quarantined subscription is refused — dropping is not evidence of consistency; follow the reseed runbook). Non-quarantined rows drop as today.

### 7. `replication_seed/control.rs` + `catalogue.rs`
- `bind_seed_creation` (control.rs:182–216): DELETE the silently-accepting branch — `streams(self, &op.profile)` errors now propagate as a typed refusal (reuse the closest existing `SeedRefusal` variant, e.g. `WrongScope`, consistent with the file's style); the destination set from `SubscriptionBinding::from_streams` must succeed (wildcard Pull → refusal). The per-repo receipt checks stay.
- `validate_seed_resume`: the "unbound legacy row keeps ordinary behaviour" branch (unresolved profile + no `seed_receipt` → `Ok(())`) is unreachable now but harmless — remove it ONLY if removal is trivially safe; otherwise leave it and note it.
- `install_seed_subscription` (catalogue.rs:138–194): compute the binding from the resolved `scope` streams (they are already concrete by `matches_scope`), write it into the NEW-row branch via `write_to_row` in the same `persist` record, and in the exact-retry branch require the existing row's binding to equal the computed one (`ConflictingBinding` otherwise).

### 8. `db_management.rs` — destination DROP/RENAME guards at the facade
- `rename_db_as`, `rename_repo_as`, `remove_repo`: call `ensure_destinations_unbound` FIRST (before any mutation), with `repo: None` for db-level ops and `Some(repo)` for repo-level ops; typed error propagates.
- `remove_db(&self, name) -> bool` (line 82): change to `DbResult<bool>` so the guard's typed error can propagate (`Ok(false)` when absent as today); update every caller (grep the whole workspace: admin dispatch in `admin_db_repo.rs`, tests) to `?` the result. Refusal must happen before any live or durable mutation.

### 9. Existing tests — keep them coherent with the new contract (no new test files in this stage)
- `crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs`: REPLACE `resync_marking_with_unresolvable_profile_fails_closed_globally` (522–608) with the binding contract: create profile_a with a CONCRETE scope via the builder, create sub_a, `mark_subscription_resync_required` (still compiles — delegates with `None`), assert persisted state/marker as before, then `DROP REPLICATION PROFILE profile_a` and assert `testdb/main` (the bound destination) is STILL refused with the typed `resync_required` code while an unrelated populated repo (`testdb/side` — create it in the fixture if needed) still returns its actual data. Name it e.g. `resync_marking_keeps_bound_destinations_blocked_without_profile`. Keep the status/list leg.
- Scan the same file plus `execute/replication_seed/tests/*.rs` and `shamir_db/shamir_db/seed/tests/catalogue_tests.rs` for: (a) raw subscription rows written without binding fields — add binding fields (build a `SubscriptionBinding` and `write_to_row`); (b) `set_profile`/bare-db-scope fixtures that the new guards would refuse — switch them to equal-destination profiles or concrete scopes; (c) anything relying on the global sentinel or on `reason_for` wildcards.
- In `crates/shamir-server/src` (compile-compat only; the supervisor itself is stage W2): grep tests creating profiles/subscriptions with BARE-DB Pull scopes (`repl_scope("db")` without `.repo(`, or `ReplScope { repo: None }` with `Pull`) used in subscription creation — since creation now refuses wildcards, give those fixtures concrete repos. Also check `subscriptions/tests/quarantine/quarantine_termination_tests.rs`, `db_handler/tests/repl_handler_tests.rs`, `replication/seed/tests/loop_tests.rs`, `replication/tests/*` and `replication_seed/tests/disk_export_tests.rs` for the same pattern and for direct `mark_subscription_*` calls (signatures unchanged — they still compile; only fixtures need care).

## Acceptance (self-review, since compiling is forbidden)

- `wc -l` on every touched file, printed at the end; every file ≤1000 lines; no touched folder exceeds 7 entries (new `repl_binding/` folder has exactly 4; `crates/shamir-db/src/` goes 4 → 5).
- Every moved symbol's old definition is gone and every caller resolves (grep for `global_sentinel_key`, `unresolvable_profile_reason`, `quarantine_row_entries`, `row_is_quarantined`, `resync_quarantine_entries`, `mark_subscription_terminal_generation`, `ensure_destinations_unbound`, `SubscriptionBinding`).
- No `DashMap` remains in `quarantine_cache.rs`; no `allow` attributes added; imports at file tops; `mod.rs` re-exports only.
- Final step: `cargo fmt -p shamir-db -p shamir-storage` (this is formatting only, not a build).
- Report: files changed with line counts, each design decision you had to make where this brief was silent, and any call sites you could not resolve.
