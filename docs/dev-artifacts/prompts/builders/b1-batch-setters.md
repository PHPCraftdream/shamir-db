IMPLEMENTATION TASK (TDD). Do NOT commit, do NOT push. Tests via ./scripts/test.sh (raw cargo test blocked). Touch ONLY crates/shamir-query-builder/src/batch/ — do NOT touch engine, ddl/, query/, val/, or any other crate (other agents are working there).

Goal (ACTION-ITEMS B1): expose the two `BatchRequest` fields the builder currently hardcodes.

File: crates/shamir-query-builder/src/batch/batch.rs. Read it first — `build()` (~line 628) hardcodes `result_encoding: ResultEncoding::default()` and an empty `interner_epochs`.

Add two chainable setters on `Batch`:
1. `pub fn result_encoding(mut self, enc: ResultEncoding) -> Self` — stores the encoding; `build()` emits it instead of the hardcoded default. (`ResultEncoding` is already imported — it's currently used only as the default value.)
2. `pub fn interner_epochs(mut self, epochs: <the field's type>) -> Self` — stores the interner-epochs map; `build()` emits it instead of `Default::default()`. Check the exact type of `BatchRequest.interner_epochs` in shamir-query-types and mirror it.

Keep the existing build() behavior identical when the setters are NOT called (default encoding + empty epochs) — backward compatible.

Tests (TDD, follow CLAUDE.md test layout — the batch tests live under crates/shamir-query-builder/src/batch/tests/):
- 🔴 a Batch with `.result_encoding(ResultEncoding::Id)` (or whatever the non-default variant is) → built `BatchRequest.result_encoding == Id`.
- 🔴 a Batch with `.interner_epochs(<a populated map>)` → built `BatchRequest.interner_epochs` equals it.
- a Batch WITHOUT the setters → default encoding + empty epochs (unchanged).
- 🟢 implement until green.
- Run: ./scripts/test.sh -p shamir-query-builder (read FULL output; never pipe|grep).

Keep the diff surgical, imports at top. End with a final message: the two setter signatures + test pass count.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents. Use grep/view directly.
