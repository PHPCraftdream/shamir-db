# `shamir-lang`: in-process language design

**Status:** proposed PROC-2 design; implementation and all proposed tests remain unverified. This specification replaces the earlier incomplete draft and makes the grammar, runtime, compiler boundary, and staged capabilities implementable. Facts below are cited to repository paths; choices are explicitly marked as design decisions.

## 1. Surface syntax and complete grammar

**Decision:** a statically typed, expression-oriented subset with ASCII identifiers, UTF-8 strings, explicit types, braces, required semicolons, `@entry`, and postfix optional types (`T?`). It is not JavaScript or Rust. Postfix optional syntax avoids confusing `? T` with prefix unary operators and matches common typed-language notation. Function signatures always spell a return type.

Only an `@entry` declaration carries a kind keyword (`scalar`, `validator`, `procedure`); every other function is an `fn` helper, so an unmarked `scalar`/`validator`/`procedure` declaration is a syntax error (`S001`, expected `fn` or `@entry`), enforced by the grammar itself. A module has exactly one `@entry`. The lexer consumes the longest valid token; EBNF ranges use inclusive code point ranges and `ε` means empty.

```ebnf
module             = { function_decl }, EOF ;
function_decl      = entry_decl | helper_decl ;
entry_decl         = "@entry", function_kind, identifier,
                     "(", [ parameter_list ], ")", "->", type, block ;
helper_decl        = "fn", identifier,
                     "(", [ parameter_list ], ")", "->", type, block ;
function_kind      = "scalar" | "validator" | "procedure" ;
parameter_list     = parameter, { ",", parameter }, [ "," ] ;
parameter          = identifier, ":", type ;
type               = primitive_type, [ "?" ]
                   | "[", type, "]", [ "?" ]
                   | record_type, [ "?" ] ;
primitive_type     = "i32" | "i64" | "f64" | "bool" | "string" | "bytes"
                   | "null" | "Validation" | "Batch" | "Request" | "Response" ;
record_type        = "{", [ type_field, { ",", type_field }, [ "," ] ], "}" ;
type_field         = identifier, ":", type ;

block              = "{", { statement }, "}" ;
statement          = variable_decl | assignment | if_stmt | while_stmt
                   | range_for | list_for | return_stmt | break_stmt
                   | continue_stmt | throw_stmt | expression_stmt | block ;
variable_decl      = ( "let" | "const" ), identifier, [ ":", type ], "=", expression, ";" ;
assignment         = lvalue, "=", expression, ";" ;
lvalue             = identifier | postfix, ".", identifier | postfix, "[", expression, "]" ;
if_stmt            = "if", "(", expression, ")", block,
                     [ "else", block | "else", if_stmt ] ;
while_stmt         = "while", "(", expression, ")", block ;
range_for          = "for", "(", identifier, "in", expression, "..", expression, ")", block ;
list_for           = "for", "(", identifier, "in", expression, ")", block ;
return_stmt        = "return", [ expression ], ";" ;
break_stmt         = "break", ";" ;
continue_stmt      = "continue", ";" ;
throw_stmt         = "throw", expression, ";" ;
expression_stmt    = expression, ";" ;

expression         = conditional ;
conditional        = logical_or, [ "?", expression, ":", conditional ] ;
logical_or         = logical_and, { "||", logical_and } ;
logical_and        = equality, { "&&", equality } ;
equality           = comparison, { ( "==" | "!=" ), comparison } ;
comparison         = additive, { ( "<" | "<=" | ">" | ">=" ), additive } ;
additive           = multiplicative, { ( "+" | "-" ), multiplicative } ;
multiplicative     = unary, { ( "*" | "/" | "%" ), unary } ;
unary              = ( "!" | "-" | "+" ), unary | postfix ;
postfix            = primary, { call_suffix | field_suffix | index_suffix } ;
call_suffix        = "(", [ argument_list ], ")" ;
field_suffix       = ".", identifier ;
index_suffix       = "[", expression, "]" ;
argument_list      = expression, { ",", expression }, [ "," ] ;
primary            = identifier | integer | float | string | bytes | boolean
                   | "null" | "(", expression, ")" | list_literal | record_literal ;
boolean            = "true" | "false" ;
list_literal       = "[", [ argument_list ], "]" ;
record_literal     = "{", [ field_value, { ",", field_value }, [ "," ] ], "}" ;
field_value        = identifier, ":", expression ;

integer            = decimal_integer | hex_integer ;
decimal_integer    = ( "0" | nonzero_digit, { digit } ), [ integer_suffix ] ;
hex_integer        = "0", ( "x" | "X" ), hex_digit, { hex_digit }, [ integer_suffix ] ;
integer_suffix     = "i32" | "i64" ;
float              = digits, ".", digits, [ exponent ], [ "f64" ]
                   | digits, exponent, [ "f64" ] ;
exponent           = ( "e" | "E" ), [ "+" | "-" ], digits ;
digits             = digit, { digit } ;
nonzero_digit      = "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
digit              = "0" | nonzero_digit ;
hex_digit          = digit | "a" | "b" | "c" | "d" | "e" | "f"
                   | "A" | "B" | "C" | "D" | "E" | "F" ;
letter             = "A".."Z" | "a".."z" ;
identifier         = ( letter | "_" ), { letter | digit | "_" } ;
string             = '"', { escaped_scalar | non_quote_scalar }, '"' ;
bytes              = 'b"', { byte_escape | byte }, '"' ;
escaped_scalar     = "\\", ( '"' | "\\" | "/" | "b" | "f" | "n" | "r" | "t"
                   | "u{", hex_digit, { hex_digit }, "}" ) ;
non_quote_scalar   = any_scalar - ( '"' | "\\" | newline ) ;
byte_escape        = "\\", ( '"' | "\\" | "n" | "r" | "t" | "0"
                   | "x", hex_digit, hex_digit ) ;
byte               = any_scalar where UTF-8 encoding has exactly one byte
                   and the scalar is not '"', "\\", or newline ;
any_scalar         = any Unicode scalar value except surrogate code points ;
newline            = U+000A | U+000D | U+000D U+000A ;
line_comment       = "//", { any_scalar - newline }, [ newline ] ;
block_comment      = "/*", { any_scalar except the first closing "*/" }, "*/" ;
whitespace         = whitespace_char, { whitespace_char } ;
whitespace_char    = U+0009 | U+000A | U+000D | U+0020 ;
```

Integer syntax forbids leading zeroes except `0`; for floats this restriction applies only to the integer part, while fractional and exponent digit sequences may contain leading zeroes. `\u{...}` contains one to six hex digits (leading zeroes allowed) and must encode one valid Unicode scalar. Raw U+0000..U+001F and U+007F are forbidden in strings and byte literals. CRLF is one newline; lone CR and LF are each one newline. Byte literals accept only one-byte unescaped scalars or listed byte escapes. Float literals that parse to NaN or infinity are rejected; runtime arithmetic may still produce non-finite f64 values.

**Reserved keywords:** `scalar`, `validator`, `procedure`, `fn`, `let`, `const`, `if`, `else`, `while`, `for`, `in`, `return`, `break`, `continue`, `throw`, `true`, `false`, `null`, and the primitive/opaque type names `i32`, `i64`, `f64`, `bool`, `string`, `bytes`, `Validation`, `Batch`, `Request`, `Response`. Builtin function names are also reserved throughout the module namespace; user declarations and bindings cannot shadow them. `args` is a reserved parameter name to avoid wrapper collision. `@entry` is a reserved annotation token, not an identifier.

| Precedence | Operators | Associativity |
|---|---|---|
| 1 (highest) | call, field, index | left |
| 2 | prefix `! - +` | right |
| 3 | `* / %` | left |
| 4 | `+ -` | left |
| 5 | `< <= > >=` | left; chaining rejected |
| 6 | `== !=` | left; chaining rejected |
| 7 | `&&` | left, short-circuit |
| 8 | `||` | left, short-circuit |
| 9 (lowest) | `?:` | right |

Assignment is a statement, not an expression. Parentheses group. Calls are only direct named calls: parser requires the call target be an identifier, so functions are not values and `f()()`/`(f)(x)` are rejected.

Simple statements, including `return`, `break`, `continue`, `throw`, declarations, assignments, and expression statements, end in `;`. `if`/`else`, `while`, and `for` require blocks. `else` accepts only a block or another `if`; it cannot directly contain an unbraced statement. A block itself may be a statement. `for (i in start..end)` is a half-open integer range: evaluate `start`, then `end`, exactly once, left-to-right, before entering the loop. For `for (i in start..end)`, range variable `i` is immutable and the half-open loop performs zero iterations when `start >= end`. Range bounds have identical i32 or i64 type; the loop increments by one, and continue executes that increment. Before increment, if current equals end minus one, exit instead; this prevents signed overflow at the upper bound. A list continue advances to the next element; a while continue rechecks its condition. Break exits only the innermost loop. Every condition is bool; operands and conditions evaluate left-to-right, with short-circuit logical operators. `for (item in list)` evaluates the list once, takes a lexical immutable snapshot/view, iterates in index order, and binds a fresh immutable item in a per-iteration scope; nested shadowing is legal. Lists are fixed-size immutable values, so iteration cannot be invalidated.

