# Brief 78 — #1075 (MEDIUM): typed `CreateIndex` constructors silently discard already-set builder options

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect (F-7 MEDIUM + F-8 LOW, independently re-verified against the code)

`crates/shamir-query-builder/src/ddl/create_index.rs` — `CreateIndex` is a
mutable builder with two kinds of methods:

- **Stringly setters** (mutate `self`, return `Self`): `.fields(...)`,
  `.field(...)`, `.unique()`, `.sorted()`, `.repo(...)`, `.index_type(...)`,
  `.fts_tokenizer(...)`, `.fts_language(...)`, `.functional_op(...)`,
  `.functional_args(...)`, `.vector_dim(...)`, `.vector_metric(...)`,
  `.vector_quantization(...)`, `.include(...)`, `.if_not_exists()`.
- **Typed terminal constructors** (consume `self`, return `BatchOp` directly):
  `.hash(fields)`, `.unique_index(fields)`, `.sorted_index(field)`,
  `.sorted_with_include(field, include)`, `.fts(field, tokenizer)`,
  `.fts_with_language(field, tokenizer, language)`,
  `.functional(field, func)`, `.functional_with_args(field, func, args)`,
  `.vector(field, dim, metric, quantization)`.

Every one of the 9 typed terminal constructors reads exactly 4 fields off
`self` (`name`, `table`, `repo`, `if_not_exists`) and **silently ignores**
whatever else was set via the stringly setters — `unique`, `sorted`, the
previously-set `fields`/`field`, `index_type`, all FTS/functional/vector
`Option` side-fields, `include`. Verified directly, e.g.
`create_index.rs:73-81`:

```rust
pub fn hash(self, fields: impl Into<Vec<Vec<String>>>) -> BatchOp {
    let spec = IndexSpec::Hash {
        fields: fields.into(),
        unique: false,            // self.unique NOT read
        index_type: None,         // self.index_type NOT read
    };
    BatchOp::CreateIndex(spec.into_op(self.name, self.table, self.repo, self.if_not_exists))
}
```

**Data-corrupting scenario** (the only finding across three independent
reviews that corrupts data rather than just failing):

```rust
let op = create_index("idx_email", "users")
    .unique()                                   // developer asked for UNIQUE
    .hash(vec![vec!["email".to_string()]]);     // gets a REGULAR non-unique index
```

`unique: false` goes over the wire. The server creates a plain hash index.
The application believes a uniqueness constraint exists and gets duplicate
rows. No error anywhere, client or server. Same silent loss for
`.include(...).sorted_index(...)` (covering fields vanish, queries silently
degrade to full fetch) and `.vector_dim(768).hash(...)`.

This is the exact trap `#1017` (the typed-constructor migration) primed:
some state survives the typed call (`repo`, `if_not_exists`), some doesn't,
so a caller migrating from the stringly style to the typed style in a mixed
fashion gets something that compiles and often "looks right" while quietly
losing intent.

### F-8 — `try_build()` exists and is NOT what the typed constructors use

`crates/shamir-query-builder/src/ddl/create_index.rs:449-466` — `try_build()`
already does the right thing: it converts through
`TryFrom<&CreateIndex> for IndexSpec` (`create_index.rs:501-617`), which runs
all 12 validation checks (empty fields, unique+sorted, sorted multi-field,
vector-dim-required, cross-family option leakage, etc. — see
`CreateIndexBuildError`'s 11 variants in
`crates/shamir-query-builder/src/ddl/create_index_build_error.rs`) BEFORE
building the corresponding `IndexSpec` variant, then flattens it via
`IndexSpec::into_op`.

The 9 typed constructors build an `IndexSpec` variant **directly**, bypassing
`TryFrom` entirely — so `create_index("i","t").hash(vec![])` (empty fields)
produces a valid-by-TYPES `BatchOp` that `try_build()` would have rejected as
`CreateIndexBuildError::EmptyFields`. Their doc comments (e.g.
`create_index.rs:61-63`) claim "This is a **strict-by-default** typed
constructor: it produces a valid `BatchOp` directly with no need for
`try_build()`" — true only for the specific shapes the type signatures make
inexpressible (e.g. `.vector()`'s `dim: NonZeroU32` parameter rules out a
zero dimension at the call site), NOT true for empty `fields` vectors, which
every one of the 9 accepts as a plain `Vec`/`impl Into<Vec<...>>` with no
non-emptiness guarantee.

### TS side — narrower gap, same family of bug

