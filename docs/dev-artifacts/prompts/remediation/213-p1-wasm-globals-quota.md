# Stage G5c-quota — RV-WASM-HOST-51B448DF: bound guest-retained globals (split from G5c)

Implement EXACTLY the reviewed design below. Card text: `### RV-WASM-HOST-51B448DF` in docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md. Only crate touched: shamir-wasm-host. Local gates at the end (all must exit 0): `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`, `cargo fmt -p shamir-wasm-host -- --check`, `python -I scripts/ci/check_layout.py`. Do not delete or reword any existing comment you are not required to change. Final message: files changed, and for each regression test the revert that makes it fail.

## Design for 51B448DF (from @oh review): bound the globals that guests keep

Base: branch `fix/p1-wasm-runtime-design`. Line references below are to that base.

### What is wrong today

- Guest writes have no limit. `runtime/context.rs:157-159` stores plain values, `set` (`:170-178`) can never fail, and `wasm/bindings/globals.rs:56` calls it directly.
- `len()` (`context.rs:205-209`) walks the whole scc map, which is O(N).
- `globals.rs:38-39` copies guest memory before checking its size.
- A bare `wasmtime::Error::msg` trap message gets lost. The quota trap must therefore wrap a `FunctionError::Compute`, following the precedent at `call.rs:209`. `map_wasm_error` (`wasm_function.rs:744-767`) gets it back by downcast.

### Data structures (all in `runtime/context.rs`; `runtime/` already has 7 entries, so add no new file there)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalsQuota { pub max_entries: usize, pub max_bytes: usize, pub max_value_bytes: usize }
impl Default for GlobalsQuota { /* 16_384 entries, 64 MiB, 1 MiB */ }

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GlobalsQuotaError {
    #[error("globals quota exceeded: value is {len} bytes, max {max}")] ValueTooLarge { len: usize, max: usize },
    #[error("globals quota exceeded: entry limit {max}")] Entries { max: usize },
    #[error("globals quota exceeded: byte limit {max}")] Bytes { max: usize },
}

struct Slot { value: QueryValue, cost: usize }
const SLOT_OVERHEAD: usize = std::mem::size_of::<Slot>() + std::mem::size_of::<String>();
#[derive(Clone, Copy, Default)] struct Charge { entries: usize, bytes: usize }
enum Admission { Capped, Uncapped }
#[cfg(test)] pub(crate) type AdmitProbe = Arc<dyn Fn(&str) + Send + Sync>;

pub struct GlobalVars {
    data: scc::HashMap<String, Slot, THasher>,
    entries: AtomicUsize,   // O(1) mirror: published entries plus in-flight reservations
    bytes: AtomicUsize,
    quota: GlobalsQuota,
    #[cfg(test)] admit_probe: Option<AdmitProbe>,   // set per instance, never a global
}
```

**Invariant.**
- Every counter increase happens before the state it pays for becomes visible.
- Every decrease happens after the old state is gone.
- So each counter is always at least the true total of what is stored. The saturating arithmetic is only a safety net.

### Algorithm

Free helpers:

```rust
fn slot_cost(key: &str, n: usize) -> usize { key.len().saturating_add(n).saturating_add(SLOT_OVERHEAD) }
fn encoded_len(v: &QueryValue) -> usize { v.to_bytes().map_or(0, |b| b.len()) }   // native path only

fn add_capped(c: &AtomicUsize, amount: usize, max: usize) -> bool {
    // A non-growing write is admitted even when a native `set` already pushed us over the cap.
    amount == 0 || c.fetch_update(AcqRel, Acquire, |cur| cur.checked_add(amount).filter(|n| *n <= max)).is_ok()
}
fn add_saturating(c, a) // fetch_update(|x| Some(x.saturating_add(a))), skipped when a == 0
fn sub_saturating(c, a) // fetch_update(|x| Some(x.saturating_sub(a))), skipped when a == 0

impl Charge {
    fn growth(old: Option<usize>, cost: usize) -> Self  // None → {1, cost}; Some(o) → {0, cost.saturating_sub(o)}
    fn covers(self, need: Self) -> bool                  // entries >= need.entries && bytes >= need.bytes
    fn surplus(self, need: Self) -> Self                 // field-wise saturating_sub
}
```

Methods on `GlobalVars`:

```rust
fn reserve(&self, c: Charge) -> Result<(), GlobalsQuotaError> {
    if !add_capped(&self.entries, c.entries, self.quota.max_entries) { return Err(Entries{..}) }
    if !add_capped(&self.bytes, c.bytes, self.quota.max_bytes) { sub_saturating(&self.entries, c.entries); return Err(Bytes{..}) }
    Ok(())
}
fn charge(&self, c: Charge)  { add_saturating both }
fn release(&self, c: Charge) { sub_saturating both }