**Deliberately absent:** closures/function values, classes, modules/imports, generics, type inference for function signatures, dynamic types/`any`, exceptions/try-catch, `do` expressions, switch, mutation of record/list elements, comprehensions, macros, recursion, indirect calls, arbitrary pointer operations, implicit casts, and mutable aggregates. `throw` is terminal and does not unwind/catch; it raises a user-visible error.

## 2. Type system, flow analysis, and numeric semantics

**Decision:** types are structural; records have named fields, immutable values, and a declared record type contains exactly its declared fields at typed language boundaries. A dedicated wire projection permits ignoring database metadata fields when materializing a declared record. Lists are homogeneous and invariant. `T?` admits implicit injection from `T` and the `null` literal; no other implicit conversions exist. The null literal may also have expected type `null`. `is_null(value: T?) -> bool` is defined; it does not refine types. `unwrap(value: T?) -> T` accepts optional inputs and raises kind 2 if null.

Record literals require an expected structural record type; duplicate/unknown/missing fields reject, fields evaluate in UTF-8 key order. Structural assignment is exact; width subtyping is absent and explicit projection is required. `i32`, `i64`, and `f64` are primitive `type` productions; `T?` applies to any primitive, list, or record type. The metavariables `Scalar`, `ScalarRecord`, and `Record` used in section 4's intrinsic-signature tables are specification shorthand only, not type productions or source tokens. There is no public dynamic `Value` type and no dynamic type construct at any language entry point. The runtime may use internal tagged cells for heterogeneous msgpack decoding; that implementation detail is not an exposed language value or type.

### Typed boundaries and mapping

Scalar entry params and results are limited to wire-safe types: i32, i64, finite f64, bool, string, bytes, optional forms, lists and records recursively; validator entry result is Validation, and its two record params are optional. Procedures use the same scalar/list/record wire types, never opaque values at the external boundary.

| Language type | Accepted MessagePack | Result / failure |
|---|---|---|
| `i32` | signed/unsigned integer representable in i32 | exact signed i32; reject floats/out-of-range |
| `i64` | signed integer or unsigned integer <= `i64::MAX` | exact i64; reject floats and larger u64 |
| `f64` | msgpack float only | finite values accepted; reject NaN/infinity at input and output (kind 3); integer rejected |
| `bool` | bool | exact |
| `string` | string | require valid UTF-8 (decoder rejects invalid); binary is not string |
| `bytes` | binary | exact; string is not bytes |
| `null` | MessagePack nil | exact; also injects into an expected optional type |
| `T?` | T or null; absent map key is treated as null only for optional record/parameter fields | typed optional; required missing key is invalid argument |
| `[T]` | array/list | recursively decode homogeneous items, reject whole value on mismatch |
| structural record | string-key map | require every nonoptional declared field; optional omitted becomes null; extra fields ignored in boundary projection; duplicate keys rejected |
| `Validation` | not accepted as input | constructed only by validation builtins |

`Validation` is likewise opaque; it is allowed only as a validator result or internal validator-helper value, never a wire input, optional, list element, or record field. Reject `null?` and opaque optional types semantically, even though grammar admits the suffix.

`Batch`, `Request`, and `Response` are opaque stage-2 values. They cannot be optional, list elements, record fields, wire parameters, or generic entry values; only matching stage/effect builtins may produce/consume them. Functions may accept/return opaque values internally when their stage/effects permit it. `T?` admits implicit injection from `T` and `null` only; no numeric promotion.

Typed output rejects non-finite floats (kind 3); finite negative zero is normalized. Runtime decoding is bounded to 32 containers as required by the SDK. `+` is numeric only; list/string/bytes concatenation is absent. `len(string|bytes|list)` returns `i32`; string length is UTF-8 byte count. Empty list literals require an expected list type. There is no implicit numeric promotion. Integer literals default to i64; an explicit `i32` or `i64` suffix selects that type, and an expected type may select i32 when the unsuffixed literal is representable. No implicit conversion except injection `T -> T?` and `null -> T?`. Explicit conversions only: `i32 -> i64`, `i32 -> f64`, `i64 -> f64` via named cast builtins `to_i64(i32)->i64`, `to_f64_i32(i32)->f64`, `to_f64_i64(i64)->f64`; they are not implicit and i64 conversion may round. Reverse casts are checked builtins `to_i32(i64)->i32`, `to_i32_f64(f64)->i32`, `to_i64_f64(f64)->i64`; reject nonintegral, NaN, infinities, and values outside [-2^31,2^31) and [-2^63,2^63), with exclusive upper bounds. The exact negative MIN literal is accepted in unary context. Numeric `+` and `-` are supported; `%` is integers only; `!` is bool; conditions require bool. There are no implicit numeric promotions; common numeric operators require identical operand types. Equality of optional values is permitted only when underlying types support equality; `null` compares only against optional/null. Structural/list equality is deep and type-identical. Ordering exists only for same-type numeric values and strings (lexicographic UTF-8 bytes); bool, bytes, optional, list, record ordering is a type error.

### Definite initialization, returns, scope, and mutation

All variables are explicitly `let` or `const`, initialized at declaration, and statically typed from initializer or optional annotation. `const` is immutable; `let` allows reassignment of its binding only, and its type never changes. Both shadow names in nested lexical blocks; same-scope duplicate parameters, locals, or record fields are errors. A declaration's initializer is resolved before the new binding becomes visible, so `let x = x;` uses an outer `x` or errors. Assignment requires a mutable `let` local, never a parameter, `const`, function, field, or list slot. Records/lists and all aggregate descendants are immutable; assignment to `r.field` or `xs[i]` always errors. Field access requires a statically present field; list indexing requires `i32`; a negative or out-of-range index raises kind 2. Optional field/index navigation is not special: check null first as specified below.

Every non-`null` function return type requires every reachable path to return a value. `return;` is permitted only for a function declared `-> null`, and `return expr;` must type-match the declared result. A function declared `-> null` may fall through; any other function may not. A `throw expr;` terminates its path and requires `string`; it does not count as a return. Constant conditions do not exempt unreachable branches from type checking. `break`/`continue` are legal only in loops. Definite assignment is structural because declarations require initializers; no uninitialized variable state exists. Flow analysis joins branches; a return in only one branch does not establish function completion.

All conditions are bool and evaluate left-to-right; `&&` and `||` short-circuit left-to-right. Range variables are immutable; a range with `start >= end` performs zero iterations. `is_null(value: T?) -> bool` is explicitly defined and performs no type refinement; `unwrap` is required to use the underlying value.

### Arithmetic

Integer overflow, division/remainder by zero, and `MIN / -1` or `MIN % -1` raise kind 2 through `raise_error`; none trap or wrap. Internal f64 follows IEEE-754 (including NaN and infinities); NaN comparisons follow IEEE rules and signed zero compares equal. At typed output boundaries, non-finite f64 is rejected with kind 3 because database-row `QueryRecord` decoding rejects non-finite values (`crates/query/shamir-query-types/src/read/core/query_record.rs:117-120`). `QueryValue` itself accepts f64 values including non-finite ones (`crates/foundation/shamir-types/src/types/value.rs:201-203`); the design deliberately enforces the stricter finite boundary for database consistency. Finite `-0.0` is normalized to `+0.0`. Canonicalization codec test: non-finite rejection and finite signed-zero normalization. Explicit conversion failures raise kind 2. String `+` is not concatenation; `+` is numeric only.

## 3. Entry points, effects, validation, and calls

Calls are only direct named calls. `fn` declarations are ordinary pure-or-effect-inferred helpers; exactly one `@entry` declaration is required and builtin names share the reserved global namespace.

| Kind | Required entry signature | Effects |
|---|---|---|
| scalar (MVP) | `@entry scalar name(p: T, ...) -> T { ... }` | pure computation; scalar intrinsics only |
| validator (MVP) | `@entry validator name(record: { ... }?, old_record: { ... }?) -> Validation { ... }` | pure computation; validator constructors only |
| `procedure` (stage 2) | `@entry procedure name(p: T, ...) -> T { ... }` | database and typed batch builtins only |

Every helper is declared with `fn name(...) -> T`; helpers have no entry annotation. Effects are inferred from reachable calls. Entry kind is a capability ceiling; validator entries may call constructors and pure helpers, procedure entries may call pure helpers and procedure builtins, scalar entries only pure builtins. Calling another marked entry is rejected. No other native `FnEntry` is implicitly available; scalar user-function resolution uses the existing resolver catalog, which must be explicitly registered by the integration. No builtin name is made an external function import. Recursion is rejected by call-graph cycle detection.

