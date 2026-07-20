# Documentation Accuracy 6f — fix phantom/wrong function names in 05-functions.md's funclib table

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Sixth item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09. `docs/guide-docs/guide/05-functions.md` §8
("Встроенная библиотека (funclib)", lines ~326-344) has a category table
listing example function names. This is a DOCS-ONLY fix — the task is to
make the table match the real funclib registry exactly, not to implement
any missing function.

## Ground truth — the real funclib registry (verified exhaustively by
grepping every `reg.register("...")` call across `crates/shamir-funclib/src/`)

The canonical folder-prefix wiring is in `crates/shamir-funclib/src/lib.rs`
(`reg.in_folder("<folder>", <module>::register)` calls, ~lines 51-64) plus
`agg::register` for aggregates (not folder-prefixed the same way — check
`agg.rs` yourself for the aggregate names, they're used in `GROUP BY`, not
via `$fn`). The REAL folders and a representative sample of their
functions (NOT exhaustive — ~150 total functions exist; pick good, real
examples for the doc, don't try to list all of them):

| Real folder | Sample real functions (verified via `reg.register(...)` in the module) |
|---|---|
| `math/` | `abs`, `ceil`, `floor`, `round`, `trunc`, `sign`, `neg`, `pow`, `sqrt`, `exp`, `ln`, `log`, `mod`, `clamp`, `min`, `max`, **`between`** |
| `null/` | `coalesce`, `if_null`, `nullif`, `is_null` — **entire folder missing from the doc's table** (added this campaign's task 4a) |
| `strings/` | `lower`, `upper`, `trim`, `ltrim`, `rtrim`, `length`, `byte_length`, `substring`, `concat`, `replace`, `split`, `starts_with`, `ends_with`, `contains`, `index_of`, `repeat`, `reverse`, `pad_left`, `pad_right`, `is_reg_match`, `reg_query`, `reg_query_all`, `reg_captures`, `reg_replace`, `reg_split`, `reg_count`, `reg_find_index` |
| `arrays/` | `length`, `get`, `slice`, `contains`, `index_of`, `first`, `last`, `flatten`, `distinct`, `sort`, `sort_desc`, `join`, `sum`, `min`, `max`, `avg` |
| `cast/` | `to_int`, `to_float`, `to_dec`, **`to_string`** (NOT `to_str`), `to_bool`, `parse_int`, `parse_float`, `try_cast` |
| `datetime/` | `now`, `age`, `to_epoch_s`, `to_epoch_ms`, `from_epoch_s`, `from_epoch_ms`, `parse_rfc3339`, `format_rfc3339`, `parse`, `format`, `year`, `month`, `day`, `hour`, `minute`, `second`, `weekday`, `is_weekend`, `add_secs`, `add_days`, `diff_secs`, `start_of_day`, `start_of_week`, `start_of_month`, `truncate` |
| `value_nav/` | `get_path`, `array_length`, `keys`, `type_of`, `exists` — **the doc's row is named `value/` (WRONG folder name) and lists `parse`/`stringify`/`get_path` — `parse`/`stringify` do NOT exist anywhere in this folder** (the closest real analogues for JSON parse/stringify are `encode/parse_json` / `encode/to_json`, a DIFFERENT folder — see below) |
| `validate/` | `is_email`, `is_url`, `is_uuid`, `is_ipv4`, `is_ipv6`, `is_phone`, `luhn`, `in_range`, `matches`, `is_json`, `is_empty`, `len_between` |
| `encode/` | `base64_enc`, `base64_dec`, `base64url_enc`, `base64url_dec`, `hex_enc`, `hex_dec`, `base32_enc`, `base32_dec`, `url_encode`, `url_decode`, `html_escape`, `json_escape`, `to_json`, `parse_json` — **entire folder missing from the doc's table** (`to_json`/`parse_json` added this campaign's task 4f) |
| `gen/` | `uuid_v4`, `random`, `random_bytes` — **entire folder missing from the doc's table** (added this campaign's task 4d) |
| `object/` | `keys`, `values`, `entries`, `has_key`, `get_path`, `merge`, `pick`, `omit` — **entire folder missing from the doc's table** |
| `text/` | `normalize_nfc`, `normalize_nfkc`, `slugify` (NOT `slug`), `levenshtein`, `jaro_winkler`, `word_count`, `truncate_ellipsis` — **the doc's examples `slug`/`camel`/`snake` are WRONG: `slug` should be `slugify`, and `camel`/`snake` (case-conversion) DO NOT EXIST AT ALL** — this is genuinely unimplemented (a P1 backlog item from this campaign's earlier funclib gap analysis), not a naming mismatch |
| `crypto/` | `sha256` (NOT `hash_sha256`), `sha512`, `sha3_256`, `blake3`, `hmac_sha256`, `ct_eq`, `argon2id`, plus `canonical_hash` (from `canonical.rs`, also registered under the `crypto/` folder) |
| **`compare/`** | **This folder DOES NOT EXIST AT ALL.** `crates/shamir-funclib/src/compare.rs` is an internal cross-type comparator utility (`pub fn compare(a: &QueryValue, b: &QueryValue) -> Ordering`) used INTERNALLY by `min`/`max`/`sort`/`median`/`mode`/`percentile` — it is never registered as a callable `$fn` scalar folder anywhere in `lib.rs`. The doc's entire `compare/` row (`gt`, `lt`, `eq`, `between`) is phantom: `gt`/`lt`/`eq` aren't funclib functions at all (comparison in queries is done via filter operators `$gt`/`$lt`/`$eq` in `WHERE`, a completely different mechanism — see the doc's own earlier filter-operator sections), and `between` IS real but lives under **`math/`**, not `compare/`. |

Aggregates (used in `GROUP BY`, dispatched differently from `$fn` scalars
— check `crates/shamir-funclib/src/agg.rs` yourself for the full list):
`count`, `count_distinct`, `sum`, `avg`, `min`, `max`, `median`,
`percentile`, `string_agg`, and others — the doc's closing sentence
already mentions `median`/`percentile` correctly; verify the full
aggregate list yourself and confirm nothing else needs adding to that
one sentence (don't turn it into a giant table — the funclib category
table above is the main fix target).

## The task

1. Rewrite `05-functions.md`'s §8 category table (lines ~330-341) so
   every row uses the REAL folder name and REAL function names, verified
   against the ground truth above (re-verify yourself — grep the actual
   `crates/shamir-funclib/src/*.rs` files rather than trusting this
   brief's transcription blindly, per this campaign's standing practice).
2. Add the four missing folders (`null/`, `encode/`, `gen/`, `object/`)
   as new rows with a few representative real examples each.
3. Remove the phantom `compare/` row entirely — replace it with nothing,
   or if you think a one-line clarifying note is warranted ("comparison
   in filters uses `$gt`/`$lt`/`$eq` operators, not funclib functions —
   see [filter section]"), your call, but don't invent a `compare/`
   folder that doesn't exist.
4. Fix `text/`'s phantom `camel`/`snake` entries — since these are
   genuinely unimplemented (not just misnamed), either remove them from
   the example list (showing only real functions like `slugify`,
   `levenshtein`) or, if you think it reads better, keep a one-line note
   that case-conversion functions are a planned-but-not-yet-implemented
   gap (check whether this campaign's Этап 4 P1 backlog notes already
   track this — if so, just don't claim it exists; don't invent a
   roadmap reference if none exists).
5. Fix the two specific wrong-name bugs: `cast/to_str` → `cast/to_string`,
   `crypto/hash_sha256` → `crypto/sha256`.
6. Fix the wrong folder name `value/` → `value_nav/`, and its wrong
   example functions `parse`/`stringify` → real ones (`get_path`,
   `array_length`, `keys`, `type_of`, `exists` — pick 2-3 good examples).
7. Do a final sanity pass: for every function name you put in the
   rewritten table, confirm it's a genuine hit in a `reg.register("name"`
   call in the corresponding module file — don't leave any entry
   unverified.

## Out of scope

- Do NOT implement `text/camel` or `text/snake` (case-conversion) or any
  other genuinely-missing function — this is a docs-only accuracy fix,
  not a feature request. If you want to flag this gap for a future task,
  say so in your summary rather than adding code.
- Do NOT rewrite the rest of `05-functions.md` (the `#[shamir::function]`/
  `#[shamir::procedure]`/`#[shamir::scalar]`/`#[shamir::validator]`
  sections, folders-for-functions §7, lifecycle §9, etc.) — this brief
  is scoped to §8's category table only.
- Do NOT touch anything from the already-completed Этапы 1-5 or tasks
  6a-6e — this brief is scoped to this one table.

## Verification (MANDATORY before you report done)

- No `cargo test`/`clippy`/`fmt` gate applies (docs-only) — state this
  explicitly.
- For EVERY function name in your final table, report the exact grep
  command + hit you used to verify it's real (e.g. `grep -n
  'reg.register("slugify"' crates/shamir-funclib/src/text.rs`) — a
  simple list of "verified: yes" is not sufficient, show the evidence.
- Confirm the `compare/` row is gone and `between`'s real location
  (`math/`) is reflected if you chose to keep `between` as an example
  anywhere.
- Confirm you did NOT invent any function name that doesn't have a real
  `reg.register(...)` hit.
