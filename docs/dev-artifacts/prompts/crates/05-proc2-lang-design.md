# PROC-2a: design document for the in-process language `shamir-lang`

Goal: write the design that PROC-2 (the in-process language compiler, ADR PROC-0 decision D4) is implemented from.
ADR D4 says: "The grammar is a separate document at PROC-2 start" and "the guest runtime design is PROC-2's first
deliverable". This task produces that document. It is a DOCUMENT ONLY task: no Rust or TypeScript code, no Cargo
changes, no commands except the layout check. The document is the durable artefact; later executors will be briefed from
its milestone list, so it must be precise enough to implement and to review against, and must decide, not survey.

## Read first (all in the repository)

* `docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md`: D1 (module contract: exports `memory`,
  `shamir_alloc(i32)->i32`, `shamir_call(i32,i32)->i64`, sections `shamir.abi`, `shamir.origin`), D2 (admission gates and
  limits: the compiler output is admitted exactly like an upload), D4 (the constraints this document refines), D5 (tiers).
* `crates/shamir-wasm-admission/src/` (`abi.rs`, `types/limits.rs`, `types/policy.rs`, `scan/`, `sanitizer.rs`): what a module
  may contain (feature set, caps, import allowlist `SANCTIONED_HOST_IMPORTS`). The generated module must pass these.
* `crates/shamir-sdk/src/` (`runtime/`, `api/`, `error.rs`, `prelude.rs`) and `crates/shamir-sdk-macros/src/lib.rs`: the existing
  guest-side Rust SDK: how `shamir_call` receives and returns msgpack, how errors are raised (`raise_error`), the host
  imports (`db_get`, `db_insert`, `db_query`, `db_execute`, `batch_*`, `global_*`, `call`, `http_fetch`) and their exact
  signatures and encodings. The language must reuse this wire convention, not invent another one.
* `crates/shamir-wasm-host/src/wasm/` (engine, `wasm_function.rs`, host import registration): how the host calls the guest,
  allocates, suspends on async imports (the host calls `shamir_alloc` while the guest is suspended: the allocator must be
  safe to re-enter), fuel, epoch and memory limits.
* `docs/guide-docs/guide/05-functions.md`: user-facing function model (validators, stored procedures, `call`).
* `crates/shamir-funclib/` only as far as needed to see how scalar functions are currently named and typed.

## Decisions the document must make (each stated as a decision with a one-line reason, no options left open)

1. **Surface syntax and grammar** of the TypeScript-like subset: complete EBNF (lexical grammar: identifiers, literals incl.
   integer/float/string/bytes, comments, keywords; syntactic grammar: module = list of function declarations, one marked as
   an entry; statements `let`/`const`, assignment, `if`/`else`, `while`, `for` (range and over list), `return`, `break`,
   `continue`, `throw`/error raising; expressions with a precedence table; calls; record literals and field access; list
   literals and indexing). Semicolons, blocks, and what is deliberately absent (closures, classes, generics, exceptions
   beyond `throw`, imports, dynamic typing, `any`).
2. **Type system:** `i32`, `i64`, `f64`, `bool`, `string`, `bytes`, `null`/optional (decide how absence is typed),
   records (structural or nominal: decide), lists (homogeneous), the msgpack-dynamic value at the boundary (how a typed
   function receives `params` and returns a result through the existing msgpack call convention: exact mapping
   table type <-> msgpack, including integer width checks, float/int coercion rules, string UTF-8 validation, error
   behaviour on mismatch). Numeric semantics (overflow wraps or traps: decide; division by zero; float NaN rules;
   comparison). No implicit conversions other than the ones tabulated.
3. **Entry points and function kinds:** the exact signature a user writes for (a) a scalar/validator function (pure
   over parameters, MVP) and (b) a procedure with database access (stage 2), how `shamir_call` is generated around it,
   how the function name and parameters are declared, how errors and `raise_error` map, how a validator reports
   field-level errors (look at how validators return errors today).
4. **Database access (stage 2):** how `db_get`/`db_insert`/`db_query`/`db_execute` are exposed in the language (typed
   builtins), how query results map to records and lists, and what stays out. State the MVP boundary precisely.
5. **Guest runtime** (the hardest part, the first implementation deliverable): the allocator (design it re-entrant: the host
   calls `shamir_alloc` while the guest is suspended mid-import; state the memory layout: stack, heap, data segment,
   global pointers, growth policy, and the per-call reset or free policy: a bump allocator reset per call versus a free
   list, with the memory-leak and fragmentation consequences), the msgpack encoder and decoder (what subset, where it
   lives, how it is emitted: pre-assembled wasm bytes embedded in the compiler versus generated per module through
   `wasm-encoder` helper functions; the size in bytes it adds to every module, and why that fits the admission size caps
   from `types/limits.rs`; dead-runtime elimination if used), string/bytes/list/record representation in linear memory,
   and the equality and ordering primitives. Deterministic output: the same source always produces the same bytes.