The generated wrapper exports ABI `shamir_call(ptr:i32,len:i32)->i64`, decodes one msgpack envelope map, validates and projects typed parameters, invokes the entry, encodes its declared result, and packs output as `((out_ptr as u64)<<32)|out_len`. The generated wrapper raises kind 2 for checked arithmetic, bounds, unwrap, and invalid arguments; kind 3 for codec failures; kind 1 for `throw`; kind 4 for guest-detected host-contract failures; host-supplied traps propagate unchanged. Allocation exhaustion and invalid memory access trap. The SDK/host category mapping is verified in `crates/wasm/shamir-sdk/src/error.rs` and `crates/wasm/shamir-wasm-host/src/wasm/bindings/raise.rs`: 1 user, 2 invalid argument, 3 codec, 4 host. Generated wrapper has a terminal `unreachable` after the noreturn `raise_error` import as a defensive fallback.

### Positional and named call envelope

The existing facade does not normalize call argument names: `crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs` stores numeric keys `"0"`, `"1"`, … and an `args` list. A generated wrapper therefore supports two forms, with these exact rules:

* **Positional form:** numeric keys, when present, are authoritative. If `args` is also present it is redundant and ignored (including any equality comparison); this avoids NaN/non-reflexive-value mismatches. If there are no numeric keys, `args` supplies values. Numeric keys must be contiguous and exact arity. `args` reserved PARAM name avoids wrapper collision. Mixing positional and named parameters rejects. Named form requires declared parameter names; argument names are required in named mode, optional parameters may be omitted. Unknown keys and duplicate MessagePack keys reject.

### Validator contract and operation envelopes

The validator's real adapter is `crates/engine/shamir-engine-validator/src/validator/execution/wasm_record_validator.rs`, not a database gateway: it materializes new/old records as QueryValue maps, substitutes `QueryValue::Null` for absent records, and invokes with params `record` and `old_record`. Deletion therefore has `record:null` and the previous value as `old_record`; insertion has a record and `old_record:null`. The language entry receives a typed projected record only when non-null; validators must declare nullable inputs (`record: { ... }?`, `old_record: { ... }?`) to handle insert/delete safely. Adapter fields such as real database `id` and other undeclared metadata are ignored during projection, while declared fields are checked; output remains typed, not dynamically exposed.

Opaque `Validation`, `Batch`, `Request`, and `Response` are rejected semantically when optional, fields, list elements or wire types; functions may use opaque values at their permitted stage/effect. Optional constructors permit only `T -> T?` and `null -> T?`; there is no numeric promotion. (overload selected by static checker): `accept() -> Validation`; `reject(field: string, code: string) -> Validation`; `reject_path(path: [string], code: string) -> Validation`; `record_error(code: string) -> Validation`; `with_error(value: Validation, field: string, code: string) -> Validation`; `with_record_error(value: Validation, code: string) -> Validation`; `stop(value: Validation) -> Validation`. The path form supports nested record fields; the one-field form is the one-segment shorthand. `accept` has no errors and `stop=false`; reject/record_error append one error and default stop=false; with_error/with_record_error append in order; stop preserves errors and sets stop=true. Calls with wrong types are compile errors. No raw list/map error representation is exposed.

Canonical result is msgpack `{ "errors": [ { "field": ["name"], "code": "name_required" } ], "stop": false }`; record-level error omits `field`. Empty errors accept; nonempty errors reject; stop halts subsequent validators. Exact shape agrees with SDK `Validation::into_value` in `crates/wasm/shamir-sdk/src/runtime/validation.rs` and decoder `crates/engine/shamir-engine-validator/src/validator/codecs/decode.rs` (which additionally accepts null, bare lists, string list items, and optional fields); compiler emits only the canonical shape.

## 4. Database stage and builtins

` raise_error` is the sole MVP host import when an error path is reachable; no database or other capability imports are available. This design consistently uses kind 2 for checked arithmetic and unwrap errors, kind 3 for codec/output errors, kind 1 for explicit `throw`, and kind 4 for guest-detected host-contract failures; host-supplied traps propagate unchanged. OOM, invalid memory access and impossible invariants trap.

The host imports are exactly the signatures registered in `crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs` and implemented in `wasm/bindings/`:

| Import `(i32...) -> ...` in `shamir_host` | Wire payload and result | Language policy |
|---|---|---|
| `batch_put(i32,i32,i32,i32)->()` | UTF-8 key + msgpack value; synchronous | not exposed |
| `batch_get(i32,i32)->i64` | UTF-8 key; zero absent, otherwise packed msgpack value; async | not exposed |
| `global_set(i32,i32,i32,i32)->()` | UTF-8 key + msgpack value; synchronous | not exposed |
| `global_get(i32,i32)->i64` | UTF-8 key; zero absent, else packed value; async | not exposed |
| `call(i32,i32,i32,i32)->i64` | UTF-8 function name + msgpack parameter map; async | not exposed |
| `db_get(i32,i32,i32,i32)->i64` | UTF-8 table + msgpack QueryValue key; zero missing, else packed record; async | stage 2 builtin |
| `db_insert(i32,i32,i32,i32)->i64` | UTF-8 table + msgpack QueryValue document; returns packed stored record; async | stage 2 builtin |
| `db_query(i32,i32,i32,i32)->i64` | UTF-8 table + optional msgpack QueryValue filter; zero filter length means no filter; result packed list; async | stage 2 builtin |
| `db_execute(i32,i32)->i64` | msgpack BatchRequest -> packed BatchResponse bytes; async | stage 2 opaque builder API |
| `raise_error(i32,i32,i32)->()` | kind, message pointer/length; terminal | generated wrapper/runtime only |
| `http_fetch(i32,i32)->i64` | msgpack request -> packed `[ok,payload]`; async; headers are pair lists `[name,value]` to preserve duplicates | not exposed |

`+` is numeric only. Helpers may use `raise_error` through `throw` and checked operations; no DB imports in MVP.

Typed database builtins, no generic syntax:

| Builtin | Signature | Semantics |
|---|---|---|
| `db_get(table,key)` | `table:string`; key non-null scalar or scalar-valued predicate record; context `{...}?` | missing row null |
| `db_insert(table,document)` | document any statically typed wire record (nested records/lists allowed); context record | returns stored record |
| `db_query(table,filter)` | filter null/scalar/scalar-field record; context `[{...}]` | null means no filter |

#### Stage-2 intrinsic signatures and result descriptors

The following signatures use `Scalar`, `ScalarRecord`, and `Record` only as notation in this table; they are not grammar types or aliases available to source authors. `Scalar` is exactly the closed union `i32 | i64 | f64 | bool | string | bytes | null`, and `ScalarRecord` is a structural record whose fields each have a `Scalar` type. Builtin signatures are compiler intrinsic schemes resolved from actual argument types plus a required contextual result type; this is the sole polymorphic mechanism, with no generic syntax.

| Intrinsic | Exact typing rule |
|---|---|
| `db_get(table, key)` | `table:string`; key nonnull scalar/predicate record; required optional-record context |
| `db_insert(table, document)` | document is a typed wire record (nested fields/lists allowed); required record context |
| `db_query(table, filter)` | `filter:Scalar|ScalarRecord|null`; required list-of-record context |
| `batch()` | `()->Batch` |
| `batch_id(batch,id)` | `(Batch,string)->Batch` |
| `batch_read(batch,alias,table,filter)` | `(Batch,string,string,Scalar|ScalarRecord|null)->Batch` |
| `batch_insert(batch,alias,table,document)` | `(Batch,string,string,Record)->Batch` |
| `request(batch)` | `(Batch)->Request`, supported read/insert operations only |
| `db_execute(request)` | `(Request)->Response` |
| `response_rows(response,alias)` | `(Response,string)->[{...}]?`; read aliases only |
| `response_inserted(response,alias)` | `(Response,string)->[{...}]?`; insert aliases only |

A context is supplied by an annotated local initializer, function return type, or an enclosing typed binding. The checker propagates this expected type into the intrinsic before it builds IR; no contextual expectation or multiple incompatible expectations yields `T010` at the intrinsic call. Thus `let user: { id: i64, name: string }? = db_get("users", key);` and `let rows: [{ id: i64, name: string }] = db_query("users", null);` are valid. The host encodes `null` query filter as zero bytes, never as msgpack nil; nofilter means list of all records. If a selected record lacks a declared required field, has wrong scalar representation, or an optional/required boundary is incompatible, result decoding raises kind 3; extra fields are ignored. Predicate maps must contain scalar values only and are encoded in canonical key order. No other filter shape, arbitrary query expression, builder object, or dynamic result is accepted.

`db_execute` is not an arbitrary serialized object and cannot receive a Rust SDK builder object. It is staged as opaque language values `Batch`, `Request`, `Response` with explicit intrinsic subset:

The opaque query builder types are parameter-free source types with these exact intrinsic signatures:

