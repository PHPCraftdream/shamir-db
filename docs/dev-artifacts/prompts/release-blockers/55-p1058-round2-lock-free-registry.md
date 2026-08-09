# Brief 55 — #1058 round 2: registry must be lock-free (scc::HashMap), not std::sync::Mutex

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right

The logic is correct: capture points inside `plan_record_created`/
`plan_record_updated`/`plan_record_deleted`, per-def `Building`+in-flight
conditional, `Ready` defs on the same write still get normal ops, the
"Building but not yet in-flight" case correctly preserves today's direct-
write behavior, and all 5 required test scenarios plus solid registry API
unit tests are present and correct. Do NOT change any of the test logic or
the capture conditionals — only the underlying concurrency primitive.

## The one thing to fix — `in_flight_builds` must be `scc::HashMap`

The original brief was explicit: *"По идеологии проекта — lock-free
(`scc::HashMap` с `THasher`), не `Mutex`"* for the in-flight build registry.
The delivered code instead added `pub(super) in_flight_builds: Arc<Mutex<BTreeMap<u64, ()>>>`,
with a doc comment claiming *"The brief allows `std::sync::Mutex` as a
sanctioned low-frequency fallback for DDL operations"* — that is not what
the brief said; the brief said the opposite. Fix the doc comment along with
the type.

**Why this matters beyond "the brief said so":** `is_build_in_flight` is
called from inside `plan_record_created`/`plan_record_updated`/
`plan_record_deleted` — these are the SAME shared planning methods every
insert/update/delete in the system funnels through (per the write-path
audit, `docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md`).
The call is short-circuited by `def.state == IndexState::Building` first, so
it's NOT paid on every write to every `Ready` index — but for ANY table with
a `Building` index (which includes the ordinary, already-existing
synchronous whole-barrier CREATE INDEX flow, not just online builds), EVERY
write now takes a `std::sync::Mutex::lock()`. This repo's stated idiom
(`CLAUDE.md`, "Code ideology") bans blocking mutexes on hot paths precisely
for this reason — this is a new one, not a pre-existing one being copied
forward.

(Separately, an unrelated audit found `dropping_regular`/`dropping_unique`
— the existing precedent this round 1 copied — ALSO violate this same idiom
already. That's tracked as its own task and is NOT an excuse to add a third
instance here; fix this one properly rather than extending the pattern.)

## What to change

1. `in_flight_builds`: change to `scc::HashMap<u64, (), shamir_types::types::common::THasher>`
   (check the exact import path for `THasher` used elsewhere in this file —
   `regular_provenance` or nearby code likely already imports it; match the
   existing convention). Update `mark_build_in_flight`, `is_build_in_flight`,
   `clear_build_in_flight` to use `scc::HashMap`'s API
   (`insert_async`/`contains_async`/`remove_async` or the sync variants —
   check what's already used elsewhere on `IndexManager` for a similar
   registry, e.g. how `renaming_regular`/`renaming_unique` or any other
   `scc::HashMap`-based field on this same struct is accessed, and mirror
   that exact style). These methods are currently sync (no `.await`) — if
   `scc::HashMap`'s API requires async for the operations you need, either
   make these methods async (and update the 3 call sites inside
   `plan_record_created`/`updated`/`deleted`, which are already `async fn`,
   to `.await` them) or use `scc::HashMap`'s sync-compatible methods if
   available (check `scc` crate's actual API — don't assume, verify against
   what's already used in this codebase).
2. `dirty_sets`: leave as `Arc<Mutex<BTreeMap<u64, Arc<Mutex<BTreeSet<RecordId>>>>>>`
   for THIS round — it's only touched once `is_build_in_flight` has already
   returned true (i.e., only for indexes genuinely mid-online-build, a
   legitimately rare/low-frequency case unlike the registry check itself).
   Converting it too is not required now; leave a `// TODO` note if you
   think it should follow later, but don't block this round on it.
3. Update the doc comments on both fields to accurately state what's lock-
   free and why, and correct the false "brief allows Mutex" claim.

## Do not touch

- The capture conditionals inside the three planning methods (correct as-is).
- Any of the 15 tests in `p1058_in_flight_build_registry_tests.rs` — they
  should all still pass unchanged after this refactor, since they only
  observe `is_build_in_flight`/`mark_build_in_flight`/`clear_build_in_flight`'s
  external behavior, not the internal storage type. If any test breaks
  because it directly inspects `in_flight_builds`' internals rather than
  going through the public methods, fix the TEST to use the public API
  instead of reaching into the field — don't weaken the assertion.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
```

Report the exact diff and confirm all 15 existing tests in
`p1058_in_flight_build_registry_tests.rs` still pass unchanged (or, if any
needed a mechanical fix to go through the public API instead of touching
`in_flight_builds` directly, name which ones and what changed).
