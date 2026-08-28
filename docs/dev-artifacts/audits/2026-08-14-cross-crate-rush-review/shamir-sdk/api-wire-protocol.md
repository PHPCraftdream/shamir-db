# shamir-sdk -- API & wire-protocol design

## Summary

The crate's msgpack `Value` mirror and the `Validation` ABI are genuinely well
done: byte-identity with the host `QueryValue` is enforced by bidirectional
conformance tests, and the validator result shape is pinned against the
engine's `decode_validation_result`. The weak spots are (1) the non-gated
`Table::get/query` raw-`Value` filter surface, which sits outside the mandated
query builder with undocumented, silently-lossy key-convention semantics, and
(2) a habit of coercing protocol/decode failures into plausible-looking
successes (`Ok(vec![])`, `None`, `Value::Null`) that makes host/guest wire bugs
nearly undebuggable. There is also no versioning on the `db_execute` wire path,
and the HTTP envelope codec is the one wire contract with no tests.

## Findings

### 1. Raw-`Value` filter surface (`Table::get`/`Table::query`) bypasses the builder-only rule, with no required exception comment

- **File:line:** `crates/shamir-sdk/src/db.rs:5-9, 79-109`; host counterpart `crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:41-85`
- **Severity:** high
- **Issue:** CLAUDE.md ("Query construction -- builder only") mandates that all
  queries/filters go through the query builder, forbids hand-assembling a
  filter/wire op from raw `Value`, and requires a one-line comment stating *why*
  wherever the builder genuinely does not apply. `Table::get(key: Value)` and
  `Table::query(Option<Value>)` are a non-feature-gated query surface built from
  raw `Value`s whose semantics live in an ad-hoc host-side mini-interpreter
  (`FacadeDbGateway::key_to_filter`): map -> conjunction of `Eq`, scalar -> `Eq`
  on `"id"`, empty map -> match-all. No comment in the SDK justifies the
  builder exemption, and the crate docs (db.rs module doc, `Ctx` examples)
  present this as a first-class query API. It also silently diverges from the
  builder path that exists in the same crate (`Db::execute`, feature
  `query-builder`): no comparison ops, no projection, no pagination (host
  hardcodes `Pagination::None` and `Temporal::Latest`), and unsupported filter
  values (Dec/Big/List/Set/nested Map) are silently coerced to
  `FilterValue::Null` (`db_gateway.rs:49-52`) rather than rejected.
- **Failure scenario:** a guest filters on a decimal or list-typed field; the
  filter silently becomes `Null` and the query returns wrong/empty results with
  no error. Or an author assumes builder semantics (limits, ordering) and gets
  an unbounded scan.
- **Suggested fix:** either deprecate the raw filter parameters in favor of the
  `query-builder` path (route `Table` through `Batch` + builder ops internally),
  or keep it but add the required justification comment, document the key
  convention (including empty-map = match-all and the `Null` coercion) in
  `db.rs`, and make the host error out on unsupported filter value types
  instead of coercing to `Null`.

### 2. `Table::get` with an empty `Value::Map` returns the table's first record

- **File:line:** `crates/shamir-sdk/src/db.rs:74-81`; host `db_gateway.rs:62-67`
- **Severity:** medium
- **Issue:** the SDK never checks the key; the host maps an empty map to
  "no filter" (match-all), and `get` returns `records.first()`. This convention
  is documented only in host code, not in the SDK's public docs.
- **Failure scenario:** a guest builds its key from request params that turn out
  to be empty (missing fields, empty `Value::Map` payload); "get by primary key"
  silently returns an arbitrary (first) row and the function proceeds operating
  on the wrong record -- e.g. re-validating or returning another tenant's/user's
  document.
- **Suggested fix:** reject an empty-map key in `Table::get` (SDK-side
  `Error::user` before crossing the ABI) or have the host return an error for
  empty-map keys on `get`; at minimum document the edge in `db.rs`.

### 3. Decode failures are silently coerced into success values in the host-import decoders

