בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Allocation & hasher policy — airtight enforcement (plan, NOT yet active)

**Status:** design doc to execute LATER. Nothing here is wired into the live
gate yet. Goal: make two rules **airtight** (block every workaround), then roll
out behind a measured codebase sweep.

**Two hard rules the user wants:**
1. **No standard-hasher (SipHash/`RandomState`) collections** — and block every
   bypass (`::new`, `::default`, `Default::default()`, `.collect()`, aliases).
2. **No zero-capacity construction** of allocating collections — only
   `with_capacity*` is allowed (block `::new`, `::default`, `vec![]`-empty,
   `Default::default()`, `.collect()` into an unsized allocation).

> ⚠️ **Do NOT activate while the RecordView migration is in flight.** The repo
> gate is `cargo clippy --workspace --all-targets -- -D warnings`, so the moment
> any new `disallowed_*` rule lands it becomes a hard error on every existing
> violation — that breaks every in-flight migration agent's gate. Activate only
> after the migration lands AND the sweep (Step C) is done.

---

## 1. What already exists (do not re-invent)

- **`clippy.toml`** (root) already bans the *named* default-hasher constructors:
  `std::collections::HashMap::{new,with_capacity}`, `HashSet::{new,with_capacity}`,
  `dashmap::DashMap::new`, `scc::HashMap::new`. `with_hasher` /
  `with_capacity_and_hasher` stay allowed.
- **`crates/shamir-collections/src/lib.rs`** already provides the alias model:
  - `THasher = BuildHasherDefault<FxHasher>`
  - `TMap`/`TSet` = `IndexMap`/`IndexSet` + THasher (ordered) + `*_with_capacity` ctors
  - `TFxMap`/`TFxSet` = `std::HashMap`/`HashSet` + THasher (unordered) + `*_with_capacity` ctors
- **`Cargo.toml`** has `[workspace.lints.clippy]` with `disallowed_methods = "warn"`
  (→ effectively `deny` under the `-D warnings` gate).

So the **alias destinations exist** and the *named* hasher constructors are
already blocked. The gaps below are the **bypasses** rule 1 wants closed, and
**all of rule 2**.

## 2. The open holes (why current config is NOT airtight)

**Hasher (rule 1):**
- `HashMap::<K,V>::default()` → `HashMap<_,_,RandomState>` — NOT banned (we only
  ban `::new`/`::with_capacity`; `::default` is allowed because the sanctioned
  `HashMap::<_,_,THasher>::default()` uses it too — indistinguishable by method path).
- `Default::default()` with inferred `HashMap` type — invisible to `disallowed_methods`.
- `iter.collect::<HashMap<_,_>>()` / `collect()` into an inferred SipHash map.
- Direct `std::collections::HashMap<K,V,THasher>` written inline instead of `TFxMap`
  (works, but bypasses the single-sanctioned-location discipline).

**Capacity (rule 2):** nothing is enforced at all today — `Vec::new`,
`String::new`, `VecDeque::new`, `HashMap::default`, `vec![]`, `collect()` all pass.

## 3. Airtight design

### 3a. Hasher → AIRTIGHT via `disallowed-types` (type ban) + single allow-site
Ban the **types**, not just the constructors. A banned type is unnameable, so
SipHash is unconstructable by ANY path (`new`/`default`/`collect`/inference all
produce a value whose type is the banned `HashMap<_,_,RandomState>`, and the type
is flagged wherever it is named — including turbofish `collect::<HashMap<_,_>>()`
and `let x: HashMap<_,_> = ...`).

```toml
disallowed-types = [
  { path = "std::collections::HashMap",
    reason = "use shamir_collections::TFxMap / TMap (Fx hasher); raw std HashMap only at the one #[allow] alias site" },
  { path = "std::collections::HashSet",
    reason = "use shamir_collections::TFxSet / TSet" },
  { path = "std::collections::hash_map::RandomState",
    reason = "SipHash builder — never construct it" },
]
```

**Single sanctioned location** = `shamir-collections/src/lib.rs`. The alias defs
and ctors there reference raw `HashMap`/`HashSet`, so that block (and ONLY that
block) gets `#[allow(clippy::disallowed_types)]`:
```rust
#[allow(clippy::disallowed_types)] // the ONE place raw std HashMap/HashSet may be named
mod hash_aliases {
    use super::THasher;
    use std::collections::{HashMap, HashSet};
    pub type TFxMap<K, V> = HashMap<K, V, THasher>;
    pub type TFxSet<T>    = HashSet<T, THasher>;
    // ... with_capacity ctors ...
}
pub use hash_aliases::*;
```

