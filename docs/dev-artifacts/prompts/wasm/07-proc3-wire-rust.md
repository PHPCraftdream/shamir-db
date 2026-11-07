# PROC-3b stage 1 (Rust): wire ops, executor handlers and Rust builders for the function facade

Implements the Rust half of ADR PROC-0 D6 on top of the PROC-1 facade that is already in this tree
(`ShamirDb::{describe_function_as, export_function_as, check_function_as, alter_function_optimize_as}`,
`FunctionDescription`, `FunctionExport`, `FunctionState`, `ModuleReport`, `CreateFunctionOptions.optimize`).
Read first: `docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` (D5, D6, Implementation
notes), `CLAUDE.md`, then the code you will touch. The TypeScript client, TypeScript builders and the guide docs
are another executor's work (`08-proc3-wire-ts.md`); do not touch `crates/shamir-client-ts` or `docs/guide-docs`.
Out of scope: the language `lang = "shamir"` (it is reserved: the server rejects it with the message
`lang "shamir" is not available yet`), the server option for `AdmissionLimits`.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-proc3` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: `shamir-query-types`,
`shamir-query-builder`, `shamir-db`, plus every crate that fails to compile because of an exhaustive match
over `BatchOp`/`OpKind` (find them with grep for `BatchOp::RenameFunction` and `OpKind::RenameFunction` across
the whole workspace, `shamir-server` and `shamir-client` included). If you believe you need a build or test
run, say so in the final message. No other shell commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits.

## Repository rules that bind this work

At most 7 direct entries per folder, at most 1000 lines per source file (`batch_op.rs` is at 858: keep it under
1000); `mod.rs` re-exports only; one file = one primary export; tests live in `tests/<topic>/`, never inline
`#[cfg(test)] mod`; no local `use`; no visibility widening, no new `#[allow]`; JSON literals in tests
multi-line; queries in tests go through the builder except in serde round-trip tests whose subject is the wire
format; comments terse; no machine-local paths; do not mention how or where CI is run. Count folder entries by
hand for every folder you add files to and nest instead of passing 7. Do not delegate edits to children that
run commands; children, if any, only edit files and the brief's NO-BUILD policy is quoted verbatim to them.

## Wire contract (fixed; the TypeScript executor builds against exactly this)

Existing op keys carry the name (`{ "drop_function": "name" }`). New and changed ops, JSON field names exact:

1. `create_function` gains two optional fields on `CreateFunctionOp` (both `skip_serializing_if` none, both
   plain strings parsed the way `visibility`/`security` are parsed):
   * `lang`: `"wasm" | "rust" | "shamir"`. Absent: inferred (`wasm` field present means `wasm`, `source` means
     `rust`). Present: must agree with the field supplied (`wasm` with `wasm`, `rust` with `source`), else a
     clear error; `"shamir"` is rejected as reserved (message above); unknown values are rejected.
   * `optimize`: `"auto" | "now" | "never"`, absent means `auto`; unknown values are rejected.
   The handler passes `optimize` into `CreateFunctionOptions`.
2. `{ "describe_function": "<name>" }` (struct `DescribeFunctionOp { describe_function: String }`). Result: one
   map record with keys `name` (string), `lang` (string), `origin` (string or null), `abi` (int or null), `hash`
   (string or null), `size` (int or null), `optimize` (string), `tier` (null or a map `{ tier: "fast"|"optimized",
   mode: "auto"|"now"|"never", calls_observed: int, upgrade_state: "idle"|"scheduled"|"failed"|"done"|"exhausted"
   }`), `state` (map `{ kind: "loaded" }` or `{ kind: "unloadable", reason }` or `{ kind: "corrupt", expected,
   actual }`). Use the same `QueryResult` shape that `describe_table` uses for a single record.
3. `{ "export_function": "<name>" }` (`ExportFunctionOp`). Result: one map record `{ wasm: <base64 string>,
   source: <string or null>, hash: <string> }`.
