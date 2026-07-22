# Checkpoint — 2026-07-21 14:xx [ri7-in-flight]

## Session summary

Continuation of a long `/babygoal`-driven session ("реализуй задачи с помощью
`/crush`, между задачами делай коммиты, покрой их тестами"). Previous window
closed out the entire 2026-07-17 release-audit campaign (Этапы 1-8 +
#695/#729/#715). This window opened with the user asking me to review TWO
new external release-readiness reports (2026-07-20/21): one on release
*infrastructure* (versioning, Docker, CI, security release-blockers,
positioning) and one on *functional* gaps (transactional scan RYOW,
`with_version`/CAS, u64 truncation, journal-gap handling, streaming cursors,
plus a long tail of SQL/Mongo/Redis-parity gaps). I verified the concrete
factual claims of both reports against the actual code (spot-checked ~25
citations across both, all confirmed accurate) before accepting them, then
decomposed the accepted findings into two task series after clarifying
scope/priority with the user via `AskUserQuestion`:

- **RI-1 … RI-13** — release-infrastructure campaign (doc-accuracy tail,
  Docker fix, versioning v0.1.0-alpha.1 + CHANGELOG, tag-based release
  workflow, TS/Node CI wiring, auth test-vector suite + fuzz/power-fail
  honesty, actor-semantics threading, resource-profile safety, bootstrap-
  token lifecycle, replication proto_ver + journal-gap honesty, restore
  command, README positioning rewrite, and a final frozen-commit gate).
- **FG-1 … FG-5** — functional-correctness series (u64→Big promotion,
  `with_version`/CAS full contour, tx-scan read-your-own-writes full
  overlay, a public KNOWN_LIMITATIONS.md, and a post-alpha server-side
  cursor/streaming task explicitly deferred past the alpha gate).

Both series were confirmed via explicit `AskUserQuestion` decision points
with the user (RYOW: full overlay; u64: promote to Big; with_version:
implement fully; cursors: post-alpha) before being written into the
TaskList. The user's "все должно быть с тестами, e2e тестами, ts e2e
тестами — отражаться на всех уровнях, на всех query builders" requirement
is baked into every FG-task's description.

**RI-1 through RI-6 are DONE, verified, and committed** (see commit list
below) via the established prompt-first `/crush` discipline: investigate →
write brief → commit brief → launch `crush run` in background → wait for
real completion → zero-trust re-verification (full diff read + independent
re-run of build/test/fmt/clippy, never trusting the agent's own envelope) →
commit. Two noteworthy zero-trust catches this window:
- RI-1's delegated agent itself found (but correctly left out of its own
  scope) a `deploy/README.md` CLI argument-order bug
  (`--config` must precede the `backup` subcommand, not follow it) — I
  verified this empirically against the built binary and fixed it myself
  as a trivial 2-line addition to the same commit.
- RI-5's CI-wiring surfaced two PRE-EXISTING stale TS e2e tests (once a
  real server backed the suite for the first time) — I investigated both
  personally (read the real planner code and the real TS builder code)
  and confirmed BOTH are stale-test issues, not bugs from RI-5: the
  `$cond`-planner bug the first test was pinning is already fixed
  (`extract_deps_from_filter_value` already recurses into `Cond`/`Expr`/
  `FnCall`), and the second test predates a since-added client-side
  `efSearch` guard. Rather than silently fix or fold these into RI-5, I
  created a dedicated follow-up task (**RI-14**, #748) and wired it as a
  blocker on the final gate (#742/RI-13).

**RI-7 is IN FLIGHT right now** — this is the actor-semantics correctness
fix (validators + nested WASM `ctx.call` currently hardcode
`Actor::System` instead of threading the real caller/parent actor). Before
writing this brief I did NOT unilaterally decide the actor-semantics
contract — I used `AskUserQuestion` to surface the two real design options
(caller-actor vs. definer-actor for validators; parent-inheriting vs.
System-default for nested WASM), and the user explicitly asked me to
consult two sub-agents for a second opinion (`@fh` and `@fx`) rather than
decide alone. Both independently converged on the SAME recommendation I
had reached: **caller actor for validators, parent-actor-inheritance for
nested WASM**, with an explicit Definer/setuid opt-in mechanism reserved
as future/out-of-scope work for any genuine escalation need. `@fx` also
flagged a related, lower-priority finding: `db`/`repo`/`net`/`secret_grants`
are ALSO not threaded through nested WASM calls today (fails CLOSED, so
lower urgency than the actor fail-OPEN issue) — noted in the brief as an
optional bonus scope item, not mandatory.

The RI-7 brief (`docs/dev-artifacts/prompts/correctness/
02-validator-nested-wasm-actor-threading.md`, commit `b9933809`) was
written with this decided contract baked in (not re-litigated by the
delegate), covering: threading a real `actor: &Actor` parameter through
`execute_insert_tx`/`execute_update_tx`/`execute_delete_tx`
(`write_exec.rs`) from `QueryRunner.actor` (6 call sites in
`query_runner.rs`), replacing 8 internal `&Actor::System` uses; and adding
an `actor: Actor` field to `HostState` (`shamir-wasm-host`), threading it
through `WasmFunction::call` and `host_call.rs`'s two phases so nested
`ctx.call` inherits the parent's actor. Mandatory regression tests were
required, each proven to fail against the OLD hardcoded-System behavior
before the fix (revert-confirm-fail-then-reapply discipline, matching how
task #729's fix was verified earlier in the campaign).

**Current literal state**: the `ri7-actor-threading` crush session is
`alive` (fresh heartbeat as of the last check before this checkpoint was
written). The working tree already shows the expected shape of files
touched (`write_exec.rs`, `query_runner.rs`, `table_manager_validators.rs`,
`host_call.rs`, `wasm_function.rs`, plus several existing test files and
two NEW test files — `validator_actor_tests.rs` and
`nested_actor_tests.rs`) — this is IN-PROGRESS agent output, NOT yet
zero-trust-verified or committed. A few stray `check_*.log`/`test_actor.log`
files are visible at repo root — these are transient scratch logs the
agent is still working with; they must be cleaned up before the eventual
commit, per this session's established discipline.

Mid-session, the user replaced the standing `/babygoal`-implied directive
with an explicit `/goal` Stop hook carrying the SAME core instruction plus
one addition: **"Если /crush войдёт в пиковые часы, то переключайся на
агентов @sh"** — i.e., if a `crush run` refuses due to a provider peak-hours
window, fall back to `@sh` (Sonnet-high local sub-agents) specifically,
rather than the crush skill's own default fallback aliases
(`@ao46l`/`@ash`). I acknowledged this explicitly and it is now the
binding fallback rule for the rest of this campaign. No peak-hours refusal
has actually occurred yet this session — all `crush run` launches (RI-1
through RI-7) have completed normally.

## Active goal

A `/goal` Stop hook is currently ARMED (set by the user mid-session, not by
me) with this exact condition text:

> реализуй задачи с помощью /crush , между задачами делай коммиты, покрой
> их тестами. Если /crush войдет в пиковые часы, то переключайся на
> ангетов @sh

This hook will block session-stop until the TaskList is fully drained
(mirroring the babysit/babygoal TaskList-driven completion model) — the
peak-hours clause is a standing fallback instruction, not a completion
condition per se.

## TaskList

### in_progress
- #736 RI-7. Actor-семантика validators и nested WASM host_call: контракт + тесты (blockedBy: none)

### pending
- #737 RI-8. Безопасные resource-профили: Argon2, result cap, connection limits (small/medium)
- #738 RI-9. Bootstrap-token lifecycle: TTL/автоудаление/выход, согласовать код, CLI-доки и спеку
- #739 RI-10. Репликация: минимальная proto_ver-валидация + честный experimental-статус в доках (+ journal-gap → resync_required, scope expanded mid-session)
- #740 RI-11. Restore-команда: shamir-server restore + manifest/checksums + инвалидация session tickets
- #741 RI-12. Переписать позиционирование в README: убрать «замена PostgreSQL/MySQL/MongoDB/Redis/Memcached»
- #742 RI-13. Frozen commit: полный локальный gate + push + зелёный удалённый CI (blockedBy: #736,#737,#738,#739,#740,#741,#743,#744,#745,#746,#748 — only user may authorize the actual push/tag)
- #743 FG-1. Единый контракт u64: промоция в Big вместо wrapping-каста и клампа — все уровни + оба builders
- #744 FG-2. with_version: полный CAS-контур — версия в результатах, expected_version, conflict-ошибка, оба builders, TS SDK
- #745 FG-3. Tx-scan read-your-own-writes: полный overlay staged write_set в потоковые сканы + флип C8-теста
- #746 FG-4. KNOWN_LIMITATIONS.md (blockedBy: #739,#743,#744,#745 — written after the behavior it documents lands)
- #747 FG-5 (пост-альфа). Server-side cursors / streaming результатов — deferred past the alpha gate per explicit user decision
- #748 RI-14. Update 2 stale TS e2e tests (planner-bug fix already landed; client efSearch guard already shipped) — discovered during RI-5 verification, blocks #742

### recently completed
- #735 RI-6. Auth test vectors + fuzz/power-fail
- #734 RI-5. Подключить Node/TS e2e к CI
- #733 RI-4. Tag-based release workflow
- #732 RI-3. Версионирование v0.1.0-alpha.1
- #731 RI-2. Docker build fix
- #730 RI-1. redb-остатки и CLI-расхождения
- (earlier, prior session) #695, #729, #715, #723-#728 — entire 2026-07-17
  release-audit campaign, Этапы 1-8

## Decisions

- **Both external reviews' factual claims were independently verified
  against the code before acceptance** — not taken on faith. ~25 citations
  spot-checked across both reports; all confirmed accurate (Dockerfile
  1.83-vs-1.93 + dead `COPY src`, TS `"private": true`, 102 unpushed
  commits, missing `auth_v1.msgpack`, `resync`-less journal-gap handling,
  `Actor::System` hardcoding in both validators and nested WASM, etc.).
- **u64 contract: promote to `Big`** (not wrapping-cast, not clamp, not a
  brand-new `UInt(u64)` type) — user's explicit choice, smallest-blast-
  radius option that is still lossless.
- **`with_version`: implement the full CAS contour**, not just hide the
  half-wired flag — user's explicit choice.
- **Tx-scan RYOW: full overlay** (not reject-scan, not document-only) —
  user's explicit choice, acknowledged as the largest of the four options
  but the only semantically-correct one.
- **Server-side cursors/streaming: explicitly POST-ALPHA**, not part of
  the alpha gate — user's explicit choice; task #747 exists so the scope
  isn't lost, not so it blocks #742.
- **RI-14 (2 stale TS test fixes) was NOT folded into RI-5** — kept as an
  independent, explicitly-scoped follow-up task instead, since RI-5's own
  brief scoped fixing them out ("report only") and they are genuine,
  separate findings (a planner-bug fix landing silently, and a client-side
  guard shipping silently) that deserve their own dedicated verification.
- **RI-7's actor-semantics contract was NOT decided by me alone** — the
  user explicitly redirected me to consult `@fh` and `@fx` as independent
  advisors before finalizing; both converged on caller-actor /
  parent-inheriting-actor, matching my own preliminary read of the
  evidence, giving high confidence in the contract now baked into the RI-7
  brief.
- **Peak-hours crush fallback is now `@sh` specifically** (per the user's
  `/goal` addendum), overriding the crush skill's own default fallback
  aliases (`@ao46l`/`@ash`) for the remainder of this campaign.

## Open questions

None outstanding from the user requiring a stop — RI-7 is mechanically in
flight (crush session alive) and the rest of the TaskList is a clear,
already-decided sequential backlog. The only "soft" open item is RI-9's
bootstrap-token design (TTL/output-path specifics) and RI-8's exact
resource-profile numbers, which their own task descriptions note may need
a quick confirm-via-brief but are not blocking anything right now.

## Repo state

```
 M crates/shamir-engine/src/query/batch/query_runner.rs
 M crates/shamir-engine/src/table/table_manager_validators.rs
 M crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs
 M crates/shamir-engine/src/table/tests/insert_tx_tests.rs
 M crates/shamir-engine/src/table/tests/s_read_server_tests.rs
 M crates/shamir-engine/src/table/tests/s_write_server_tests.rs
 M crates/shamir-engine/src/table/tests/set_byte_merge_parity_tests.rs
 M crates/shamir-engine/src/table/tests/write_exec_tests.rs
 M crates/shamir-engine/src/table/write_exec.rs
 M crates/shamir-engine/src/tx/tests/recovery_tests.rs
 M crates/shamir-engine/src/validator/tests/mod.rs
 M crates/shamir-engine/src/validator/tests/validator_db_tests.rs
 M crates/shamir-engine/tests/crash_recovery.rs
 M crates/shamir-wasm-host/src/tests/mod.rs
 M crates/shamir-wasm-host/src/wasm/host_call.rs
 M crates/shamir-wasm-host/src/wasm/wasm_function.rs
?? check_engine.log
?? check_tests.log
?? check_wasm.log
?? crates/shamir-engine/src/validator/tests/validator_actor_tests.rs
?? crates/shamir-wasm-host/src/tests/nested_actor_tests.rs
?? docs/checkpoints/... (this file + prior-session checkpoints)
?? test_actor.log
```

(All of the above is the RI-7 crush session's IN-PROGRESS, NOT-YET-VERIFIED
output — the stray `.log` files must be deleted before the eventual
commit.)

```
b9933809 docs(prompts): brief for RI-7 -- thread caller actor through validators + nested WASM calls
d9aaa44e test(connect): full byte-exact auth test-vector suite + honest fuzz/power-fail status (RI-6)
2e02b558 docs(prompts): brief for RI-6 -- full auth test-vector suite + fuzz/power-fail decision
b90a4472 ci: wire TS unit tests into per-PR gate, TS/Node e2e into nightly (RI-5)
5d6c93bf docs(prompts): brief for RI-5 -- wire TS unit tests + TS/Node e2e into CI
0facc28a ci(release): add tag-based release workflow (RI-4)
c041e0e5 docs(prompts): brief for RI-4 -- tag-based release workflow
b7982131 chore(release): version everything as 0.1.0-alpha.1, add publish=false
```

## Active timers

A babysit cron (job id `53635720`, `7,22,37,52 * * * *`, session-only —
re-check via `CronList` at the start of the next session, since job ids
rotate and crons auto-expire after 7 days) has been ticking every 15
minutes throughout this window, correctly reporting "still running #736"
on each tick while the RI-7 crush session is active. A `/goal` Stop hook is
ALSO now armed (see Active goal above) — both mechanisms are watching the
same underlying TaskList-completion condition from different angles.