`crates/shamir-client-ts/src/core/builders/ddl.ts`: unlike Rust, the TS typed
constructors (`hashIndex`, `uniqueIndex`, `sortedIndex`,
`sortedWithIncludeIndex`, `ftsIndex`, `functionalIndex`, `vectorIndex`,
`ddl.ts:358-588`) are **plain standalone functions**, not methods on a shared
mutable builder — so the DEFECT-1 state-loss scenario above (mixing a setter
call with a typed terminal) is structurally impossible in TS; there is no
shared mutable state to lose. Confirm this yourself by re-reading `ddl.ts`
before assuming otherwise — do not "fix" a bug that doesn't exist there.

What TS DOES share with Rust's F-8: `hashIndex()` / `uniqueIndex()` /
`sortedIndex()` / `sortedWithIncludeIndex()` / `ftsIndex()` /
`functionalIndex()` build their `CreateIndexOp` directly with **no
`fields.length === 0` check** (contrast `createIndex()`, the legacy path,
`ddl.ts:203-209`, which does check this). `vectorIndex()` is the one typed
constructor that already validates (`dim <= 0` throws, `ddl.ts:568-573`) —
use it as the template for what "validated typed constructor" should look
like for the other 6.

## The fix

### Rust — close both F-7 and F-8 on the SAME path

Recommended design (confirm against the code before committing, and clearly
state in your final report if you diverge and why):

1. **Every typed terminal constructor must detect "the builder already
   carries state this call would discard" and return an ERROR, not silently
   drop it.** Do not silently merge/overwrite either — the task's own
   analysis (re-verified) is explicit that naive overwrite cannot
   distinguish "caller mixed styles by mistake" from anything else, and
   masking is worse than a compile-time-adjacent runtime error for a
   footgun this sharp. Concretely, before constructing the `IndexSpec`
   variant, each typed constructor must check the ORTHOGONAL-TO-ITS-OWN-
   PARAMETERS setter fields are still at their default:
   - `.hash(fields)`: reject if `self.unique`, `self.sorted`,
     `self.index_type`, any FTS/functional/vector `Option` field, or
     `self.include` is non-default. (`self.fields`/`self.field` being
     non-default from an earlier `.fields(...)` call should ALSO reject —
     the `fields` parameter passed to `.hash(...)` is authoritative, a
     stale `self.fields` from an earlier setter call is exactly the kind of
     silently-discarded state this task exists to catch.)
   - `.unique_index(fields)`: same as `.hash`, except `self.unique` being
     `true` is fine (redundant, not a conflict) — but `self.unique == false`
     after an explicit `.unique()`... there's no way to distinguish
     "never called `.unique()`" from "called and it's still false" since
     `unique: bool` has no `Option` wrapper. Decide and document: is
     `unique: bool` (and `sorted: bool`) checked at all here, or is checking
     limited to fields that ARE distinguishable from their default (the
     `Option<_>` fields, `include: Vec<_>` non-empty, `fields: Vec<_>`
     non-empty, `index_type: Option<_>`)? State your reasoning — a `bool`
     with no sentinel for "never touched" is a real ambiguity, not an
     oversight to paper over.
   - Same pattern for `.sorted_index`, `.sorted_with_include`, `.fts`,
     `.fts_with_language`, `.functional`, `.functional_with_args`,
     `.vector` — each checks every field it does NOT itself set is still
     default, erroring otherwise.
   - Add whatever new `CreateIndexBuildError` variant(s) this needs (e.g. a
     single `ConflictingBuilderState { method: &'static str, field: &'static str }`
     used by all 9, or per-case variants — pick whichever keeps
     `create_index_build_error.rs`'s existing style, which favors specific
     named variants with a `Display` impl citing the server-side rejection
     source; this case has no server-side equivalent since it's purely a
     client-side builder-misuse detector, so word the message accordingly:
     e.g. "`.hash()` ignores `.unique()`; call `.unique_index()` instead, or
     drop the `.unique()` call").
2. **Change all 9 typed constructors' return type to
   `Result<BatchOp, CreateIndexBuildError>`.** This is a breaking API change
   — alpha (`0.1.0-alpha.1`, format not yet frozen) is the correct window
   per the task description; do not add a parallel non-breaking method
   instead. Update every call site in the workspace (search
   `\.hash\(|\.unique_index\(|\.sorted_index\(|\.sorted_with_include\(|\.fts\(|\.fts_with_language\(|\.functional\(|\.functional_with_args\(|\.vector\(` scoped to
   `CreateIndex` builder usages — check doctests inside `create_index.rs`
   itself too, they currently write `let op = create_index(...).hash(...)`
   with no `?`/`.unwrap()`).
3. **Route the empty-fields case (F-8) through the SAME check.** Once each
   typed constructor validates its own field-count requirement (all 9 need
   ≥1 field; `.sorted_index`/`.sorted_with_include`'s `field` parameter is
   already a single `Vec<String>`, not `Vec<Vec<String>>`, so "multi-field"
   is inexpressible there — but an EMPTY inner path, e.g. `vec![]` as the
   one field, is still possible and still needs rejecting), return
   `CreateIndexBuildError::EmptyFields` for consistency with `try_build()`'s
   existing behavior — do not invent a second empty-fields error variant.
4. **`build()` stays exactly as-is** (infallible, permissive, legacy —
   unaffected by this task). `IntoBatchOp for CreateIndex` also stays as-is
   (calls `.build()`, per its own established semantics — this task is
   scoped to the 9 typed terminal constructors + F-8's validation gap, NOT
   a re-litigation of `build()`'s permissiveness, which is a separate,
   already-settled design decision from earlier work).

### TS — close the F-8-equivalent gap only (F-7's state-loss scenario does not apply — see above)

Add the SAME `fields.length === 0` check (mirroring `createIndex()`'s
existing check at `ddl.ts:203-209`, same error message convention) to
`hashIndex`, `uniqueIndex`, `sortedIndex`, `sortedWithIncludeIndex`,
`ftsIndex`, `functionalIndex` — throw, matching every other validation in
this file's style (`vectorIndex` already does this for `dim <= 0`; use it as
the template). Do NOT add builder-mixing detection to TS — there is no
shared mutable builder for these functions to mix state through; confirm
this remains true after your Rust changes (i.e. you are not restructuring TS
into a builder as part of this task — scope is additive validation only).

