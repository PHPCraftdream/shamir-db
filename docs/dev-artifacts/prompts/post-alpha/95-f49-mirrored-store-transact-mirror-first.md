# Brief for F-49 (#860, P0) — MirroredStore::transact mirror-first ordering

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

The 2026-07-28 readonly review
(`docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, §3
P0-4) found that `MirroredStore::transact` still mutates `primary`
BEFORE the durable mirror commit — the OPPOSITE order of what F-39/F-41
already fixed for the single-key `set`/`remove` methods in the SAME file.
**Read `crates/shamir-storage/src/storage_mirrored.rs` in full first** —
it's short (~580 lines) and every method's doc comment is directly
relevant.

**Read `set` (line 351-364) and `remove` (line 381-390) first** — these
are the ALREADY-FIXED mirror-first pattern (F-41) this task must apply to
`transact` too: for a classified (durable) key, the mirror write happens
FIRST; only if it succeeds does `primary` get mutated. `set`'s doc
comment (line 313-350) explains exactly why this ordering matters (error
atomicity: a mirror failure must mean "nothing happened", not "primary
already mutated but caller got Err") and why the REVERSE-direction
residual (mirror succeeds, primary then fails) is provably unreachable —
`InMemoryStore::set`/`remove` are structurally infallible (no `?`
propagation, no `Err` return path; confirmed by reading the actual
`InMemoryStore` source, not assumed).

**The gap**: `transact` (line 536-580) does NOT follow this same
ordering. Its three phases are:

1. Ephemeral ops → `primary` (per-op).
2. **Durable ops → `primary` (per-op) — "for immediate read visibility".**
3. **Durable ops → `mirror` atomically** (`self.mirror.transact(durable_ops)`).

Phase 2 runs BEFORE Phase 3. If Phase 3 (`mirror.transact`) returns
`Err`, the caller sees the failure — but `primary` was ALREADY mutated in
Phase 2, so the live process is already reading/serving the new
config/index metadata as if the write had succeeded, even though nothing
was durably committed. This is exactly the class of bug F-41 closed for
`set`/`remove`, reopened here for `transact`. It is "especially dangerous
for index rename/metadata transactions" (the review's own words) — the
exact use case `transact`'s cross-op atomicity was added for (F-33
hybrid-repo campaign).

## What to do

### 1. Adversarial red test FIRST

Write a deterministic test proving the CURRENT bug: use (or extend) the
existing fault-injection test doubles in
`crates/shamir-storage/src/tests/storage_mirrored_tests.rs` (read that
file first — it already has `FailingTransactMirror`/`ObservingMirror`
test doubles from F-39, and `FailingSetRemoveMirror`/`CapturingLogger`
from F-41; reuse/extend these rather than inventing new ones) that makes
`mirror.transact` fail. Prove that, on the CURRENT code, a subsequent
`primary.get`/`iter_stream` read (via the `MirroredStore`'s own read
path, immediately after the failed `transact` call returns `Err`) shows
the durable ops' effects ALREADY applied — i.e. the live process
"lied" about the failed durability, exactly as the doc for `set` warns
against. This is the review's own explicit ask: "fault tests должны
после mirror failure проверять И immediate live reads, И reopen" (not
just reopen behavior, which F-39's existing tests likely already cover —
check).

### 2. Apply the fix — reorder to mirror-first

Reorder `transact`'s durable-subset handling: commit to `mirror` FIRST
(atomically, via `self.mirror.transact(...)`), and only apply to
`primary` AFTER that succeeds. Since `mirror.transact` consumes its
`Vec<KvOp>` argument by value, and the durable ops must ALSO be applied
to `primary` afterward, clone the durable ops before handing them to
`mirror.transact` (`KvOp` already derives `Clone` — verified by reading
`crates/shamir-storage/src/types.rs:20-24`). Mirroring `set`/`remove`'s
already-established comment style, document:
- the new ordering (mirror-first, primary-second) and why (error
  atomicity: a mirror failure now means "nothing happened" for the
  durable subset, matching `set`/`remove`);
- that the reverse-direction residual (mirror succeeds, primary then
  fails) is unreachable for the SAME reason `set`/`remove`'s doc already
  established (`InMemoryStore::set`/`remove` infallibility) — cite that
  existing doc rather than re-deriving it from scratch, this file
  already has the investigation.
- Keep Phase 1 (ephemeral ops → primary) unchanged — first, per-op, no
  atomicity — that's not what this task is about; the concurrent-reader
  residual for the ephemeral phase is F-39's own separately-documented,
  accepted-and-out-of-scope finding, do not re-investigate it.

Whether to keep the exact 3-phase structure (ephemeral → durable-to-mirror
→ durable-to-primary) or restructure further is your call, but the
observable behavior must be: **no read of `primary` after a failed
`mirror.transact` may show any of the durable subset's effects.**

The review also suggests (as an alternative, more involved design) "для
согласованного read publish — собрать новый primary snapshot и
RCU-swapнуть его после durable success" — a full snapshot+RCU-swap
mechanism. Given `InMemoryStore::set`/`remove`'s confirmed infallibility
(see above), a simple mirror-first reorder is sufficient to close this
specific bug without that larger mechanism; only reach for the
snapshot/RCU approach if you find during investigation that the simple
reorder is somehow insufficient (explain why, if so).

### 3. Update the doc comment

`transact`'s own doc comment (line 454-535, especially the "# Ordering —
ephemeral-first, then durable" section at line 478-511) currently
describes the OLD (bug) ordering as if it were intentional design.
Rewrite it to describe the NEW ordering and why, mirroring how F-41
updated `set`/`remove`'s doc comments when IT changed their ordering.

## Constraints

- Do not touch the ephemeral-ops phase's ordering or its documented
  concurrent-reader residual (F-39's own accepted, out-of-scope
  finding) — this task is about the durable subset's mirror-vs-primary
  ordering only.
- Do not touch `set`/`remove`/hydration (`MirroredStore::new`) — those
  are F-41's already-correct code, unrelated to this task except as the
  pattern to mirror.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-storage -- --check` and
  `cargo clippy -p shamir-storage --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage -- storage_mirrored
./scripts/test.sh -p shamir-storage --full
```

When done, give your final summary as plain text: the red test's proof
(what it demonstrated failing on the unfixed code — the specific live
read that showed durable-subset effects after a failed transact — with
actual test output), the exact fix applied, why the reverse-direction
residual remains unreachable (citing the existing `set`/`remove` doc's
investigation rather than re-deriving it), full test run output, and
confirmation fmt/clippy are clean.
