# Brief: CR-B8 — registry chores: `by_session` leak + `THasher` convention (#774)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem R-3 (MED) — `CursorRegistry::by_session` entry leak, verified against the current tree 2026-07-23

`crates/shamir-server/src/cursor_registry.rs::free_session_slot`
(~lines 420-424) only decrements the session's `AtomicUsize` counter:

```rust
fn free_session_slot(&self, owner_sid: &[u8; 32]) {
    if let Some(counter) = self.by_session.get(owner_sid) {
        counter.value().fetch_sub(1, Ordering::AcqRel);
    }
}
```

The `by_session: DashMap<[u8; 32], Arc<AtomicUsize>>` (~line 305) map entry
is NEVER removed, even once its count reaches 0. Session ids are random,
per-connection `[u8; 32]` values (see `Session::new`), so EVERY connection
that ever opened at least one cursor permanently leaks one
`[u8; 32] -> Arc<AtomicUsize>` map entry — unbounded growth on a long-lived
server with many short connections. Compare
`crates/shamir-server/src/tx_registry.rs::TxRegistry::remove` (~lines
243-251), which DOES clean up its own `by_session` entry via
`self.by_session.remove_if(&arc.owner_sid, |_, h| *h == handle)`.

### Fix — atomic decrement-and-maybe-remove via `DashMap::remove_if`

`TxRegistry`'s `by_session` maps directly to a single `u64` handle
(one-tx-per-session), so its `remove_if` is an identity check. `CursorRegistry`'s
`by_session` is a COUNTER (multiple cursors per session, up to a cap), so
the analogous fix needs to combine the decrement and the "now zero, so
remove" check into ONE atomic step — otherwise a concurrent `register()`
racing between "decrement" and "remove because it was zero" could observe
a stale zero, get a fresh counter via `or_insert_with`, bump it, and then
have ITS entry removed by the racing `free_session_slot` — silently
losing this session's true open-cursor count and breaking the per-session
cap enforcement (a session could then exceed `max_per_session` across two
independently-created counters).

**Preferred fix**: change `free_session_slot` to use `DashMap::remove_if`
with a predicate that performs the decrement AND reports whether the
result reached zero, in one call:

```rust
fn free_session_slot(&self, owner_sid: &[u8; 32]) {
    // `remove_if`'s predicate runs while dashmap holds the shard's write
    // lock for this key — the same lock a concurrent `register()`'s
    // `.entry(owner_sid)` call needs to acquire for this key. So the
    // decrement-then-maybe-remove below is atomic relative to a
    // concurrent register: either register's entry() call happens
    // strictly before this remove_if (sees the pre-decrement count, adds
    // to the same Arc, remove_if's predicate then observes a NON-zero
    // result and does not remove), or strictly after (remove_if already
    // removed the entry, register's entry() finds nothing and creates a
    // fresh AtomicUsize(0) via or_insert_with, correctly starting over
    // for a session that just hit zero). Either interleaving preserves
    // correct cap accounting.
    self.by_session.remove_if(owner_sid, |_, counter| {
        counter.fetch_sub(1, Ordering::AcqRel) == 1 // pre-decrement value == 1 means the NEW value is 0
    });
}
```

**Verify the concurrency claim above against the actual `dashmap` version
this workspace pins** (check `Cargo.lock`/`dashmap`'s docs for
`remove_if`'s locking guarantee — it should hold the shard's write lock
for the duration of the predicate call, same as `TxRegistry::remove`'s
existing `remove_if` usage already relies on implicitly) before committing
to this design. If your own reading of `dashmap`'s source/docs disagrees
with the reasoning above, STOP and use the brief's documented fallback
instead: sweep zero-count `by_session` entries during the existing
reaper's periodic tick (`crates/shamir-server/src/cursor_registry.rs`'s
`spawn_reaper_task`/reaper sweep function, ~line 464 onward — add a
`by_session.retain(|_, counter| counter.load(Ordering::Acquire) != 0)`
call alongside the existing `sweep_tombstones` call) — simpler,
eventually-consistent, and avoids the TOCTOU reasoning above entirely at
the cost of a bounded delay before a zero-count entry is reclaimed.
Whichever you choose, **document the choice and the race reasoning in a
code comment**, and write a concurrent stress test (see Tests below)
proving the cap is never violated across many parallel register/remove
cycles for the same session.

## Problem B-1 (LOW) — hasher convention violation in both registries

