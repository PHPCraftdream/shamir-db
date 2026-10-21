# Decision record DEC-3: rulings on Q1-Q5 (adjudicator @oh, 2026-10-21)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations). Panel: three rush reviewers (end-state, adversary, operations), adjudicated by @oh, who checked the reviewers' claims in `worktrees/review-dec3-endstate` (origin/master `d5f49fa8`), the three PR branches, the wasmtime 48.0.5 / cranelift 0.135.5 registry sources and read-only `gh api` calls. Marks: V = read/verified by @oh, X = claim false, U = could not verify.

## Claims checked

**CI and GitHub settings**
- V `ci.yml` has no `merge_group` trigger. Triggers: `push`, `pull_request` (opened/synchronize/reopened/ready_for_review), `workflow_dispatch`; no workflow-level path filter.
- V The `merge-gate` job `needs:` covers 13 jobs and matches `REQUIRED_JOBS` in `merge_gate_check.py`. Neither includes supply-chain `deny` or NUMA.
- V `supply-chain.yml` (holds `deny`) is path-filtered on PRs and skipped on drafts.
- V NUMA `tier1-mock` runs `./scripts/test.sh -p shamir-numa`, a lib-test subset the `test` job already runs on all 3 OS: no extra coverage.
- V Campaign steps (`source-integrity` "compute merge base" and "campaign gates", `test-discovery` "campaign selection") run only when `event_name == 'pull_request'`.
- V No job in `ci.yml` uses secrets, so Dependabot PRs can satisfy the gate.
- V Via `gh api`: owner type is User (not Organization), repo public, all three merge methods on, auto-merge off, delete-on-merge off, no rulesets, `master` not protected (404).
- U Merge-queue availability (recalled: queues only for organization-owned repositories; banner not confirmed). Every reviewer missed this.

**Supply chain**
- V `cooldown.toml` has `lockfile-baseline = "floor"` and documents `--precise` as the exception.
- V CI runs `cargo cooldown check` (cargo-cooldown 0.3.4).
- U How cargo-cooldown actually behaves on #225 (source not installed locally; #225 is a draft, `cooldown` job skipped).
- V #225 also re-points `windows-sys` edges from 0.60.2 to 0.61.2 for `anstyle-query`, `anstyle-wincon`, `nu-ansi-term`; both versions already in the lock, so no new version is admitted.
- U Which advisory fixes which package and exact publication dates (orchestrator later established from the deny log: RUSTSEC-2026-0382 -> rustls-webpki >=0.103.16; 0378 and 0379 -> wasmtime >=48.0.7; 0381 -> webpki-roots >=1.0.10).

**Q2 (Node binding)**
- V `crates/shamir-client-node/Cargo.lock` and `crates/shamir-client-node/package-lock.json` are both gitignored: the binding's Rust and npm dependency graphs are unlocked and not covered by cargo-deny or the cooldown.
- V Its `rust-toolchain.toml` says `channel = "stable"` (unpinned); the root pins `1.95.0`.
- V The binding is never formatted or linted in CI (outside the workspace, no job covers it).
- V `napi.targets` lists `x86_64-pc-windows-gnu`, contradicting "MSVC-only on Windows".
- V `proof-typed-errors.js` re-implements the wrapper's decode logic; `09-errors.test.js` parses message text and documents an obsolete limitation.
- V There is no `packages/` or `clients/` directory; `shamir-client-ts` already sits under `crates/`.

**Q3 (PR8b-3 design)**
- V Authorization defect: in 131 4a step 1 the challenge is consumed at (iii), after the `Prepared` row became durable at (i). A challenge failure leaves the row `Prepared` with remedy "resubmit the same id", and 131:420 skips the challenge/digest check for an existing id: a replayed, already-consumed confirmation can get a destruction executed.
- V Archival contradiction: 131b Appendix B (C8/F1b), 131a 11.7 and 131 3e.7 release a final-`Blocked` row's reservations, contradicting I1 (131:49) and I11 (131:59).
- V 131:75 requires a legacy-row backfill (a migration).
- V 131:125 keeps an unreachable enum variant "for schema stability".
- V 131:440 claims `admit` is called from only 3 sites; 131a lock inventory rows 27-30 list dozens, including table-write and recovery paths.
- V Stale paths: `shamir-server/src/replication/...` is now `src/sync/replication/`. "REQUIRES #188" appears 6 times in 131 although #188 is merged.
- V The migration API can be switched on in production (`security.enable_experimental_migration_api`, `launch.rs:366`).
- V Hybrid replication lineage is minted per boot (`repo_identity.rs:24-32`).
- V `stop_all` joins every loop under `reconcile_lock`; `quiesce_subscription_lease_locked` drains only the apply gate.

