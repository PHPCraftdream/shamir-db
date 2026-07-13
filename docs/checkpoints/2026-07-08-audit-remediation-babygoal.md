# Checkpoint — 2026-07-08 [audit-remediation-babygoal]

## Session summary

This session continued from an earlier bench-normalization effort (see the prior checkpoint `2026-07-07-bench-normalization-crush-fanout.md`) and then pivoted to `/babygoal`-driven remediation of the 5-agent project-wide security/durability/perf/concurrency/client audit from `docs/audits/2026-07-06-*.md` (produced in an earlier session by 5 parallel `@fxx` review agents). The user explicitly asked (verbatim): "Реализуй остальные задачи последовательно с помощью агентов /crush, ревью делай с помощью агентов @sh (agent), после ревью (и необходимых правок) делай коммиты" — i.e. strict sequential pipeline: brief committed first (prompt-first rule) → `/crush` agent implements → `@sh` agent reviews adversarially → orchestrator (me) independently re-runs the diff read + gate (fmt/clippy/tests) → commit. `/babygoal` armed a `/babysit` cron (job id `92faac60`, every ~20 min, off-minute cron `7,27,47 * * * *`) that resumes/advances whichever task is `in_progress` on each tick.

Strategy established and followed throughout: read the relevant `docs/audits/2026-07-06-*.md` section for exact file:line, write a numbered brief under `docs/prompts/audit/NN-*.md`, commit it alone (`docs(prompts): brief for ...`), launch `crush run --role smart --session <slug> --timeout 60m < brief > out 2> err` in the background, wait for the task-notification, read the agent's own report AND independently `git diff` the actual changed files (never trust the report alone), delegate an adversarial review to the `@sh` sub-agent (Sonnet, via the `Agent` tool with `subagent_type: "sh"`) with explicit numbered scrutiny points, personally re-run `cargo fmt -p <crate> -- --check` / `cargo clippy -p <crate> --all-targets -- -D warnings` / `./scripts/test.sh -p <crate>` before trusting SHIP IT, then commit with a long descriptive message crediting the review and citing what was independently re-verified. When a cluster task (e.g. "HIGH-durability: ... (1.3/1.5-1.9, 2.2-2.6)") contained multiple distinct findings, the pattern established was: fix the highest-value/most-tractable finding(s) first, then split the task — mark the original `completed` and spin the remainder into a new numbered residual task (e.g. `#476` MEDIUM-durability residual, `#477` HIGH-security residual, `#478` HIGH-perf residual, `#480` HIGH-client residual) — rather than leaving one giant task open indefinitely. This was applied consistently to #443, #444, #445, #446.

**Major outcome so far: ALL 8 original CRITICAL findings are closed, PLUS a 9th CRITICAL bug was discovered mid-session** (not in the original audit) — `ShamirClient.resume()` in the TS client (`crates/shamir-client-ts`) decoded the server's `resume_ok` response as a named msgpack MAP when the server actually sends a POSITIONAL ARRAY (confirmed by reading the actual `rmp_serde::to_vec` vs `to_vec_named` call and the Rust client's own positional decode of the same struct) — meaning `resume()` was **completely non-functional against any real server**, always throwing "session_id must be 32 bytes". This was tracked as task #479 (CRIT-9), fixed with a positional decode matching the existing `auth_ok` pattern in `protocol.ts`, and — critically — a genuine NEW live-server e2e test (`e2e-resume.test.ts`) was added that actually round-trips a real resume call against a real server binary, closing the test-coverage gap that let the bug ship undetected in the first place (the only 2 pre-existing tests touching `resume()` both mocked a MAP, never an array, so nothing caught it).