fn admit(&self, key: String, mut slot: Slot, mode: Admission) -> Result<(), GlobalsQuotaError> {
    loop {
        let observed = self.data.read_sync(&key, |_, s| s.cost);   // short read, no guard kept
        let held = Charge::growth(observed, slot.cost);
        match mode { Capped => self.reserve(held)?, Uncapped => self.charge(held) }  // NO scc guard held here
        #[cfg(test)] if let Some(p) = &self.admit_probe { p(&key) }
        match self.publish(key.clone(), slot, held) {
            Ok(rel)  => { self.release(rel); return Ok(()) }   // guard already dropped
            Err(back) => { self.release(held); slot = back }    // key changed in between: retry
        }
    }
}

/// Under `entry_sync`: publish only if `held` covers the real growth; otherwise hand the slot back.
fn publish(&self, key: String, slot: Slot, held: Charge) -> Result<Charge, Slot> {
    match self.data.entry_sync(key) {
        Entry::Occupied(mut e) => {
            let old = e.get().cost;
            let need = Charge::growth(Some(old), slot.cost);
            if !held.covers(need) { return Err(slot) }
            let shrink = old.saturating_sub(slot.cost);
            *e.get_mut() = slot;
            let mut rel = held.surplus(need);
            rel.bytes = rel.bytes.saturating_add(shrink);
            Ok(rel)
        }
        Entry::Vacant(v) => {
            let need = Charge::growth(None, slot.cost);
            if !held.covers(need) { return Err(slot) }
            v.insert_entry(slot);
            Ok(held.surplus(need))
        }
    }
}
```

No counter changes while the scc guard is held. When the reservation does not match what is actually there, the cost is a retry, never a reservation taken under the guard.

### Public API

- `pub fn new()` becomes `Self::with_quota(GlobalsQuota::default())`.
- `pub fn with_quota(GlobalsQuota) -> Self` and `pub fn quota(&self) -> GlobalsQuota`.
- `pub fn try_set(&self, key: impl Into<String>, value: QueryValue, encoded_len: usize) -> Result<(), GlobalsQuotaError>`:
  - return `ValueTooLarge` when `encoded_len > max_value_bytes`;
  - otherwise call `admit(.., Capped)`.
- `set` stays infallible: `let _ = self.admit(key, Slot{.., cost: slot_cost(&key, encoded_len(&value))}, Uncapped);`. `seed_env` is unchanged.
- `update`: the closure runs under the guard.
  - Inside the guard, `self.charge(Charge::growth(old, cost))`. It is uncapped and cannot fail.
  - Write the slot.
  - After the guard drops, release the shrink `old - cost`.
  - Vacant case: charge `{1, cost}`, then insert. `incr` goes through `update`.
- `remove`: on `Some((_, s))`, `self.release(Charge{entries:1, bytes:s.cost})`.
- `get` clones `s.value`.
- `len()` becomes `self.entries.load(Acquire)`.
  - REMOVE the `#[allow(clippy::disallowed_methods)]`.
  - Doc: "O(1); includes in-flight guest admissions; exact when quiescent."
- Test-only:
  - `#[cfg(test)] pub(crate) fn with_admit_probe(mut self, p: AdmitProbe) -> Self`
  - `#[cfg(test)] pub(crate) fn bytes_used(&self) -> usize`
- KEEP every existing doc comment: the `GlobalVars` docs, the method docs and the whole `FnCtx` doc block. The failed round deleted them.

### Other files

- `runtime/mod.rs:8` and `lib.rs:40-43`: also re-export `GlobalsQuota` and `GlobalsQuotaError`.
- `wasm/bindings/globals.rs` `host_global_set` (`:26-58`):
  1. Before reading guest memory (`:33`):
     ```rust
     let max = caller.data().globals.quota().max_value_bytes;
     if let Ok(len) = usize::try_from(val_len) { if len > max { return Err(quota_trap(GlobalsQuotaError::ValueTooLarge{len, max})) } }
     ```
     A negative length keeps the existing `read_guest_mem` error.
  2. At `:56`: `caller.data().globals.try_set(key, value, val_bytes.len()).map_err(quota_trap)`.
  3. Add `fn quota_trap(e) -> wasmtime::Error { wasmtime::Error::new(FunctionError::Compute(format!("global_set: {e}"))) }`.
  4. Leave the comments at `:44-46` and `:62-64` untouched.

