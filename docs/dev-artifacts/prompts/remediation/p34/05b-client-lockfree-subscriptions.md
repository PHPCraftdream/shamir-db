# P34-05b - shamir-client: lock-free subscription routing state (RV-CLIENT-B80C0D09, owner decision "variant B")

Source: card L3-RV-CLIENT-B80C0D09 (std::sync::Mutex on the demux hot paths without a sanctioned category) in the P3 audit and the owner's answer of 2026-11-01 recorded below. P34-05 already moved the pending-request registry to `scc::HashMap` (`PendingRegistry` + `ClosedState` in `crates/shamir-client/src/client/transport.rs`); the one production `std::sync::Mutex` left in the crate is `SubsInner` behind `SubsMap` (transport.rs, "Standard mutex guards the coherent subscription state"). Read first: `CLAUDE.md` (section "Code ideology" and its table of primitives), `crates/shamir-client/src/client/transport.rs` (`ClosedState`, `PendingRegistry`, `SubsInner`, `Lifecycle::shutdown`, the reader task that routes push frames, around line 595), `subscription.rs` (`SubscriptionHandle`, `Drop`, the caps), `api.rs` (the `subscribe_push` registration around line 335), and the tests `crates/shamir-client/src/tests/client/demux_tests.rs`, `lifecycle_tests.rs`.

## Owner decision (binding, 2026-11-01)

Lock-free wherever it is possible, in this case too. Do NOT add a fourth sanctioned `std::sync::Mutex` category to CLAUDE.md and do NOT leave the mutex with a better comment. Mutexes are discussed only for a weighty reason: if you believe a mutex is unavoidable for some part, stop and say so in the final report with the concrete invariant that no lock-free structure can keep; the orchestrator takes it to the owner.

## Goal

Replace `SubsMap = Arc<StdMutex<SubsInner>>` by a lock-free structure that keeps every invariant the mutex gives today:

1. One registry entry per subscription id with an atomic state transition: `Early(buffered pushes)` (pushes that arrived before the subscription registered) -> `Live(sender + termination channel)`. Use `scc::HashMap<u64, _, THasher>` (workspace default hasher) and its per-key atomic entry/update operations; no `std::sync::Mutex`, no `parking_lot`, no `RwLock`. A single map with a state enum is the recommended shape, but you design it; justify it in the design note.
2. Registration vs reader routing: a push for an id that is not live yet is buffered, bounded by `EARLY_BUFFER_CAP` per id and `EARLY_BUFFER_TOTAL_CAP` over all ids (the total must stay exact: reserve budget with a CAS/`fetch_update` loop on an `AtomicUsize`, release it on registration, drop and shutdown; keep a debug oracle `total == sum of buffer lengths` reachable from tests). On registration the buffered envelopes are delivered to the new channel in arrival order BEFORE any later push, with no loss and no duplicate; a full channel drops with the existing warning exactly as today.
3. Close protocol: reuse the existing `ClosedState` (cause published BEFORE the drain, first cause wins) exactly like `PendingRegistry`: a registrant re-reads the close state AFTER a successful insert; an insert that landed before the drain is resolved by it, one that lands after removes its own entry and the handle is created already terminated with the published cause (the behaviour of subscribing on a closed client today). `Lifecycle::shutdown` stays idempotent and the single drainer: it sends the cause to every termination channel, clears live and early state and resets the total to zero, and no registration may survive it. Choose and document the memory orderings; `SeqCst` where a store-buffering race is possible (publish-then-drain vs insert-then-check), with the argument written in the doc comment of the type.
4. `SubscriptionHandle::drop` removes the route and the termination channel only if the registered sender is still the handle's own channel (`same_channel` check), atomically (a conditional remove on the entry), so a re-registered id is never evicted by a stale handle.
5. No behaviour change on the wire or in the public API: `PushSender`, `PushReceiver`, `SubscriptionHandle` and the `Client` methods keep their signatures; the constants keep their values.
6. Source-text tests that look for the literal `state.subs.lock()` (lifecycle_tests.rs around lines 758 and 772) assert the old implementation; replace them with behavioural assertions of the same intent (registration and routing are atomic w.r.t. shutdown) or delete them with a written reason; never leave a vacuous test.

## Deliverables

- A short design note committed at `docs/dev-artifacts/audits/2026-11-01-p34-05b-lockfree-subscriptions/DESIGN.md`, written FIRST: the state machine (states, transitions, who may perform each), the race analysis for every pair of concurrent operations (route vs register, register vs shutdown, route vs shutdown, drop vs register of the same id, route vs route for the early budget), the memory-ordering argument, and the test plan.
- Tests (red first in CI where the old code can fail them; no sleeps; use the existing `LifecycleHooks` / `HookPoint` seams and multi-threaded tokio tests): a stress test with concurrent router tasks, registrants and a shutdown that asserts the invariants (every handle ends terminated with the one published cause; the early total returns to zero; no envelope is delivered twice; order is preserved per id); a test that the early budget never exceeds the total cap under concurrent routers; a test that a stale handle drop does not evict a re-registered id; a test that registration after shutdown yields a terminated handle and leaves no residue in the map.
- The `HookPoint` test fixture under `#[cfg(test)]` keeps its `std::sync::Mutex` (test fixture, allowed by CLAUDE.md); say so in the report.

## Scope

Crate `shamir-client`: `client/transport.rs`, `client/subscription.rs`, `client/api.rs`, their tests, and the design note. New files are allowed for the registry type (one primary export per file; `transport.rs` is already large, do not grow it past 1000 lines, move the registry into a new file under `client/`, and keep the folder at 7 entries or fewer, split into a subfolder if needed). No new dependency (`scc` and `shamir_collections` are already used here). Public signatures of the crate do not change.

## Coordination with other tasks

Nothing else edits `shamir-client`. P34-03b edits engine/db/server files, P34-07b and P34-07c edit `shamir-index`.

## Decisions

RV-CLIENT-B80C0D09: **IMPLEMENT variant B** (owner, 2026-11-01): rewrite onto lock-free primitives (`scc`); no new sanctioned-mutex category.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-client (check shamir-client-node when a public signature changed). Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