`CursorRegistry::open`/`by_session` (~lines 304-305) and
`TxRegistry::open`/`by_session` (`tx_registry.rs`, ~lines 184-186) all use
the DEFAULT `DashMap<K, V>` (implicit `std::collections::hash_map::RandomState`,
SipHash), while `CursorRegistry::reaped_tombstones` in the SAME struct
(~line 306) already correctly uses `DashMap<u64, Instant, THasher>`. This
violates `CLAUDE.md`'s pillar 4 ("Fx hash everywhere; we accept no
untrusted hash inputs" — session ids and cursor/tx handles are
server-generated, not attacker-chosen, so there's no DoS-hardening reason
to keep `RandomState` here). Both structs currently derive `#[derive(Default)]`,
which — because the field types don't name an explicit hasher — silently
constructs each `DashMap` via `Default`, not the banned `DashMap::new()`
call syntax, so `clippy.toml`'s `disallowed-methods` lint (which bans
`dashmap::DashMap::new` by call-site pattern) does NOT catch this today —
confirm this yourself by noting the lint doesn't currently fire on either
file.

### Fix

Add the explicit `THasher` generic to all four fields:

- `cursor_registry.rs`: `open: DashMap<u64, Arc<Cursor>, THasher>`,
  `by_session: DashMap<[u8; 32], Arc<AtomicUsize>, THasher>` (already
  `use shamir_collections::THasher;` is imported, ~line 43 — reuse it).
- `tx_registry.rs`: `open: DashMap<u64, Arc<InteractiveTx>, THasher>`,
  `by_session: DashMap<[u8; 32], u64, THasher>` (add the `THasher` import
  if not already present in this file).

`DashMap<K, V, S>` implements `Default` when `S: Default` (verify this
against the `dashmap` version in use — `THasher` is
`BuildHasherDefault<FxHasher>`, which implements `Default`), so
`#[derive(Default)]` on both structs should continue to work unchanged
with no other code changes needed — this is a type-only change. If for
some reason it does NOT compile as a pure type change (e.g. a
`DashMap::with_capacity` call elsewhere that would need updating too),
grep both files for every `DashMap::` construction site and update each
to pass `THasher::default()` explicitly.

Public API (every method signature) must stay identical — this is an
internal representation change only.

## Tests (TDD — write failing tests first)

In whatever test module already covers `CursorRegistry`
(`crates/shamir-server/src/tests/cursor_registry_tests.rs` — check the
existing file) and, if `TxRegistry` has an equivalent test module, mirror
there too for the hasher change (no behavioral test needed for the hasher
swap itself beyond "existing tests still pass" — it's behavior-neutral):

- **`by_session` does not retain zero-count entries.** After an
  open→close (register→remove) cycle for a session, and separately after
  an open→idle-reap (register→`remove_for_idle_reap`) cycle, assert the
  `by_session` map has no entry for that session id. This needs a
  test-visible probe: add a `#[cfg(test)]` (or just `pub(crate)`, matching
  this file's existing visibility conventions — check what
  `open_count_for_session` already uses) accessor, e.g.
  `by_session_len(&self) -> usize` or `has_session_entry(&self, sid: &[u8;
  32]) -> bool` — pick whichever is more useful for the assertion, and
  note in a doc comment that this is a test/diagnostic-only probe, not a
  hot-path method (the O(1)-cardinality rule this codebase cares about is
  about NOT calling `.len()` on a hot path in PRODUCTION code — a test
  probe calling `DashMap::len()` is fine; `scc`'s `len()` ban from
  `clippy.toml` does not apply to `dashmap` at all, confirm this yourself
  by re-reading `clippy.toml`'s `disallowed-methods` list before writing
  the probe).
- **Concurrency stress test proving the per-session cap is never
  violated**: spawn many concurrent register/remove cycles against the
  SAME session id (interleaved with occasional zero-count-triggering
  removes) and assert `open_count_for_session` never reports a value
  above `max_per_session`, and that the FINAL state (after all tasks
  join) has a `by_session` entry count consistent with 0 (fully drained)
  — this is the test that would catch the TOCTOU bug described above if
  your `remove_if`-based fix (or the reaper-sweep fallback) has a flaw.
- **Regression**: all existing `CursorRegistry`/`TxRegistry` tests stay
  green (hasher swap must be fully behavior-neutral).

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`cursor_registry.rs`, `tx_registry.rs`, their tests). Do NOT touch
`db_handler/cursor_handlers.rs` or any of the Wave A/B cursor-pagination
logic — this task is purely internal registry-representation hygiene, no
wire-visible behavior changes.