### Tests: no global hooks, no timing assertions

Native tests go in `src/tests/runtime/globals_quota_tests.rs`. Guest tests go in `src/tests/wasm/bindings/globals_quota_tests.rs`. Register both. Byte checks compare against a fresh reference instance that did one clean `try_set`, so `SLOT_OVERHEAD` is never exposed.

| `rv_wasm_host_51b448df_…` | What it does | Revert that makes it fail |
|---|---|---|
| `entry_cap_rejects_new_key_keeps_existing` | `max_entries=1`: `try_set a` returns Ok, `try_set b` returns `Entries`; `a` is intact, `b` is absent | `try_set` → plain set |
| `byte_cap_rejects_growth` | An overwrite that would exceed `max_bytes` returns `Bytes`; the old value stays | no bytes reservation |
| `non_growing_overwrite_admitted_over_cap` | Native `set` pushes the instance over its caps; `try_set` with the same or smaller size returns Ok; a new key returns Err | drop the `amount == 0` shortcut |
| `remove_refunds_capacity` | Remove frees capacity for a new key | `remove` without release |
| `reservation_precedes_publish` | Thread A `try_set("a")` parks in the per-instance probe (two Barriers; parks only on the first call, via an AtomicUsize). Main `try_set("b")` returns Err. Release A; A returns Ok; `len()==1` | reserving after publish |
| `concurrent_overwrite_refunds_surplus` | A parks after reserving `{1,c1}`; main native `set("a", big)`; release A. Then `len()==1`, `bytes_used()==reference`, `get("a")==v1` | `publish` ignores the real old cost |
| `concurrent_remove_forces_retry` | `a` exists; A reserves `{0,growth}` and parks; main `remove("a")`; release A. Probe calls `==2`, `len()==1`, bytes equal the reference | no covers/retry |
| `native_accounting_saturates` | Caps at `usize::MAX`; `try_set("a", Int(1), usize::MAX-1)`; native `set("b")`; `try_set("c",…,1)` returns `Bytes` | wrapping `fetch_add` |
| `update_charges_bytes` | `update("k", |_| big)` on an empty map gives `len()==1` and bytes equal a `set` reference | `update` does not charge |
| `guest_global_set_within_quota_succeeds` (control) | WAT below with the default quota: `a==Int(1)`, `b==Int(2)` | proves the fixture decodes |
| `guest_global_set_quota_traps` | Same WAT with `max_entries=1`: `Compute(msg)` contains "quota", `a==Int(1)`, `b` absent | `globals.rs` back to `.set()` |
| `guest_oversize_value_traps_before_read` | `max_value_bytes=8`; `global_set(0,1, 65536,16)` on 1-page memory: message contains "quota", `k` absent | remove the pre-check |

There is no test for O(1) `len`. Clippy guards it: once the allow is removed, `disallowed_methods` fails a return to `data.len()`.

Guest WAT fixture. WAT escapes are `\hh`, never `\xHH`. `\01` and `\02` are msgpack fixints, `Int(1)` and `Int(2)`.

```wat
(module
  (import "shamir_host" "global_set" (func $global_set (param i32 i32 i32 i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "a")
  (data (i32.const 1032) "b")
  (data (i32.const 1040) "\01")
  (data (i32.const 1048) "\02")
  (global $bump (mut i32) (i32.const 2048))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param i32 i32) (result i64)
    (call $global_set (i32.const 1024) (i32.const 1) (i32.const 1040) (i32.const 1))
    (call $global_set (i32.const 1032) (i32.const 1) (i32.const 1048) (i32.const 1))
    (i64.const 0))
)
```

The oversize fixture: `(memory (export "memory") 1)`, `(data (i32.const 0) "k")`, and `shamir_alloc` returns `(i32.const 1024)`.

`context.rs` ends up around 810 lines, which is under 1000.

### Out of scope

58C052B0 (compiler teardown) is split out to its own stage. Do NOT touch `compile.rs`, `Cargo.toml`, `Cargo.lock`, or the `#![forbid(unsafe_code)]` attribute.