**Residual hole:** a `HashMap` value that is NEVER named anywhere (fully inferred
through a chain that also never names it) — vanishingly rare and usually still
names the type in some signature. Closed by the dylint lint (§3c) for 100%.

**Consequence for the codebase:** every existing `HashMap<K,V,THasher>` /
`HashSet<…,THasher>` written inline (i.e. the TFxMap pattern not via the alias)
will now be flagged → must migrate to `TFxMap`/`TFxSet`. Audit count in Step A.

### 3b. Capacity → clippy `disallowed-methods` (named ctors) — PARTIAL
Add the zero-capacity named constructors. `warn` level (→ deny under gate).
```toml
disallowed-methods = [
  # ... keep existing hasher entries ...
  # linear / string
  { path = "std::vec::Vec::new",              reason = "Vec::with_capacity" },
  { path = "std::vec::Vec::default",          reason = "Vec::with_capacity" },
  { path = "std::string::String::new",        reason = "String::with_capacity" },
  { path = "std::string::String::default",    reason = "String::with_capacity" },
  { path = "std::collections::VecDeque::new",     reason = "VecDeque::with_capacity" },
  { path = "std::collections::VecDeque::default", reason = "VecDeque::with_capacity" },
  # std HashMap/HashSet ::new/::with_capacity already banned; once the TYPE is
  # banned (3a) these become redundant but keep for a clearer message.
  { path = "std::collections::HashMap::default", reason = "TFxMap::with_capacity" }, # now safe to ban: alias site is #[allow]'d
  { path = "std::collections::HashSet::default", reason = "TFxSet::with_capacity" },
  # ecosystem (only where with_capacity exists & is meaningful)
  { path = "indexmap::IndexMap::new",     reason = "TMap::with_capacity" },
  { path = "indexmap::IndexMap::default", reason = "TMap::with_capacity" },
  { path = "indexmap::IndexSet::new",     reason = "TSet::with_capacity" },
  { path = "indexmap::IndexSet::default", reason = "TSet::with_capacity" },
  { path = "dashmap::DashMap::default",   reason = "DashMap::with_capacity_and_hasher" },
  { path = "dashmap::DashSet::new",       reason = "DashSet::with_capacity_and_hasher" },
  { path = "dashmap::DashSet::default",   reason = "DashSet::with_capacity_and_hasher" },
  { path = "scc::HashMap::default",       reason = "scc::HashMap::with_capacity" },
  { path = "scc::HashSet::new",           reason = "scc::HashSet::with_capacity" },
  { path = "scc::HashSet::default",       reason = "scc::HashSet::with_capacity" },
  { path = "scc::HashIndex::new",         reason = "scc::HashIndex::with_capacity" },
  { path = "scc::HashIndex::default",     reason = "scc::HashIndex::with_capacity" },
  { path = "bytes::BytesMut::new",        reason = "BytesMut::with_capacity" },
  { path = "slab::Slab::new",             reason = "Slab::with_capacity" },
  { path = "slab::Slab::default",         reason = "Slab::with_capacity" },
]
```
**NOT banned (intentional):** `smallvec::SmallVec::new` / `tinyvec` (inline, no
heap — banning forces a wasteful heap spill); `arrayvec`/`heapless` (capacity is a
const-generic, no runtime `with_capacity`); `BTreeMap`/`BTreeSet`,
`scc::TreeIndex`, `Bag`/`Queue`/`Stack`/`LinkedList` (trees/lock-free lists have no
`with_capacity`); `lru::LruCache::new` (capacity already a required arg).

**Holes clippy CANNOT close (→ dylint, §3c):** `Default::default()` with inferred
type, `.collect()` into an unsized allocation, empty `vec![]`, alias-typed ctors.