4. `{ "check_function": { "lang"?: ..., "wasm"?: <base64>, "source"?: <string>, "optimize"?: ... } }`
   (`CheckFunctionOp { check_function: CheckFunctionSpec }`, `deny_unknown_fields` on both). Exactly one of
   `wasm`/`source` required; `lang` follows rule 1. Result: one map record `{ ok: true, size: int, abi: int,
   origin: string or null, hash: string, functions: int, imports: int, exports: int }` taken from `ModuleReport`.
   A failed check is an error result carrying the admission message (not `ok: false`).
5. `{ "alter_function": "<name>", "optimize": "<mode>" }` (`AlterFunctionOp`, `optimize` required). Result: the
   same record shape as the existing `rename_function` success result (read it; reuse the shape).
6. HMAC: the canonical form of `CreateFunction` gets two more fields appended after the existing `wasm` digest:
   first the **resolved** lang as `e.str(...)` (explicit `lang`, else `"wasm"` when `wasm` is present, else
   `"rust"`), then the resolved optimize as `e.str(...)` (explicit, else `"auto"`). The destructuring in
   `security/hmac.rs` is exhaustive: add the new fields to it. None of the four new ops requires an HMAC.
   Do NOT edit the shared vector fixture JSON and do not try to recompute vector hex values: the orchestrator
   regenerates `canonical_hex` / `tag_hex` of the `create_function` vectors. If a Rust test needs a
   create-function canonical value, assert structural properties instead (changing `lang` or `optimize` changes the
   canonical bytes, equal resolved values give equal bytes), never a hard-coded hex.
7. Permissions are those of the facade methods (Read / Write / Create plus Execute on the compiler for Rust
   source); the handlers only map the wire ops onto the facade with the executor's actor, exactly like
   `handle_create_function` does, and map errors the same way (see `err_access`, `err_code` helpers in
   `execute/admin_meta/`).

## Deliverables

1. `shamir-query-types`: the new op structs (a new file next to `function_ops.rs`, respecting the folder entry
   limit), `OpKind` variants (`DescribeFunction`, `ExportFunction`, `CheckFunction`, `AlterFunction` with the
   wire names above), `BatchOp` variants with serialize/deserialize, every exhaustive match, `CreateFunctionOp`
   fields, `hmac.rs` canonical form, public re-exports.
2. `shamir-db`: handlers in new files under `execute/admin_meta/` (do not grow `admin_function.rs` past 500
   lines), dispatch arms in `admin_dispatch.rs`, `lang`/`optimize` handling in `handle_create_function`
   (validation rules above), result construction.
3. `shamir-query-builder`: in `ddl/schema/function.rs` (or a new sibling file if it would pass 400 lines):
   `.lang(...)`/`.optimize(...)` on the create-function builder, and builders for `describe_function`,
   `export_function`, `check_function` (wasm or source, optional lang/optimize) and `alter_function`; wire them
   into `Batch` the way the drop/rename function builders are; re-exports.
4. Every other crate that matches exhaustively over the changed enums compiles again (`shamir-server`'s admin
   gating / hmac gate lists: the new ops need no HMAC and no special gate beyond the facade's).

## Tests (required)

* `shamir-query-types`: serde round trip per new op (multi-line JSON), `deny_unknown_fields`, `lang`/`optimize`
  fields round trip and default to absent, canonical-form structural properties (6), `OpKind` name mapping.
* `shamir-query-builder`: builder output equals the expected op for each new builder and the new create-function
  options.
* `shamir-db` (follow the layout of `crates/shamir-db/tests/ddl_wire_e2e/` and the function facade tests): through
  the wire executor with the builder: create with `optimize: "never"` then describe shows mode `never` and tier
  `fast`; export returns the uploaded bytes (base64) and the hash from describe; check of a valid module returns the
  report and leaves no function; check of an invalid module (SIMD) is an error naming the feature; alter changes
  the described mode; `lang` mismatch, `lang: "shamir"`, unknown `optimize` are rejected with the specified
  messages; describe/export/alter permission denials for a non-privileged actor (reuse an existing pattern).

## Done means

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` pass for
`shamir-query-types`, `shamir-query-builder`, `shamir-db` and every dependent crate you had to touch; no folder
over 7 entries, no file over 1000 lines; every changed file read through for the whole class of error. Final
message: what is done and not done per deliverable, honestly; the exact result record shapes you implemented
(the TypeScript executor reads this message); the list of files touched.
