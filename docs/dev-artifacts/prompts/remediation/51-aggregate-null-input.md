# Brief — NULL-AGG explicit NULL aggregate input is distinguishable from absent input

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- `crates/shamir-query-types/src/read/select.rs` (the aggregate `input: Option<FilterValue>` field docs/serde of the two aggregate variants) and, only if the representation needs it, a NEW small serde helper file in the same directory
- `crates/shamir-query-builder/src/` (only the select/aggregate builder entry points that set `input`)
- `crates/shamir-engine/src/query/read/aggregate.rs` and the aggregate-input evaluation next to it (only the part that decides legacy field/All input vs an expression input)
- tests: NEW topic files in `crates/shamir-query-types/src/read/tests/` (or the nearest existing tests dir), `crates/shamir-query-builder/src/` tests dir and `crates/shamir-engine/src/query/read/tests/` (+ one-line `pub mod` each)
- NO commit/WAL/tx files, NO read_exec.rs, NO FilterContext changes (other agents edit those concurrently)

## Task

NULL-AGG (new leaf of the aggregate-input work; B14-3 merged, read `crates/shamir-query-types/src/read/select.rs` aggregate variants and `docs/dev-artifacts/prompts/remediation/34-aggregate-expression-input.md`).

Defect (documented edge): the aggregate `input: Option<FilterValue>` treats "missing OR null" as "use the legacy field / All input". An explicit literal NULL input therefore round-trips (serde, msgpack) as "no input": `count(<NULL expression>)` is silently executed as the row count / field aggregate instead of aggregating over a NULL expression (which yields NULL per row, skipped by the aggregate's NULL rule: COUNT 0, SUM/MIN/MAX/AVG NULL, according to the existing NULL-skipping rule of each aggregate).
Required: make an explicit NULL expression input distinguishable from an absent input end to end - wire (serde JSON and msgpack, both directions; backward compatibility of the OLD wire shape is NOT a constraint, there is no release yet, but absent input must still mean the legacy field/All input and must still be omitted on serialization), builder (an explicit way to build an aggregate over a literal NULL expression), and the engine (an explicit NULL input follows the aggregate's NULL-skipping rule). Pick the smallest sound representation (for example a dedicated serde helper that distinguishes present-null from absent without changing the in-memory type of the common path, or a small `AggInput` enum) and say why in the report; keep the common path (no input) allocation-free.
Tests (red first): serde round-trips (JSON and msgpack) for absent vs literal NULL vs expression input in both aggregate variants; builder constructs each; engine: `COUNT(NULL expr)` = 0 while `COUNT(*)` = N over the same rows, `SUM(NULL expr)` is NULL, grouped and ungrouped. Update the field doc comments (they currently say "Missing or null means the legacy field/All input").
Do NOT run benches.