- **File:line:** `crates/shamir-sdk/src/host_imports.rs:97, 106, 131, 146, 162, 178-183, 207`; also `src/context.rs:86-88`, `src/__rt.rs:11-16`
- **Severity:** medium
- **Issue:** `rmp_serde::from_slice(...).ok()` / `.unwrap_or(...)` make a
  corrupt or truncated host reply indistinguishable from a legitimate result:
  `db_query` decode failure becomes `Value::List(vec![])` so `Table::query`
  returns `Ok(vec![])` (silent "no rows"); `batch_get`/`global_get`/`db_get`
  become `None` (indistinguishable from "absent"); `call`/`db_insert` become
  `Value::Null`. `__rt::decode_params` maps malformed host bytes to an empty
  `Params`, so every getter then fails with "missing parameter: X" instead of a
  protocol error. This violates the CLAUDE.md error-handling rule (propagate,
  don't swallow) and makes host/guest protocol bugs undebuggable. Only the
  `packed == 0` sentinel legitimately means "absent".
- **Failure scenario:** version skew or an allocator bug makes the host write a
  truncated buffer; every query "returns no rows" and every key lookup is
  "absent". The guest author chases a phantom data bug that is actually a wire
  failure.
- **Suggested fix:** decode to `Result` internally and trap/return
  `Error::user("db_query: undecodable host response: ...")` on failure; reserve
  `None`/`Null` exclusively for the `packed == 0` sentinel. Same treatment for
  `decode_params` (an undecodable params payload deserves a distinguishable
  error at first access).

### 4. No wire-format versioning or capability negotiation on `Db::execute` (or the guest ABI)

- **File:line:** `crates/shamir-sdk/src/db.rs:135-148`, `crates/shamir-sdk/Cargo.toml:14-18`; host `db_gateway.rs:285-294`
- **Severity:** medium
- **Issue:** `BatchRequest`/`BatchResponse` cross the ABI as bare msgpack with
  no version tag, magic, or handshake; the `shamir_host` import set itself
  carries no version either. Serde ignores unknown fields and defaults missing
  `Option`s, so *additive* changes decode "fine" while renames/removals silently
  change semantics. Compiled guests are persisted and run against a host that
  upgrades independently -- exactly the deployment model where this bites.
- **Failure scenario:** a function compiled against sdk 0.1.0-alpha.1 runs on a
  host upgraded a year later; a renamed `BatchRequest` field (e.g.
  `return_only`) decodes as `None` and the batch silently returns a different
  result set instead of failing loudly.
- **Suggested fix:** add an explicit `protocol_version` field (or a header
  byte/import) checked on both sides, failing closed with a clear error on
  mismatch; document the wire-compat policy for the alpha. The mitigating
  factor today is that both sides live in one repo, but that will not hold once
  guests are compiled and stored.

### 5. HTTP wire-envelope codec has zero tests; duplicate headers collapse on the wire

- **File:line:** `crates/shamir-sdk/src/http.rs:24-44, 54, 98-111, 124-160`; host `crates/shamir-wasm-host/src/wasm/host_http.rs:20-97`
- **Severity:** medium
- **Issue:** unlike `Value` (bidirectional conformance tests) and `Validation`
  (shape-pinned tests), the third wire envelope -- `HttpRequest::to_value`,
  `decode_fetch_envelope`, `HttpResponse::from_value` -- has no tests anywhere
  in the workspace; the shape is duplicated by hand between the two crates'
  doc comments as the only contract. Additionally, `HttpRequest::headers` is a
  `Vec<(String, String)>` that permits duplicate names, but the wire shape is a
  msgpack map: the host decodes into an `IndexMap` (last value wins, one header
  silently dropped), and response headers are deduped the same way.
  `HttpResponse::from_value` also truncates status via `Int as u16` and silently
  drops non-`Str` header entries and non-`Bin` bodies.
- **Failure scenario:** a guest adds `Authorization` twice (or a retry loop
  appends it); one wins silently and upstream auth fails mysteriously. A future
  refactor of either side's envelope drifts undetected because nothing pins the
  bytes.
- **Suggested fix:** add msgpack round-trip tests against the host's exact
  envelope shape (mirroring `value_tests.rs`), reject duplicate header names in
  `HttpRequest::header` (or define last-wins explicitly), and make
  `from_value` strict (`Err` on non-u16 status / non-`Bin` body).

### 6. `Error` is an unkind-ed message string; internal protocol failures are constructed as "user" errors

- **File:line:** `crates/shamir-sdk/src/error.rs:6-23`; misuse sites `src/db.rs:89, 105-107, 141-147`
- **Severity:** low
- **Issue:** CLAUDE.md prescribes `thiserror` error enums for library crates.
  The SDK's public error type has a single message variant with no kind/code,
  and wire-protocol failures ("execute: decode response: ...", "db_query
  expected list, got unexpected value", "db_insert returned null") are built
  with `Error::user()`, misclassifying host/protocol faults as guest-caused
  ones. Callers cannot branch (retry on transport vs. surface to user), and the
  misleading "returned null" message hides the real `packed == 0` case.
- **Suggested fix:** a `thiserror` enum with at least `User`, `Protocol`,
  `Decode` variants; the host only consumes the stringified message, so the
  wire encoding is unchanged.

### 7. `pub mod __rt` contradicts its own "not part of the public SDK surface" doc

- **File:line:** `crates/shamir-sdk/src/lib.rs:18`; `crates/shamir-sdk/src/__rt.rs:1-3`
- **Severity:** low
- **Issue:** `__rt` is declared `pub` with `pub fn decode_params/encode_value/
  leak_result/trap`, which makes them semver-public and rustdoc-visible while
  the module doc claims the opposite. They do need to stay `pub` (the
  proc-macro-generated code lives in *consumer* crates), but as-is the crate
  accidentally commits them to its public API.
- **Suggested fix:** `#[doc(hidden)] pub mod __rt;` plus an explicit
  semver-exemption note in the module doc.

### 8. Dec/Big down-level to `Str` on the wire -- guests get silent no-matches when re-filtering on those fields

- **File:line:** `crates/shamir-sdk/src/value.rs:8-11`; interaction with `src/db.rs:74-79` and host `db_gateway.rs:47-52`
- **Severity:** low
- **Issue:** the lossy Dec/Big -> `Str` mapping is documented for *reads*
  ("lossy but stable"), but its interaction with the key convention is not: a
  decimal field read from a record arrives as `Value::Str("123.456")`; passing
  it back inside a `get`/`query` key produces `FilterValue::String`, which
  never equals the decimal column value host-side -- a silent, permanent
  no-match. Without the (feature-gated) builder the guest has no typed way to
  express a decimal predicate.
- **Failure scenario:** the natural "fetch record, then `get` by one of its own
  fields" pattern returns `None` forever for decimal/bigint keys; the author
  concludes the data is missing.
- **Suggested fix:** document the trap in `Table::get/query` docs; longer term,
  funnel filtering through the builder (see finding 1) where decimal predicates
  are typed.

### 9. Guest ABI passes pointers as signed `i32`; host rejects addresses >= 2 GiB

- **File:line:** `crates/shamir-sdk/src/host_imports.rs:60-66, 79-86`; host `crates/shamir-wasm-host/src/wasm/wasm_function.rs:302-304`
- **Severity:** nit
- **Issue:** `encode_leak` casts `bytes.as_ptr() as i32`, which is negative for
  guest addresses >= `0x8000_0000`, and the host's `read_guest_mem` rejects
  `ptr < 0`. Harmless for typical (<= 2 GiB) linear memories, but every host
  import fails on a 2-4 GiB guest memory even though wasm32 addresses are
  unsigned by construction.
- **Suggested fix:** on the host, reinterpret as `u32` (`(ptr as u32) as usize`)
  before the bounds check, or document a 2 GiB linear-memory limit as part of
  the ABI contract in `wasm_function.rs`.
