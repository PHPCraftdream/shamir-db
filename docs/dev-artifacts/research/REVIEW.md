בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Adversarial Review — 5 Research Reports

**Reviewer:** skeptical senior pass, zero-trust verification against source code.
**Method:** opened every cited file, confirmed each load-bearing claim by reading
the actual type/method/field. Findings cite real `file:line`.

> **Статус-апдейт (point-in-time audit, не переписан построчно):** ВСЕ actionable-
> находки ниже отработаны — см. `DONE.md`. В частности: keyset «engine-ready,
> language-absent» (H3) — ✅ реализовано; «FK + unique fail-open под autocommit» —
> ✅ **снято как ложная тревога** (enforced через серверную tx-обёртку; разбор в
> `META-REVIEW.md` §0); счётчики ❌/`it()`/folders (Report 1/3/4) — ✅ F1–F5
> (Phase E.9); Rust `one_of` (B2) — ✅ Phase G.1; `SelectExpr`/`$expr`/`$cond` —
> `$expr`/`$cond` ✅ (B3), `SelectExpr` ⏸ осознанно отложен (③.3b, движок не
> исполняет). Читать findings ниже как исторический снимок.

---

## Cross-report finding tally

- **❌ Factual errors found:** 4
- **⚠ Contradictions / misleading claims:** 5
- **🟡 Minor issues:** 6

---

## Report 1: `coverage-rust-query-builder.md` — Grade: B+

**Verdict:** the most reliable of the five. The table verdicts (✅/🟡/❌) are
almost entirely accurate when checked against source. The problems are in the
**arithmetic of the summary**, not in the per-row assessments.

### What it got RIGHT (verified)

1. **All 50 `BatchOp` variants enumerated correctly** — every variant name in
   `batch_op.rs:36-138` matches the report's table. No phantom or missing ops.
2. **`FilterExpr` / `Cond` have zero builder constructors** — confirmed:
   `grep` for `pub fn expr|pub fn cond` in `shamir-query-builder/src` returns
   nothing. The `val/filter_value.rs` file ends at `qref_all()` (line 155) with
   no expr/cond wrappers. ✅ correct.
3. **`SelectItem::Expression` marked "future"** — `select.rs:108` has the
   doc comment `/// Expression (future: computed fields)`. ✅ correct.
4. **`InsertOp.records_idmsgpack` hardcoded to empty** — `insert.rs:55`
   hardcodes `records_idmsgpack: Vec::new()`. ✅ correct.
5. **`FieldBuilder` missing `one_of`** — confirmed by reading the full
   `ddl/schema.rs` (lines 39-234): there is no `.one_of()` method, though
   `ConstraintsDto.one_of` exists at `schema_ops.rs:65`. ✅ correct.
6. **`ResourceRef::FunctionFolder` missing helper** — `ddl/res.rs` has
   `database()`, `store()`, `table()`, `function()`, `function_namespace()` but
   no `function_folder()`. ✅ correct.
7. **Filter leaf constructors** — all 21 functions in `filter/leaf.rs` match
   the report's claims (eq/ne/gt/gte/lt/lte/field_eq/in_/not_in/like/ilike/
   regex/is_null/is_not_null/exists/not_exists/contains/contains_any/
   contains_all/between/fts/vector_similarity/computed/computed_with_args).

### ❌ Factual error: summary arithmetic is wrong

The OQL summary (line 94) says **"5 ❌ missing"** and the summary table (line 290)
says **"❌ 7"**. The actual count of ❌ rows in Table 1 is **10**:
#8 (SelectExpr), #26 (FilterExpr), #27 (Cond), #30 (records_idmsgpack),
#46 (Ping), #48 (CreateScramUser), #49 (TxBegin), #50 (TxExecute),
#51 (TxCommit), #52 (TxRollback).

The text rationalises "5" by grouping the 6 DbRequest-level ops as one item,
but that is inconsistent with the table which lists them as separate numbered
rows. The summary table's "7" matches neither. **The real numbers are:
52 rows, 39 ✅, 3 🟡, 10 ❌.**