```text
batch() -> Batch
batch_id(Batch, string) -> Batch
batch_read(Batch, string, string, Scalar | ScalarRecord | null) -> Batch
batch_insert(Batch, string, string, Record) -> Batch
request(Batch) -> Request
db_execute(Request) -> Response
response_rows(Response, string) -> [{...}]?
response_inserted(Response, string) -> [{...}]?
```

`Scalar` and `ScalarRecord` are metanotation from the preceding table and cannot be written as type names. Result descriptors for both response projections use the same contextual expected-type rule as direct DB builtins; an ambiguous or absent descriptor is `T010`. `request` validates aliases, uniqueness, supported operation shape and fixed defaults before constructing the named request DTO. `db_execute` accepts only `Request`; it does not accept bytes, maps, BatchRequest-like records, or the Rust SDK builder value. `response_rows` and `response_inserted` return nullable lists of records. Opaque types are never generic entry values or documents. Only read/insert batch operations are supported; update/delete are excluded.

The language DB bridge is compiler-owned typed-plan lowering, not construction of a Rust SDK builder value or arbitrary raw BatchRequest maps/bytes. Native SDK `Batch` methods are `.query(alias, Query)` (not `.read`) and `.insert(alias, Insert)`; the typed plan is immutable, retains an operations array in insertion order, and allows at most 50 operations. Aliases are unique ASCII identifiers of at most 64 bytes; invalid or duplicate aliases raise kind 2. `batch_id` changes the request ID from initially nil to the supplied string. Functions create compiler-owned typed plans, not Rust SDK values; the WASM runtime serializes the lowered typed builder representation. The oracle is native Rust builder construction followed by `to_vec_named`; direct raw-JSON construction is not an example or oracle. The direct `db_query`/`db_get` bridge remains the intentional exception: scalar/equality predicates use `QueryValue` for this WASM bridge.

### Typed builder plan semantics

`batch_read` lowers to `Query::from(table)` and applies `.where_eq` for each static field in sorted-key order. Every predicate-record field, including a null-valued field, adds equality on that field; only a null whole filter means no filter. A null whole filter adds none. An empty predicate record therefore matches all records; `db_get` selects the first match. Typed `db_get` rejects a null chosen key. Scalar record fields may be null and encode nil equality; scalar-non-null typing disambiguates a non-null scalar key from a predicate record. `batch_insert` lowers to `Insert::into(table).row(document)`, exactly one row per operation. `request` validates the plan and constructs an immutable `Request`; `db_execute` accepts only that `Request`, never raw bytes. Unsupported shapes include nested batches, update/delete/DDL, references, transaction flags and other unsupported operations. Direct builtins use the host-context repository; builder `Query::from(table)` and `Insert::into(table)` use the explicit/default `main` table reference, not ambient repository context.

### Exact DTO mapping

`BatchRequest` serialization field order is `id`, `transactional`, `queries`, `return_all`, `limits`, `result_encoding`. Values/defaults are ID nil initially or string after `batch_id`, `transactional=false`, queries in insertion order, `return_all=true`, default limits, and `result_encoding="name"`. `name`, `isolation`, `durability`, `index_visibility`, `visibility_wait`, and `return_only` are absent when their options are `None`; empty `interner_epochs` is omitted. The DTO definitions and serialization are in `crates/query/shamir-query-types/src/ops/batch/req/batch_request.rs:53-175`.

`BatchLimits` field names, values, and serialization order are exactly `max_queries=50`, `max_dependency_depth=10`, `max_execution_time_secs=30`, `max_result_size=10485760`, `max_nesting_depth=4`, `max_iterations=1000`, `max_total_queries=10000`, `max_total_work=1000000`, `max_page_size=100000`, `max_offset=1000000` (`crates/query/shamir-query-types/src/ops/batch/req/batch_limits.rs:34-91,164-177`). Each `QueryEntry` serializes flattened operation fields followed by `return_result=true`; `after` is omitted when empty and `when` is omitted when None (`crates/query/shamir-query-types/src/ops/batch/ops/query_entry.rs:38-63`).

A read operation serializes `from` as `TableRef` (default repository `main`, bare table string, not ambient repo), `select` is a nested map with fields `items:[{type:"all"}]` and `distinct:false`; Its where field is the optional equality filter. Default `group_by`, `order_by`, pagination, `count_total`, temporal, `with_version`, and `explain` are omitted (`crates/query/shamir-query-types/src/read/core/read_query.rs:15-47`, `select.rs:21-35,59-64`, `ops/table_ref.rs:42-48`). An insert serializes `insert_into` as the bare table string and `values` as a one-element list containing the typed record; empty `records_idmsgpack` and `select=None` are omitted (`crates/query/shamir-query-types/src/ops/write/types.rs:84-108`).

The equality filter is lowered from typed predicate IR using the native filter representation and serialization; it is not serialized directly from raw user data. Response projection returns null for a missing alias. A present alias must match the operation kind remembered in the plan: query results are `QueryResult.records` lists; inserted results are bare `InsertedRecord` maps with string `_id`, not `{fields,id}`. Additional fields are ignored subject to decoder bounds; unsupported binary-ID rows reject. `result_encoding="name"`. Present aliases with wrong shape or missing required fields raise kind 3; missing root `results` also raises kind 3, never null. All typed DB builtins retain native gateway authorization. Direct calls and batches with `transactional=false` do not promise atomicity: a write may commit before result decoding fails, with no automatic rollback or retry.

## 5. Guest runtime: first implementation milestone

**Decision:** implement runtime design before lexer/parser. Use a monotonic per-instance bump arena that never resets or frees until Store teardown. The host creates a fresh Store per call and destroys its linear memory on success, error, and cancellation (`wasm_function.rs`, AccountedStore lifecycle); per-call reset is unsafe because host imports may suspend and re-enter guest `shamir_alloc`. Re-entry is safe because allocator state is monotonic, no live allocation is invalidated, and allocator invokes no host imports. Host allocates input before `shamir_call`; all response allocations, final output and error scratch stay valid until instance teardown. A second `shamir_call` on the same instance is rejected; `shamir_alloc` remains allowed while call is active.

### Linear memory layout and allocation

Page size is 65,536 bytes. Address layout is fixed: guard `[0,64)`; error scratch `[64,1088)`; static data begins at 1088 and ends at `1088+S`, with `S <= 4 MiB`; workspace begins at `align8(1088+S)` and occupies exactly 65,536 bytes; heap begins at `align8(workspace_end)`. Minimum pages are `ceil((heap_base+65,536)/65,536)`; maximum is 1,024 pages (64 MiB), within host Store limits. memory.grow receives DELTA (target pages minus current pages), not total required pages. Every offset/length uses checked u64 before narrowing; ABI pointers/lengths are nonnegative i32 and ranges end at or below 2^31. `shamir_alloc(0)` returns a stable aligned address (the heap base); positive allocations align to 8 bytes. Publish the bump pointer only after successful growth. Allocation has no imports and is safe during serialized host-import re-entry.

There is no shadow stack: Wasm call frames are managed by the engine. The compiler uses a separate fixed 8 MiB native compiler-worker stack. Globals are only `heap_next` and `call_state`; cached heap end is refreshed from current memory size on each allocation. Host code must reacquire memory slices after guest calls/imports; no slice is retained across possible memory growth. Monotonic allocation means alignment padding only (no external fragmentation), but short-lived allocations survive for the whole call; fresh Store teardown reclaims all memory, so no leak across calls.

Runtime hard budget: **64 KiB of emitted runtime code bytes**, a design cap, not a measured claim. Allocation: allocator/range/error helpers 8 KiB; primitive/aggregate codec dispatch 20 KiB; UTF-8 and numeric helpers 8 KiB; equality and ordering 8 KiB; ABI glue 12 KiB; 8 KiB reserve. This bound is independently enforced at emission and must fit the 8 MiB module cap. Literal plus metadata static data together <=4 MiB (not 4 MiB plus metadata); module bytes <=8 MiB; data segment total <=4 MiB per observed admission defaults. Runtime helper DCE is dependency-based: mark ABI decoder, encoder, allocator, error raiser, operation imports, equality/ordering helpers reachable from generated functions; walk dependency graph in stable topological order; emit each marked helper once. On cycles in helper dependency metadata, compiler internal error. No WAT bytecode template is embedded: helper instructions are emitted via `wasm-encoder` functions, pinned at `=0.254.2` to match `wasmparser = 0.254.2` in admission.

### Values and layouts

Each cell uses stable tag numbers: 0 unused, 1 null, 2 bool, 3 i32, 4 i64, 5 f64, 6 string, 7 bytes, 8 list, 9 typed record, 10 Validation, 11 Batch, 12 Request, 13 Response, 14 internal dynamic map. Layout is 16 bytes: tag u32, auxiliary u32, payload u64. List payload is vector offset, auxiliary count; element descriptors live in compiler static metadata. Record payload is vector offset, auxiliary field count; descriptor (type id plus sorted field names/types) is in static metadata, with payload low u32 offset/high u32 type id. Strings/bytes payload offset, auxiliary byte length; unused high bits zero. Optional is null tag or underlying cell; vectors immutable and never move. Lists may contain heterogeneous internal decoder cells; typed projection checks each item. Equality uses statically known types; runtime values do not infer types from user data.

