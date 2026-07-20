# F6/F9/F10/F11 — misc low-risk perf tail (last item of Этап 8)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Sixth and LAST item of "Этап 8 — Performance" (post-blocker, не гейт
релиза; `docs/dev-artifacts/research/2026-07-17-release-audit/
00-WORK-PLAN.md`), sourced from report 07
(`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), findings **F6** (Medium), **F9**
(Low-Med), **F10** (Low), **F11** (Low) — the work-plan's own item 8.5
groups these as "мелочь ... по мере касания соответствующего кода"
(touch-as-you-go tail). **This is deliberately the lowest-priority task in
the whole batch** — fine to do only some of these, or none, if the
remaining ones prove riskier than expected. Tasks 8a-8e already landed
(shamir-engine's filter/read/batch modules, shamir-index's vector code) —
read their commits if useful context, but this task's sites are
independent of all of them.

**Verify every code citation below against the ACTUAL current file** before
writing anything — several earlier tasks in this campaign found report 07's
line numbers had drifted after prior fixes landed; the sites described here
were re-read fresh for this brief, but re-confirm before editing.

## Priority order (do them in this order; stop whenever remaining capacity/confidence runs low — this is fully acceptable)

### 1. F10 — MANDATORY (most mechanical, safest, do this one regardless)

`crates/shamir-engine/src/query/filter/filter_node.rs` stores `field_path:
SmallVec<[u64; 4]>` (aliased as `CompactPath` — but check
`intern_field_path_compact` in `resolve.rs:62-72`, the type alias
`pub(super) type CompactPath = SmallVec<[u64; 4]>` is defined at
`filter_node.rs:155`) on EVERY compiled node variant that has a field path
(`Compare`, `InSet`, `In`, and ~13 more arms per report 07 — grep
`field_path.iter().map(|&id| InternerKey::new(id)).collect()` in this file
to find every occurrence, do not trust the report's line list, it predates
this session's other Этап 8 edits). Each `matches()` call rebuilds a
`SmallVec<[InternerKey; 4]>` from the stored `u64`s — stack-only (no heap
for ≤4 segments) so cheap, but pure repeated work at the highest-frequency
call site in the engine.

**Fix**: change the stored type from `SmallVec<[u64; 4]>` to
`SmallVec<[InternerKey; 4]>` at COMPILE time — a single-site type change in
`intern_field_path_compact` (`resolve.rs:62-72`, currently returns
`Option<CompactPath>` where `CompactPath = SmallVec<[u64;4]>`; change it to
build `SmallVec<[InternerKey;4]>` directly, e.g.
`keys.push(InternerKey::new(interned.id()))` instead of pushing the raw
`u64`). Then update `CompactPath`'s definition
(`filter_node.rs:155`) to `SmallVec<[InternerKey; 4]>`, and every
`matches()` arm that currently does `field_path.iter().map(|&id|
InternerKey::new(id)).collect()` to just use `field_path` directly (it's
already the right type — pass `field_path` where `&ipath` was used, or
rename the local binding, whichever keeps the diff smallest per arm).
Check `select_projection.rs:113-115` too (same rebuild pattern per the
report — verify against current line numbers) and its own field-path
storage.

**Risk**: `InternerKey` is confirmed a `u64` newtype (check
`shamir_types::core::interner::InternerKey`'s definition to confirm this
before assuming layout compatibility) — the type change should be
essentially free (`SmallVec<[u64;4]>` and `SmallVec<[InternerKey;4]>` have
identical size/layout if `InternerKey` is `#[repr(transparent)]` or a plain
tuple-struct newtype around `u64`; confirm this, don't assume). Every
non-filter-eval caller of `intern_field_path_compact`/`CompactPath` (grep
for both names workspace-wide) must still compile — fix any call site that
assumed the old raw-`u64` element type.

### 2. F6 — MANDATORY (the InSet-vs-In inconsistency; OPTIONAL for the deeper allocation-free probe)

**Mandatory half**: `FilterNode::InSet::matches` (grep `FilterNode::InSet`
in `filter_node.rs`, currently ~line 435-458) uses `record.materialize_at(&ipath)`
(an OWNED `InnerValue` clone of the field — expensive for container fields,
e.g. clones a whole nested Map/List just to probe one leaf) +
`inner_value_to_query_value` (a second conversion pass), where the sibling
`FilterNode::In`'s dynamic branch (`record.scalar_at(&ipath)`, same file,
just below `InSet`) uses the BORROW-based `scalar_at` → `ScalarRef` (zero
clone for scalar fields). **Fix**: switch `InSet::matches` to
`record.scalar_at(&ipath)` + `set_contains_coercing` (the `ScalarRef`-based
coercing probe already used by `In`'s dynamic path, defined just above
`InSet` in this file — read its full signature and coercion rules first)
instead of `materialize_at` + `inner_value_to_query_value` +
`set_contains_coercing_qv`. This removes the InnerValue clone AND one
conversion pass for every row where the field is a plain scalar (the common
case) — verify existing `InSet`/`$in` tests (grep for them) exercise a
field that IS a scalar (not e.g. a Map) so this switch doesn't silently
change behavior for a container-valued field (`scalar_at` returning `None`
for a non-scalar field is DIFFERENT from `materialize_at` walking into a
container — confirm existing behavior for a filter target that happens to
be a Map/List field, add a regression test for that edge case if none
exists, matching whatever the CORRECT documented semantics is — read the
`InSet`/`In` doc comments for what "field is not a scalar" should do).