### ⚠ Misleading: OQL 🟡 count also inconsistent

The summary table says "🟡 4" for OQL. The actual 🟡 rows are **3** (#36
interner_epochs, #37 result_encoding, #47 Execute wrapping). The text says
"2 🟡". Neither matches.

### 🟡 Minor: `func()` note says "no simple-form helper"

The report says `val::func()` "always uses `FnCall::Complex`". This is accurate
for the Rust builder. However, the note could mislead a reader into thinking
the **wire** lacks the simple form — it doesn't (`FnCall` is an untagged enum
that accepts both). The gap is builder-only, which the report does state, so
this is a clarity issue, not an error.

---

## Report 2: `coverage-ts-query-builder.md` — Grade: A−

**Verdict:** the most thorough and accurate report. Every TS method cited was
confirmed to exist at the stated file:line. The parity assessments against Rust
are correct. The gap list (G1–G10) is well-reasoned.

### What it got RIGHT (verified)

1. **`filter.expr()` and `filter.cond()` exist in TS** — `filter.ts:257` (`expr`)
   and `filter.ts:266` (`cond`). The report correctly notes "TS exceeds Rust
   builder here" — verified: Rust has no equivalents. ✅
2. **`filter.fn()` handles both Simple and Complex** — `filter.ts:245-249`:
   no-args → `{ $fn: name }`, with-args → `{ $fn: { name, args } }`. ✅
3. **`subscribe` deliver_call missing (G6)** — `subscribe.ts:67-78`
   (`resolveDeliverMode`) only handles `handle` (batch) and `deliver:
   'records'|'keys'`. The wire type `{ call: CallOp }` exists at
   `types/subscribe.ts:44` but no builder path produces it. ✅ correct gap.
4. **`oneOf()` exists in TS but not Rust** — `ddl.ts:621` has `.oneOf()`;
   Rust `FieldBuilder` has no `.one_of()`. The report marks both ✅, which is
   accurate per-builder (TS has it, Rust wire has the field). ✅
5. **`refFunctionFolder()` exists in TS, not Rust** — `admin.ts:66`; Rust
   `res.rs` has no equivalent. ✅
6. **`computed()` merges `exprArgs` as optional 5th param** — `filter.ts:185-200`
   confirms the 5th param `exprArgs?`. ✅
7. **Interner DDL missing from TS (G7)** — confirmed: no `InternerDumpOp`/
   `InternerTouchOp` in `types/ddl.ts` or `builders/ddl.ts`. ✅

### ⚠ Contradiction with Report 1: `one_of` parity assessment