### Codec contract

Decoder and encoder operate iteratively, not via recursive calls, with explicit frame stack and maximum nesting depth 32 matching the SDK value codec. The envelope itself counts as a container; depth check occurs before pushing any frame. Supported MessagePack values are nil, bool, signed/unsigned integer, f32/f64, str, bin, array, map with string keys. f32 promotes exactly to f64. Positive integer values up to i64 max decode as signed i64; larger u64 reject (stricter than SDK's conversion to decimal string). Float-to-integer coercion is never performed. Extension/fixext, timestamp extensions, reserved marker, invalid UTF-8, non-string map keys, duplicate keys, truncated values, trailing bytes after root, malformed lengths, and nested depth >32 reject as codec errors. Container counts and byte lengths are checked against limits before any allocation; no allocate-then-check. Decoding internal tagged cells does not expose dynamic public values.

Two encoders exist and never mix: the general value encoder (call results, errors, validation output, `db_get`/`db_insert`/`db_query` arguments) emits maps in UTF-8 key order; the DTO serializer (only the typed `Request` produced by `request(batch)`) emits struct fields in the fixed declaration order of section 4 and aliases in insertion order, as a separate generated routine that is not subject to the sorting rule. Both emit shortest integer width and deterministic arrays; finite f64 uses float64, normalizing negative zero. Non-finite output is kind 3. Strings use str, bytes use bin. Codec caps: depth 32 including envelope, 1,000,000 total cells, 100,000 entries per container, 1 MiB per string/bin, 8 MiB input and output, 64 KiB cumulative map-key bytes. At most 100,000 containers; cell budget accommodates both key and value cells for 100,000 map entries. Runtime decoder frames use the 64 KiB workspace and all 32 frames fit by construction. Duplicate-key detection is iterative linear scanning and charges guest execution fuel, not compiler fuel. Internal dynamic maps use tag 14 with payload pointer to `{keyCell,valueCell}` pairs and auxiliary count; typed record tag 9 stores low-32-bit pointer plus high-32-bit type descriptor. Decoding a dynamic map (tag 14) into a declared record projects to typed record (tag 9); tag 14 is internal and never public/wire output. Container counts and lengths are checked before allocation.

### Error bridge

Generated `raise_error(kind,ptr,len)` uses existing host import exactly, with kinds 1=user, 2=invalid argument, 3=codec, 4=host. Error UTF-8 is decoded lossily, invalid sequences and control scalars replaced by U+FFFD, maximum message 1,024 bytes (truncated with marker when possible), based on `bindings/raise.rs`. Invalid pointer/length, pointer arithmetic overflow, OOM, allocator exhaustion, or impossible runtime invariants trap; host imports may terminate with host-supplied traps; kind 4 is reserved for guest-detected host-contract failures; terminal call has `unreachable` fallback. Explicit `throw string` raises kind 1. Checked arithmetic errors use kind 2, codec errors kind 3.

Each helper parameter/result lowers to one Wasm value: i32/bool to i32, i64 to i64, f64 to f64, and optional/aggregate/opaque values to i32 cell offsets. IR uses typed virtual locals, explicit basic blocks and terminators (branch, conditional branch, return); structured source loops retain region IDs so the emitter can select br/br_if depths without unstructured jumps. Checked arithmetic is explicit in IR; lowering cannot silently replace it with wrapping Wasm arithmetic. IR and AST nodes live in flat arenas. Calls use lexical symbol indices, and the emitter allocates types/imports/functions/globals/data in stable index order.



## 6. Compiler architecture, API, tree, and host orchestration

**Decision:** `shamir-lang` is a pure runtime-free compiler crate. Production dependencies are `shamir-wasm-admission`, `wasm-encoder = "=0.254.2"`, and `thiserror = "2.0"` (the latter is already used by admission). Test-only dependencies are `wat = "1.254.2"` and matching `wasmprinter = "0.254.2"`. There is no production Wasmtime, Tokio, filesystem, or process dependency. Add a scoped Rust guard rejecting `std::process` in the compiler; a PATH-empty test is not sufficient. Host engine tests remain host-owned dev/integration tests.

`CompileOptions` contains `stage: Stage` (`Stage::Mvp` by default or `Stage::Procedures`), named `CompileLimits`, and admission operator limits. Effective compiler limits are the minimum of hard ceilings and any lower positive options; zero is permitted only in tests to exercise rejection. `ModuleReport` remains unchanged. `CompiledModule` remains `{ wasm: Vec<u8>, report: ModuleReport }`. `Span` byte ranges are half-open; line and Unicode-scalar columns are 1-based, and CRLF counts as one newline. Pipeline: lex, parse flat AST, resolve, effect/type/flow analysis, typed IR, emit, then `admit_module(&wasm, &AdmissionPolicy::Admission(options.admission))`. Compiler callbacks are pure; tier selection never changes emitted bytes.

`compile(source: &str, options: &CompileOptions) -> Result<CompiledModule, CompileError>` is the only production entry. `CompileOptions { stage: Stage, limits: CompileLimits, admission: AdmissionLimits }`. `CompiledModule { wasm: Vec<u8>, report: ModuleReport }`. `CompileError` variants are `Diagnostics(Vec<Diagnostic>)`, `Admission(AdmissionError)`, `LimitExceeded { code: &'static str, span: Span, limit: u64, observed: u64 }`, and `InternalInvariant(&'static str)`. `Diagnostic { code: &'static str, message: String, primary: Span, related: Option<RelatedSpan> }`; `RelatedSpan { span: Span, message: String }`; `Span { start_byte: u32, end_byte: u32, start: Position, end: Position }`; `Position { line: u32, column: u32 }`. Positions are 1-based Unicode-scalar columns; byte spans are half-open. Public structs live in their named api files; api/mod.rs and lib.rs re-export the surface.


 Exact parameters are `engine: Arc<WasmEngine>, source: String, options: CompileOptions, limits: WasmLimits, mode: TierMode`. Create returns bytes and function report; check returns check report. Check mirrors the requested tier policy without persistence: Auto tries the fast engine and falls back to uncached Cranelift, Now uses uncached Cranelift, Never uses only fast when available and uncached Cranelift only when no fast engine exists. `options.admission` must match the engine's actual allowed capabilities.

The new host-root public factories are `create_shamir_source(engine: Arc<WasmEngine>, source: String, options: CompileOptions, limits: WasmLimits, mode: TierMode) -> FnResult<(WasmFunction, Vec<u8>, ModuleReport)>` and `check_shamir_source(engine: Arc<WasmEngine>, source: String, options: CompileOptions, limits: WasmLimits, mode: TierMode) -> FnResult<ModuleReport>`. Both own one gate permit; check persists nothing and uses uncached tier-specific compilation. Host `lang-compiler` feature defaults off and enables optional compiler dependency; the compiler has no host dependency.

There is exactly one owned compile permit, acquired before launching the actual 8 MiB `std::thread` worker. Join through the blocking pool. The 10-second async timeout begins after permit grant and cannot cancel the worker; retain the permit until the real worker exits, not merely until its join handle is dropped. Timeout never persists output. Existing `run_gated_compile` is `pub(in crate::wasm)` and `WasmEngine::compile_permits` is `pub(crate)`; `check_module` and `from_binary_admitted_blocking` own gates and must not be nested. Planned private ungated constructors/check callbacks must live in their owning modules and be called through new public service APIs used by DB; private items cannot be assumed accessible outside their owner. Split owner files before 1,000-line growth as a future task. Stack size is not isolation: catch_unwind cannot protect stack overflow. Depth limits and flat arenas are real bounds.

The complete proposed crate tree follows; counts are direct entries and each file's primary owner is its enclosing module. `mod.rs` files only declare/re-export; internal modules remain private or `pub(crate)`, the API root reexports public items, and `testsupport` is `cfg(test)` only. Tests are registered by their owner parent; `lib.rs` declares `api`, `front`, `sem`, `back`, and cfg-test `testsupport`. Imports live in file headers, never local `use` blocks; no new allow attributes; future impls stay in the owning module. Layout documentation is exempt; current compiler source count is zero.

```text
crates/shamir-lang/ [2]
├── Cargo.toml
└── src/ [6]
    ├── lib.rs
    ├── api/ [7]
    │   ├── mod.rs
    │   ├── compile.rs
    │   ├── options.rs
    │   ├── compiled_module.rs
    │   ├── error.rs
    │   ├── diagnostic.rs
    │   └── span.rs
    ├── front/ [4]
    │   ├── mod.rs
    │   ├── lexer/ [4]
    │   │   ├── mod.rs
    │   │   ├── token.rs
    │   │   ├── scan.rs
    │   │   └── tests/ [2]
    │   │       ├── mod.rs
    │   │       └── lexical_tests.rs
    │   ├── parser/ [3]
    │   │   ├── mod.rs
    │   │   ├── parse.rs
    │   │   └── tests/ [2]
    │   │       ├── mod.rs
    │   │       └── parse_tests.rs
    │   └── ast/ [3]
    │       ├── mod.rs
    │       ├── node.rs
    │       └── tests/ [2]
    │           ├── mod.rs
    │           └── ast_tests.rs
    ├── sem/ [6]
    │   ├── mod.rs
    │   ├── resolve.rs
    │   ├── types.rs
    │   ├── effects.rs
    │   ├── flow.rs
    │   └── tests/ [5]
    │       ├── mod.rs
    │       ├── resolve_tests.rs
    │       ├── type_tests.rs
    │       ├── effect_tests.rs
    │       └── flow_tests.rs
    ├── back/ [6]
    │   ├── mod.rs
    │   ├── ir.rs
    │   ├── runtime/ [7]
    │   │   ├── mod.rs
    │   │   ├── alloc.rs
    │   │   ├── layout.rs
    │   │   ├── cell.rs
    │   │   ├── codec/ [5]
    │   │   │   ├── mod.rs
    │   │   │   ├── decode.rs
    │   │   │   ├── encode.rs
    │   │   │   ├── frames.rs
    │   │   │   └── tests/ [3]
    │   │   │       ├── mod.rs
    │   │   │       ├── decode_tests.rs
    │   │   │       └── encode_tests.rs
    │   │   ├── error.rs
    │   │   └── tests/ [5]
    │   │       ├── mod.rs
    │   │       ├── alloc_tests.rs
    │   │       ├── layout_tests.rs
    │   │       ├── cell_tests.rs
    │   │       └── error_tests.rs
    │   ├── emit/ [7]
    │   │   ├── mod.rs
    │   │   ├── module.rs
    │   │   ├── scalar.rs
    │   │   ├── validator.rs
    │   │   ├── procedure.rs
    │   │   ├── abi.rs
    │   │   └── tests/ [6]
    │   │       ├── mod.rs
    │   │       ├── module_tests.rs
    │   │       ├── scalar_tests.rs
    │   │       ├── validator_tests.rs
    │   │       ├── procedure_tests.rs
    │   │       └── abi_tests.rs
    │   ├── admission.rs
    │   └── tests/ [5]
    │       ├── mod.rs
    │       ├── runtime_tests.rs
    │       ├── emission_tests.rs
    │       ├── golden_tests.rs
    │       └── corpus_tests.rs
    └── testsupport/ [6]
        ├── mod.rs
        ├── fixtures.rs
        ├── msgpack.rs
        ├── wasm.rs
        ├── source.rs
        └── vectors.rs
```

The host compile-gate facade exposes source create/check service methods with `engine: Arc<WasmEngine>, source: String, options: CompileOptions, limits: WasmLimits, mode: TierMode`; create returns bytes/function report and check returns check report. Existing gate ownership constraints apply as above. The compiler's source pipeline is lex, parse flat arena, resolve, type/effect/flow check, typed IR, emit, self-admit. Final self-admit uses `options.admission`, not defaults.

Admission `max_compile_time` defaults to 10 seconds and begins after permit grant; timeout rejects request but does not cancel worker. `AdmissionPolicy::Load` remains distinct and does not inherit Admission timeout/caps.
## 7. Bounds, work accounting, and crash safety

Compiler hard limits and lowerable options are the named `CompileLimits` fields below; all are `usize` except `compile_fuel: u64`. Hard ceilings are: `max_source_bytes=1048576`, `max_tokens=200000`, `max_ast_nodes=200000`, `max_ast_depth=128`, `max_type_depth=128`, `max_functions=256`, `max_locals_per_function=1024`, `max_parameters=32`, `max_record_fields=128`, `max_single_literal_bytes=65536`, `max_total_literal_bytes=4194304`, `max_static_bytes=4194304`, `max_output_bytes=8388608`, `max_runtime_bytes=65536`, `max_function_body_bytes=1048576`, `max_operators_per_function=500000`, `max_ir_nodes=200000`, `max_call_chain=64`, `compile_fuel=20000000`. Apply source limits to all declarations, including dead code. Options may only lower hard ceilings and must be positive except test-only zero values; reject a zero production option. Diagnostics are fixed at 64 items and 1,024 bytes/message, not options.

Generated-module hard constraints (separate from source limits): at most 384 defined functions including ABI functions, 512 types, 4,096 locals, params+results <=33, 5 imports, 3 exports (memory is not a function), 2 globals; no references, tables, elements, or start; one active data segment; block depth <=256. Guard before allocation/codegen and retain the function-body bound.

Runtime fixed caps: input/output 8 MiB; string/bin 1 MiB each; cumulative map-key bytes 64 KiB; containers, list entries, and map entries each 100,000; total cells 1,000,000; depth 32 including envelope. Guest memory max 1,024 pages (64 MiB), including static data. Wire parameter type depth is <=30 containers: the standard facade envelope carries every positional value twice (numeric key and the redundant `args` list), so the deepest placement is envelope (1) + `args` list (1) + value (<=30) = 32; the cap of 32 is never raised to absorb it. Result type depth <=32 (the result root is the only container level it adds). Admission's observed default caps are separately authoritative:

| Admission limit (`limits.rs:62-85`) | Default |
|---|---:|
| module bytes | 8 MiB |
| functions / types | 20,000 / 5,000 |
| imports / exports | 64 / 64 |
| globals / locals | 1,000 / 10,000 |
| function body / operators | 1 MiB / 500,000 |
| block depth / branch targets | 256 / 4,096 |
| params + results | 128 |
| table min/max | 100,000 |
| element segments / items | 1,000 / 200,000 |
| data segments / bytes | 10,000 / 4 MiB |
| memory minimum | 1,024 pages |
| source bytes / compile time | 1 MiB / 10 s |
| concurrency | max(1, available cores / 2) |

Admission feature profile allows MVP, mutable globals, saturating float conversions, sign extension, multivalue, bulk memory, and partial `funcref` reference support; no `externref`, only `call_indirect` table operation. SIMD, threads, GC, exceptions, tail calls, 64-bit memory, multi-memory, and components reject. Actual Load skips the Admission contract/caps but retains parsing of feature/import sections; it is not the same policy.

| Resource | Hard compiler/runtime cap | Check point |
|---|---:|---|
| source bytes | `max_source_bytes` | before decoding/lexing |
| tokens | `max_tokens` | before token append |
| AST nodes/depth | `max_ast_nodes` / `max_ast_depth` | before append/nesting |
| type depth | `max_type_depth`; wire parameter <=31, result <=32 | before node/boundary |
| functions / locals / parameters | `max_functions` / `max_locals_per_function` / `max_parameters` | before append |
| record fields | `max_record_fields` | before AST/type allocation |
| generated functions / types / locals | 384 / 512 / 4,096 | before emission |
| params+results / imports / exports / globals | 33 / 5 / 3 / 2 | source validation/emission |
| block depth / tables / elements / start | 256 / 0 / 0 / absent | before emission/admission |
| literals: single / total | 65,536 / 4 MiB | before growth/append |
| static data / output / runtime code / body | 4 MiB / 8 MiB / 64 KiB / 1 MiB | before layout/write/emission |
| operators per function / IR nodes / call chain | 500,000 / 200,000 / 64 | before append; recursion rejected |
| decoder depth / cells / containers / entries | 32 / 1,000,000 / 100,000 / 100,000 | before allocation/frame |
| string/bin / map-key bytes | 1 MiB / 64 KiB | before allocation |
| codec input/output / guest memory | 8 MiB each / 64 MiB | before allocation/write/growth |
| compiler fuel | 20,000,000 | charge before work |

Fuel charges one per byte scanned, copied, compared, or emitted, and before token/node/edge/type steps; loops must be metered, never unbounded. External admission is precharged once by wasm length; its structural caps bound parser work. External Wasmtime compilation is not fuel-metered, only structurally/time/concurrency bounded.
Fuzz/deep test acceptance: arbitrary UTF-8 input never panics; deep left-associated expressions, nested blocks/record/list/type constructs stop at caps; iterative AST/IR drop and formatting handle limit-sized trees; type equality is fuel bounded; recursive/mutual call cycles reject; malformed MessagePack never allocates beyond count/byte budget. For every limit, tests exercise 0 where meaningful, exactly limit, and limit+1. Cap tests are proposed acceptance criteria, not executed here.

Cumulative heap allocations never reset/free. Exact base addresses and minimum pages are specified in section 5; memory grows to exact required pages. No linear-memory stacks exist. Fresh Store teardown reclaims all bytes; re-entry remains safe because allocations are monotonic.

## 8. Stable diagnostics and existing error surface

**Decision:** stable codes identify compiler rejection category independent of wording. `Diagnostic` renders deterministic text as `CODE:line:column: message`; line/column 1-based, byte-span half-open, unicode scalar columns. Related duplicate location prints after the primary in stable order. Messages avoid hash-map order, addresses, debug formatting, and platform-specific paths. `CompileError::Admission` uses stable admission variant/category and deterministic display; its nested engine/parser text is retained only after sanitized deterministic formatting.

| Code | Category | Example trigger |
|---|---|---|
| `L001` | invalid character | unrecognized source scalar |
| `L002` | malformed literal/escape | overflow, bad escape |
| `L003` | unterminated construct | comment/string EOF |
| `P001` | expected token | missing delimiter/type |
| `P002` | declaration/entry | missing/duplicate entry |
| `P003` | syntax | malformed statement/expression |
| `N001` | unresolved name | unknown local/function/field |
| `N002` | duplicate binding | same-scope declaration |
| `T001` | type mismatch | expression/argument/return |
| `T002` | conversion | unsupported cast |
| `T003` | operation | invalid operator/type |
| `T004` | call | invalid arity/target |
| `T005` | flow | return/loop-control error |
| `T006` | mutation | immutable target |
| `T007` | effect | entry capability violation |
| `T008` | recursion | call cycle |
| `T009` | boundary | unsupported entry type |
| `T010` | context | ambiguous DB result descriptor |
| `B001` | compiler limit | source/fuel/output/resource cap |
| `B002` | self-admission | emitted module rejected |
| `I001` | internal invariant | compiler invariant failed |
| `R001` | runtime label | runtime error label, not compile diagnostic |

Negative tests cover each code, with source spans at offending tokens; budget categories verify exact/over boundary. Existing facade `check.rs`, `source_gate.rs`, and `management.rs` map failures through `DbError::Function(String)` (for example `map_err(|e| DbError::Function(e.to_string()))`); no current structured compiler-diagnostic wire form exists. Future integration preserves stable code and rendered span text in that same deterministic string. Structured diagnostic fields are deferred to separate wire/API design; this design does not invent them.

## 9. Testing and acceptance plan

All items in this section are proposed tests, not claims of execution. Tests should use the host engine only in host-owned integration tests, avoiding a production dependency cycle. Compiler goldens compare both normalized WAT and exact binary bytes: `wat` can assemble WAT but cannot disassemble arbitrary wasm, so planned test-only `wasmprinter` renders emitted bytes for readable comparison; expected WAT is independently pinned, assembled with `wat`, and compared to compiler bytes only where byte-for-byte encoding settings match. Also compare deterministic repeat compilation output. No golden may claim a `wat` round-trip itself is a disassembler.

* Lexer: token vectors/spans; Unicode columns and UTF-8 byte spans; every escape; malformed numeric/string/bytes/comments; arbitrary UTF-8 fuzz no panic; byte limits.
* Parser/AST: complete grammar examples, precedence, postfix optional, direct calls, `else` restriction, semicolons, nesting 0/limit/limit+1; iterative destruction and formatting of deep left-associated trees.
* Semantics: each type and conversion row, expected-type rules, `is_null(value: T?) -> bool` is defined without refinement; shadowing, assignment, structural records/lists, definite return, all stable codes, effect propagation, direct/mutual recursion, fuel-bounded type equality.
* Runtime codec: exact vectors for each MessagePack tag; reject ext, trailing bytes, duplicate/nonstring map keys, invalid UTF-8, depth 32/33, huge array/map count before allocation, float promotion, u64 rejection, shortest integer encoding, canonical map order, non-finite rejection, and finite signed-zero normalization.
* Scalar execution: generated ABI input/output, checked arithmetic incl MIN/-1 division and remainder, NaN semantics, optional conversions, throw kind, pointer range failures; differential test against an independent small reference interpreter (not the compiler's evaluator), random bounded typed expressions.
* Validator execution: canonical accept/reject/record-level/stop msgpack; insertion (`record` nonnull, `old_record` null), update (both maps), deletion (`record` null, `old_record` map); extra persisted fields ignored by typed projection; missing declared field rejected; run adapter via `WasmRecordValidator` and assert engine decoder semantics.
* Stage 2: exact host signature test, per-operation named BatchRequest golden fixtures generated by compiler builder intrinsics and independently checked against Rust SDK builder; query equality/null no-filter; response shape/missing/null projection failures; async allocator re-entry and Store teardown.
* Admission: every compiled corpus module passes runtime-free `admit_module`; disallowed import/section/export/feature rejection; module, body, operators, memory, data caps at 0/limit/limit+1.
* Host compile gate: compile permit acquired once, permit retained until real worker completion after timeout/caller cancellation; source compile + self-admit + host compile do not nest; blocking worker uses fixed 8 MiB stack; dry-run uses uncached engine and persists nothing.
* Stable diagnostics: every code has a negative fixture and exact rendered text/span; code stability checked independent of message wording changes.
* Property/fuzz suite: lexer/parser/type checker/codec on arbitrary bounded inputs, deterministic bytes and diagnostics, no panics for in-cap inputs; hostile over-cap input rejected before disproportionate allocation.

Runtime budgets and performance numbers are target limits, not measurements. Tests should assert 64 KiB helper code maximum and 4 MiB combined static data once backend exists; until then no size claim is verified.

## 10. Milestones (runtime first)

1. **Runtime and ABI foundation.** Depends: none. Crates: `shamir-lang`; host dev integration only. Files: `back/runtime/`, runtime ABI implementation in its owning module, `testsupport/`. Public surface: runtime internals only; pure compile API not available until M4. Tests: manual runtime vectors first; future runtime tests, not run now. Done: runtime layout/codec executable and ABI fixture defined. Risk: runtime size budget is planned, not measured; ABI boundary.
2. **Lexer, AST, parser.** Depends: M1. Crates: `shamir-lang`. Files: `front/lexer/`, `front/ast/`, `front/parser/`. Public surface: none beyond internal modules. Tests: lexer/parser tests registered at owners; future only. Done: grammar and spans implemented within bounds. Risk: escaping and precedence.
3. **Resolution and semantic analysis.** Depends: M2. Crates: `shamir-lang`. Files: `sem/resolve.rs`, `types.rs`, `effects.rs`, `flow.rs`. Public surface: internal typed representation/diagnostics. Tests: semantic owner tests, future only. Done: invalid source cannot reach lowering. Risk: contextual record projection.
4. **Typed IR and scalar emission.** Depends: M1 and M3. Crates: `shamir-lang`. Files: `back/ir.rs`, `back/emit/{module,scalar}.rs`, API compile/options modules, test support source fixtures. Public surface: pure compile API and report. Tests: golden/interpreter tests; future only. Done: deterministic admitted scalar module. Risk: Wasm control flow.
5. **ABI glue and validators.** Depends: M4. Crates: `shamir-lang`, host dev integration. Files: `back/emit/{abi,validator}.rs`, host adapter owner. Public surface: validator compile. Tests: future host adapter tests, not run now. Done: real adapter accepts validator outputs. Risk: nullable projection.
6. **Host source service and PROC3B2 wiring.** Depends: M5. Crates: `shamir-wasm-host`, `shamir-db`. Files: new host module `crates/wasm/shamir-wasm-host/src/wasm/lang_compile.rs` (owns the fixed-stack worker thread, the shared compile semaphore use and the call of `shamir_lang::compile` followed by the existing admission gate; it is exported through the host crate and the `shamir-engine` re-export like the existing compile entry points); `shamir-wasm-host` takes a normal (non-optional) dependency on `shamir-lang`, no feature gating or propagation; in `shamir-db`: `db/function/facade/source_gate.rs` (dispatch `lang = "shamir"` to the host module under the `Execute`-free source path), `db/function/facade/check.rs`, `db/function/management.rs`, and the `FunctionSource` enum. The host `check_module` stays in `crates/wasm/shamir-wasm-host/src/wasm/admission/gate/check.rs` and is called unchanged on the compiler output; the reserved-language rejection is lifted by PROC3B2 in the DDL handler (`execute/admin_meta/admin_function_wire.rs`). Public surface: host source create/check service, single owned gate and fixed-stack worker. Tests: future host/DB integration tests; the host dev dependency on compiler does not create a production cycle. Done: genuine later implementation compiles source under shared gate with correct persistence and dispatch. Risk: ownership/privacy and cancellation lifecycle.
7. **Procedures and typed database stage.** Depends: M6. Crates: compiler, host, query DTO integration. Files: `back/emit/procedure.rs`, typed runtime intrinsics and builder/DTO conformance docs. Public surface: stage-2 builtins. Tests: future DTO goldens and host tests, not run now. Done: supported DTO bytes match and raw maps are rejected. Risk: DTO evolution.
8. **Hardening and final conformance.** Depends: M1-M7. Crates: compiler, host, DB fixtures. Files: owner-registered fuzz/deep/golden/negative tests. Public surface: no expansion. Tests: future fuzzing, deep-boundary cases, deterministic goldens, negative caps and final conformance, not run now. Done: acceptance criteria pass and wiring is separately reviewed. Risk: capability creep and module budgets.


## Worked examples

### Scalar

```text
fn add(a: i64, b: i64) -> i64 { return a + b; }
@entry scalar sum(a: i64, b: i64) -> i64 {
  return add(a, b);
}
```

Named input `{"a":3,"b":5}` MessagePack: `82 a1 61 03 a1 62 05`; result `08`. Positional input `{"0":3,"1":5,"args":[3,5]}`: `83 a1 30 03 a1 31 05 a4 61 72 67 73 92 03 05`; result `08`. Numeric keys are authoritative and redundant `args` is ignored (its value is still depth-counted and skipped without materialization). These are manual vectors, not executed.

The same module namespace includes `fn` helpers, which are not exports or entries.

### Typed validator

```text
@entry validator require_name(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  if (is_null(record)) {
    return record_error("record_required");
  }
  let current: { name: string } = unwrap(record);
  if (current.name == "") {
    return reject("name", "name_empty");
  }
  return accept();
}
```

Insertion input with name `Ada`: `82 a6 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a3 41 64 61 aa 6f 6c 64 5f 72 65 63 6f 72 64 c0`. Accept output: `82 a6 65 72 72 6f 72 73 90 a4 73 74 6f 70 c2`.

Empty-name input: `82 a6 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a0 aa 6f 6c 64 5f 72 65 63 6f 72 64 c0`. Error output: `82 a6 65 72 72 6f 72 73 91 82 a4 63 6f 64 65 aa 6e 61 6d 65 5f 65 6d 70 74 79 a5 66 69 65 6c 64 91 a4 6e 61 6d 65 a4 73 74 6f 70 c2` (`name_empty` is 10 bytes; `record_required` is 15 bytes).

Deletion input: `82 a6 72 65 63 6f 72 64 c0 aa 6f 6c 64 5f 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a3 41 64 61`. Output: `82 a6 65 72 72 6f 72 73 91 81 a4 63 6f 64 65 af 72 65 63 6f 72 64 5f 72 65 71 75 69 72 65 64 a4 73 74 6f 70 c2`. Missing required `name` is a kind-2 typed decode failure, not a `name_empty` validation error. These are manual vectors, not executed.

Emitted module sketch: `Type; Import(raise_error); Function; Memory(min=computed,max=1024); Global(heap_next,call_state); Export(memory,shamir_alloc,shamir_call); Code; Data(1); Custom(shamir.abi=[01 00 00 00], shamir.origin=UTF-8 "shamir-lang")`. No start, table, or element section.

Decisions with close alternatives: structural rather than nominal records (no type declarations); nullable projection rather than public dynamic Value (static checking); explicit unwrap rather than flow narrowing (no alias analysis); checked integers rather than wrapping (data integrity); finite wire floats rather than preserving NaN/Infinity (row compatibility); per-instance bump arena rather than free-list (re-entry and immutable aliases); generated helpers rather than embedded binary runtime (deterministic indexing/DCE); typed IR rather than direct AST emission (checked semantics); typed builder subset rather than raw wire (schema conformance); runtime-first milestones rather than parser-first (ABI risk first).



## Non-goals

No closures, classes, generics, dynamic `Value`/`any`, arbitrary MessagePack dynamic language value, recursion, exceptions/try-catch, imports, WASI, component model, multi-module linking, hot reload, optimizer passes, arbitrary host capabilities, scalar-to-procedure effect escalation, `call`, HTTP, globals, batch scratch, or custom database query AST in MVP/stage 2. `db_execute` supports only the explicit typed intrinsic subset, not Rust builder values or raw wire maps. No TypeScript or Rust source compilation changes; `lang="shamir"` server wiring is a later task. No changes to ABI v1, engine fuel/epoch/memory policy, validator scheduling, admission feature set, or existing structured wire error shape.

## Verified against the code

- ABI export names/types, ABI section name/version/default and origin cap: `crates/wasm/shamir-wasm-admission/src/abi.rs` lines 3-16; ABI contract and section behavior: `docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` lines 17-24, 58-70.
- Admission defaults (8 MiB module, 20,000 functions, 5,000 types, 64 imports/exports, 1,000 globals, 10,000 locals, 1 MiB body, 500,000 operators, depth 256, 1,024 memory pages, source 1 MiB, 10 seconds): `crates/wasm/shamir-wasm-admission/src/types/limits.rs` lines 62-85; policy distinction in `types/policy.rs`.
- Host module uses fresh Store and async imports can call allocator/write response; guest ABI packed result: `crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs` lines 10-18, 38-42, plus invocation/Store implementation later in the same file; exact import registrations lines 211-250.
- All 11 sanctioned import names: `crates/wasm/shamir-wasm-admission/src/sanitizer.rs` lines 87-99. Exact host functions: `wasm/bindings/{batch,globals,call,db,http,raise}.rs`; HTTP pair-list header encoding in `bindings/http.rs` lines 40-70, 87-106.
- SDK dynamic internal Value variants and nesting 32: `crates/wasm/shamir-sdk/src/api/value.rs` lines 22-49; parameter map shape/accessors: `runtime/params.rs` lines 6-42; raise-error categories in `error.rs` lines 3-10, 74-83.
- Validator adapter actual `record`/`old_record` envelope, null substitution, and materialized QueryValue: `crates/engine/shamir-engine-validator/src/validator/execution/wasm_record_validator.rs` lines 58-76. Validator output canonical SDK shape: `crates/wasm/shamir-sdk/src/runtime/validation.rs` lines 9-16, 146-177. Decoder accepts null/list/map, optional stop, string/error map items and field path list: `crates/engine/shamir-engine-validator/src/validator/codecs/decode.rs` lines 27-71, 83-121.
- Existing call facade stores `"0"`, `"1"`, ... and `"args"`, without normalizing names: `crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs` lines 25-43.
- `db_get/insert/query/execute` signatures and payload rules: `crates/wasm/shamir-wasm-host/src/wasm/bindings/db.rs` lines 14-18, 70-76, 115-123, 170-179; `db_execute` forwards raw batch bytes to gateway lines 191-199. SDK equality-filter restrictions and query-builder API: `crates/wasm/shamir-sdk/src/api/db.rs` lines 5-13, 78-123, 152-204.
- Batch request DTO required fields/defaults and named query map: `crates/query/shamir-query-types/src/ops/batch/req/batch_request.rs` lines 21-53, 124-175; response DTO: `req/batch_response.rs` lines 13-65; operation enum is broader than proposed language subset: `ops/batch/ops/batch_op.rs` lines 52-193.
- Error import pointer validation/message cap/UTF replacement/kinds: `crates/wasm/shamir-wasm-host/src/wasm/bindings/raise.rs` lines 4-49, 51-84.
- Compiler gate ownership and timeout/no cancellation: `crates/wasm/shamir-wasm-host/src/wasm/admission/gate/compile_gate.rs` lines 8-53; check path admits and compiles in one gate and uses uncached optimized engine: `gate/check.rs` lines 11-34, 62-66; existing admitted constructor takes gate once: `wasm_function.rs` lines 370-392.
- `DbError::Function(String)` mappings verified in `crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/facade/check.rs`, `facade/source_gate.rs`, and `db/function/management.rs`; no structured compiler diagnostic wire type was inspected or found in those paths.
- User-facing function kinds and validator model: `docs/guide-docs/guide/05-functions.md` lines 13-22; scalar resolution precedence and dispatch: `crates/query/shamir-funclib/src/scalar_resolver.rs` lines 79-135.
- Matching `wasmparser` dependency is pinned `=0.254.2` in `crates/wasm/shamir-wasm-admission/Cargo.toml` (reported by source search). ABI D4 and stage boundaries: ADR lines 142-167.

## Not verified / design-only

### M1 implementation deviations

- The workspace exceeds the fixed 64 KiB by the codec cell/sort budget
  (decoder cell slots, encoder pair-index window, reject scratch) so the
  decoded cells stay strictly below the heap base.
- Error-message sanitation/truncation is host-side per
  `crates/wasm/shamir-wasm-host/src/wasm/bindings/raise.rs`: the guest
  validates the full range and forwards the declared length, so the host's
  1024-byte clamp + `[truncated]` marker is preserved.
- `testsupport` ships behind the `testsupport` Cargo feature instead of
  `cfg(test)` — a `cfg(test)` module is invisible to the host crate's
  dev-dependency, which is the only consumer.
- Static data is emitted at offset 1088 (64-byte guard + 1024-byte error
  window) with origin tag "shamir-lang"; the runtime code budget is
  enforced on the code section (<= 64 KiB) and static data has its own
  4 MiB cap.

- No `shamir-lang` implementation exists; grammar, compiler API, type rules, runtime layout, caps beyond observed admission defaults and all acceptance tests are proposed, not measured.
- The selected encoder dependency/version and exact SDK/DTO byte encodings for supported builders remain unverified; golden tests are required before implementation is accepted.
- Layout gate was run by orchestrator and passed; it checks tracked sources only and excludes documentation; new design remains untracked. No executable tests or builds were run.

