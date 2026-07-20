# Built-in function library — design & implementation plan

**Status:** design / proposed. Sibling of `FUNCTIONS.md` (the WASM *engine* /
procedural layer). This doc is the **built-in catalogue** of scalar + aggregate
functions over numbers, strings, arrays, columns, geo, time, crypto.

## Three execution shapes (do NOT conflate)

| Shape | Layer | Cardinality | Indexable? | Today |
|---|---|---|---|---|
| **Scalar / row** | scalar registry → `IndexExpr` | 1 record → 1 value | **yes** (pure) | Lower/Upper/Trim/Length/Substring/ValuePath/Concat/Mod/Coalesce |
| **Aggregate / column** | `AggregateFn` + GROUP BY | N records → 1 value | n/a | Count/Sum/Avg/Min/Max |
| **Procedural** | `ShamirFunction` (WASM + builtins) | side-effects / DB / non-deterministic | no | argon2id |

The **folder catalogue (#118)** is the unifying *namespace + discovery* over all
three; execution stays specialised (and that's the point — scalars stay
indexable, aggregates stream, procedural stay sandboxed).

## Core architecture — one scalar registry, three consumers

```
ScalarFn  = fn(&[InnerValue]) -> Result<InnerValue, ScalarError>
ScalarReg = name → { f: ScalarFn, arity, pure: bool, deterministic: bool, arg/ret type hints }
```

`InnerValue` already gives us a rich value set — `Int / Dec(Decimal) /
Big(BigInt) / Str / Bin(bytes) / Bool / List / Set / Map` — so decimal math
uses `Dec`, crypto returns `Bin`, etc. No new value type needed.

The **one** registry is consumed by three callers:
1. **Query expressions** (SELECT / WHERE computed) — inline eval.
2. **Functional indexes** — add a single generic node `IndexExpr::Call { name,
   args: Vec<IndexExpr> }` that dispatches to the registry. A `Call` to a
   `pure + deterministic` fn is indexable (so `lower(email)`, `geohash(pt)` get
   O(log n) lookups). Existing specialised `IndexExpr` variants keep working;
   new functions arrive purely as registry entries reached via `Call`.
3. **WASM functions / validators** — the same registry is exposed through
   `FnCtx`, so a WASM validator can call any library fn (regex, hash, email
   check) — see VALIDATORS.md.

Aggregates are **stateful** (N rows → 1 value), so they get their **own
registry in the same `shamir-funclib` crate** — a peer to the scalar registry,
not a scalar:

```
trait Aggregator {                       // one instance per group
    fn accumulate(&mut self, v: &InnerValue) -> Result<(), ScalarError>;
    fn finalize(self: Box<Self>)          -> Result<InnerValue, ScalarError>;
    fn merge(&mut self, other: Box<dyn Aggregator>) {}   // optional, for partial/parallel aggregation
}
AggRegistry = name → factory(params) -> Box<dyn Aggregator>
```

So the crate hosts **two registries** — scalar (`Fn(&[InnerValue])`) and
aggregate (`Aggregator` accumulators) — both over `InnerValue`, both
unit-testable in isolation (feed a sequence → finalize → assert). The engine's
GROUP BY consumes the aggregate registry by name (extending today's
`AggregateFn`), exactly as functional indexes consume the scalar registry.

MVP `/agg` set: count, count_distinct, sum, avg, min, max, median, stddev,
variance, percentile(p), first, last, string_agg(sep), array_agg, bool_and,
bool_or, mode, range. (count/sum/avg/min/max already exist in the engine — the
registry supersedes/extends them.)

Procedural functions (random / uuid / argon2 / asym / PQC) stay in the existing
`ShamirFunction` registry — not in this crate.

## Categories

**Core (the base library):**

| Folder | Shape | Examples |
|---|---|---|
| `/math` | scalar | abs, ceil, floor, round, trunc, sign, neg, pow, sqrt, exp, ln, log, mod, clamp, **min/max (n-ary, between values)**, between(x,lo,hi)→bool |
| `/str` | scalar | lower, upper, trim/ltrim/rtrim, length, byte_length, substring, concat, replace, split, starts_with, ends_with, contains, index_of, repeat, reverse, pad_left/right |
| `/str` regex family | scalar | is_reg_match(s,pat)→bool, reg_query(s,pat)→first match/capture, reg_query_all→array, reg_captures→groups, reg_replace(s,pat,repl with $1), reg_split→array, reg_count→int, reg_find_index→int |
| `/array` | scalar | length, get, slice, contains, index_of, first, last, flatten, distinct, sort, join, sum/min/max/avg-over-elements |
| `/cast` | scalar | to_int, to_float, to_dec, to_string, to_bool, parse_int, parse_float, try_cast |
| `/datetime` | scalar | now (unixtime), parse_rfc3339, format_rfc3339, parse(s,pattern), format(ts,pattern), year, month, day, hour, minute, second, weekday, is_weekend, add/sub(secs/days), diff_secs, start_of(day/week/month), truncate(unit), to_epoch_ms/s, from_epoch_ms/s, age |
| `/value` | scalar | get/path, array_length, keys, type, exists |
| `/validate` | scalar (→ pairs with #142) | is_email, is_url, is_uuid, is_ipv4/v6, is_phone, luhn (card), in_range, matches(regex), is_value, is_empty, len_between |
| `/encode` | scalar | base64_enc/dec, base64url, hex_enc/dec, base32, url_encode/decode, html_escape, value_escape |
| `/object` | scalar | keys, values, entries, has_key, get_path, merge, pick, omit |
| `/text` | scalar | normalize(NFC/NFKC), slugify, levenshtein/edit_distance, jaro_winkler, word_count, truncate_ellipsis |
| `/id` | procedural | uuid_v4, uuid_v7, ulid, nanoid |
| `/agg` | aggregate | count, count_distinct, sum, avg, min, max, median, stddev, variance, percentile(p), first, last, string_agg(sep), array_agg, bool_and/or |
| `/geo` | scalar+agg | distance(haversine), bearing, within_radius, geohash(→indexable); centroid, bbox (aggregate) |
| `/crypto` | split | **pure (scalar):** sha256, sha512, sha3, blake3, hmac, ct_eq, **signature `verify(pubkey, msg, sig)`** (deterministic → indexable/validator-friendly) · **procedural:** argon2id (done), random_bytes, random_int, **keygen / sign (secret-grants)**, KEM encapsulate/decapsulate |
| `/crypto/asym` | split | classical: ed25519 (sign/verify — we already use Ed25519 for server identity), x25519 (ECDH), ecdsa_p256, rsa_pss · **see PQC below** |

**Secondary (later, as needed):** `/cond` (if/case, nullif, coalesce, is_null,
default, greatest/least), `/bit` (and/or/xor/not/shift, popcount), `/net`
(ip parse, cidr contains, parse_url, domain_of_email), `/checksum` (non-crypto
bucketing/sharding — crc32, fnv, murmur), `/type` (type_of, is_number/string/
array, coerce), `/random` (random_int/float/choice/shuffle — procedural).
`/regex` folds into `/str`.

**Bigger future (own design, NOT this library):** *window functions*
(row_number, rank, lag, lead, running aggregates over ordered partitions — a
distinct execution shape, neither pure-scalar nor simple-aggregate);
*approximate aggregates* (HyperLogLog distinct-count, t-digest percentiles);
*statistical 2-arg aggregates* (covariance, correlation, histogram);
full *timezone* support for `/datetime` (MVP is UTC + epoch).

### Asymmetric & post-quantum crypto (`/crypto/asym`, `/crypto/pqc`)

Officially standardised PQC (NIST FIPS, Aug 2024 + selections through 2025) —
this is what to target:

| Standard | Algorithm (was) | Role |
|---|---|---|
| **FIPS 203** | **ML-KEM** (CRYSTALS-Kyber) | key encapsulation (KEM) — primary |
| **FIPS 204** | **ML-DSA** (CRYSTALS-Dilithium) | signatures — primary |
| **FIPS 205** | **SLH-DSA** (SPHINCS+) | hash-based signatures (conservative, larger) |
| **FIPS 206** | **FN-DSA** (Falcon) | compact lattice signatures (finalising) |
| selected 2025 | **HQC** | backup KEM (code-based, different math than ML-KEM) — std forthcoming |
| SP 800-208 | **LMS / XMSS** (HSS, XMSS^MT) | stateful hash signatures (firmware-signing niche) |

Library exposure (purity split holds):
- **`verify(pubkey, msg, sig) -> bool`** for ML-DSA / SLH-DSA / Ed25519 is
  **pure + deterministic** → scalar registry, **indexable**, and a perfect
  **validator** primitive: sign a record's payload, store the sig in a field, a
  validator verifies it on every write → tamper-evident rows.
- **sign / keygen / KEM encapsulate-decapsulate** touch secret material +
  randomness → **procedural** (`ShamirFunction`), using the existing
  **secret-grants** in `FnCtx` to fetch private keys from the secret store.
- Hybrid (classical X25519 + ML-KEM) is the deploy-grade pattern for the future
  P2P / Interconnected layer (the "I" in S.H.A.M.I.R.).

Caveat: the Rust PQC impls (RustCrypto `ml-kem`/`ml-dsa`/`slh-dsa`, or
`pqcrypto` wrappers) are maturing + non-trivial deps → **PQC is its own phase**
(after the base library lands), gated behind a cargo feature so the lean core
isn't forced to carry it.

### Parallel families (symmetry)

Reduction / comparison functions must exist in **every applicable shape**, with
the same semantics, disambiguated by folder (no name collision):

| Family | scalar over args (`/math`) | scalar over an array (`/array`) | aggregate over rows (`/agg`) |
|---|---|---|---|
| min / max | `min(a,b,c)` | `array.min/max` | `agg.min/max(col)` |
| sum / avg | — | `array.sum/avg` | `agg.sum/avg(col)` |
| count | — | `array.length` | `agg.count` |

`clamp`, `between`, `greatest`/`least` are scalar comparison helpers. The rule:
**any convenient comparison/reduction a user wants between a few values is
available as a scalar, and over many rows as an aggregate** — same names, folder
tells which.

### Key conventions to settle
- **datetime:** canonical timestamp = `Int` epoch-millis (UTC); parse/format
  bridge RFC3339 strings. (Avoids a new value type; revisit if a tagged
  temporal type is wanted.)
- **crypto purity split:** hashing/hmac/encode of inputs are **pure**
  (deterministic → scalar registry, even indexable); randomness / uuid / argon2
  are **procedural** (non-deterministic → `ShamirFunction`, never indexed).
- **geo point:** `{lat, lng}` object vs `[lng, lat]` array — pick one before
  `/geo` (Phase 5).
- **regex:** use Rust's `regex` crate (RE2-style, **linear-time → ReDoS-safe**,
  so user/wire-supplied patterns can't catastrophically backtrack) + an
  LRU **compile cache** keyed by pattern (compilation is the cost; matching is
  cheap). A `reg_*` family is one consistent naming set.

## Implementation plan (phased)

- **P0 — scalar infra (foundation).** `ScalarFn` + `ScalarRegistry` + metadata +
  `ScalarError`; `IndexExpr::Call { name, args }` dispatch; wire the registry
  into filter/expr eval, functional-index build, and `FnCtx`. Land 2-3 fns
  end-to-end (e.g. `/math/abs`, `/str/lower` via Call) to prove the path +
  indexability. **Everything after is just populating registries.**
- **P1 — /math + /str + /array + /cast** (pure scalars; per-fn tests).
- **P2 — /datetime + /value** (settle the timestamp convention).
- **P3 — /agg** (in `shamir-funclib`): the `Aggregator` trait + `AggRegistry` +
  the MVP aggregators (count, count_distinct, sum, avg, min, max, median,
  stddev, variance, percentile, first, last, string_agg, array_agg, bool_and/or,
  mode, range) + per-aggregator tests. Built as ONE cohesive module after the
  scalar phase. Engine GROUP BY wiring (consume the registry, extend the wire
  `AggregateFn`) is a separate integration step alongside #143.
- **P4 — /crypto (symmetric)** — pure hashes/hmac/encode → scalar registry;
  random/uuid/argon2id → `ShamirFunction`.
- **P5 — /geo** (point convention + haversine + geohash-index + centroid/bbox).
- **P6 — catalogue + folders (#118)** over the populated registries: slash-path
  addressing, listing, system-reserved immutable namespaces (`/math`, … like
  `/usr/bin`), user folders for WASM.
- **P7 — secondary** (/cond, /bit, /net, /checksum, /type, /random) as demand appears.
- **P8 — /crypto/asym + /crypto/pqc** (feature-gated): classical Ed25519/X25519/
  ECDSA/RSA-PSS + PQC ML-KEM (FIPS 203) / ML-DSA (FIPS 204) / SLH-DSA (FIPS 205).
  `verify` → pure scalar (validator-friendly); sign/keygen/KEM → procedural with
  secret-grants. Lands after the base library; deps behind a cargo feature.

Then **validators (#142)** sit on top — they call library functions via `FnCtx`.

Each phase: precise prompt → `ao46m` → zero-trust verify (read diff + re-run the
gate) → commit. Folders (P6) come **after** the library is populated (P1-P5), so
they wrap a real catalogue, not an empty shell.