## Tests — required minimum

**Rust** (`crates/shamir-query-builder/tests/create_index_matrix.rs` and/or a
new dedicated test file for the typed constructors — check whether the
existing matrix file's harness can be extended to cover typed-constructor
call sites, or whether a new file is cleaner given the typed constructors
take different call shapes than the fluent-setter + `try_build()` path the
matrix already covers; your call, but do not silently skip adding fixture
coverage for these paths):

1. For EACH of the 9 typed constructors: a case where a builder-mixing call
   precedes it and IS a conflict (e.g. `.unique().hash(fields)`,
   `.include(vec![...]).sorted_index(field)`, `.vector_dim(NonZeroU32::new(4).unwrap().get()).hash(fields)`
   — adjust to whatever setter shapes actually exist) → asserts `Err(...)`,
   not silent success with the wrong `BatchOp`.
2. Each of the 9 typed constructors called with EMPTY fields (or an empty
   single-field path for the single-field ones) → asserts
   `Err(CreateIndexBuildError::EmptyFields)`.
3. Each of the 9 typed constructors called with NO prior setter calls (the
   documented, intended usage) → still asserts `Ok(...)` and, where
   practical, wire-byte-identity against the equivalent `try_build()` output
   (mirroring the existing matrix fixture's `wire_hex` pattern) — this is
   the regression guard that your fix doesn't change the HAPPY-path output.
4. Update `crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`
   if you extend it to cover typed-constructor cases — keep the existing
   `_comment`/`_key_order_note`/`_check_order_note` documentation style, and
   do not change any EXISTING case's `expect`/`wire_hex` (they cover the
   fluent-setter + `try_build()` path, which this task does not touch).

**TS** (`crates/shamir-client-ts/src/core/builders/__tests__/create_index_matrix.test.ts`
or a sibling test file):

1. Each of the 6 newly-validated typed constructors called with empty
   `fields` → asserts a `throw`.
2. Existing happy-path typed-constructor tests (if any) still pass
   unmodified.

**Mandatory revert-and-check**: for at least 2 of the new Rust
builder-mixing-conflict tests, temporarily revert your `create_index.rs`
change locally, confirm the test goes GREEN incorrectly (i.e. currently
silently produces the wrong `BatchOp` instead of erroring — this is the
inverse of the usual revert-check since the CURRENT bug is silent success,
not a panic/error; assert on the WRONG-but-successful output to prove the
bug reproduces), then restore the fix and confirm the same test now asserts
`Err(...)`. Report this outcome explicitly, including what the pre-fix
`BatchOp` actually contained (e.g. "produced `unique: false` despite
`.unique()` having been called").

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder --full
```

For the TS side, use whatever this repo's established TS test command is —
check `crates/shamir-client-ts/package.json` scripts (e.g. `npm test` /
`npm run test`) and run it scoped to the ddl / create_index test files if
possible; report the actual command and its output.

Paste the actual final summary line from each command — literal output, not
a paraphrase. List every test you added/touched by name with individual
pass/fail status, and the outcome of the mandatory revert-and-check. If
anything fails, fix it before reporting done — everything you report must be
something you personally watched pass, with the command's actual output as
evidence, not an assumption.