**Q5 (wasm compile cache)**
- V `enable_incremental_compilation` is a safe fn, gated on `incremental-cache` + `cranelift`.
- V `wasmtime-internal-cranelift` always enables cranelift-codegen's `incremental-cache` feature: turning on the wasmtime feature adds no new crates.
- V Key = SHA-256 over (stencil, ISA name, triple, flags, ISA flags). `VersionMarker` is a unit struct with `derive(Hash)`: the Cranelift version is NOT in the key, it is checked only when the value is deserialized.
- V On a hit the code is accepted without recompiling; the equality check exists only behind `enable_incremental_compilation_cache_checks`, reachable only via the `unsafe fn cranelift_flag_enable`.
- V `Engine::precompile_compatibility_hash()` (safe) hashes triple, flags, ISA flags, tunables, features, wmemcheck and module_version.
- V `CacheStore::insert`'s return value is ignored upstream.
- V The wasmtime `runtime` feature compiles `helpers.c` through `cc`; `aws-lc-sys`, `ring`, `libmimalloc-sys` are also in the lock. Dropping zstd does not make the build C-free.
- V #227 reads whole files with no size bound, trims only at open, checks only mode bits, takes its path from env variables, uses blake3 as an unkeyed digest.
- V WASM compilation runs inline on async paths (`core.rs:441`, `function_management.rs:183`).
- X The claim that #227's `unsafe` would be "the first `unsafe` in product code": `unsafe` already exists in `shamir-index` (SIMD), `shamir-numa`, `shamir-transport-ipc` (Windows DACL FFI), `shamir-transport-tcp/framing.rs`, `shamir-types/entries/secret.rs`, `shamir-engine/query/read/order.rs`. The owner's rejection stands on its merits: deserializing executable code is a different class of obligation.
- V Backup copies every file under `data_dir` (`backup.rs` header).
- U napi CLI v3 semantics for `--output-dir`, `--js`, `--dts`; whether `cargo nextest run --lib` respects `[lib] test = false`; whether 48.0.7 / 0.135.7 sources match 48.0.5 / 0.135.5.

---

## Q1 - Merge gate for `master`

**RULING:** Make `merge gate (required)` the single required check through two repository rulesets, with strict up-to-date branches and no merge queue, after one workflow PR has made the gate complete.

### Exact specification

**Workflow PR "PR-GATE" (must land first; depends on PR-SC from Q4):**
1. Move `cargo deny check` into `ci.yml` as job `deny`, named `cargo deny check`:
   - `if: github.event_name != 'pull_request' || github.event.pull_request.draft == false`
   - same pinned steps as today (`cargo install --locked --version 0.20.2 cargo-deny`, `cargo deny check`)
   - add `deny` to `merge-gate.needs` and to `REQUIRED_JOBS`
   - extend `scripts/ci/tests/test_merge_gate_check.py`
2. `supply-chain.yml`: delete the `push` and `pull_request` triggers and the `deny` job. Keep `schedule` + `workflow_dispatch` with the `audit` and `sbom` jobs.
3. `numa.yml`: delete the `tier1-mock` job (redundant with `test`). Keep `tier3-qemu`. The workflow is non-gating.
4. Add `scripts/ci/tests/test_gate_inventory.py` (stdlib only). It parses `ci.yml` (top-level job keys at 2-space indent, plus the `needs: [...]` line of `merge-gate`) and asserts: every job except `merge-gate` is in `needs`; `needs` equals `REQUIRED_JOBS`; exactly one job is named `merge gate (required)`.
5. Do not add `merge_group` (queue unavailable, would be an untested dead path). Do not add `converted_to_draft` (GitHub already refuses to merge drafts).

**Owner settings (exact values under OWNER ACTIONS):**
- Ruleset `master-delivery`: require a PR, 0 approvals, conversation resolution on, squash only; required check `merge gate (required)`, integration_id 15368 (GitHub Actions), `strict_required_status_checks_policy: true`; linear history; bypass: admin repository role, `bypass_mode: pull_request` only (emergency merge of a PR, never a direct push).
- Ruleset `master-integrity`: block deletion and non-fast-forward pushes, no bypass actors.
- Repository: squash only, delete branch on merge, auto-merge available (set per PR by the owner or orchestrator, never as a blanket rule for Dependabot).
- Dependabot: same rules, no bypass. Its security PRs need an owner commit that adds the Q4 exception entry.

**Strict-mode cost.** Each merge makes the other ready PRs out of date, and each update reruns the full matrix (macOS 30+ min). Operating rule: keep parallel work in draft (fast tier); promote and merge one PR at a time; after each merge `gh pr update-branch` the next candidate and set auto-merge on it. With one maintainer this is the same serialization a single-entry queue would impose.

**Why there is no deadlock.** The check name is literal and stable. `ci.yml` has no path filter and the gate is `if: always()`, so the check always reports. Path-filtered workflows are never required. Draft failure is intended. An advisory published against the lock turns every PR red until a remediation PR lands; that PR fixes the advisory, so it passes its own gate (deliberate "stop the line").

### Reviewer disagreements
- Queue now (endstate, adversary) vs strict first then queue (ops): took neither. Strict is the end state because the repository is User-owned (verified) and merge queues are, as far as @oh can tell, organization-only; even if available, the docs aim it at "many different users".
- NUMA into the aggregate (endstate): rejected, duplicates `test`.
- Emergency path (adversary: temporarily edit the rule): rejected in favour of a PR-only bypass actor plus a separate integrity ruleset with no bypass.
- Auto-merge: enable the capability; arm it per PR only.

### Implementation order
1. PR-SC (Q4) - CI-only. 2. PR-GATE - CI-only, also requires one green `merge gate (required)` on the master push run after it merges. 3. Owner applies the settings - verified with `gh api` reads. 4. Rebase every open PR.