### 3c. Capacity airtight → dylint custom lint `no_unsized_alloc`
clippy.toml is path-based; it cannot see inference/`collect`. A
[dylint](https://github.com/trailofbits/dylint) lint matches by the **result type**
of an expression and catches the bypasses. Sketch:
- For every expression `e` whose resolved type is an allocating collection
  (`Vec`, `String`, `VecDeque`, `HashMap`/`HashSet` family, `IndexMap`/`IndexSet`,
  `DashMap`, `BytesMut`, …), check how it was produced:
  - `::with_capacity*` → OK.
  - `::new` / `::default` / `Default::default()` / `<_>::collect()` / empty `vec![]`
    → LINT.
  - value flowing from a fn return / field / pattern → OK (not a construction site).
- Implement as a `LateLintPass`: on `ExprKind::Call`/`MethodCall`/`Path` whose
  type is in the allocating set and whose callee is a zero-capacity constructor,
  emit. Use `cx.typeck_results().expr_ty(expr)` for the result type; match the
  constructor `DefId` against a built table (mirror the clippy list + the
  `Default::default`/`FromIterator::collect` DefIds clippy can't express).
- Ship as `lints/no_unsized_alloc/` (a dylint driver crate); run in CI via
  `cargo dylint --all` as a SEPARATE step from `cargo clippy` (dylint needs the
  nightly toolchain pinned in `rust-toolchain` for the driver).

This is the only way to reach 100% on capacity. The hasher half is already 100%
via §3a's type ban (no dylint needed for hasher except the inference edge).

### 3d. Lint levels (`Cargo.toml`)
```toml
[workspace.lints.clippy]
absolute_paths    = "warn"
disallowed_methods = "deny"   # was warn — promote once the sweep is clean
disallowed_types   = "deny"   # new — hasher type ban
```
Under the existing `-- -D warnings` gate, `warn` already behaves as deny; setting
`deny` explicitly makes intent clear and survives a future gate that drops `-D warnings`.

## 4. Rollout (measure-first — MANDATORY)

The gate turns every new rule into an instant hard error. So:

- **Step A — Audit (read-only).** Count violations per proposed rule across
  `crates/` (e.g. `Vec::new`, `String::new`, inline `HashMap<_,_,THasher>`,
  `HashMap::default`, `.collect()` sites). Decide scope/effort from real numbers.
  Likely large for `Vec::new` (mostly tests) → consider `allow-*-in-tests` or
  excluding `#[cfg(test)]` via a dylint allowance.
- **Step B — shamir-collections.** Wrap the alias/ctor block in
  `#[allow(clippy::disallowed_types)]`; add any missing capacity-required ctors
  (`TFxMap::with_capacity` etc. already exist — verify).
- **Step C — Sweep (the bulk).** Per crate, gated: replace every flagged site —
  `Vec::new`→`Vec::with_capacity(n)`, raw `HashMap<_,_,THasher>`→`TFxMap`,
  `HashMap::default`→`TFxMap::with_capacity`, empty `vec![]`/`collect` per the
  dylint. Commit per crate (`chore(<crate>): preallocate + Fx-only`). This is the
  big, mechanical, parallelisable wave. MUST run AFTER the RecordView migration
  (else the two collide on the same files + gate).
- **Step D — Activate clippy.toml additions** (§3a, §3b) + bump levels (§3d).
  Single `chore(lint): airtight alloc+hasher policy` commit; add SHA to
  `.git-blame-ignore-revs` if it touches many lines.
- **Step E — dylint.** Add `lints/no_unsized_alloc`, pin `rust-toolchain` for the
  driver, add `cargo dylint --all` to the pre-push hook + CI as a separate step.

## 5. Honest limits
- **Hasher: airtight** via the type ban (§3a) — SipHash is unconstructable; the
  only residual (a never-named inferred HashMap) is closed by dylint if desired.
- **Capacity: airtight only WITH dylint** (§3c). clippy.toml alone (§3b) catches
  named `::new`/`::default` but not `Default::default()`/`collect()`/`vec![]`.
  If dylint is too heavy, accept clippy-partial + a CONTRIBUTING note and treat
  the residual as cultural, not enforced.
- **Path fragility:** `disallowed-*` paths resolve by `DefId`; a dep moving a type
  between modules across a major bump silently stops matching. Re-verify the
  `indexmap`/`dashmap`/`scc` paths against the pinned versions on each major bump.
- **msrv:** set `msrv` in `clippy.toml` (or rely on `package.rust-version`) to the
  real workspace minimum so Clippy doesn't suggest too-new features.

## 6. Open decisions (resolve at execution time)
1. dylint in CI now (true airtight capacity) or clippy-partial + culture first?
2. Tests: exempt `#[cfg(test)]` from the capacity rule (lots of throwaway
   `Vec::new`) or hold tests to it too?
3. Sweep granularity: one big commit vs per-crate (recommend per-crate +
   blame-ignore for any pure-mechanical bulk).