**Optional half (only if genuinely low-risk)**: `set_contains_coercing`
itself (read it in full, just above `InSet` in this file) allocates a
`String`/`Vec<u8>` PER ROW for the `Str`/`Bin` arms
(`ScalarRef::Str(s) => set.contains(&QueryValue::Str(s.to_string()))` /
`ScalarRef::Bin(b) => set.contains(&QueryValue::Bin(b.to_vec()))`) just to
probe a hash set — the O(1) set lookup was the whole point, the per-probe
allocation gives some of it back. The report's suggested fix (a
`hashbrown`-style raw-entry / `Equivalent`-based lookup so a `&str`/`&[u8]`
can probe a `TSet<QueryValue>` without an owned key) requires a custom
wrapper type whose `Hash` impl matches `QueryValue::Str`/`Bin`'s DERIVED
`Hash` exactly (including whatever discriminant/tag the enum's derive
emits) — this is real, somewhat fragile coupling to `QueryValue`'s
internals. **Attempt this ONLY if you can verify byte-for-byte that your
wrapper's `Hash` output matches `QueryValue::Str(s)`'s for the same `s`
(write a test proving `hash(MyWrapper(s)) == hash(QueryValue::Str(s.into()))`
for several inputs before relying on it for a real lookup)** — if you
can't get full confidence, skip this half and say so; the mandatory half
above is already a genuine improvement on its own.

### 3. F9 — OPTIONAL (only if F10 + F6-mandatory are done and verified first)

`crates/shamir-engine/src/query/read/order.rs`,
`QvSortKey::from_query_value` (currently ~line 179-192): `QueryValue::Str(s)
=> QvSortKey::Str(s.clone())` — one `String` alloc per row per string
ORDER BY column, in both call shapes (`apply_order_by_qv` phase-1 sort and
the top-K heap — grep for both call sites). Fix direction: `QvSortKey`
becomes lifetime-generic, holding `Cow<'a, str>` (borrowed for the `Str`
case, owned for the `Dec`/`Big`-derived string forms which don't have a
source `&str` to borrow from) instead of an owned `String`. This requires:
verifying the report's claimed invariant ("the records outlive the key
vector in both call shapes") is actually true at BOTH call sites before
introducing the lifetime — if it's wrong, the borrow checker will simply
refuse to compile (not a silent correctness bug), so this is a real-but-
bounded implementation-effort risk, not a hidden-runtime-risk one. If the
lifetime plumbing turns out to ripple further than the two cited call
sites (e.g. `QvSortKey` is stored somewhere with a shorter effective
lifetime than the records), STOP and revert this item rather than forcing
an `unsafe`/`Rc`/clone-anyway workaround — report what you found.

### 4. F11 — OPTIONAL, lowest priority (only if everything above is done and verified, and you have clear remaining capacity)

`crates/shamir-engine/src/query/batch/param_subst.rs` (grep for the exact
current lines — report cited ~199-201/224-226, may have shifted):
- **Fast-path deep clone + deep-eq**: `resolve_write_value`'s fast path
  returns `value.clone()` (deep-clones the whole write document even when
  nothing needs substitution); the Update/Set callers then run a deep
  equality compare (`subst_set == op.set`) to detect the no-op. Fix
  direction: a `Cow<'_, QueryValue>` return (Borrowed on the fast path)
  removes both the clone and the compare — check every caller of
  `resolve_write_value` compiles against a `Cow` return before committing
  to this.
- **Per-marker serde round-trip**: each `$query`/`$fn`/`$cond`/`$expr`
  marker is converted `QueryValue → msgpack bytes → FilterValue` per
  occurrence, per op, per ForEach iteration. A pointer-keyed decoded-marker
  cache (same `CondCache`/`FieldPathCache`/`QueryRefCache` precedent from
  tasks 8a/8b of this batch — read those files for the pattern) or a direct
  `QueryValue → FilterValue` structural conversion (the SAME kind of
  "write a `Serializer`/converter instead of round-tripping bytes" idea
  task 8e (F5, commit `e4305c33`) just used for the analogous
  `QueryResult → QueryValue` conversion — read
  `crates/shamir-engine/src/query/batch/query_value_serializer.rs` for the
  template if you go this route) would remove the serde round-trip.
  **This is the largest, most novel piece of remaining work in this
  batch** — only attempt if you have real confidence and capacity; skipping
  it with a one-line note is a fully acceptable outcome, matching the
  report's own "ranked low" severity for this item and its explicit note
  that it "multiplies under F5's iteration loop" (i.e., it compounds an
  ALREADY-fixed hot loop, not an unfixed one — the marginal urgency is
  lower now that F5 landed).

## Verification (MANDATORY before you report done, scaled to whichever items you actually did)

- For EACH item you implement: `./scripts/test.sh -p shamir-engine -p
  shamir-query-types --full` green, run TWICE (this session's flake-triage
  discipline). Any existing test touching `$in`/`InSet`/ORDER BY/write-value
  markers must pass UNCHANGED — these are pure perf fixes, zero intended
  behavior change (except the F6 container-field edge case explicitly
  called out above, which needs its OWN new/updated test if the semantics
  genuinely change).
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- For each of F6/F9/F10/F11: state explicitly whether you implemented it,
  partially implemented it (e.g. F6's mandatory half only), or skipped it,
  and why — this task's whole framing is "do what's safely achievable,
  stop when risk/effort no longer justifies it for a post-blocker perf
  tail", so an honest partial completion is the expected, correct outcome,
  not a shortfall.

## Out of scope

- Do NOT touch tasks 8a-8e's artifacts (already landed, different files) or
  F1-F5/F7/F8 (F8 — fjall double point-lookup — was explicitly excluded
  from this entire Этап 8 batch back in the very first task's brief: it's
  a trait-level `Store` API change tracked as its own separate approved
  follow-up, never in scope here).
- Do NOT force any of F6's optional half / F9 / F11 if the risk assessment
  in each section above says to stop — this is the deliberate design of
  this brief, not a fallback to apologize for.
