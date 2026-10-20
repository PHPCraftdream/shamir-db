# S5c-X PR6b-3 / W5 — server & shamir-db tests + remaining callers of retired APIs

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(§4 "Test disposition", server bullet). W1-W4 landed: typed refusals end-to-end
(`DbError::FollowerStateRefused` → `ReplError::FollowerStateRefused` → supervisor
`resync_required`), v3-only bookmark module, seed writer v3 baseline, receipt-only apply,
deletion of `apply_replicated` (free + wrapper), `advance_replication_bookmark`, and the
legacy writers. Scope here: `crates/shamir-server` and `crates/shamir-db` ONLY (engine test
ports are W4 — do not touch engine files except read-only reference checks).

## Hard rules
- ⛔ NEVER run any git command that mutates the working tree or index. Only edit files; the
  orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command. Only `cargo fmt -p shamir-server -p shamir-db` as your LAST step.
- Never silence a lint with `#[allow]`. Gate test-only imports with `#[cfg(test)]`.
- Tests in `tests/` folders, one topic per file, wired into manifests; remove deleted files
  from manifests. Imports at top. ≤7 entries/folder, ≤1000 lines/file, over-limit trees must
  not grow. Multi-line indented JSON literals in tests. No machine-specific absolute paths.
- IMPORTANT (merge-cleanliness with a parallel PR, PR8b-2, which is changing the subscription
  catalogue rows and quarantine cache in shamir-db/shamir-server): keep your edits to what
  this design requires — the fixture rewrites and the retired-API ports listed below. Do not
  restructure catalogue/quarantine internals, do not rename rows/columns, do not "tidy".

## Deliverables

### 1. `crates/shamir-server/src/replication/tests/supervisor_tests.rs` — partial-history fixture
The old fixture (~lines 509-540) deliberately seeded a LEGACY v2 bookmark
(`save_bookmark_and_clean_horizon(&info, 0, 0)`) and injected a per-table `Store::set`
failure that receipt atomic batches bypass — that expectation ("ordinary history I/O failure
necessarily causes reopen") is retired. Replace it with TWO new tests (keep the file's
harness conventions: `build_db`, `create_profile_and_subscription`, supervisor reconcile):
1. `receipt_io_pause_retry_convergence`: a TRANSIENT receipt-path I/O failure must converge
   WITHOUT extra allocations. Achieve the transient failure through the same store-swap
   technique the old fixture used (swap the table's MVCC store for a wrapper whose `get` /
   `get_current_many` fails ONCE, then delegates) so the receipt pipeline's retry loop
   (3 attempts) succeeds; assert: the batch eventually applies (bookmark advanced once to
   the leader version), local row counters advanced exactly once (one local version per
   event — no replacement allocations), and the subscription stays running.
2. `terminal_receipt_corruption_stops_supervisor_and_refuses_resume`: corrupt the durable
   state unambiguously — write a v3 record whose `outstanding_receipt` pointer references a
   missing receipt (encode `BookmarkV3 { .., outstanding_receipt: Some([0u8;16]) }` via
   `save_bookmark_v3`) — then run a pull: the pipeline fail-stops (`RepoRequiresReopen`
   family), the supervisor marks the subscription terminal, and a subsequent `resume` is
   REFUSED (the supervisor's resume-path classification, landed in W2, returns the typed
   refusal). Assert the terminal state and the refused resume.
Do NOT preserve the old reopen-on-ordinary-history-failure expectation anywhere in the file;
sweep the file for other legacy bookmark seeds (`save_bookmark_and_clean_horizon`,
`save_replication_bookmark`) and convert them to `save_bookmark_v3` fixtures.

### 2. `crates/shamir-server/src/replication/identity_fence/tests/adoption_durability_tests.rs`
Keep the failed/cancelled-flush and retry-before-contact coverage (~59-68, ~98-119). Convert
the fixtures to v3: identity adoption must land on a v3 record (seed the bookmark via
`save_bookmark_v3` where the old tests seeded v1/v2 via `save_fence_record`) and the
assertions must verify preservation of bookmark, J (`clean_through`) and — where a receipt
pointer exists — the outstanding receipt reference across `save_fence_record_v3_aware`
adoption. Where the old tests asserted legacy v1/v2 records being written, they now assert
v3 in-place updates or the typed refusal (floor-without-v3 refuses adoption with
`LegacyFollowerState`).

### 3. Retired-API ports (`advance_replication_bookmark`, legacy writers)
- `crates/shamir-server/src/replication/identity_fence/tests/warn_tests.rs` (~line 39):
  `advance_replication_bookmark(7)` → seed `save_bookmark_v3(.., &BookmarkV3 { fence: None,
  bookmark: 7, clean_through: None, outstanding_receipt: None })` (match the fixture's
  intent).
- `crates/shamir-server/src/replication/tests/recovery_gap_consumer_tests.rs` (~line 328):
  same port for `advance_replication_bookmark(h)`.
- Sweep BOTH crates' tests for `advance_replication_bookmark`, `save_replication_bookmark`,
  `save_fence_record\b`, `save_bookmark_and_clean_horizon`, `apply_replicated_batch_legacy`,
  `apply_replicated\b` (free fn / method) and port every hit. `save_fence_record_v3_aware`
  via `recorded.rs` is FINE (that's the surviving adoption writer).
- `crates/shamir-server/tests/repl_convergence_e2e.rs`: update the module doc that still
  advertises `advance_replication_bookmark` (~line 11) and any legacy-bookmark fixture; the
  e2e flow (seed install → adopt → receipt tail) is the surviving contract — verify its
  assertions still state v3 semantics (upstream cursor vs local J distinct).

### 4. `crates/shamir-db`
- `src/shamir_db/execute/replication_seed/tests/lifecycle.rs` (~line 224):
  `advance_replication_bookmark(u64::MAX)` → the intent was "bookmark beyond any source
  version"; port to `save_bookmark_v3` with `bookmark: u64::MAX` (keep the surrounding
  scenario; note `SeedReceipt` validation rejects `source_version == u64::MAX`, so check
  what the test actually asserts and preserve its meaning — if the test verified the
  bookmark-vs-source guard, keep THAT guard's outcome unchanged).
- `src/shamir_db/shamir_db/execute/replication_seed/status.rs` + `control.rs`: they read
  `load_fence_record` / `replication_bookmark()` (v3 projections — still exist). Verify
  compile-coherence and the error type they surface; a typed refusal must propagate as-is
  (do NOT stringify it into a generic error).

### 5. Residue sweep
`rg -n` BOTH crates for every deleted name (see the W3 brief §7 list) — zero remaining hits
when you finish (tests included). Also confirm no test still constructs v1/v2 bookmarks as
SUCCESS fixtures (legacy bytes are rejection fixtures only).

## Last step
`cargo fmt -p shamir-server -p shamir-db`.

## Report back
Per file: rewritten/ported/deleted with one-line rationale; test names for the two new
supervisor tests; zero-residue confirmation or hits; files over 900 lines; deviations.