### What would change my mind
- If the owner's ruleset UI/API offers "Require merge queue", keep strict mode until a follow-up PR adds `merge_group: {types:[checks_requested]}` to `ci.yml`, derives `MERGE_BASE` from `github.event.merge_group.base_sha` in the 3 PR-only steps, and is observed green on a real merge group; then enable the queue (squash, group size 1, build concurrency 1, timeout 180 min).
- A second independent human maintainer would justify 1 required approval with stale-dismissal and last-push approval.

---

## Q2 - `crates/shamir-client-node`

**RULING:** Keep the package colocated, make the crate a workspace member, split the root into `js/` (handwritten) and `native/` (napi-generated), commit the npm lockfile, delete the per-crate toolchain and the proof script, and empty the baseline entry.

### Exact specification

**End-state root (7 tracked entries):**
```
Cargo.toml  build.rs  src/  package.json  package-lock.json  js/  native/
js/wrapper.js  js/wrapper.d.ts
native/index.js  native/index.d.ts  (tracked, generated)
native/shamir-client.<triple>.node  (gitignored)
```

**PR Q2-a: workspace membership.**
1. Root `Cargo.toml`: remove `crates/shamir-client-node` from `exclude`.
2. Crate `[lib]`: `crate-type = ["cdylib"]`, `test = false`, `doctest = false`.
3. Delete the crate's `[profile.release]` (Cargo ignores profiles in non-root members). Add to the root: `[profile.release.package.shamir-client-node] opt-level = "z"`, `codegen-units = 1`, `strip = true` (fat LTO is already set at the root).
4. Delete `crates/shamir-client-node/rust-toolchain.toml` (its `stable` channel floats against the repository pin, and `targets` does not force MSVC; CI's Windows runners are MSVC hosts on root 1.95.0).
5. `.gitignore`: delete the lines `crates/shamir-client-node/Cargo.lock`, `crates/shamir-client-node/target/` and `crates/shamir-client-node/package-lock.json`. Change `crates/shamir-client-node/*.node` to `crates/shamir-client-node/native/*.node`.
6. The binding's dependencies now enter the root `Cargo.lock`; they pass cargo-deny and the Q4 admission checker, or get exceptions.
7. Update CLAUDE.md "Workspace" and the root `exclude` comment.
8. Lower the layout baseline to 9.
9. Fallback, only if CI shows `./scripts/test.sh` building or linking a test harness for the cdylib: add `--exclude shamir-client-node` to the workspace selection in `scripts/test.sh`. Nothing else.

**PR Q2-b: layout and coverage.**
1. Moves: `wrapper.js` -> `js/wrapper.js`; `wrapper.d.ts` -> `js/wrapper.d.ts`; `index.js` -> `native/index.js`; `index.d.ts` -> `native/index.d.ts`; delete `proof-typed-errors.js`.
2. In `js/wrapper.js`, change `require('./index.js')` to `require('../native/index.js')`.
3. `package.json`:
   - `"main": "js/wrapper.js"`, `"types": "js/wrapper.d.ts"`
   - `"files": ["js/wrapper.js","js/wrapper.d.ts","native/index.js","native/index.d.ts","native/shamir-client.*.node"]`
   - `"build": "napi build --platform --release --output-dir native --js index.js --dts index.d.ts"`
   - `"build:debug": "napi build --platform --output-dir native --js index.js --dts index.d.ts"`
   - `@napi-rs/cli` and `typescript` pinned exactly, to the newest releases at least 30 days old
   - remove `x86_64-pc-windows-gnu` from `napi.targets`
   - commit `package-lock.json`
4. `tests/e2e/package.json` `build:binding`: `cd ../../crates/shamir-client-node && npm ci && npm run build`.
5. Rewrite `tests/e2e/tests/09-errors.test.js`: delete the obsolete NOTE and the message parsing. Through the public `shamir-client` package assert `err instanceof ShamirDbError`, `err.code === 'unknown_db'`, `err.retryable === false`, one retryable code where the suite already has a deterministic trigger (`tx_conflict` or `read_only_replica`) otherwise the exported `isRetryableCode` truth table against the real wrapper, and a success-path call.
6. New full-tier job `node-binding` in `ci.yml` (windows-latest, unconditional, added to `needs` and `REQUIRED_JOBS`): setup-node 22 (npm cache keyed on the binding lockfile) and Rust 1.95.0; `npm ci`, then `npm run build:debug`; `git diff --exit-code -- native/index.js native/index.d.ts` (drift in generated files); `npm pack` into `$RUNNER_TEMP`, install the tarball into a clean consumer directory; `node -e` requiring `shamir-client` and checking that `ShamirClient` and `ShamirDbError` are functions; `tsc --noEmit` on `tests/e2e/packaging/consumer.ts` with its `tsconfig.json` (`tests/e2e` reaches 7 entries).
7. Update the explanatory comments in `ts-e2e-nightly.yml` (toolchain-file rationale; commands stay), path comments in `release.yml` and `ci.yml`. No npm publish pipeline.
8. Delete the `dir crates/shamir-client-node` line from `layout_baseline.txt`.
9. Acceptance: full matrix with `node-binding` green, plus one `gh workflow run ts-e2e-nightly.yml --ref <branch>` run green with the real-server `09-errors` assertions.

**Risk.** Nothing here can be verified without CI: napi relative-path semantics, cdylib-in-workspace linking on Windows/macOS, nextest's handling of `test = false`.

### Reviewer disagreements
- Layout: endstate and ops wanted `js/` + `native/` with no lockfile; adversary wanted a single `js/` to leave room for a lockfile. @oh took `js/` + `native/`; only possible together with deleting the toolchain file and joining the workspace (removes the binding's own `Cargo.lock`), and adds `package-lock.json`. No reviewer noticed the untracked lockfiles or the floating toolchain.
- `files` field: ops' `["js","native"]` rejected in favour of the explicit list.
- Location: all three rejected option C (separate package directory); @oh agrees.

### Implementation order
After PR-SC and PR-GATE: Q2-a (CI-only), then Q2-b (CI-only plus a dispatched nightly run).

### What would change my mind
- If Q2-a cannot be made green on windows-latest with the `--exclude` fallback: keep the crate excluded, track its own `Cargo.lock`, cover that lock in the Q4 checker and cargo-deny, and merge `js/` + `native/` into one `js/` (root stays at 7).
- If napi v3 cannot emit into `--output-dir` with these flags: keep the generated files and wrapper together in `js/`.

---

## Q3 - PR8b-3 design and draft #196

**RULING:** The architecture is right but the text is not implementable as written. Land the amendments below as commits on #196's own branch, then mark #196 ready and squash-merge it as the single normative baseline before any 8b-3 code.

### The nine questions

| # | Ruling | Exact modification |
|---|---|---|
| 1 | Accept, strengthened | Hybrid writes `StorageOwnership` into its durable fjall tier. Ownership is minted only on the controlled creation path and published durably before the repository is exposed. Missing or corrupt ownership gives `unsafe_storage_ownership` (refuse to open or delete), never minted lazily. Storage ownership is stated as distinct from replication lineage, which stays per-boot for hybrid. |
| 2 | Modify | Accepted for the 8b-3 series only. Windows reseed is refused before any install mutation, certified on Windows CI (case 15). Windows destruction and name release stay required. Record "Windows seed install" as a mandatory end-state follow-up, not a permanent product limitation. |
| 3 | Modify | Every lifecycle transition that writes >=2 system-store records does so in ONE system-store transaction plus ONE flush, through a typed `SystemStore::commit_durable(batch) -> DurableReceipt`, the only lifecycle write path. "Commit ok, flush failed" is a typed `DurabilityUnknown`, resolved by a forced flush and then a re-read. Add an explicit per-boundary recovery table (each durable prefix -> meaning -> boot action). Fix the `remove_database` missing flush in 8b-3a, not 8b-3c. |
| 4 | New targeted drain API; reject `stop_all` | `drain_targets(targets, generations) -> DrainReceipt`: (a) under control, invalidate and drain each affected lease (existing `quiesce_subscription_lease_locked`) and mark those subscriptions `retiring` so reconcile and start skip them, then release control; (b) outside control, stop and join only those loops, with a bounded timeout (timeout -> `Blocked`, fail-closed); (c) re-take control, verify the generations are unchanged and no lease exists, return the receipt. A task must never be joined while control is held. |
| 5 | Accept | System-store table, permanently. No separate journal. |
| 6 | Modify | The API is reachable through config today (verified), so migration must be interlocked now. It acquires both namespace admissions in global `(db, repo)` order before control and write locks, re-validates identity after acquisition, and refuses reserved targets. The CI case "migration during reserved name" becomes mandatory in 8b-3c, not conditional. |
| 7 | Reject release; accept no expiry | `destruction_archive` never releases protection. Eligible only for Repo/Database `Destroyed` + fully acked, and SubscriptionRemoval `Completed`. Archival replaces the row with a permanent compact tombstone `{operation_id, kind, final_state, frozen storage_object_ids, sha256(full row), archived_at, archived_by}`; operation ids are never reusable. A final `Blocked` operation exits only through `destruction_retry` -> `Destroyed`. A `Blocked` SubscriptionRemoval at `RowDeleted` whose re-verification found data converts that data into an R2' target or an incident, never a release. Backpressure: a quota on concurrent non-terminal operations; new destruction is refused when the quota is reached; nothing protected is ever evicted. |
| 8 | Accept the proof rule; tighten the log | Event id = `(operation_id, transition_ordinal)`, bound to a SHA-256 of the canonical payload. The same id with a different digest is corruption: poison plus incident, never dedup. Define the record framing (length + checksum). On open, the sink truncates an incomplete or bad-checksum final record before any append. Exactly one serialized writer. At most one duplicate per ambiguous append. "Found after restart" is never proof. |
| 9 | Accept | Flush-before-return stays. The barrier I/O runs off Tokio workers (`spawn_blocking`) with a bounded timeout; on timeout the row stays `Prepared` with `destruction_audit_unavailable`. Group commit allowed only if each acknowledgement covers its own sequence number. Latency exposed as a metric. |

### Soundness and top risks
Sound enough to build 8b-3a..c on only after the amendments. Top 3 risks:
1. Authorization bypass through the stranded-`Prepared` resubmission (a consumed or stale challenge becomes authority).
2. Resurrection: stale `RepoInstance` clones, lazy `repo_wal()` `create_dir_all`, Windows handle semantics can recreate storage after deletion; the retire-fence argument rests on a false 3-site `admit` inventory.
3. Staged exposure and lock order: a destructive dispatch could become reachable before publisher wiring (8b-3c), or a drain could join tasks under control.

### Amendments that must land on the #196 branch before merge
- **A1 (authorization):** at Prepare (i), challenge DELETE-IF-MATCHES (challenge_id + digest) and the `Prepared` PUT-IF-ABSENT happen in ONE system-store transaction under the held admissions. If the challenge fails, no row is written (insert-if-absent `Rejected`, or just an error). Delete the C6 "resubmit same id" remedy. A resubmission can find a row only if authorization succeeded.
- **A2 (archival):** rewrite item 7 per the table. Update 131 3e.7, 131a 11.7 and 131b Appendix B. Mark 131c C8 and F1b as superseded.
- **A3:** delete the legacy backfill (131:75). Pre-8b-3a data directories are refused with a typed `storage_format_unsupported` error. No migrations.
- **A4:** delete the unreachable `UnsupportedBackend` row variant (131:125).
- **A5:** re-derive the retire fence from the full `admit` inventory (131a rows 27-30) and delete the 131:440 "only 3 sites" claim. Linearization point: `retire` sets the flag under the repo admission mutex and drains permits. Every `admit`, `admit_with_reservation` and `admit_pair` checks the flag under the same mutex. `repo_wal()` creation requires a live permit. The operation token is the exclusion mechanism; epochs only detect stale work.
- **A6:** the item 4 drain contract, verbatim.
- **A7:** the item 3 durable-mutation API and recovery table.
- **A8:** the item 6 migration interlock, with the case made mandatory.
- **A9:** the item 8 log contract.
- **A10:** the item 9 execution rule.
- **A11:** item 1 recorded as DECIDED.
- **A12:** item 2 refusal plus the follow-up entry.
- **A13:** refresh every citation against current master (`src/sync/replication/...`, the `follower_loop` path). Replace every "REQUIRES #188" with OBSERVED merged references and re-verify line numbers.
- **A14 (staging gate):** the 8b-3b executor and authority are `pub(crate)` and not dispatchable from wire or admin. Destructive dispatch becomes reachable only in 8b-3c, together with all publisher wiring. State this as an acceptance criterion of 8b-3b and 8b-3c.
- **A15:** one normative set (131, 131a 9-11, 131b) with the "round N" tags consolidated into final DECIDED text. 131c and 131d are marked "historical, non-normative". Add a header citing this decision record (DEC-3).

Then #196 goes ready and is squash-merged. Implementation briefs for 8b-3a..e cite the merged file.

### Reviewer disagreements
- #2: ops accepted unchanged; @oh sided with endstate and adversary (modify).
- #3: endstate accepted; @oh sided with ops and adversary (modify, atomic transaction plus recovery table).
- #196 timing: all three said "merge after folding in"; @oh agrees.
- A1, A3 and A5 come from single reviewers (endstate for A1 and A3, adversary for A5); each verified and adopted.

### Implementation order
Amendment commits (docs; review is the only check, CI trivial) -> #196 ready -> merge -> 8b-3a -> 8b-3b -> 8b-3c -> 8b-3d -> 8b-3e (each CI-only). Independent of the other questions.

### What would change my mind
A durable protection-transfer protocol that provably cannot reopen old storage would allow archival with release. A product decision that destruction is a global-maintenance operation would allow `stop_all`.

---

## Q4 - Security fixes vs the 30-day cooldown (#225)

**RULING:** Adopt the fixes now through exact, CI-validated, expiring exceptions in the same PR as a new admission checker. `--precise` plus lockfile grandfathering is no longer an approval mechanism.

### Exact specification

**PR-SC: #225 rebuilt on its own branch.**
1. Lock changes: regenerate with `cargo update -p <pkg> --precise <ver>` for exactly the advisory closure: the wasmtime family 48.0.7 and cranelift family 0.135.7 (each crate named individually), plus rustls-webpki 0.103.16; webpki-roots 1.0.10 stays only if an advisory names it, otherwise revert it to 1.0.7; resolver edge re-points that admit no new version (the `windows-sys` edges) are allowed.
2. New file `cooldown-exceptions.toml` at the repository root, one entry per admitted young version:
   ```toml
   [[exception]]
   package = "wasmtime"
   version = "48.0.7"
   source = "registry+https://github.com/rust-lang/crates.io-index"
   checksum = "<from Cargo.lock>"
   published = "<crates.io created_at, RFC 3339>"
   expires = "<date(published) + 30 days>"
   kind = "security"
   advisories = ["RUSTSEC-2026-03xx"]    # primaries only
   # companion_of = "wasmtime@48.0.7"    # instead of advisories, for lockstep companions
   reason = "..."
   approved_by = "PHPCraftdream"
   approved_on = "2026-10-21"
   ```
3. New `scripts/ci/gates/dependency_admission_check.py` (stdlib) with tests in `scripts/ci/tests/test_dependency_admission_check.py`. It:
   - diffs `Cargo.lock` and `tools/rust-guard/Cargo.lock` against the base (`origin/master` on PRs, `HEAD^1` on push) by `(name, version, source, checksum)`;
   - for each newly admitted crates.io identity, fetches `created_at` from `https://crates.io/api/v1/crates/<n>/<v>` (with a User-Agent), failing closed on any error;
   - reads the age from `cooldown.toml` `global-min-publish-age`;
   - requires each too-young admission to match exactly one exception (same checksum and source);
   - requires `published` to equal the registry value and `expires == date(published) + 30d`;
   - requires `advisories` XOR `companion_of`;
   - for each advisory, requires `crates/<package>/<ID>.md` to exist in a fresh `--depth 1` clone of `rustsec/advisory-db`;
   - requires each `companion_of` primary to be in the file and to depend, transitively per `Cargo.lock`, on the companion at that exact version;
   - fails on wildcards, duplicates, unused entries (not in the head lock) and expired entries;
   - emits `::warning` 7 days before expiry.
4. The `cooldown` job keeps its id and name. Its steps become: checkout (fetch-depth 0) + the checker. The `cargo install cargo-cooldown` and `cargo cooldown check` steps are deleted. `cooldown.toml` stays as the developer configuration for `cargo cooldown update`. `supply-chain.yml`'s weekly schedule also runs the checker in expiry-report mode.
5. Policy text. Replace `cooldown.toml` lines 16-17 and the `lockfile-baseline` comment with:
   > Dependency admission. A registry package version may enter any committed Cargo.lock only if it was published at least 30 days before admission. "New" means absent from the protected base (current master); presence in a proposed lockfile is never approval, and `cargo update --precise` records a resolution, not an authorization. A younger version may be admitted only for a security fix, through an entry in `cooldown-exceptions.toml` naming the exact package, source, version, checksum and registry publication time; the RUSTSEC advisories it fixes (or the exact primary it is a required lockstep companion of); the reason; the approver and approval date; and an expiry equal to its 30-day maturity date. Exceptions waive publication age only - never advisory, license, source, integrity or test gates. Wildcards, families, ranges and labels are not exceptions. CI fails on a missing, mismatched, unused or expired exception; at maturity the entry is deleted by a reconciliation PR (history keeps the record) and the version is never downgraded.
6. For #225 specifically, exception expiry is 2026-11-18 for versions published on 2026-10-19 and 2026-11-19 for 2026-10-20, recomputed from the actual timestamps.

### Reviewer disagreements
- Keep cargo-cooldown as the gate (endstate): rejected; its floor semantics are unverified and the gate must not depend on how a tool interprets the candidate lockfile.
- Atomic policy plus patch (ops) vs sequential: atomic adopted (no master state with a rejecting gate but no exceptions; the security fix is not held behind a separate PR).
- Expiry handling: fail-closed in the required gate, with a 7-day warning.

### Implementation order
PR-SC first of everything (CI-only; the advisory mapping is checked by CI). PR-GATE follows.

### What would change my mind
Evidence of a compromised publisher, a mismatched release, or a patch that does not fix the advisory: halt and disable the affected feature instead. Native Cargo `min-publish-age` with per-package exceptions: replace the checker with it.

---

## Q5 - Compile cache with no C and no `unsafe` (#227)

**RULING:** Rewrite #227 around wasmtime's safe `Config::enable_incremental_compilation` with a bounded, framed, per-key disk `CacheStore` under `<data_dir>/cache/wasm/`, on Unix only at first. Delete the whole-module serialize/deserialize cache.

### What the key covers, and the trust boundary
- Soundness across settings: Cranelift's key (stencil + ISA + flags + ISA flags) is upstream's reuse contract. Fuel and epoch instrumentation and memory bounds checks are in the IR. The Cranelift version is not in the key but is rejected when the value is deserialized. The design adds an outer namespace over `Engine::precompile_compatibility_hash()` (tunables, features, flags, triple, wasmtime version), so different engine settings (fuel, epoch, pooling, `SHAMIR_WASM_NO_POOL`) never share entries.
- Trust is moved, not removed: a tampered entry is executable code. The safe API removes our `unsafe` call site, not the attack. A digest detects corruption only. Therefore: private directory, never replicated, never in a backup or restore, never sourced from an environment variable.

### Exact specification

**`Cargo.toml` (`shamir-wasm-host`):**
- wasmtime features: `["cranelift","runtime","std","wat","async","pooling-allocator","incremental-cache"]`. `cache` is removed; the feature adds no new crates (verified).
- `sha2` at the version already in the lock (0.10.x).
- `[target.'cfg(unix)'.dependencies] rustix = { version = "<lock version used by wasmtime>", default-features = false, features = ["std","process"] }` for `geteuid`.
- Drop `blake3`.
- `src/lib.rs` gets `#![forbid(unsafe_code)]`.

**Modules (`src/wasm/compile_cache/`):** `mod.rs` (re-exports), `disk_store.rs`, `entry_frame.rs`, `entry_index.rs`, `private_dir.rs`, `namespace.rs`.

**Wiring:**
- `WasmEngine::new()` means no persistent cache.
- `WasmEngine::with_compile_cache(dir)`: (1) builds `Arc<DiskCacheStore>` unbound (a `OnceLock`); (2) calls `config.enable_incremental_compilation(store.clone())` before `Engine::new`; (3) after the final engine exists (including the pooling fallback), calls `store.bind(&engine)`, which validates the directory, computes the namespace and scans the index. An unbound or failed store returns `get -> None` and `insert -> false`.
- `ShamirDb::init_inner` passes `data_root.join("cache").join("wasm")` when `data_root` is `Some`. For an in-memory DB, or on Windows, it uses `new()`.
- `WasmEngine::compile` stays a plain `Module::from_binary`.

**Namespace:** `ns = SHA-256("shamir-wasm-icache\0" || FORMAT_VERSION u32 LE || CARGO_PKG_VERSION || engine.precompile_compatibility_hash() streamed through a SHA-256 std::Hasher)`. The directory is `v1-<first 32 hex of ns>`. At bind, every other `v*-*` namespace directory is deleted.

**On-disk layout:**
```
<data_dir>/cache/wasm/v1-<ns>/tmp/
<data_dir>/cache/wasm/v1-<ns>/<00..ff>/<64-hex key>.e
```

**Frame:**

| Offset | Bytes | Content |
|---|---|---|
| 0 | 8 | magic `SHMWIC\0\1` |
| 8 | 32 | ns |
| 40 | 32 | key |
| 72 | 8 | payload_len, u64 LE |
| 80 | 32 | SHA-256(bytes[0..80] || payload) |
| 112 | payload_len | payload |

**Limits:** payload <= 4 MiB (larger values: `insert -> false`); total <= 256 MiB; entries <= 65,536; when either cap is exceeded, trim to 75% by the oldest logical tick, single trimmer guarded by an `AtomicBool`.

**Index:** `scc::HashMap<[u8;32], EntryMeta{len, tick: AtomicU64}, THasher>`, plus `AtomicU64` total bytes and `AtomicUsize` count (no `scc::len`). Built at bind from directory metadata only.

**`get`:** key length != 32, or not in the index -> `None` with no I/O; otherwise check `metadata.len() == 112 + len <= cap` before allocating, read exactly, verify magic/ns/key/len/digest; on any mismatch remove the file and index entry, increment `rejected`, return `None`; a hit bumps the tick.

**`insert`:** key already present -> `true`; write `tmp/<pid>-<seq>-<keyhex>` with `create_new` and mode 0600, no fsync; rename to the final path; update the index; trim if needed; any error -> `false`. Startup removes everything in `tmp/`. Torn or absent files are misses. Cache failure never fails module validity.

**Directory trust (Unix, std plus `rustix::process::geteuid`):** canonicalize `data_dir`; `cache` and `cache/wasm` are created with mode 0700; `cache` and `cache/wasm`: no symlinks (`symlink_metadata`), `uid == euid`, `mode & 0o077 == 0`; `data_dir`: `uid == euid`, `mode & 0o022 == 0`; every ancestor up to `/`: `uid in {0, euid}` and (`mode & 0o022 == 0` or sticky); any violation -> disk tier disabled, logged at warn.

**Windows:** disk tier disabled in this PR.

**Backup and restore:** `backup.rs` skips the top-level `cache/` subtree. `restore.rs` rejects a snapshot manifest that lists any `cache/` path.

**Delete from #227:** `Module::serialize`, `unsafe { Module::deserialize }`, and the key derived from the wasm bytes; `CompileCache::module` and `Outcome`; `default_dir`, `SHAMIR_WASM_CACHE_DIR`, `SHAMIR_WASM_NO_CACHE`, and the LOCALAPPDATA/XDG/HOME path logic; the blake3 framing and the `blake3` dependency; trim-at-open-only and the mode-bits-only `create_private_dir`; the routing of `WasmEngine::compile` through the cache; the current `compile_cache_tests.rs`.

**Keep from #227:** removal of the `cache` feature and of the `wasmtime::Cache::new(CacheConfig::new())` block; the resulting Cargo.lock removals (zstd, zstd-sys, wasmtime-internal-cache, ...); `from_wat` compiling the sanitized `wasm_bytes`; the single `WasmEngine::compile` entry point.

**Tests (`src/tests/wasm/compile_cache/`, CI):** a second engine on the same directory gets hits (store counters); a flipped payload byte -> miss and the file removed; a frame with the wrong key -> miss; an oversized file is rejected without being read; changing `consume_fuel` gives a new namespace and removes the old one; trimming respects the caps; concurrent same-key inserts; group-writable or symlinked directory -> disabled; cold vs warm runs give identical results for fuel-exhaustion, epoch-interrupt and memory out-of-bounds traps; backup excludes and restore rejects `cache/`.

**Before the PR goes ready:** re-read the 48.0.7 / 0.135.7 sources that `cargo check` downloads (`config.rs`, `compiler.rs` `compile_maybe_cached`, `incremental_cache.rs`, `ir/function.rs` VersionMarker) and confirm they match the analysis above.

### Rejected alternatives
An audited micro-crate around `deserialize` (still our `unsafe`, same executable-code trust); a database-stored artifact (still needs `deserialize`, puts machine code inside the replication and backup trust domain); Pulley (different execution model, not a cache answer); no cache (kept only as the fallback).

**"No C" is not achieved by this PR:** `wasmtime/build.rs` compiles `helpers.c`, and `aws-lc-sys`, `ring`, `libmimalloc-sys` are in the lock. That is an owner decision.

### Reviewer disagreements
- Location: all three chose an OS-level private service directory; @oh chose `data_dir` (per instance, tied to the node's lifecycle, backup exclusion added).
- Windows: reviewers wanted ACL validation; @oh disables the disk tier until Q5-c (the only ACL code in the repo is `unsafe` FFI).
- Default: ops and adversary wanted it optional until measured; @oh chose on by default for disk-backed databases on Unix, with Q5-d deciding whether it stays.
- Extra in-memory tier (adversary): rejected; the registry already holds compiled `InstancePre`.
- Hash: sha2 instead of blake3 (already in the graph).

### Implementation order
1. Q5-a, the #227 rewrite, after PR-SC (48.0.7) - CI-only.
2. Q5-b: wrap `WasmFunction::from_binary` at the async call sites (`core.rs` boot loop, `function_management.rs`, `validator_management.rs`) in `tokio::task::spawn_blocking` - CI-only.
3. Q5-c: extract the owner-only DACL code from `shamir-transport-ipc/src/windows.rs` into one shared crate with a safe `create_private_dir` / `verify_private_dir`, plus a reparse-point check through std `file_attributes() & 0x400`. Enable the Windows disk tier - CI-only.
4. Q5-d: bench cells `compile_cold` / `compile_warm` / `boot_k_functions` in `crates/shamir-engine/benches/wasm_invoke.rs` (`bench_scale_tool`), run through perf-gate `workflow_dispatch` - CI-only.

### What would change my mind
Q5-d warm/cold > 0.8 on the representative module set: remove the disk tier and ship no persistent cache (never bring back `deserialize`). An upstream advisory against incremental-cache soundness, or a 48.0.7 behaviour change: disable it.

---

## Global implementation order

| # | Step | Depends on | Verification |
|---|---|---|---|
| 1 | PR-SC (Q4 + #225) | - | CI-only |
| 2 | PR-GATE (Q1 workflow) | 1 | CI-only, plus a green master push run |
| 3 | Owner GitHub settings | 2 | `gh api` read-back |
| 4a | #196 amendments -> merge | - (parallel) | review only |
| 4b | Q5-a | 1 | CI-only |
| 4c | Q2-a | 3 | CI-only |
| 5a | Q2-b | 4c | CI-only + nightly dispatch |
| 5b | Q5-b, Q5-c, Q5-d | 4b | CI-only |
| 6 | 8b-3a...e | 4a | CI-only |

## OWNER ACTIONS

1. Repository settings:
   ```
   gh api -X PATCH repos/PHPCraftdream/shamir-db -F allow_squash_merge=true -F allow_merge_commit=false -F allow_rebase_merge=false -F delete_branch_on_merge=true -F allow_auto_merge=true -f squash_merge_commit_title=PR_TITLE -f squash_merge_commit_message=COMMIT_MESSAGES
   ```
2. `gh api -X POST repos/PHPCraftdream/shamir-db/rulesets --input -` with:
   ```json
   {"name":"master-delivery","target":"branch","enforcement":"active",
    "conditions":{"ref_name":{"include":["refs/heads/master"],"exclude":[]}},
    "bypass_actors":[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"pull_request"}],
    "rules":[
     {"type":"pull_request","parameters":{"required_approving_review_count":0,"dismiss_stale_reviews_on_push":false,"require_code_owner_review":false,"require_last_push_approval":false,"required_review_thread_resolution":true,"allowed_merge_methods":["squash"]}},
     {"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"do_not_enforce_on_create":false,"required_status_checks":[{"context":"merge gate (required)","integration_id":15368}]}},
     {"type":"required_linear_history"}]}
   ```
3. Second ruleset:
   ```json
   {"name":"master-integrity","target":"branch","enforcement":"active",
    "conditions":{"ref_name":{"include":["refs/heads/master"],"exclude":[]}},
    "bypass_actors":[],
    "rules":[{"type":"deletion"},{"type":"non_fast_forward"}]}
   ```
   Apply rulesets 2 and 3 only after PR-GATE is merged and green on master.
4. Check whether "Require merge queue" is offered for this repository; if it is, tell the orchestrator (Q1 "what would change my mind").
5. Approve each security exception by merging the PR that adds it.
6. Decide what "no C" means: no C in the cache, no C compiled in our build, or no native code at all. Today wasmtime's runtime, aws-lc-sys, ring and mimalloc compile C.
7. Optionally decide a workspace-wide `unsafe_code` lint policy. `shamir-wasm-host` gets `forbid`; other crates already use `unsafe`.

## RISKS ACCEPTED

- Strict mode: every merge forces a full-matrix rerun of the next candidate (macOS queue); merges are serialized.
- Stop the line: an advisory, or an expired exception, turns all PRs red until a one-line remediation PR lands.
- Gate self-trust: the gate code is controlled by the PR itself; only the owner's review guards changes to the workflow and gate scripts.
- Cache trust: the executable-cache trust domain equals the `data_dir` trust domain. The digest is integrity only, not authentication.
- Windows: no WASM compile cache until Q5-c lands.
- Cache benefit is unmeasured until Q5-d.
- Blocked destructions: a `Blocked` destruction can keep a name reserved indefinitely; the price of I1.
- npm: dependencies are locked but not under the 30-day admission rule.
- Unverifiable before CI: napi path semantics and cdylib-in-workspace behaviour (Q2), and 48.0.7 source parity (Q5).
