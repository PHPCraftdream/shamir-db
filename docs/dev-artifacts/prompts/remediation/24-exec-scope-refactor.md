# Brief — W2 ExecScope refactor

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- `crates/shamir-engine/src/query/batch/execution_deadline.rs` (ExecutionDeadline stays as the time part), NEW file `crates/shamir-engine/src/query/batch/exec_scope.rs` (ExecScope, one primary export), `crates/shamir-engine/src/query/batch/mod.rs` (re-export)
- the executor chain that threads the deadline today: `batch_execute.rs`, `query_runner.rs`, `interactive_tx.rs`, `op_watchdog.rs`, `authorized.rs` (all under `crates/shamir-engine/src/query/batch/`)
- every existing test that names `ExecutionDeadline` (Grep: `query_runner_tests.rs`, `dos_gate_tests.rs`, `interactive_deadline_tests.rs`, `call_tx_tests.rs`) — mechanical signature updates only, no assertion changes
- NO `shamir-query-types` changes (W1 already merged the types: `ExecPolicy`, `BatchLimits::{CEILING, clamped_to}`, `BatchError::LimitExceeded`), NO server files (`tx_handlers.rs` / `tx_registry.rs` belong to another task: if a server call site must change only because of a signature, make the smallest possible edit there and say so), NO commit/WAL/drainer files

## Task

W2 — a pure REFACTOR (behaviour unchanged): replace the bare `ExecutionDeadline` that is threaded through the executor chain with an `ExecScope`, the carrier of the whole per-execution budget. This is step 1 of the server-owned work-budget design (cluster #1207); later steps (W3/W5/W6/W8) will fill the counters in. Read `crates/shamir-query-types/src/batch/exec_policy.rs` and `batch_limits.rs` (merged in W1) first.

Boundary (verify BEFORE editing, then keep): no observable behaviour change — same deadline checks at the same points, same errors, same defaults. The existing suites (`dos_gate_tests`, `for_each` tests, `interactive_deadline_tests`, `query_runner_tests`, `call_tx_tests`) must pass without assertion edits.

Required result:
1. `ExecScope` in its own file: `{ limits: BatchLimits, deadline: ExecutionDeadline, work: AtomicU64, bytes: AtomicU64 }` (the two counters exist but nothing increments them yet) with constructors `ExecScope::root(limits: &BatchLimits)` (deadline built exactly like today's `from_budget_secs` path, i.e. root scope == request limits) and a test-only constructor mirroring `ExecutionDeadline::from_duration` (added by PR #83). Delegating accessors `check()` / `checkpoint` equivalents so call sites change as little as possible.
2. Pass `&'a ExecScope` through the same chain and on the same lifetime where `ExecutionDeadline` is passed today (`execute_batch*`, `execute_plan*`, `execute_plan_tx*`, `QueryRunner`, the nested Batch/ForEach re-entry). It is a REFERENCE on purpose: the async frame must get smaller or equal, not larger — read the long stack-overflow history in `execution_deadline.rs` (module doc) before touching the hot path; do not add `select!`, `Box::pin` or new locals to the per-op frame. A nested body keeps sharing the parent's scope for now (AND-clamping comes in W3).
3. `Authorized` gets a server-owned policy: a field `policy: ExecPolicy` defaulting to `ExecPolicy::EMBEDDED`, `with_policy(self, ExecPolicy) -> Self`, and an accessor. The executor entry points read it and clamp the request limits with `limits.clamped_to(&policy.caps)` when building the root scope (EMBEDDED caps == CEILING, so existing behaviour is unchanged unless a test sets a policy). The `unchecked` test constructor keeps working.
4. One small test file (NEW, in `crates/shamir-engine/src/query/batch/tests/`, + one-line `pub mod`): root scope mirrors the request limits; an `Authorized` with a stricter policy yields a root scope clamped to it; default policy leaves the limits unchanged.
5. Do NOT run benches (no local runs at all). In your final message list every file and signature changed and anything you could not keep mechanical.