All 4 original HIGH clusters (#443 durability, #444 security, #445 perf, #446 client) have had their FIRST/highest-priority finding(s) fixed and committed; each cluster's remaining findings were split into a residual task (#476-478, #480) to be picked up later, prioritized behind other HIGH work per engineering judgment (HIGH before MEDIUM, but a newly-discovered CRITICAL like #479 jumps the queue). Task #447 (HIGH-concurrency-isolation, the last untouched HIGH cluster: MVCC/SSI races A2/A3/A4/A6/A7+A8-A14 from `docs/audits/2026-07-06-concurrency-engine.md`) was just started: A5 and A7 from this cluster were ALREADY fixed earlier (A5 = CRIT-3/#437, A7 = part of #443's GroupCommit RAII fix). The brief for A2 (`publish_cell`/`seed_version` non-monotonic version regression — a drainer/recovery race that causes stale reads and masks SSI conflicts) was just written, committed (`e2a7c35a`), and a crush agent was launched (`session high-concurrency-a2`, background task `b6okpitgf`) — **this is the task in flight at the moment of this checkpoint; no result has come back yet.**

Files/areas inspected this session (read, not necessarily changed): `docs/audits/2026-07-06-{durability-storage-wal-tx,security-network-surface,perf-hot-paths,client-surface-parity,concurrency-engine,SUMMARY}.md` (all 5 panel-agent reports + summary); `crates/shamir-engine/src/table/read_exec.rs` (index2 empty-result bug, fixed); `crates/shamir-client-ts/src/core/builders/batch.ts`+`types/batch.ts`+their tests (BatchLimits missing field, fixed); `crates/shamir-wal/src/{wal_segment,segment_set}.rs` (segment poisoning, fixed); `crates/shamir-engine/src/repo/group_commit/mod.rs` + `repo_instance.rs` (GroupCommit leader cancellation, fixed); `crates/shamir-server/src/server/server_launcher.rs` + `conn_limiter.rs` (accept timeout + new PerIpLimiter, fixed); `crates/shamir-funclib/src/crypto.rs` (Argon2 concurrency cap, fixed); `crates/shamir-engine/src/table/write_exec.rs` (UPDATE/upsert dead de-interning, fixed); `crates/shamir-client-ts/src/core/client.ts`+`protocol.ts`+`scram.ts` (query_version hardcoding AND the resume() positional-decode bug, both fixed); `crates/shamir-tx/src/mvcc_store/{mod,mvcc_history}.rs` (A2 monotonicity — brief written, fix in flight).

Zero-trust verification discipline was maintained throughout: every "SHIP IT" from `@sh` was followed by the orchestrator personally re-running fmt/clippy/tests before committing, and at least twice the orchestrator caught something the fixing agent's own report understated or the reviewer flagged as needing a fix (the WAL `SegmentSet::append_batch` unconditional `payloads.clone()` on the hot path — fixed personally via an `Arc`-wrap after `@sh`'s review demanded it; and independently verifying the CRIT-9 wire-shape claim by reading `rmp_serde::to_vec` call sites directly rather than trusting either agent's assertion).

No commits have been pushed (user never asked for `git push`). All commits use the trailer `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` per CLAUDE.md convention (the model name in the trailer is a slight mismatch with the actual model — Sonnet 5 — but this was the established trailer text used throughout this multi-session campaign and was not changed).

## Active goal

None (`/goal` was never invoked this session — progress is driven purely by the `/babygoal`-created TaskList + the `/babysit` cron, per that skill's design: "No `/goal` machinery").

## TaskList

### in_progress
- #447 HIGH-concurrency-isolation: MVCC/SSI кластер (A2/A3/A4/A6/A7 + A8-A14)  (A2 sub-fix in flight — crush session `high-concurrency-a2`, background task `b6okpitgf`, no result yet as of this checkpoint)

### pending
- #476 MEDIUM-durability: WAL/recovery/MemBuffer residual cluster (1.5-1.9, 2.2-2.6)
- #477 HIGH-security residual: ticket channel-binding + subscription fanout limits + WASM fuel/SSRF
- #478 HIGH-perf residual: keyset O(N²) pagination + unbounded ts_index/cells + MemBuffer drain amplification + SQ8 SIMD fusion
- #480 HIGH-client residual: error-code typing, request/connect timeouts, wire-type drift (explain/async_index/records_idmsgpack), executeWithTouch parity, e2e/parity gaps
- #448 CLEANUP: устаревшие/лживые doc-комментарии + мёртвый код (durability §3, concurrency §2-3)
- #449 COMPLIANCE-1: cargo-deny/cargo-audit CI-гейт + SECURITY.md + captrack path-pin
- #450 COMPLIANCE-2: plaintext username в auth-логах (PII/user-enumeration) + wasmtime advisory-политика
- #451 COMPLIANCE-3: docs/security/data-protection.md — at-rest шифрование + PII retention/erasure политика
- #452 PERF-RADICAL-1: fjall zero-copy Bytes + устранение contains_key-перед-write двойного лукапа
- #453 PERF-RADICAL-2: CachedStore read-after-write + жадная материализация стримов
- #454 PERF-RADICAL-3: posting-list Arc + sorted-slice представление вместо BTreeSet-клонов
- #455 PERF-RADICAL-4: funclib distinct() O(N²) + WAL segment-open replay + interner reverse-vec clone
- #456 PERF-RADICAL-5: CREATE INDEX полная материализация таблицы + fjall spawn_blocking-per-op + TCP framing memcpy
- #457 PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → инлайн Key128(u128) сквозной ключ (архитектурная, высокая сложность)
- #458 FLAKE: vr5_cofilter_sees_staged_and_filters_residual intermittent failure — root cause not yet captured
- #459 Migrate remaining 41 criterion benches to bench-scale-tool fixed-iteration harness (largely superseded by this session's earlier bench-normalization work — needs a re-check of what remains, if anything)

### recently completed
- #479 CRIT-9: TS client resume() decodes resume_ok as named map but wire sends positional array (commit `cced2851`)
- #446 HIGH-client: query_version hardcoded + resume server_query_version staleness (commit `f2a41d39`)
- #445 HIGH-perf: UPDATE/upsert-merge dead de-interning without validators (commit `13bc42ef`)
- #444 HIGH-security: TLS/WS accept timeout + per-IP cap (commit `e37670bf`); Argon2 aggregate concurrency cap (commit `17adf821`)
- #443 HIGH-durability: WAL segment quarantine + GroupCommit leader RAII (commit `6daf76af`)
- #442 CRIT-8: TS BatchLimits missing max_nesting_depth (commit `c0061ed3`)
- #441 CRIT-7: index2 empty-result full-scan fallthrough (commit `b9abb99a`)
- #440, #439, #438, #437, #436, #435: CRIT-6 through CRIT-1 (all completed in an earlier portion of this session, before this checkpoint's visible window — see commit log for exact SHAs)

## Decisions

- **Sequential /crush pipeline, not parallel** — per explicit user instruction; every finding gets its own brief → crush → @sh review → orchestrator gate re-check → commit cycle, one at a time, never batched.
- **Split large cluster tasks rather than block on full completion** — when a task like "#443 HIGH-durability (1.3/1.5-1.9, 2.2-2.6)" bundles many distinct findings, fix the highest-value ones, mark the original task completed, and open a new residual task for the rest at lower queue priority. Chosen over either (a) doing all sub-findings in one giant delegation (too large/risky for one crush pass) or (b) leaving the task open indefinitely blocking the pipeline.
- **A newly-discovered CRITICAL (#479) jumps the queue ahead of numerically-lower-priority pending HIGH tasks** — chosen over strict TaskList ID order, since a completely-broken shipped feature (resume()) outweighs incremental HIGH-severity hardening.
- **Zero-trust re-verification is mandatory even after @sh says SHIP IT** — the orchestrator always personally re-runs fmt/clippy/tests and reads the actual diff before committing; this caught at least one real issue (`SegmentSet::append_batch`'s unconditional clone) that needed a follow-up fix before commit.
- **CRIT-9's wire-shape claim was independently re-verified from primary sources** (reading the actual `rmp_serde::to_vec`/`to_vec_named` call sites and struct field orders on both server and Rust-client sides) rather than trusting either the fixing agent's or the reviewing agent's assertion at face value — this is the standard applied to any claim that "the wire format is X."

## Open questions

- None currently blocking. The natural next steps (in likely priority order, for whoever/whatever resumes this) are: (1) let the in-flight #447/A2 crush task finish, review, commit; (2) continue #447 with A3, A4, A6 (A8-A14 are MEDIUM/lower — likely also worth splitting into a residual task once A2/A3/A4/A6 are done, mirroring the pattern used for #443-446); (3) work through #476-478, #480 (the residual HIGH/MEDIUM queues) before descending into #448 (CLEANUP) and the COMPLIANCE/PERF-RADICAL tasks (#449-457), which were explicitly lower-priority in the original audit's own "что обязаны улучшить" vs "что ускорить" framing.
- Task #457 (PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → Key128) is flagged in its own title as "архитектурная, высокая сложность" — this likely needs a dedicated design/planning pass (possibly its own `/babygoal` or at least a much longer brief with explicit design-decision checkpoints) rather than a single crush delegation, whenever it's picked up.
- Task #459 (migrate remaining 41 criterion benches) may be largely or fully superseded by the earlier bench-normalization session work (all benches were migrated to `bench-scale-tool` and normalized to ≤10ms/call) — worth a quick audit of what (if anything) remains before treating it as still-open work.
- A number of stray `.log` files have accumulated in the repo root from crush agents' own gate-verification runs (`clippy.log`, `full.log`, `full2.log`, ..., `red.log`, `green*.log`, etc. — visible in `git status --short` as untracked). These are harmless scratch artifacts but should probably be cleaned up (`rm`) at some natural pause point — not done yet, and per CLAUDE.md the orchestrator (not agents) should do this, agents are forbidden from `rm`.

## Repo state

```
?? bench-cli-stdout.log
?? bench-history.log
?? cancel.log
?? clippy.log
?? finalverify.log
?? full.log
?? full2.log
?? full3.log
?? full4.log
?? full5.log
?? gate_gc.log
?? gate_wal.log
?? gc.log
?? gc2.log
?? green.log
?? green2.log
?? green_full_engine.log
?? green_set_merge.log
?? green_update_merge.log
?? green_validator_gate.log
?? green_write_exec.log
?? greenfinal.log
?? poison.log
?? red.log
?? red_tests.log
?? redcheck.log
?? redcheck2.log
?? redfinal.log
?? redfinal2.log
?? wal_all.log
```

```
e2a7c35a docs(prompts): brief for HIGH-concurrency A2 publish_cell/seed_version monotonicity
cced2851 fix(client-ts): CRIT-9 resume() decoded positional wire array as a named map
253f1c34 docs(prompts): brief for CRIT-9 TS resume() positional array decode fix
f2a41d39 fix(client-ts): stop hardcoding query_version:1 + read server_query_version on resume
640dab69 docs(prompts): brief for HIGH-client TS query_version hardcoded + resume downgrade
```
