# Brief — #983: binary (Uint8Array) field value corrupted on write.insert round-trip

Task: #983 in the session TaskList. A **data-correctness** bug found during
#980's e2e verification, never root-caused. Read this brief in full — it
records what has ALREADY been ruled out, so you must not burn time re-checking
those.

## Symptom

Through the TS/JS client:

```js
await write.insert('blobs', [{ id: 'b1', blob: new Uint8Array([0,1,255,254,127,128]) }]);
// read the row back:
//   blob === {"0":0,"1":1,"2":255,"3":254,"4":127,"5":128}   ← WRONG
//   expected: a Uint8Array / binary value
// and:
await read.find('blobs').where(filter.eq('blob', filter.bin(payload)));
//   matches ZERO rows — should match 1
```

The `{"0":0,...}` shape is exactly what JS produces when a `Uint8Array` is
walked as a generic object (`{...u8}`, `JSON.parse(JSON.stringify(u8))`,
structured-clone-to-plain, or a msgpack encoder that fell through to its
"generic object" branch). That is a strong hint, **not** a conclusion — the
zero-row filter match suggests the value on the server side is also not
`Bin`. Do not assume either side; bisect.

## Already ruled out — DO NOT re-verify these

Verified during the first investigation pass:

1. **Client-side msgpack encoding in isolation.** `require('@msgpack/msgpack')
   .encode(...)` emits correct msgpack `bin8` (0xc4 prefix) both for a
   standalone `Uint8Array` and for one nested in `{ id, blob }`. No
   `JSON.stringify` / `structuredClone` was found anywhere in the client write
   path (`write.insert`, `Batch.add/build`, `client.execute`).
2. **`Value`'s serde `Deserialize` visitor** —
   `crates/shamir-types/src/types/value.rs` ~168-173 (`visit_bytes` /
   `visit_byte_buf`) correctly maps to `Value::Bin`. Its `Serialize` side
   (~line 75, `Value::Bin(b) => serializer.serialize_bytes(b)`) is also
   correct.
3. **`crates/shamir-types/src/codecs/interned/codec.rs`** — every
   `Value::Bin` / `RecordValue::Bin` → `InnerValue::Bin` / `QueryValue::Bin`
   arm (lines ~47, 100, 181, 249) preserves `Bin`. All four walkers
   (`query_value_to_inner_with`, `inner_value_to_query_value_with_rev`,
   `record_value_to_query_value_with_rev`, `rv_deintern_value_with`) were read
   arm-by-arm and are correct.
4. **Filter comparison side** —
   `crates/shamir-engine/src/query/filter/resolve.rs` and `eval_bytes.rs`
   (~487, `RawScalar::Bin` vs `FilterValue::Binary`) look correct.

So: every *per-value* conversion, checked individually, is right. The defect is
therefore almost certainly in a **higher-level record-walking / re-encoding
step** that bypasses one of those converters, or in a **generic fallback**
branch, or on the **TS client** side outside the encode call itself.

## Required approach — bisect bottom-up, in Rust first

⚠️ **Do NOT continue debugging through the JS e2e suite.** It is the slowest
possible bisection instrument and it conflates ~6 layers. Work bottom-up in
Rust, and only cross into TS if every Rust layer comes back clean.

Write throwaway-or-keepable Rust tests at each layer below, in order, and
report which is the FIRST to fail:

**Layer 1 — pure serde round-trip of a `QueryValue::Bin`.**
`rmp_serde::to_vec_named` → `rmp_serde::from_slice::<QueryValue>` on a
`Value::Map` containing a `Value::Bin`. Also test the raw bytes a JS client
would send (hand-write the msgpack: `0x82` map2, `"id"`/str, `"blob"`/`0xc4`
bin8) → decode as `QueryValue`. This isolates whether `deserialize_any` over
a real bin8 marker reaches `visit_bytes` in this repo's rmp-serde version.

