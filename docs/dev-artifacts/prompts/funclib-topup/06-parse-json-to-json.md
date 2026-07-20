# Funclib top-up 4f — parse_json(s) / to_json(v)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Sixth and FINAL P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
per report 10 (`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~lines 135, 158, 306):

> Missing: `parse_json` (Str → Map/List) and `to_json` (value → Str) —
> `validate/is_json` validates but cannot *parse*. A serde bridge already
> exists in the codebase.

## Investigation already done (verify yourself too — this is the key finding)

`shamir_types::types::value::Value<Key>` (aliased as `QueryValue` when
`Key = String`) already has a **hand-written, generic `serde::Serialize` +
`serde::Deserialize` impl** — `crates/shamir-types/src/types/value.rs`
(~lines 62-99 for `Serialize`, ~lines 109 onward for the `Deserialize`
visitor). This is almost certainly "the serde bridge that already exists"
the report refers to — read it in full before writing any code, it changes
the shape of this task from "write a JSON converter" to "wrap two existing,
already-correct serde impls with `serde_json::to_string`/`from_str`."

**Important, must-document behavior**: this Serialize impl is a
self-describing-format bridge, NOT a lossless round-trip for every
`QueryValue` variant:
- `Dec`/`Big` serialize via `serializer.serialize_str(&d.to_string())` —
  they become plain JSON strings. On the way back in (`parse_json`), a
  JSON string always deserializes to `QueryValue::Str` (the visitor has no
  way to know a given string was originally a `Dec`/`Big` — JSON has no
  tagging for this). This is the SAME "Value::Dec/Big decay to Str" fact
  already true of this codebase's msgpack round-trip (see
  `test_dec_and_big_roundtrip_as_string` if it exists, or similar —
  grep for it) — `parse_json(to_json(Dec(...)))` is NOT the identity, it
  becomes `Str`. This is expected, pre-existing, documented behavior, not
  a bug you need to fix.
- `Set` serializes via `serialize_seq` — indistinguishable from `List` on
  the wire. `parse_json(to_json(Set(...)))` comes back as `List`, not
  `Set`. Also expected — document it, don't try to preserve it (that would
  require inventing a new JSON convention this codebase doesn't otherwise
  use).
- `Bin` serializes via `serializer.serialize_bytes(b)` — `serde_json`'s
  `Serializer` implements this by producing a JSON array of byte-value
  numbers (JSON has no native bytes type). `parse_json(to_json(Bin(...)))`
  comes back as `List` of `Int`s, not `Bin`. Same story.

None of the above is something to "fix" — it is the honest, already-
established behavior of this codebase's generic serde bridge applied to a
self-describing format. Your job is to expose it via two funclib
functions and DOCUMENT these decay cases clearly in the module doc
comment, not to build a richer JSON encoding that preserves type fidelity
(that would be a much bigger feature — e.g. a custom tagged JSON
representation — explicitly out of scope here).

## The task

1. Add `serde_json = "1"` to `crates/shamir-funclib/Cargo.toml` (already a
   dependency of `shamir-query-builder` at this exact version string — pin
   the same one for consistency).
2. Add two functions to `crates/shamir-funclib/src/encode.rs` (the report
   suggests `encode/` or `cast/`; `encode.rs`'s own module doc comment
   ("binary/text encoding & escaping primitives") is the better fit —
   `parse_json`/`to_json` are exactly a text↔structured-value encoding,
   matching `base64_dec`/`base64_enc`'s shape conceptually):
   - **`to_json(v)`** — 1 arg, any `QueryValue`. Returns
     `serde_json::to_string(v)` as a `Str`, mapping a serialization error
     (should be rare/impossible given the impl you just read, but handle
     it) to a coded error (`"encode_failed"`, matching this file's
     existing `"decode_failed"` naming convention for the decode
     direction).
   - **`parse_json(s)`** — 1 arg, a `Str`. Returns
     `serde_json::from_str::<QueryValue>(s)`, mapping a parse error to
     `ScalarError::new("decode_failed")` (reuse the EXACT existing error
     code this file already uses for malformed decoder input — read
     `base64_dec`'s error handling to confirm the exact code string).
3. Update `encode.rs`'s module doc comment (function list + the "Encoders
   accept... Decoders accept..." convention bullets) to mention
   `parse_json`/`to_json` and the type-decay caveats above (Dec/Big → Str,
   Set → List, Bin → List-of-Int) — this is the ONE place an operator
   would look to understand the behavior, make it accurate and complete.

## Tests

1. `to_json` on a `Map` containing a mix of `Int`/`Str`/`Bool`/`Null`
   produces valid JSON text (parse it back with plain `serde_json::Value`
   in the test to confirm it's well-formed, independent of round-tripping
   through `QueryValue` again).
2. `parse_json` on a JSON object string produces `QueryValue::Map` with
   the correct keys/values.
3. `parse_json` on a JSON array string produces `QueryValue::List`.
4. `parse_json` on malformed JSON (e.g. unbalanced braces) returns
   `Err` with code `"decode_failed"`, not a panic.
5. Round-trip: `parse_json(to_json(v)) == v` for a `v` built ONLY from
   round-trip-safe variants (`Null`/`Bool`/`Int`/`F64`/`Str`/`List`/`Map`
   nested arbitrarily) — this must hold exactly.
6. Explicit non-round-trip documentation tests (these PROVE the decay,
   they are not failures): `parse_json(to_json(Dec(...)))` →
   `QueryValue::Str(...)`, NOT `Dec`; `parse_json(to_json(Set(...)))` →
   `QueryValue::List(...)`, NOT `Set`; `parse_json(to_json(Bin(...)))` →
   a `List` of `Int`s, NOT `Bin`. Name these tests clearly (e.g.
   `to_json_then_parse_json_decays_dec_to_str`) so a future reader
   understands this is intentional, not a regression waiting to happen.

## Out of scope

- Do NOT invent a richer/tagged JSON representation to preserve
  Dec/Big/Set/Bin type fidelity across the round-trip — that is a
  meaningfully bigger feature (a custom wire convention), not part of
  this P0 item.
- Do NOT touch `validate/is_json` (the existing validator) — this brief
  ADDS parsing capability alongside it, it does not change or subsume it.
- Do NOT touch any OTHER Этап 4 item — this is the LAST P0 item; once
  this is verified and committed, all 6 P0 items of Этап 4 are done, and
  whoever picks up the next task should decide whether Этап 4's P1 items
  (calendar functions, array set-ops, object path helpers, string case
  functions, min_by/max_by) get their own decomposition or whether Этап 5
  begins next — that decision is out of scope for this brief.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-funclib --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-funclib`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: you reused the EXISTING `Value<Key>` `Serialize`/
  `Deserialize` impl (via `serde_json::to_string`/`from_str`) rather than
  hand-rolling a new `QueryValue`↔JSON conversion — quote the exact one-
  or-two-line function bodies to prove this.
