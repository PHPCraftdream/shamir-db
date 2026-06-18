בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# SDK authoring — typed function kinds, builder-in-guest, discoverability

**Status:** Stages **A, C, B — DONE** (revision 2026-06-05); D optional.

How authors write ShamirDB WASM functions with **typed signatures per
kind**, **the query builder/macros inside the guest**, and full IDE
**discoverability** — so the code they ship is guaranteed-compilable and
self-explaining. Companion: [`FUNCTIONS.md`](./FUNCTIONS.md) (engine "M"),
[`WASM_SLIMMING.md`](./WASM_SLIMMING.md) (weight),
[`STORED_PROCEDURES.md`](./STORED_PROCEDURES.md) (the `call` surface).

---

## What exists today
- `shamir-sdk` (guest): `Ctx`, `Batch`, `Db`/`Table`, `Params`, `Value`,
  `Validation`, `Error`/`Result`, `Http*`, `prelude`.
- Two kinds with **compile-time signature checks**: `#[shamir::function]`
  (`ctx, batch, params → Result<Value>`) and `#[shamir::validator]`
  (`record, old_record, ctx → Validation`) — the macro panics at compile
  time on a wrong signature.
- `ctx.db()` is **low-level**: `table(name).get/insert/query(filter:
  Option<Value>)` — key-convention filter, not the full builder.
- Deps are light (`serde` + `rmp-serde` + macros); host crates not pulled.

Gaps: only one general kind (`function`) — scalar vs procedure undistin-
guished; no builder/macros in the guest; `ctx.db` can't express complex
WHERE/SELECT; one entrypoint per crate.

---

