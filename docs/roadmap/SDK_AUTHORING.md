בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# SDK authoring — typed function kinds, builder-in-guest, discoverability

**Status:** design / proposed (revision 2026-06-05).

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

## Stage B — Query builder + macros inside the guest
Let a `#[procedure]` build complex queries type-safely (same language as
the host), instead of the low-level `ctx.db().query(Option<Value>)`.

1. **Slim the guest-facing query crate first** (prerequisite — `query-types`
   today pulls `shamir-types` + `hmac`/`sha2`/`zeroize`):
   - Split or feature-gate `query-types`: a `guest`-feature core (filter/
     read/write/call DTOs on the lean value type) **without** the auth/crypto
     ops (`hmac`/`sha2`/`zeroize` behind a `server`/`auth` feature).
   - Ensure the guest core stands on the lean value type (see
     `WASM_SLIMMING.md` P4) or on a serde-only mirror — measured for weight.
2. **Re-export the builder in `shamir-sdk` behind a `query-builder`
   feature**: `Query`, `Batch`, `q!`, `filter!`, `doc!` — gated so a
   scalar author pays nothing.
3. **Host shim `ctx.db().execute(BatchRequest)`**: add a guest host-import
   that takes a serialized `BatchRequest` (built by the guest builder),
   runs it on the host (engine), returns the result. The guest **describes**
   the query (DTO) and the host **executes** — engine never enters the
   guest.
4. Author UX:
   `let rows = ctx.db().execute(Batch::new().query("q",
   Query::from("t").where_(filter!(age > 18))).build()).await?;`
5. Tests: a procedure that builds + runs a builder query via the shim,
   end-to-end through `BatchOp::Call`.
6. Per-feature wasm size measured (scalar-only vs `query-builder`-on) — see
   `WASM_SLIMMING.md` metrics.
- Commits: `feat(query-types): guest feature core (no auth/crypto)`;
  `feat(sdk): query-builder in guest behind feature + ctx.db().execute shim`.

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
- **B:** builder-in-guest scope — full `query-builder` re-export, or just
  the DTOs (`query-types` core) + manual construction? (Recommend: DTOs
  core first, then the `q!`/`filter!` sugar.)
- **B-weight:** how `query-types` guest-core relates to the value type —
  ties to `WASM_SLIMMING.md` P4/P6 (lean value).
- **D:** packs — only on real need (libraries of procedures).

## Recommended order
```
A (typed kinds: scalar/procedure first)  →  C (discoverability)  →  B (builder-in-guest, after query-types slimming)  →  D (packs, on demand)
```
Stage A is cheap and additive (new macros over the existing ABI) and gives
the "every kind has a fixed signature → guaranteed-compilable" win the user
asked for. C makes them discoverable. B is the bigger one (needs query-types
slimming first). D only on demand.

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
