# 02 - fixes after the automated review of round 1 (wasm incremental compile cache, DEC-3 Q5)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Situation

Round 1 (brief `01-wasm-incremental-cache.md`, DEC-3 Q5 in `docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md` on the PR #196 branch; the Q5 ruling is reproduced inside brief 01) is committed as the HEAD commit of this worktree. An independent review FAILED it. I verified two of its P1 claims myself in the source:
- `private_dir::prepare(data_dir)` treats its argument as the DATA directory (it canonicalizes it, validates it and its ancestors, then creates `cache` and `cache/wasm` under it), but `DiskCacheStore::bind` calls `prepare(&self.root)` with `self.root = <data_root>/cache/wasm` (that is what `ShamirDb::init_inner` passes). Production therefore fails on a fresh database or nests `cache/wasm/cache/wasm`.
- `DiskCacheStore::get` keeps the `OccupiedEntry` of `get_sync` alive and then calls `invalidate` (`remove_sync` on the same key) on its error branches - a self-deadlock on the scc bucket lock.

## Task

Same scope as brief 01 (crate `shamir-wasm-host`, the `shamir-db` wiring, `shamir-server` backup/restore; no other area). Fix EVERY finding below and these decisions:

1. **Path contract.** `WasmEngine::with_compile_cache` and `DiskCacheStore` take the DATA directory; `prepare` derives `<data>/cache/wasm`; `ShamirDb::init_inner` passes `data_root` (not `data_root/cache/wasm`). Every test uses the same contract as production (a temporary DATA root; assertions look into `<root>/cache/wasm/v1-.../`). Add a test that goes through `with_compile_cache` on a fresh temporary data root and observes a store hit after a second engine on the same root.
2. **No self-deadlock.** Never hold a map entry guard while removing; read what you need (len, a clone/handle of the tick atomic) then drop the guard before any `invalidate`. Add tests that corrupt / delete / truncate an indexed file and require `get` to return a miss within a bounded time (run the call on a thread and `recv_timeout`) and the file and index entry to be gone afterwards.
3. **Trim accounting.** Trimming must never `store` a snapshot into the shared atomics. Account every removed entry with `fetch_sub` of exactly what was removed; make the target computation overflow-safe (use `u128` or divide-then-multiply) so the `u64::MAX` / `usize::MAX` test hooks do not overflow. Add a test with concurrent inserts and a trim that asserts the final byte/count totals equal the sum over the directory.
4. **Startup recency and caps.** Entries found at bind get ticks ranked by their file modification time (oldest first, small integers) and the live tick counter starts after the largest assigned rank; `bind` trims to the caps immediately, so an over-cap cache is brought under the caps at startup. Add tests for both (recent live inserts survive, oldest startup entries go first).
5. **Tests.** Gate every disk-store test module with `#[cfg(unix)]` (the disk tier is disabled on non-Unix; add one non-Unix test that `with_compile_cache` degrades to an uncached engine). Fix the namespace and trim tests to inspect the correct directory level; fix the entry-cap test so its comment and assertion agree; make the corruption tests assert that the file is removed, not only that `rejected` moved; make the oversized test create a file larger than the cap and assert a miss without reading it (expose a counter for frames actually read); make the wrong-key test craft a frame whose digest is VALID for its content but whose key field differs; make the symlink test symlink the `cache` component under a real data root and assert the disabled tier; make the warm trap-parity tests assert that the cache served hits (counter) so a silently disabled cache cannot pass; **add the epoch-interruption trap parity case** that DEC-3 Q5 lists (the engine has `epoch_interruption(true)`; a looping guest, a store epoch deadline of one tick and `engine.increment_epoch()` from the test, cold vs warm must trap identically).
6. **Magic.** `b"SHMWIC\x00\x01"` is already 8 bytes (6 ASCII + NUL + SOH); delete the comment in `entry_frame.rs` that claims a seven-to-eight-byte adjustment.
7. **Layout gate.** Round 1 is committed, so `git ls-files` now matches the tree: `python -I scripts/ci/check_layout.py` must exit 0; report the real exit code (no simulation).
8. Keep: `#![forbid(unsafe_code)]`, no `unsafe` anywhere, the Cargo.toml / lock changes of round 1, the backup/restore exclusion (check the case question: the cache directory name is lowercase `cache`, the comparison is exact; keep it exact), imports at the top of every file, `mod.rs` files re-export only, at most 7 entries per folder, at most 1000 lines per file.

Local gates (the only allowed commands; run until clean, one cargo process at a time, `CARGO_BUILD_JOBS=4`): `cargo check -p shamir-wasm-host --all-targets`, `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`, `cargo fmt -p shamir-wasm-host` and the same three for `shamir-db` and `shamir-server` when you touch them (for `fmt` run `-- --check` at the end and report its exit code), plus `python -I scripts/ci/check_layout.py`. Tests and benches are banned locally - the PR's CI runs them.

## Review findings (verbatim)

VERDICT: FAIL

## Request vs result

The request required the incremental-cache brief **in full**: safe Wasmtime cache integration, bounded disk storage, Unix directory validation, database wiring, backup/restore exclusion, all specified regression cases, and clean permitted local gates.

The requested architecture and integration are substantially present. However, file inspection found a production path mismatch, a corruption-path deadlock, broken regression tests, and incomplete coverage. The mandatory layout command also remained unsuccessful. The implementation cannot be accepted as complete.

## Checked myself

These checks used read-only tools in this review turn; no commands or tests were executed.

| Claim/check | Evidence | Result |
|---|---|---|
| The crate forbids unsafe code and enables incremental caching instead of whole-module deserialization. | `view`, `git_read diff`: `crates/shamir-wasm-host/src/lib.rs:1`; `Cargo.toml:17–26`; `wasm/wasm_engine.rs:82–85,196–198` | **Confirmed.** |
| The cache binds after the final engine, including pooling fallback. | `view`: `crates/shamir-wasm-host/src/wasm/wasm_engine.rs:88–114` | **Confirmed.** |
| Unix disk databases pass `<data_root>/cache/wasm`. | `git_read diff`: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:264–280` | **Confirmed at the call site**, but the receiving directory helper misinterprets this path. |
| Backup excludes top-level `cache`, retaining nested names. | `view`, diff: `crates/shamir-server/src/sync/durability/backup.rs:584–591`; `tests/durability/backup_tests.rs:32–52` | **Confirmed** for the implemented case-sensitive path check. |
| Restore rejects a manifest cache path before staging. | `view`: `crates/shamir-server/src/sync/durability/restore.rs:164–179`; `tests/durability/restore_tests.rs:37–77` | **Confirmed** for the tested path. |
| The lock diff changes only the host dependency list. | `git_read diff Cargo.lock` | **Confirmed:** removes direct `blake3`, adds `rustix 1.1.4` and `sha2`. |
| All required host regression cases are present. | Read all four new topic files and their module manifest. | **Refuted.** No epoch-interruption case exists; several other tests do not establish their claimed behavior. |
| The frame magic needed a seven-to-eight-byte adjustment. | `view`: `crates/shamir-wasm-host/src/wasm/compile_cache/entry_frame.rs:4–13` | **Refuted.** Six ASCII bytes plus NUL plus SOH already equal eight bytes. The implemented constant itself is correct. |

The changed-file list is otherwise aligned with the requested rewrite. The deleted old cache implementation and tests were explicitly requested. There are minor unrelated comment/fixture changes in existing backup tests and a shortened Cargo comment, but no unrelated substantial feature or version changes were found.

## Only from the orchestrator’s words

- No behavioral tests were run, as required by policy. Runtime success, trap equivalence, concurrency behavior, and corruption recovery therefore remain unobserved.
- Exact Wasmtime **48.0.7 / Cranelift 0.135.7** source parity remains unverified.
- The custom “post-commit layout simulation” is not the required layout command and does not establish that the mandatory gate passed.

The recorded worker transcripts do support later successful `check`/`clippy` executions for the host, database, and server. In particular, the initially failed server checks were followed by successful worker runs after the fixture repair. That recovery should not be confused with behavioral verification.

## Findings

### P1 — Production directory argument is interpreted incorrectly

**Location:** `crates/shamir-wasm-host/src/wasm/compile_cache/disk_store.rs:48–49`; `private_dir.rs:11–33`; `crates/shamir-db/src/shamir_db/shamir_db/core.rs:269`

**Observed:** The database passes `<data_root>/cache/wasm`; `bind` passes this unchanged to `prepare`. But `prepare` treats its argument as the **data directory**, canonicalizes it, then appends another `cache/wasm`.

**Inferred consequence:** On a fresh database, canonicalization fails because the supplied cache directory does not yet exist, disabling persistence. If it exists, entries go under `<data_root>/cache/wasm/cache/wasm`, not the specified layout.

This also invalidates the directory-trust testing: most tests pass a temporary **data root**, unlike the production caller.

### P1 — Corrupt or missing indexed entries self-deadlock

**Location:** `crates/shamir-wasm-host/src/wasm/compile_cache/disk_store.rs:217–248`, especially `223`, `228`, `236`, `242`; `144–148`

**Observed:** `get` retains the `OccupiedEntry` returned by `get_sync`. Error branches call `invalidate`, which calls `remove_sync` on the same key before that guard is dropped.

The inspected SCC source establishes the mechanism: `scc-3.8.4/src/hash_map.rs:979–1012` acquires an exclusive bucket guard for `get_sync`; `:927–939` acquires that bucket again for removal.

**Inferred consequence:** A corruption check, missing file, or read failure waits on the caller’s own held bucket lock instead of returning a cache miss. This contradicts the reported “corrupt entry → miss” behavior and can hang compilation.

### P1 — Concurrent trimming can lose accounting updates

**Location:** `crates/shamir-wasm-host/src/wasm/compile_cache/disk_store.rs:172–197,299–319`

**Observed:** Trimming snapshots byte/count totals, calculates local remaining values, then overwrites the shared atomics with `store`. Concurrent insertion updates those same atomics using `fetch_add`. The trimmer flag excludes other trimmers, not insertions.

**Inferred consequence:** An insertion between the snapshot and a later store can have its accounting overwritten. Subsequent cap enforcement can undercount retained entries and bytes. The bounded-cache guarantee is therefore not established.

### P2 — Restart ordering is not logical recency, and bind does not enforce caps

**Location:** `crates/shamir-wasm-host/src/wasm/compile_cache/disk_store.rs:41,94–126,245–248`

**Observed:** Startup entries receive nanosecond wall-clock ticks, while the live tick counter remains zero. Startup totals are published without trimming.

**Inferred consequence:** Newly inserted or recently accessed entries receive much smaller ticks than untouched startup entries and are evicted first. An over-cap scanned cache also remains over-cap until a later qualifying insertion.

### P1 — New regression suite contains deterministic failures

**Locations:**
- `crates/shamir-wasm-host/src/tests/wasm/compile_cache/namespace_tests.rs:7–40`
- `trim_tests.rs:26–47`
- `crates/shamir-wasm-host/src/wasm/compile_cache/disk_store.rs:178–179`

**Observed:**
- Namespace tests search the temporary root for `v*` directories, although `prepare` creates namespaces under its appended `cache/wasm`.
- Trim tests similarly inspect the wrong directory level.
- Trim hooks pass `u64::MAX` or `usize::MAX`, but target calculation multiplies those values by three before division.
- The entry-cap test’s own comment says five entries trimmed to three, while its assertion expects only `[4, 5]`.

**Inferred consequence:** The namespace test fails its directory-count assertion; trim tests can panic on overflow with overflow checking and do not correctly inspect the cache layout.

Additionally, most disk tests are not Unix-gated even though their helper unconditionally unwraps `bind`, and non-Unix `prepare` deliberately returns `Unsupported`. They are therefore incompatible with the specified Windows-disabled tier.

### P2 — Required coverage is missing or weaker than claimed

**Locations:** `cache_store_tests.rs:64–116`; `namespace_tests.rs:44–80`; `trap_parity_tests.rs:48–99`

**Observed:**
- No epoch-interruption test exists.
- Corruption tests check only `rejected`, not file removal.
- The “oversized” test adds 64 bytes before rebinding, remains below 4 MiB, and provides no observation proving that payload reading was avoided.
- Wrong-key mutation also breaks the digest, so that test does not isolate key validation.
- Warm trap tests do not assert cache hits; disabling caching could leave them passing.
- The symlink test passes the symlink as the argument that `prepare` canonicalizes. Its error can instead come from an unrelated default-mode directory subsequently created by the helper.

These tests do not provide the promised regression evidence.

### P2 — Required clean gate and report were not delivered

**Location:** `docs/dev-artifacts/prompts/native/01-wasm-incremental-cache.md:14–15`

The actual required layout command returned **2**, not zero. Its tracked-deletion limitation is corroborated by `scripts/ci/check_layout.py:127–162`, but a simulated alternative is not a passing required gate.

The final report also omits the requested full file list with line counts and per-ruling implementation map. It reports server `fmt --check` success, whereas the later recorded successful command was `cargo fmt`; the earlier `fmt --check` failed.

## Final answer and process

- **Presentation:** The final answer is coherent, English, substantive, and not empty or garbled. The evidence’s `final_text_checks` has **no flags**. Although its source is classified as `reaction_to_background_notice`, it is an actual work report rather than a short notice acknowledgment.
- **Accuracy:** “Implemented … in full” is unsupported. The report simultaneously claims all regression cases are present and admits the missing epoch case. Its claim of indirect epoch coverage is not supported by tests that never enable or exercise epoch interruption.
- **Stubs:** Searches found no newly added TODO/FIXME/unimplemented placeholders. The panic calls in trap tests are legitimate unexpected-success assertions, not stubs.
- **Execution:** No prohibited behavioral tests were observed. Notification-based waits were used; no shell sleep/polling loop was evident. Later worker transcripts corroborate recovery from the server compile failure.
- **Completion:** No todos or child delegations remain outstanding, but that administrative completion does not resolve the production defects, failing test constructions, or unsuccessful layout gate.

**Conclusion:** The architecture is recognizable, but the production path and corruption handling are defective, and the regression suite is not ready to substantiate the completion claim.
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

Final message (mandatory): the full file list with line counts; each of the eight items above and each finding -> where fixed; the REAL exit codes of every gate; anything you could not do exactly, with the reason. Never end a turn while a child agent is outstanding. Stop editing at 130 minutes and report exactly what is done and what is not.