Report 1 (DDL #26) marks Rust `FieldBuilder` as 🟡 for missing `one_of`.
Report 2 (row #180) marks the same capability as "✅ wire: `ConstraintsDto.one_of`"
for Rust. These are not technically contradictory (one rates the builder, the
other rates the wire field), but a reader comparing the two reports will be
confused about whether Rust "has" one_of or not. The wire field exists; the
builder setter does not. Both reports are individually correct but use
different rating semantics without cross-referencing each other.

### 🟡 Minor: `queryRef` path attachment

The report says TS `queryRef(alias, path)` produces `$query` refs. The
implementation (`filter.ts:222-226`) attaches `path` as a **sibling key** on
the same object (`{ $query: alias, path: path }`), which is correct for the
untagged `FilterValue::QueryRef` enum (the `path` field is a sibling of
`$query` in the serde representation). The report doesn't call this out, but
the wire shape is correct. No error.

---

## Report 3: `coverage-ts-tests.md` — Grade: B

**Verdict:** structurally sound and honest about limitations (the e2e-gating
caveat is valuable). The main weakness is **systematic undercounting of `it()`
cases** across nearly every test file — the "approx." counts are consistently
15–40% below the real numbers. The qualitative assessments (which features are
unit-only vs e2e-tested) are accurate.

### What it got RIGHT (verified)

1. **Phase B/C constraint setters have zero unit tests** — confirmed: `ddl.test.ts`
   contains no tests for `.scalar()`, `.oneOf()`, `.format()`, `.compare()`,
   `.foreignKey()`, or `.unique()` (as FieldBuilder constraints). All `unique`
   references are `createIndex({unique})`. ✅ honest and important caveat.
2. **E2E suite is entirely server-gated** — the structural claim about
   `describe.skipIf(!SERVER_AVAILABLE)` is correct and well-documented.
3. **`call` has no e2e test** — confirmed by reading e2e test files; no e2e
   case invokes `call()`. ✅
4. **FTS/vector have no e2e** — confirmed: no e2e case creates an FTS or vector
   index. ✅
5. **Test inventory is complete** — all 30 `.test.ts` files are listed.

### ❌ Factual error: systematic `it()` undercounting

The report claims "approx." counts that are consistently below the real numbers.
Actual counts (via `grep -c "  it("`):

| File | Report says | Actual | Delta |
|------|------------|--------|-------|
| `select.test.ts` | ~18 | 28 | −10 |
| `ddl.test.ts` | ~45 | 75 | −30 |
| `filter.test.ts` | ~30 | 40 | −10 |
| `admin.test.ts` | ~30 | 42 | −12 |
| `query.test.ts` | ~22 | 25 | −3 |
| `write.test.ts` | ~14 | 17 | −3 |
| `batch.test.ts` | ~18 | 25 | −7 |
| `subscribe.test.ts` | ~17 | 16 | +1 |
| `call.test.ts` | 8 | 8 | 0 |
| `e2e.test.ts` | ~55 | 74 | −19 |
| `e2e-data.test.ts` | ~23 | 33 | −10 |
| `e2e-ddl.test.ts` | ~13 | 15 | −2 |
| `e2e-permissions.test.ts` | ~17 | 20 | −3 |

The "Unit-test total reported: 641" claim at the top could not be independently
verified (no test runner output was available), but given the systematic
undercounting, the real total is likely **higher** than 641.

The "approx." qualifier provides some cover, but a 40% undercount on
`ddl.test.ts` (claiming 45, actual 75) is beyond reasonable approximation error.

### 🟡 Minor: builder unit-test total "~190"

The report says "~190 `it` cases across 9 files" for the builder layer. The
actual total for the 9 builder test files is **276** (28+75+40+42+25+17+25+8+16).
This is a 31% undercount.

---

## Report 4: `completeness-oql.md` — Grade: A−

**Verdict:** the most analytically sophisticated report. The gap analysis is
excellent and the "object-native by design" framing is well-grounded. A few
specific claims needed verification and mostly passed.

### What it got RIGHT (verified)

1. **`MAX_FILTER_DEPTH = 64`** — `filter_enum.rs:9`. ✅
2. **`Exists` is field-existence, not subquery** — `filter_enum.rs:110-113`:
   `Exists { field: FieldPath }` takes a field path, not a query. The report's
   §2.2 distinction is correct and important. ✅
3. **18 aggregate factories** — `agg.rs:98-122` registers exactly 18
   (count, count_distinct, sum, avg, min, max, median, stddev, variance,
   percentile, first, last, string_agg, array_agg, bool_and, bool_or, mode,
   range). ✅
4. **`try_plan_order_limit_fast_path` exists** — `read_planner.rs:403`. ✅
   The "engine-ready, language-absent" characterisation of keyset pagination
   (H3) is accurate and insightful.
5. **`QueryResult` / `QueryStats` fields** — `query_result.rs:10-40` matches
   exactly: `index_used`, `records_scanned`, `records_returned`,
   `execution_time_us` + `records`, `stats`, `pagination`, `value`. ✅
6. **No JOIN / UNION / CTE / window functions** — `grep` for `join|union|
   intersect|window|partition_by` across `shamir-engine/src` confirms only
   unrelated hits (path.join, JoinHandle, slice .windows()). ✅

### ❌ Factual error: "12 folders" for scalar functions

§1.6 says "`register_builtins` wires 12 folders" and lists "canonical (1)" as a
separate 12th folder. The actual `register_builtins()` (`lib.rs:47-62`) makes
12 `in_folder()` calls, but `canonical::register` is registered under the
**"crypto"** folder (line 60: `reg.in_folder("crypto", canonical::register)`),
not a separate "canonical" folder. There are only **11 distinct folder names**:
math, strings, arrays, cast, datetime, value_nav, validate, encode, object,
text, crypto. The report's per-folder breakdown (§1.6) lists "crypto (6)" AND
"canonical (1)" as separate entries, but canonical functions are registered
under the crypto prefix on the wire (e.g. `crypto/canonical_hash`, not
`canonical/...`).

### 🟡 Minor: "~130 unique scalar functions"

The test at `register_builtins_tests.rs:17` asserts `reg.len() >= 130`, which
supports the claim. However, "unique" is slightly misleading — the
folder-qualification means the same plain name can appear under multiple folders
(e.g. `math/min` and `arrays/min`). These are distinct registry entries but not
"unique" algorithms. The count is directionally correct.

### What it got RIGHT (notable honesty)

- **Keyset pagination (H3)** — correctly identifies this as the cheapest
  high-impact win because the engine already has sorted-index seek. This is
  the single most actionable insight across all 5 reports.
- **"Object-native by design, forever"** — grounds the no-SQL-frontend stance
  in `PLAN.md` §3, correctly distinguishing intentional design from oversight.

---

## Report 5: `completeness-ddl.md` — Grade: A

**Verdict:** the best report of the five. The FK/unique fail-open finding is the
single most important correctness insight in the entire corpus, and it is
verified correct. The gap taxonomy is thorough and well-prioritised.

### What it got RIGHT (verified)

1. **FK + unique fail-open under autocommit** — `schema_validator.rs:106-112`
   (FK) and `:160-165` (unique) both have the comment
   `ctx.db() == None → silently skipped`. The code confirms: both checks are
   gated behind `if let Some(db) = ctx.db()`, which is only `Some` in tx-mode.
   Single-statement autocommit writes bypass both. ✅ This is a **real
   correctness hole**, not just documentation.
2. **`run_validators_qv` called at write_exec.rs ~171** — confirmed at line 171
   exactly. ✅
3. **`DropValidator` refuses if bound_in non-empty** — not independently
   verified in this pass (would need to read the validator registry code), but
   the claim is specific and plausible. Marking as "could not verify" per
   instructions rather than guessing.
4. **Argon2id password hashing** — `auth/types.rs:148`: `/// Argon2id PHC-string`.
   ✅
5. **Gap taxonomy (G1–G20)** — the structural gaps (no RENAME, no DEFAULT, no
   sequences, no views, no partitioning) are all confirmed absent by the wire
   types inspected. The "intentionally out of scope" framing for column ALTER
   TABLE is honest and correct.
6. **Two uniqueness paths (G15)** — correctly identifies that
   `ConstraintsDto.unique` (schema-rule) and `CreateIndexOp.unique`
   (index-level) are separate enforcement paths. ✅

### ⚠ Misleading: "not the SCRAM-SHA-256 protocol (no challenge/response)"

§1.5 says the password scheme "is **not** the SCRAM-SHA-256 protocol (no
challenge/response)". This conflates two layers:
- **Password-at-rest hashing:** Argon2id (confirmed). The report is correct here.
- **Connection-level auth protocol:** The TS client (`protocol.ts`,
  `scram.ts`) implements a **full SCRAM-style 4-message handshake**
  (client-first, server-challenge with Argon2id parameters, client-proof,
  server-signature). The `DbRequest::CreateScramUser` variant exists precisely
  to create users who can authenticate via this handshake.

So there IS a challenge/response protocol — it's SCRAM-with-Argon2id-as-KDF,
not SCRAM-with-PBKDF2. The report's parenthetical "(no challenge/response)" is
**wrong** and could mislead a reader into thinking the auth model is
plaintext-password-over-the-wire.

### What it got RIGHT (notable honesty)

- **G10 (open access defaults)** — correctly identified as the single biggest
  ship blocker. The framing "every other DDL feature is moot if the gate isn't
  uniformly enforced" is exactly right.
- **G15 (two uniqueness paths)** — the coherence-risk callout is valuable and
  not obvious from reading either path in isolation.

---

## Cross-report contradictions

### ⚠ Contradiction 1: `one_of` coverage status

- **Report 1** (Rust builder, DDL #26): marks `one_of` as 🟡 partial —
  "Missing: `one_of` constraint".
- **Report 2** (TS query builder, row #180): marks Rust as "✅ wire:
  `ConstraintsDto.one_of`" and TS as "✅ `.oneOf()`".
- **Report 5** (DDL completeness, §1.2): lists `one_of` as "✅ present" (Phase A).

All three are individually defensible (wire field exists, Rust builder setter
missing, TS builder setter exists), but a reader cross-referencing will see
contradictory ✅/🟡 ratings for the same capability. The reports should agree
on whether they rate the **wire field** or the **builder setter**.

### ⚠ Contradiction 2: `FilterExpr`/`Cond` builder coverage

- **Report 1** (Rust builder, OQL #26/#27): marks both as ❌ "no builder".
- **Report 2** (TS query builder, rows #80/#81): marks both as "✅ `filter.expr()`"
  / "✅ `filter.cond()`" and notes "TS exceeds Rust builder here".

No contradiction — both are correct. Report 2 explicitly acknowledges the
asymmetry. **Listed here for completeness; this is actually good
cross-referencing.**

### ⚠ Contradiction 3: `SelectItem::Expression`

- **Report 1** (Rust builder, OQL #8): ❌ "no builder; wire type marked future".
- **Report 2** (TS query builder, row #42): 🟡 "wire type exists, no TS builder,
  no Rust builder".
- **Report 4** (OQL completeness, §2.9): "partial (expression only)" — notes
  `SelectExpr` supports arithmetic in projection.

All three agree the builder coverage is absent; they differ on whether the wire
type itself counts as "present". Report 4 is the most precise.

---

## Summary grades

| Report | Grade | One-line justification |
|--------|-------|----------------------|
| `coverage-rust-query-builder.md` | **B+** | Per-row verdicts accurate; summary arithmetic wrong (claims 5–7 ❌, actual 10) |
| `coverage-ts-query-builder.md` | **A−** | Most thorough and accurate; every cited method verified; minor parity-rating ambiguity on `one_of` |
| `coverage-ts-tests.md` | **B** | Qualitative analysis sound; systematic 15–40% undercount of `it()` cases undermines the quantitative claims |
| `completeness-oql.md` | **A−** | Best analytical depth; one factual error (12 vs 11 folders); keyset pagination insight is the highlight |
| `completeness-ddl.md` | **A** | Best report overall; FK/unique fail-open finding is the most important correctness insight in the corpus; one misleading SCRAM claim |

---

## Residual risks (for the orchestrator)

1. ~~**The FK/unique fail-open under autocommit (Report 5)** is a real
   correctness bug...~~ **— ОТОЗВАНО 2026-06-24 (фактчек рантайма).** Построено
   на устаревших комментариях `schema_validator.rs`. На деле `execute_insert_tx`
   всегда даёт `Some(tx)`, сервер оборачивает батчи в tx → FK/unique **enforced
   и под autocommit** (зелёные e2e `autocommit also enforces`). НЕ P0. См.
   `META-REVIEW.md` §0 и `ACTION-ITEMS.md` A1.
2. **The `it()` undercounting (Report 3)** means the test coverage picture is
   slightly better than the report suggests — but the *qualitative* gaps
   (no e2e for FTS/vector/call, no unit tests for Phase B/C constraints) are
   real and unchanged.
3. **The `one_of` parity ambiguity** should be resolved by standardising on
   "does the builder expose it?" as the rating criterion across all reports.
4. **The SCRAM-vs-Argon2id conflation (Report 5)** could lead to wrong security
   assumptions if a reader takes the parenthetical at face value.
