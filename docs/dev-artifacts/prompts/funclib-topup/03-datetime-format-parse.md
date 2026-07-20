# Funclib top-up 4c — datetime/format(ts, pattern) + datetime/parse(s, pattern)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Third P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
per report 10 (`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~lines 46, 129, 155, 302):

> Datetime is UTC-epoch-only with RFC-3339-only formatting — no custom
> format/parse patterns... `datetime/format`/`datetime/parse` don't exist,
> despite being advertised in docs. This is the #1 datetime need for any
> report/projection; chrono is already a dependency.

## Investigation already done (verify yourself too)

`crates/shamir-funclib/src/datetime.rs` (read the WHOLE file first — it's
short) already establishes every convention you need:

- Canonical timestamp representation: `i64` epoch-millis UTC (module doc
  comment, ~lines 1-20).
- `to_dt(ms: i64) -> Result<DateTime<Utc>, ScalarError>` (~lines 28-34) —
  the existing epoch→`DateTime<Utc>` conversion helper, reused by every
  other function via `dt_arg(a, i)` (~lines 36-39). Use it for `format`
  too.
- `parse_rfc3339`/`format_rfc3339` (~lines 99-122) are the closest existing
  precedent — `format_rfc3339` does `dt_arg(a, 0)?.to_rfc3339()`;
  `parse_rfc3339` does `DateTime::parse_from_rfc3339(s).map_err(|_| "parse")?
  .with_timezone(&Utc)`. Your new `format`/`parse` are the strftime-pattern
  generalization of these two.
- Error code convention: `"out_of_range"` for un-representable timestamps,
  `"parse"` for unparseable input (module doc comment, ~line 18-20) — reuse
  `"parse"` for both a bad pattern string AND an unparseable input string in
  the new `parse` function (see the panic-safety note below for why the
  pattern itself needs its own validation step, distinct from the input
  string failing to match).

## ⚠️ chrono footgun — read this before writing `format`

`chrono::DateTime::format(pattern)` does **NOT** validate `pattern` eagerly
— it returns a lazy `DelayedFormat` that only interprets the format
specifiers when actually displayed (`.to_string()` / `write!`), and an
**unknown or malformed specifier in `pattern` causes chrono to panic at
that point**, not return an `Err`. Since `pattern` here is caller-supplied
(a filter/projection argument, effectively untrusted input from the
engine's perspective), calling `.format(pattern).to_string()` directly
would let a malformed pattern string panic the whole process — this is
exactly the kind of bug this cleanup campaign has been fixing (see task
3e's checked-arithmetic fixes for the same "untrusted input must not panic"
principle, just for date-format strings instead of numeric overflow).

**Required fix**: validate `pattern` BEFORE formatting, using
`chrono::format::StrftimeItems::new(pattern)` — this produces an iterator
of parsed format items WITHOUT panicking, and includes an `Item::Error`
variant for any invalid specifier it encounters. Scan the iterator first;
if any item is `Item::Error`, return `Err(ScalarError::new("parse"))` (or a
more specific code like `"bad_pattern"` if you prefer — pick whichever is
more consistent with this file's existing two-error-code convention,
document your choice) BEFORE calling `.format(pattern)` for real. Only
after confirming every item parsed cleanly is it safe to call
`dt.format(pattern).to_string()`.

## The task

Add two functions to `crates/shamir-funclib/src/datetime.rs`, mirroring
`parse_rfc3339`/`format_rfc3339`'s registration shape exactly:

1. **`format(ts, pattern)`** — 2 args: `ts` (canonical epoch-millis `Int`),
   `pattern` (strftime-style format string, e.g. `"%Y-%m-%d %H:%M:%S"`).
   Validates `pattern` via `StrftimeItems` first (see above), then returns
   `dt.format(pattern).to_string()` as a `Str`.
2. **`parse(s, pattern)`** — 2 args: `s` (the string to parse), `pattern`
   (the same strftime-style format string). Use
   `chrono::NaiveDateTime::parse_from_str(s, pattern)` first; if that fails
   (common case: a date-only pattern like `"%Y-%m-%d"` with no time
   component — `NaiveDateTime::parse_from_str` requires the pattern to
   fully specify a datetime), fall back to
   `chrono::NaiveDate::parse_from_str(s, pattern)` and assume midnight
   (`.and_hms_opt(0, 0, 0)` or the current chrono API's equivalent — check
   what's available in this workspace's pinned chrono version, `grep
   chrono = crates/shamir-funclib/Cargo.toml` and the workspace lockfile).
   Both attempts failing → `Err(ScalarError::new("parse"))`. Treat the
   successfully-parsed `NaiveDateTime` as UTC (matching this module's
   documented "canonical timestamp is UTC" convention — there is no
   timezone-conversion feature in this file yet, don't add one), convert
   to epoch-millis via `.and_utc().timestamp_millis()` (or whatever the
   pinned chrono version's equivalent conversion path is), and return as
   `Int`.

Update:
- The module's own doc comment (~lines 4-8, the function list) to include
  `format parse`.
- `register()` to add both, positioned near `parse_rfc3339`/
  `format_rfc3339` (logically grouped).
- The workspace's function-inventory documentation if it lists `datetime/`
  functions explicitly and would now be stale (check
  `docs/dev-artifacts/roadmap/FUNCTION_LIBRARY.md` and any `05-functions.md`-
  style doc under `docs/` — if updating docs is a rabbit hole beyond a
  one-line addition, skip it and note that in your summary; a full
  documentation pass is Этап 6's job, not this brief's).

## Tests

1. `format(ts, "%Y-%m-%d")` on a known epoch-millis value returns the
   correct date string (pick a value you can hand-verify, e.g. a
   well-known UTC timestamp).
2. `format` with a MALFORMED pattern (e.g. containing an unrecognized `%`
   specifier) returns `Err`, does NOT panic — this is the test that would
   fail (by panicking, not by a clean assertion failure) if the
   `StrftimeItems` pre-validation step above were skipped. Make this test
   explicit about what it's proving.
3. `parse("2024-03-15", "%Y-%m-%d")` → correct epoch-millis for midnight
   UTC on that date.
4. `parse("2024-03-15 10:30:00", "%Y-%m-%d %H:%M:%S")` → correct
   epoch-millis including the time component.
5. `parse` with a string that doesn't match the pattern → `Err`, not a
   panic or garbage value.
6. Round-trip: `parse(format(ts, pattern), pattern) == ts` for a pattern
   with full date+time precision (millisecond-level precision may not
   round-trip through second-granularity patterns — pick a pattern/value
   pair where round-tripping is actually expected to hold, and say so in
   the test's comment).
7. Regression: `parse_rfc3339`/`format_rfc3339` and every other existing
   `datetime.rs` test continues to pass unchanged.

## Out of scope

- Do NOT add timezone-conversion support (this file is UTC-only by
  documented design; adding TZ support is a bigger feature tracked
  separately, if at all).
- Do NOT touch any OTHER Этап 4 P0 item (null functions — done in 4a; agg
  wire params/distinct — done in 4b; uuid_v4, arrays/sort, parse_json/
  to_json — separate later leaf tasks).
- Do NOT add `add_months`/`add_years`/`diff_days`/calendar-arithmetic
  functions — those are Этап 4's P1 tier, a separate, capacity-contingent
  decision made later.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-funclib --full` green, including all new
  tests — in particular, confirm the malformed-pattern test does NOT crash
  the test binary (a panic inside a `#[test]` fn is caught by the test
  harness as a failure, which is fine and expected to happen ONLY if you
  reintroduce the footgun — the test itself must assert `Err`, not merely
  rely on the harness catching a panic).
- `cargo fmt --all -- --check` clean (or scoped to `shamir-funclib`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: the malformed-pattern case is caught by
  `StrftimeItems` validation BEFORE `.format()` is called for real, not by
  accident/coincidence — walk through exactly which line prevents the
  panic.
