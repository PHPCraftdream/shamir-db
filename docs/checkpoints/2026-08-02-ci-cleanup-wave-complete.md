# Checkpoint — 2026-08-02 17:50 [ci-cleanup-wave-complete]

## Session summary

This session picked up directly from the prior F-69..F-87 remediation wave (all closed, ending at commit `ed8bba00`). The user's compound instruction "удали вообще все задачи, делай коммит и пшу, наладь ci" (delete all tasks, commit, push, fix CI) triggered a multi-stage CI investigation and repair effort. The FIRST push of the session revealed `origin/master` had been 45 commits behind local for the whole session's history — every scheduled CI run for 10+ days had run against stale code. Supply-chain (`cargo-deny`) issues were resolved: `event-listener` bumped to fix RUSTSEC-2026-0221; `wasmtime`'s RUSTSEC-2026-0222 deferred via a triaged `deny.toml` ignore (its only patched versions need Rust 1.94.0, one minor above this workspace's pinned 1.93.0 toolchain — a separate task).

The user later gave a new `/goal`: "реализуй задачи с помощью /crush, между тасками делай коммиты. После завершения всей работы сделай /checkpoint, обнови чейнджлог, закомить все мд и запусти ревью агента @oh". This drove the bulk of the session: task #916 (CI-1) diagnosed and fixed the `e2e-permissions.test.ts` "connection closed" cascade — root cause was the server's per-subnet `auth_init` rate limiter's 60s post-boot warmup window (divided by 4, so 2.5/sec) rejecting a legitimate connection burst; fixed by raising `auth_init_rate_per_second` to 1000 in the TS e2e harness config, mirroring the existing Rust integration-test workaround. Confirmed fixed via two consecutive clean real-CI runs.

Task #917 (CI-2) started as "wrong error code" but was re-diagnosed mid-investigation: the node napi e2e replication tests failed with `hmac_required` because `16-replication.test.js`/`17-replication-convergence.test.js` hand-built `chmod` wire objects with no hmac field. Fixing this (via tasks #919/#920, a two-stage rewrite of `tests/e2e` to use `@shamir/client`'s platform-agnostic query-builder library instead of hand-assembled JSON, per this repo's "builder only" convention) surfaced a SECOND, deeper bug: `SetReplicator` — a real, working, server-tested wire op — had no client-side implementation anywhere (`shamir-client`, `shamir-client-node`, `shamir-client-ts`). Task #921 added it (mirroring `create_scram_user`'s HMAC-computation pattern) and fixed the two test files to use it instead of passing `'replicator'` as a role string. Two more small gaps were found and fixed while chasing full-green CI: the `ts-e2e-nightly.yml` workflow never built `@shamir/client`'s `dist/` (a gitignored build artifact) before the node-e2e job, causing `MODULE_NOT_FOUND`; and `tests/e2e`'s shared server config never set `enable_experimental_migration_api`, so `13-migration.test.js`'s two happy-path tests always failed with `experimental_feature_disabled`. After both fixes, `tests/e2e` went from 121/9 → 130/0 locally, and `ts-e2e-nightly` passed FULLY GREEN (both jobs) on real CI for the first time in 10+ days.

Task #918 (CI-3) investigated whether F-68b's original two 600s CI hangs were resolved by the earlier F-70 lock-order-inversion fix. A real `ci.yml` run (auto-triggered on push during this session) showed the `observability_http` hang DID recur — two tests (`metrics_exposes_finite_byte_budget_gauges`, `metrics_exposes_unbounded_sentinel_when_no_byte_budget`) hung in exact lockstep to 600s TIMEOUT on ubuntu-latest, strongly suggesting shared-resource contention, not independent slowness. This was NOT fixed in this session — tracked as new task #922 for dedicated diagnostic-instrumentation investigation, per this repo's own "instrument, observe on real CI, then fix" convention; raising the timeout is explicitly against repo policy. A second, unrelated flake was discovered as a side effect: `hnsw_rs_contract_tests::parallel_insert_surfaces_all_ids_and_matches_bruteforce_top1` (shamir-index, HNSW recall-threshold assertion) panicked on windows-latest lib tests — tracked as new task #923, not yet investigated.