6. **Compiler architecture:** pipeline (lex, parse to AST, name resolution, type check, lowering to a small IR or directly
   to wasm, emit through `wasm-encoder`), the crate layout (`shamir-lang` is a pure crate: no `wasmtime`, no `tokio`, no
   process spawning; depends on `shamir-wasm-admission` to validate its own output and to emit the ABI sections;
   name every module and what it owns, respecting "one file = one primary export", at most 7 entries per folder and
   1000 lines per file), the public API (`compile(source, options) -> Result<CompiledModule, CompileError>` with diagnostics
   carrying spans, `CompiledModule { wasm, report }`), how the compiler stamps `shamir.abi = 1` and
   `shamir.origin = shamir-lang`, and how the server calls it (blocking pool, dedicated thread with a fixed stack size,
   the shared compile semaphore, time or fuel cap) without the compiler knowing about any of that.
7. **Bounds and crash safety** (ADR D4 "Crash safety"): numeric caps with chosen values (source bytes, tokens, AST depth,
   type-checker depth, function count, locals per function, literal sizes, output bytes, compile fuel units and what one
   unit means) and where each is enforced; why recursion is bounded by construction; fuzz and deep-nesting test criteria.
8. **Diagnostics:** error type shape, spans (byte offsets and line/column), message style, a stable error code list for
   the categories (lexical, syntax, type, limit), how they surface through the existing `check_function` and
   `create_function` error path.
9. **Testing strategy and acceptance criteria**, per milestone: golden tests (source to expected wasm via `wat`
   round-trip), execution tests (how a compiled module is run: the host crate engine in an integration test), property
   or fuzz tests for the lexer, parser and type checker, negative tests per diagnostic code, a conformance corpus.
10. **Milestones:** break the implementation into 6 to 10 milestones, each a self-contained piece for one executor (crate,
    files, public surface, tests, "done when"): suggested order lexer+AST+parser, type checker, codegen for scalars,
    guest runtime (allocator + msgpack), `shamir_call` glue and validators, `lang = "shamir"` wiring in
    `shamir-db` (that is task PROC3B-2), database builtins. For each milestone: dependencies, the crates it touches, and
    the risk to watch.

## Constraints on the document

* One file: `docs/dev-artifacts/design/shamir-lang-design.md` (the folder is documentation, no line limit, but keep it
  structured and scannable: numbered sections matching the list above, tables for mappings and caps, EBNF in fenced
  blocks). English. No machine-local paths. Do not mention how or where CI is run.
* Decide, do not survey: every open point gets a chosen answer and a short reason; a final "Non-goals" section lists what
  is deliberately out. Where a decision depends on a fact in the code, cite the file and the fact.
* Include a worked example: one validator and one scalar function in source, the msgpack they receive and return, and a
  sketch of the emitted module sections and exports (not full wasm).
* Honest status: end with a "Verified against the code" list naming the facts you checked (signatures, caps) and a
  "Not verified" list.
* Do not touch any file other than the design document. The only command you may run is
  `python -I scripts/ci/check_layout.py`.

## Done when

The document exists, covers all ten sections with decisions, contains the worked example and the milestone list with
acceptance criteria, and its caps are consistent with the admission limits you read. Final message: what is covered,
what is uncertain, and every place where you had to pick between close alternatives.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-lang` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: none (documentation only; no cargo run), plus every
dependent whose public signature or path you changed (grep the workspace). `Cargo.lock` may change only by what
this task needs: run `cargo check` once WITHOUT --locked so cargo writes it, and say so. If you believe you need a
build or test run, say so in the final message. No other shell commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits (and has already done every file
move: you cannot move or delete files, so work with the layout you find).

## Repository rules that bind this work

At most 7 direct entries per folder, at most 1000 lines per source file; `mod.rs` re-exports only; one file = one
primary export; tests live in `tests/<topic>/`, never inline `#[cfg(test)] mod`; no local `use`; no visibility
widening and no new `#[allow]`; no `impl` blocks outside the owning module (rust-guard); JSON literals in tests
multi-line; comments terse; no machine-local paths; do not mention how or where CI is run. Count folder entries by
hand for every folder you touch (the layout script cannot see untracked files) and nest instead of passing 7.
Children, if you use any, only edit files, never run commands, and get the NO-BUILD policy above quoted verbatim.
