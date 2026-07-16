# #634 — TS client `chown`/`addGroupMember` with resolved `principal64` fails (BigInt wire)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context — investigate first, do not assume the fix

Unlike other recent bugfix briefs in this repo, this one is NOT fully
root-caused yet. An e2e test already exists and already exercises the
exact failure mode described in the task title:
`crates/shamir-client-ts/src/__tests__/e2e-principal.test.ts` — its tests
`'chown with resolved principal64 works (BigInt on wire)'` and
`'addGroupMember with resolved principal64 works (BigInt on wire)'`.
These tests are currently SKIPPED in this environment because no release
server binary exists (disk space was cleared this session). Your FIRST
step is to build the release server and get a REAL run of this test suite
(passing or failing) — do not rely on static code reading alone to declare
the bug fixed.

```
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target cargo build --release -p shamir-server
```

(Forward slashes only — a known Git-Bash backslash-escaping bug creates a
stray `devrust.cargo-target*` directory in the repo root otherwise; delete
it before finishing if it appears, never commit it. This build takes
around 25-30 minutes — expect it, don't treat it as a hang.)

Then run:

```
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target npx vitest run src/__tests__/e2e-principal.test.ts
```

(from `crates/shamir-client-ts/`) and capture the REAL error message. If it
unexpectedly PASSES on the first real run, do not declare victory — dig
into WHY the task believed it was broken (check git history / task
tracker context for how #634 was originally reported; it may be
intermittent/data-dependent, see the hypothesis below) and write a
regression test that reliably reproduces the failure mode before
concluding there's nothing to fix.

## A concrete, PLAUSIBLE (not confirmed) hypothesis — check this first, but verify against the real error, don't assume it's correct

Read `crates/shamir-client-ts/src/core/framing.ts`'s `encode`/
`promoteWideInts` functions in full (already-existing code, not something
to change blindly) — this file already handles some real "wide integer
encodes as msgpack float64 instead of int64" bugs (see its doc comments)
using `@msgpack/msgpack`'s `useBigInt64: true` option.

The `principal64` values involved here (`ChownOp.owner: u64` and
`AddGroupMemberOp.user: u64` on the Rust side — see
`crates/shamir-query-types/src/admin/access.rs:104-110` and `:184-191`)
are real, randomly server-assigned 64-bit ids (task #548) — meaning they
are uniformly distributed across the full 64-bit space, NOT small
sequential numbers. A JS `bigint` in the range `0..2^63-1` and a `bigint`
in the range `2^63..2^64-1` may get encoded by `@msgpack/msgpack` using
DIFFERENT msgpack wire formats (signed "int 64" vs unsigned "uint 64") even
though both are semantically valid non-negative integers — if the Rust
side (`rmp_serde`, deserializing into a `u64` field) is strict about which
wire format tag it accepts for a `u64` target, THIS COULD explain a bug
that only manifests for ROUGHLY HALF of randomly-generated principal64
values (whichever half happens to encode via the "wrong" format for a
`u64` target) — consistent with a bug that's easy to miss in ad-hoc manual
testing but reliably reproduces once you test across enough real random
ids (exactly what an e2e test with a real server-assigned id does).

If this hypothesis is confirmed by the real error message, the fix belongs
on WHICHEVER side is actually wrong:
- If `@msgpack/msgpack` is emitting a format the Rust side + `rmp_serde`
  cannot accept for a `u64` target, check whether `rmp_serde`'s version in
  this workspace already handles both formats correctly (it may be a
  library version issue, or the actual Rust type needs a custom
  `Deserialize` shim) — do NOT weaken the Rust side to `i64` (would corrupt
  ids with the high bit set); the correct field type is `u64`.
- If the JS encoder needs to be told to use unsigned format specifically
  for principal64-shaped values, check whether `framing.ts`'s
  `promoteWideInts`-style pre-pass (or a similar narrow, targeted fix) is
  the right place, OR whether `@msgpack/msgpack`'s encode options support
  forcing unsigned encoding for specific fields.

If the REAL error message points somewhere else entirely (a different bug
than this hypothesis), investigate and fix THAT instead — this hypothesis
is a lead, not a mandate.

## Fix + verify

1. Root-cause with evidence (the real captured error from the rebuilt
   server + e2e test run).
2. Fix precisely, in whichever layer (TS client encode, Rust server
   decode, or both) the evidence points to.
3. If the existing `e2e-principal.test.ts` doesn't already reliably
   reproduce the bug (e.g. if it only fails ~50% of the time due to random
   id generation), strengthen it or add a companion test that forces both
   the "high bit set" and "high bit clear" id cases deterministically (you
   may need to seed/loop until you get an id in each range, or find
   another way to control the id's bit pattern for a targeted regression
   test) so this can't silently regress again.
4. Update the guide docs / any doc that describes `chown`/`addGroupMember`
   principal64 usage if the root cause reveals a caller-facing caveat.

## Verification (MANDATORY before you report done)

- The real e2e test (`e2e-principal.test.ts`) passes reliably — run it
  more than once if there's any randomness involved, to build confidence
  it's not a coin-flip pass.
- `./scripts/test.sh -p shamir-query-types -p shamir-server --full` green
  if you touch any Rust code.
- TS builder/unit tests green for any touched TS files.
- `cargo fmt --check` clean for any touched Rust crate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean if any
  Rust code was touched.
- Report the literal ORIGINAL failing error message you captured (before
  your fix) alongside the passing output after — this is the evidence that
  proves you fixed the real bug, not a symptom.
- Check for and delete any stray `devrust.cargo-target*` directory in the
  repo root before finishing — never commit it.

## Out of scope

- Do NOT touch #659 or any other task.
- If the release build genuinely cannot complete in your session (e.g.
  hits the same kind of transient issue seen earlier this session), report
  that clearly rather than silently giving up or declaring success without
  real evidence.