## Stage A — Typed function kinds (specialized macros)
Give each kind its own macro → fixed signature, guarantees, and prelude
(so the author sees exactly what's available for that kind).

1. `#[shamir::scalar]` — pure value function (for select/filter/computed):
   signature `(args) -> Value`, **no `ctx.db`** (purity guarantee → safe in
   filters/indexes). Macro rejects any `ctx` arg.
2. `#[shamir::aggregate]` — `init` / `accumulate(acc, row)` / `merge` /
   `finalize` shape (the aggregate contract); macro enforces the four
   pieces (or a struct impl).
3. `#[shamir::procedure]` — getter-procedure (the `BatchOp::Call` target):
   `(ctx, params) -> Result<Value>` **with** `ctx.db` (explicitly "may
   read/write"); macro wires the same ABI as today's `function`.
4. `#[shamir::validator]` — already exists; keep.
5. For each macro: a per-kind `prelude` re-export bringing only the types
   that kind may use (scalar sees `Value`/args; procedure sees `Ctx`/`Db`).
6. Tests: a compile-pass example per kind + `trybuild`-style compile-fail
   for wrong signatures (scalar touching `ctx.db` → error).
7. Keep `#[shamir::function]` as the general escape hatch.
- Commit per kind: `feat(sdk-macros): #[scalar] / #[aggregate] /
  #[procedure] typed function kinds`.

## Stage B — Query builder + macros inside the guest ✅ DONE
A `#[procedure]` builds complex queries type-safely (the same language as
the host) and runs them via `ctx.db().execute(&batch)`, instead of the
low-level `ctx.db().query(Option<Value>)`.

**What made it cheap — the thin-waist (not the deferred P4).** Stage B was
parked behind "slim the guest query crate first," which the docs framed as
the wide `shamir-value` refactor (WASM_SLIMMING P4). Investigation showed
that premise was **false**: `query-types` has no `QueryValue` — its DTOs use
`QueryValue` + a self-contained `FilterValue`, and its only ties to
the heavy `shamir-types` were the `TMap`/`TSet` aliases and a host-only
`ResourcePath` adapter. Three surgical cuts replaced P4:

1. **`shamir-collections`** leaf crate owns `TMap`/`TSet` (re-exported by
   `shamir-types`) — `52be3b3`.
2. **`server` feature** on `query-types` gates the host-only `to_path`
   adapter; `shamir-types` is now an optional dep (enabled by `server`,
   default-on). `query-builder` depends on `query-types` with
   `default-features = false` → lean by construction; host builds still get
   `crypto`+`server` via feature unification — `e934a2f`. (`crypto` gate was
   B1, `c0c27fe`.)
3. Net: `cargo tree -p shamir-query-types --no-default-features` pulls no
   `shamir-types`; the builder compiles to `wasm32` (`indexmap` +
   `shamir-collections` + DTOs only).

**Then B2 itself, small:**
4. **Host shim `db_execute`** — a guest host-import that takes a msgpack
   `BatchRequest`, runs it through the same executor a wire client uses (as
   the function's effective actor), returns the `BatchResponse`. Generalises
   the existing `db_get`/`db_insert`/`db_query`. `DbGateway::execute` +
   `FacadeDbGateway::execute` + `host_db_execute` — `d71f9a0`.
5. **`shamir-sdk` `query-builder` feature** re-exports the builder as
   `shamir_sdk::builder` and adds `Db::execute(&Batch) -> Result<
   BatchResponse>`; off by default so a scalar author pays nothing —
   `ca79b1f`.
6. Author UX (worked example `examples/fn-procedure-builder`):
   ```rust
   let mut b = Batch::new();
   b.query("rows", Query::from("items").where_gte("n", 2_i64));
   let resp = ctx.db().execute(&b)?;
   ```
7. Tests: native-fn e2e (`builder_execute_e2e.rs`) drives
   `ctx.db_gateway().execute()` → real engine under a real actor;
   guest example compiles to `wasm32` at **337 KB** vs the 71 KB no-builder
   baseline (builder + DTOs + rmp_serde + indexmap, no heavy graph).
> The engine never enters the guest: the guest **describes** the batch
> (DTO), the host **executes**. One query language, one builder, one
> executor, three callers — network client, in-process client, guest
> procedure.

> **Remaining nicety (optional):** the `q!`/`filter!` proc-macros are
> reachable via `shamir_sdk::builder::{q, filter}` but a first-class
> per-kind prelude re-export is not yet wired — pull only on real need.

## Stage C — Discoverability (prelude + docs + examples)
1. Rich per-kind `prelude` (Stage A) so `use shamir_sdk::prelude::*;` brings
   exactly the right surface for IDE autocomplete.
2. Doc-comments on `Ctx`/`Db`/`Params`/`Value` listing what's reachable
   (`ctx.db()`, `ctx.call()`, `ctx.http_fetch()`, secret grants).
3. One worked example per kind under `examples/` (scalar / aggregate /
   procedure / validator) — compilable, doubles as a smoke test.
4. (If feasible) a `cargo doc` page or `FUNCTIONS.md` section enumerating
   built-in scalars/aggregates the author can call via `ctx.call` / `func`.
- Commit `docs(sdk): per-kind prelude + worked examples + reachable-API docs`.

## Stage D — (optional) Function packs (multiple fns in one wasm)
> Default stays **one function per wasm** (simple isolation / versioning /
> per-fn setuid / `replace`). A pack trades that for one compile/cache and
> shared code across related procedures.
1. `#[shamir::export]` to mark multiple functions in one crate.
2. Macro emits **named exports** (`shamir_call_<name>`); the ABI gains a
   name dispatch (single `shamir_call` → named entries or a dispatcher).
3. Host: a pack is registered as a set; invocation selects by name.
4. Decide versioning/isolation semantics for a pack (atomic deploy of the
   set; setuid/owner per-pack vs per-fn).
5. Tests: a 2-function pack, both callable; isolation preserved per call.
- Commit `feat(sdk): function packs — multi-export wasm + host dispatch`.
> Only when libraries of related procedures appear; not the default.

---

## Forks to decide
- **A:** specialized macros now, or keep only `function`+`validator`?
  (Recommend: add `scalar`/`procedure` first — clearest value; `aggregate`
  when WASM aggregates are actually authored.)
- **B:** ✅ resolved — full `query-builder` re-export (`shamir_sdk::builder`)
  behind a default-off feature, plus `Db::execute(&Batch)`.
- **B-weight:** ✅ resolved — no lean-value crate needed; the thin-waist
  (`shamir-collections` + `server` feature) made `query-types`/`builder`
  guest-lean. WASM_SLIMMING P4 is **retired**.
- **D:** packs — only on real need (libraries of procedures).

## Recommended order
```
A (typed kinds) ✅  →  C (discoverability) ✅  →  B (builder-in-guest) ✅  →  D (packs, on demand)
```
A/C/B are **done**. Stage A (scalar/procedure macros) gave the
"fixed-signature → guaranteed-compilable" win; C made it discoverable; B
(builder-in-guest via `ctx.db().execute`) turned out cheap once the
thin-waist dissolved the supposed P4 prerequisite. **D (packs) only on real
need** — when libraries of related procedures actually appear.

## Files touched
- `crates/shamir-sdk-macros/src/lib.rs` — new attribute macros (A, D).
- `crates/shamir-sdk/src/prelude.rs` + per-kind preludes, docs (A, C).
- `crates/shamir-sdk/Cargo.toml` — `query-builder` feature (B).
- `crates/shamir-query-types/` — guest feature core, auth/crypto behind a
  feature (B).
- `crates/shamir-engine/src/function/` — `ctx.db().execute` host import (B);
  pack dispatch (D).
- `examples/` — per-kind worked examples (C).

## Guardrails
- Each kind's macro enforces its signature at compile time (the author's
  guarantee of compilable code).
- Builder-in-guest is feature-gated — a scalar pays nothing (weight).
- Guest describes queries (DTO); host executes — engine never in the guest.
- "Don't over-build": packs and aggregate-macro only when actually authored.
