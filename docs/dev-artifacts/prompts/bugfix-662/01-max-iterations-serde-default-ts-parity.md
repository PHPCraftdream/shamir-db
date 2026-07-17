# #662 — `BatchLimits.max_iterations` breaks TS `.limits()` (wire-breaking)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (already root-caused, confirmed by the orchestrator — do not
## re-investigate)

`crates/shamir-query-types/src/batch/batch_limits.rs`'s `BatchLimits`
struct gained a `max_iterations: usize` field in Epic04/B (#653) with NO
`#[serde(default = ...)]` attribute:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchLimits {
    pub max_queries: usize,
    pub max_dependency_depth: usize,
    pub max_execution_time_secs: u64,
    pub max_result_size: usize,
    pub max_nesting_depth: usize,
    pub max_iterations: usize,   // <-- no #[serde(default)] here
}
```

serde's derive makes this field MANDATORY on deserialization whenever a
`limits` map is present on the wire. The TS client's `BatchLimits`
interface (`crates/shamir-client-ts/src/core/types/batch.ts:119-126`),
`DEFAULT_LIMITS` constant, and `.limits()` builder method
(`crates/shamir-client-ts/src/core/builders/batch.ts:34-40`, `:284-297`)
were never updated for #653 — they only know the original 5 fields. The
result: ANY TypeScript client that calls `.limits(partial)` on a `Batch`
sends a 5-field `limits` map, which the Rust server's serde deserializer
rejects with `"missing field \`max_iterations\`"`. This is a real,
confirmed wire-compatibility break, found during a final-session code
review and independently re-confirmed by grepping both files (zero
`max_iterations` occurrences anywhere in `crates/shamir-client-ts/src/core/types/batch.ts`
or `crates/shamir-client-ts/src/core/builders/batch.ts`).

## Fix

1. **Rust**: add `#[serde(default = "default_max_iterations")]` (or
   equivalent — match whatever idiom, if any, this codebase already uses
   for defaulted struct fields; if none exists, a small private
   `fn default_max_iterations() -> usize { 1000 }` next to the struct is
   fine) to `BatchLimits.max_iterations`, so a `limits` map that omits it
   deserializes successfully with the same `1000` value
   `BatchLimits::default()` already uses. This is the MINIMAL fix that
   restores backward compatibility for every client (TS, older Rust
   clients, hand-rolled wire payloads) that doesn't know about
   `max_iterations` yet.
2. **TS type**: add `max_iterations: number;` to the `BatchLimits`
   interface (`crates/shamir-client-ts/src/core/types/batch.ts:119-126`).
3. **TS builder default + method**: add `max_iterations: 1000` to
   `DEFAULT_LIMITS` (`builders/batch.ts:34-40`) and thread
   `partial.max_iterations ?? DEFAULT_LIMITS.max_iterations` into the
   `.limits()` method (`builders/batch.ts:284-297`), matching the existing
   pattern for the other 5 fields exactly.
4. **Regression test — the critical part.** The reason this broke silently
   is that NO test exercises `.limits()` against a REAL server (only
   shape-level unit tests that never round-trip through serde on the Rust
   side). Add (or extend an existing) e2e test — check
   `crates/shamir-client-ts/src/__tests__/` for the right home, likely
   alongside other batch e2e tests — that builds a batch with
   `.limits({...partial...})` (a PARTIAL object, deliberately omitting
   `max_iterations`, to prove the Rust-side default kicks in correctly)
   and executes it against a real running server, asserting the request
   succeeds (not just that the TS-side shape looks right). Also add a
   companion case that explicitly sets `max_iterations` to confirm the
   full round-trip works when it IS provided.
5. **Rust unit test**: add a test in `batch_limits.rs`'s test module (or
   wherever `BatchLimits` serde behavior is already tested — check for an
   existing test file) confirming that a `limits` JSON/msgpack payload
   MISSING `max_iterations` deserializes successfully with `max_iterations
   == 1000` (the same default), proving the fix directly at the Rust
   layer independent of the TS client.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types --full` green.
- The new/extended e2e test passes against a real server — you will need
  a fresh release build if the current one is stale relative to your
  changes (`CARGO_TARGET_DIR=D:/dev/rust/.cargo-target cargo build
  --release -p shamir-server`, forward slashes only; check first whether
  the existing binary at `D:/dev/rust/.cargo-target/release/shamir-server.exe`
  is already fresh enough — if you only touched `shamir-query-types` and
  the server binary already embeds a rebuilt `shamir-query-types`, you may
  not need a full rebuild; verify via the e2e harness's own staleness
  check rather than guessing).
- TS unit tests for `batch.ts`/`Batch.limits()` pass.
- `cargo fmt -p shamir-query-types -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Check for and delete any stray `devrust.cargo-target*` directory in the
  repo root before finishing — never commit it.

## Out of scope

- Do NOT touch #661, #663, #665, #666, #667 — separate tasks.
- Do NOT change `BatchLimits::default()`'s actual default VALUES — only
  add the missing serde-default wiring and TS parity.