Every delegated crush run in this session followed prompt-first discipline (brief committed to `docs/dev-artifacts/prompts/post-alpha/142-145.md` before each launch) and was zero-trust verified by the orchestrator (full diff read, personal re-run of fmt/clippy/tests, personal rebuild+test of the e2e suite) before committing — this caught several issues crush's own runs didn't fully resolve (two crush runs hit their timeout budget mid-verification; the orchestrator finished the build+test+commit cycle personally both times) and one crush-introduced JS syntax error (a backtick inside a comment inside a backtick-delimited template literal — happened twice, in two different files, both caught and fixed by the orchestrator before commit).

## Active goal

"реализуй задачи с помощью /crush, между тасками делай коммиты. После завершения всей работы сделай /checkpoint, обнови чейнджлог, закомить все мд и запусти ревью агента @oh - пусть проверит всю работу" — all steps satisfied except the final one (launching the `@oh` review agent), which is the immediate next action after this checkpoint write.

## TaskList

### pending
- #922 F-68b: observability_http 600s hang recurs on ubuntu-latest (needs new diagnostics) — confirmed recurring via real CI (run 30757334929), not yet investigated further this session.
- #923 hnsw_rs_contract_tests recall flake panics on windows-latest lib tests — discovered as a side effect of #918's investigation, not yet started.

### recently completed
- #921 Add SetReplicator client support (shamir-client + napi binding) for CI-2
- #920 tests/e2e: rewrite the 17 test files to use @shamir/client query builders
- #919 tests/e2e: wire up @shamir/client query builders + rewrite shared helpers
- #918 CI-3: confirm F-68b's two 600s CI hangs are resolved (or investigate if not) — completed by confirming recurrence + spawning #922/#923, not by fixing.
- #917 CI-2: fix node napi e2e ReplHello wrong error code (hmac_required vs bad_role) — final root cause was SetReplicator missing client support, fixed by #919-921.
- #916 CI-1: fix TS client e2e "connection closed" cascade in e2e-permissions.test.ts
- #915 (umbrella, decomposed): supply-chain RUSTSEC fixes.

## Decisions

- Deferred wasmtime's RUSTSEC-2026-0222 fix via a triaged `deny.toml` ignore rather than bumping the Rust toolchain (out of scope, invasive, needs its own dedicated task).
- For CI-1, chose the "add missing diagnostic first, observe on real CI, then fix" workflow (this repo's own established convention) over guessing blind — the actual root cause (rate limiter warmup window) would have been very hard to find without the server.logs()-on-failure diagnostic.
- For CI-2, chose a full query-builder rewrite of tests/e2e (per the user's explicit instruction after noticing the hand-assembled-JSON anti-pattern) over a narrow one-line hmac fix — this was a larger investment but structurally correct and caught the deeper SetReplicator gap that a narrow fix would have missed entirely.
- For CI-3/#918, refused to mark F-68b's hang "fixed" on the strength of one earlier clean CI run — required actually re-triggering and observing recurrence before declaring the task's outcome, consistent with zero-trust verification discipline.
- Every crush timeout (three occurred: #919's first attempt, #921's attempt, one Stage-B run that succeeded) was followed by the orchestrator personally reading the diff, rebuilding, and running the full test suite before committing — never trusted a crush run's own "it works" narration.

## Open questions

- None outstanding requiring user input. #922 and #923 are tracked but not blocking — genuinely new discoveries from this session's investigation, out of the originally-scoped CI-1/CI-2/CI-3 work.

## Repo state

```
 M CHANGELOG.md
?? docs/checkpoints/2026-08-02-ci-cleanup-wave-complete.md
?? docs/checkpoints/2026-08-02-ci-followup-in-flight.md
```
```
992fb0c4 chore(client-node): regenerate index.d.ts for set_replicator (#921 follow-up)
e27ceacf fix(e2e): enable experimental migration API for tests/e2e's shared test server
f29a0a42 ci(ts-e2e-nightly): build @shamir/client dist before node-e2e job (fixes MODULE_NOT_FOUND)
d237d77f feat(client,client-node): #921 -- add SetReplicator client support, fixes CI-2
dd7844a0 docs(prompts): brief for #921 -- add SetReplicator client support (shamir-client + napi binding)
```

Note: `CHANGELOG.md` and the checkpoint file(s) are about to be committed per the active goal's explicit "закомить все мд" step — this checkpoint is written just before that commit, so its own repo-state snapshot shows them still uncommitted.