**Layer 2 — storage encode/decode.** `InnerValue::Bin` → the interned
on-disk MessagePack encoder → `RecordView` lens → back to `QueryValue`.
Check BOTH read paths (the `InnerValue` tree path and the `RecordView` lens
path) — the codec doc comment claims they are arm-for-arm identical; verify
that claim empirically for `Bin`, don't trust it.

**Layer 3 — engine insert/read.** `TableManager`-level insert of a record
whose field is `Bin`, then read it back; then a `filter.eq(field, Binary)`
query against it. This covers the record-walking step between the wire type
and storage.

**Layer 4 — full Rust wire round-trip.** Through `shamir-db`'s e2e harness
(`crates/shamir-db/tests/`, copy the setup shape from an existing test there):
insert a `Bin` field through a real batch op and read it back over the real
wire path.

**Layer 5 (only if 1-4 are all clean) — TS client.** Then the bug is
client-side. Bisect by having the TS client build+encode the batch and dumping
the bytes it actually sends (not what a standalone `encode()` call produces —
the ruled-out check #1 tested the encoder, not the client's actual call site),
then decoding those exact bytes in a Rust test. Also check the RESPONSE
direction independently: the de-intern path in
`crates/shamir-client-ts/src/core/field-map.ts` /
`crates/shamir-client-ts/src/core/client.ts` — the Rust side has a
closure-driven de-intern (`record_view_deintern_with`, codec.rs ~220) whose TS
counterpart may not handle a `Bin` leaf.

Note the write and read directions can BOTH be broken independently. Once you
find the first failure, keep going: confirm the other direction separately
before declaring a single root cause.

## Required deliverables

1. **A clear root-cause statement**: which exact file/function/line loses the
   `Bin`-ness, in which direction (write, read, or both), and why the
   individually-correct converters listed above are not reached there.
2. **The fix**, surgical — do not refactor surrounding code.
3. **Regression tests (TDD — write the failing test FIRST):**
   - A Rust test at the LOWEST layer that reproduces the bug (this is the
     durable artefact — it must FAIL against the unfixed code; verify that
     explicitly and say so in your report).
   - A Rust test asserting `filter.eq(field, Binary)` matches a row inserted
     with that binary value.
   - If the root cause is (also) in TS: a vitest test in
     `crates/shamir-client-ts/src/**/__tests__/` covering it.
   - A JS e2e test in `tests/e2e/tests/` (extend an existing file, e.g.
     `02-basic-crud.test.js` or `05-filters.test.js` — check where a binary
     field fits best; do NOT create a new test file unless nothing fits)
     asserting the ORIGINAL symptom is gone: insert a `Uint8Array`, read it
     back as binary, and filter on it by `filter.bin(...)` matching exactly 1
     row.
4. Follow this repo's test-organisation convention: no inline
   `#[cfg(test)] mod tests { ... }` in implementation files — tests go in the
   module's `tests/` directory with `tests/mod.rs` as a re-export-only
   manifest.

## Scope discipline

- Fix ONLY what the bisection proves is broken. If you find a second,
  unrelated defect on the way, REPORT it — do not fix it in this pass.
- Do not change the wire format. If the only correct fix requires a wire
  change, STOP and report that finding instead of making the change.
- Do not touch any of the four already-verified-correct areas listed above
  unless your bisection proves one of them wrong (in which case say so
  explicitly and show the evidence — that would contradict this brief).

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-types -p shamir-engine -p shamir-db --full
```

If you touched the TS client, ALSO (from `crates/shamir-client-ts/`):

```
npm run build
npx vitest run
```

and, if you touched/added a JS e2e test, from `tests/e2e/`:

```
npm test
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Every Rust test
run goes through `./scripts/test.sh` (or `cargo t` / `cargo tl`). Use
`./scripts/test.sh -p <crate> -- <substring>` for a narrow single-test run.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the test
run, then commits. Only edit/create files and run read-only / test / gate
commands.

## What to report back

- The bisection table: layer 1-5, PASS/FAIL each, with the exact test you ran.
- The root cause: file, function, line, and the mechanism.
- The diff summary per file.
- Explicit confirmation that your lowest-layer regression test FAILS without
  the fix (state how you verified that).
- Exact gate command output.
