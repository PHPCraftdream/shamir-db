# Funclib top-up 4d — uuid_v4() (+ optional random/random_bytes) as pure:false

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fourth P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
per report 10 (`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~lines 48, 139, 156, 306):

> No generation functions — no `uuid_v4()`, no `random()`. The registry
> already models impure/non-deterministic entries (`datetime/now` sets
> `pure:false`), so there is no architectural obstacle. Docs advertise
> `{"$fn": "UUID"}`. Client-generated IDs are the current workaround;
> server-side default-value generation (computed DEFAULT) can't mint IDs
> today.

`uuid_v4()` is the REQUIRED deliverable of this brief. `random()` /
`random_bytes(n)` are explicitly OPTIONAL per the report's own wording
("and optionally") — implement them only if you have capacity after
`uuid_v4()` is done, tested, and verified; do not let them block or dilute
the required function.

## Investigation already done (verify yourself too)

- **No `uuid` crate anywhere in this workspace.** `rand = "0.9.x"` IS
  already a dependency in several crates (`shamir-types`, `shamir-engine`,
  `shamir-db`, `shamir-client`, `shamir-connect` — grep their `Cargo.toml`
  files) but NOT in `shamir-funclib` yet.
- **Recommendation: do NOT add the `uuid` crate as a new dependency.** A
  v4 UUID is simply 128 random bits with 6 fixed bits (RFC 4122 §4.4): set
  the version nibble (bits 48-51 of the 128-bit value) to `0100` (4), and
  the variant bits (top 2 bits of the byte at position 64) to `10`. This is
  trivial to construct by hand from `rand`'s existing RNG (16 random bytes,
  two bitwise fixups, format as the canonical
  `xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx` hex-with-dashes string) — no need
  for a whole new crate + its transitive dependency surface for something
  this small, especially given this project's own security-audit history
  around supply-chain surface (see `docs/dev-artifacts/research/2026-07-17-release-audit/`'s
  compliance-track findings). **If, after investigating, you find a
  compelling reason the hand-rolled approach is worse (e.g. RFC-4122
  edge-case correctness this brief's author didn't anticipate), you may
  add the `uuid` crate instead — state your reasoning explicitly in your
  summary if you deviate from this recommendation.**
- **Registry purity model already supports this** — `crates/shamir-funclib/src/datetime.rs`'s
  `now`/`age` registrations (read them, ~lines 44-68) are the exact
  precedent: construct a raw `FnEntry { f, min_args, max_args, pure: false,
  deterministic: false, trusted_pure: false }` instead of the
  `FnEntry::pure(...)` convenience constructor (which hard-codes
  `pure/deterministic: true`). Mirror this shape exactly.
- **New module**: per the report's own suggested folder name, create
  `crates/shamir-funclib/src/gen.rs` (+ `crates/shamir-funclib/src/gen/tests/`
  per this codebase's test-organisation convention), registered via
  `reg.in_folder("gen", gen::register)` in `lib.rs` — so the wire-visible
  name is `gen/uuid_v4` (mirroring `math/abs`, `null/coalesce`, etc. from
  earlier stages of this same campaign).

## The task

1. Add `rand` as a dependency of `shamir-funclib`'s `Cargo.toml` — pin the
   SAME version already used elsewhere in the workspace (check the exact
   version string other crates use, e.g. `"0.9.2"`, and match it exactly
   for consistency; don't introduce a second, slightly-different pinned
   version).
2. Create `crates/shamir-funclib/src/gen.rs`:
   - Module doc comment listing registered functions and the impurity
     convention, mirroring `datetime.rs`'s and `math.rs`'s doc-comment
     style.
   - `pub fn register(reg: &mut ScalarRegistry)` registering `uuid_v4`
     (0 args) as `pure: false, deterministic: false, trusted_pure: false`
     (a UUID generator can never back a functional index — it's the
     textbook impure case).
   - The v4 UUID construction: generate 16 random bytes via `rand`'s
     current-thread RNG (check what API surface `rand 0.9.x` exposes for
     this — the API shifted between `rand` 0.8 and 0.9, don't assume the
     old `rand::random()`/`thread_rng()` names still work verbatim, verify
     against the actual pinned version's docs/source), apply the
     version/variant bitwise fixups described above, and format as the
     canonical lowercase hyphenated hex string.
3. Wire `pub mod gen;` + `reg.in_folder("gen", gen::register);` into
   `crates/shamir-funclib/src/lib.rs`'s `register_builtins()`, alongside
   the other categories (see how `null` was wired in an earlier stage of
   this campaign for the exact pattern to copy).
4. **(Optional, only if capacity remains)**: `random()` returning a
   `F64` in `[0.0, 1.0)` and/or `random_bytes(n)` returning a `Bin` of `n`
   random bytes — same impurity model, same module. If you implement
   these, give them the same rigor (tests, docs) as `uuid_v4`; if you
   skip them, say so explicitly in your summary and don't leave a
   half-wired stub.

## Tests

1. `uuid_v4()` returns a string matching the canonical UUID format
   (`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`
   — note the fixed `4` version nibble and the variant nibble constrained
   to `8`/`9`/`a`/`b`). Use a regex or manual character-position checks —
   whichever this codebase's existing string-validation tests
   (`validate.rs`'s `is_uuid`, if it exists — check) already use as a
   pattern, for consistency; if `validate/is_uuid` already exists, the
   cleanest test is `assert!(is_uuid(&generated_string))` reusing that
   exact validator rather than re-deriving a regex.
2. Two calls to `uuid_v4()` produce DIFFERENT strings (not deterministic —
   this is the whole point of the function; a flaky "might collide"
   caveat is fine to note in a comment, collision probability is
   astronomically low for 122 random bits, don't over-engineer a
   retry-on-collision mechanism).
3. `register_builtins()` wiring regression: `gen/uuid_v4` resolves through
   the top-level registry (mirroring the `null/coalesce` wiring-regression
   test pattern from an earlier stage of this campaign).
4. If you implement the optional `random()`/`random_bytes(n)`: `random()`
   is in `[0.0, 1.0)` across many samples; `random_bytes(n)` returns
   exactly `n` bytes; `random_bytes(0)` returns an empty `Bin` (or errors,
   your call — document which).

## Out of scope

- Do NOT touch any OTHER Этап 4 P0 item (null functions — done in 4a; agg
  wire params/distinct — done in 4b; datetime format/parse — done in 4c;
  arrays/sort, parse_json/to_json — separate later leaf tasks).
- Do NOT wire `uuid_v4()` into schema DEFAULT-value machinery (the
  report's mention of "server-side default-value generation... can't mint
  IDs today" describes the MOTIVATION for this function existing, not a
  requirement that this brief also wires it into `ComputedDefault`/schema
  rules — that would be a separate, larger follow-up if ever pursued).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-funclib --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-funclib`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Confirm the exact `rand` API calls you used (crate version, RNG source
  function name) — state them explicitly, don't assume `rand::thread_rng()`
  still exists in whatever 0.9.x version this workspace pins without
  checking.
- State explicitly whether you implemented the optional `random()`/
  `random_bytes(n)` or skipped them, and why.
