/// Build a [`crate::types::value::QueryValue`] from a MessagePack-native
/// literal expression — analogous to `serde_json::json!`, but produces
/// `QueryValue` (`= Value<String>`) directly, with zero `serde_json` involved.
///
/// # Syntax
///
/// | Expression | Result |
/// |---|---|
/// | `mpack!(null)` | `QueryValue::Null` |
/// | `mpack!(true)` / `mpack!(false)` | `QueryValue::Bool(…)` |
/// | `mpack!(42)` | `QueryValue::Int(42)` — integer literal → `i64` |
/// | `mpack!(3.14)` | `QueryValue::F64(3.14)` — float literal → `f64` |
/// | `mpack!("hello")` | `QueryValue::Str("hello".to_string())` |
/// | `mpack!([e1, e2, …])` | `QueryValue::List(vec![mpack!(e1), …])` |
/// | `mpack!({ "k": v, … })` | `QueryValue::Map(…)` via `new_map()` |
/// | `mpack!(@ expr)` | `expr` verbatim — escape hatch for `Dec`/`Big`/`Bin`/`Set`/variables |
///
/// Trailing commas are accepted in both arrays and objects. Empty `[]` and
/// `{}` are valid. Nesting is fully recursive.
///
/// # Int vs float dispatch
///
/// The macro uses a local `MpackIntoValue` trait with `impl` blocks for `i64`
/// and `f64`.  When you write `mpack!(42)`, the literal `42` has an
/// unconstrained integer type; Rust defaults it to `i64` — giving
/// `QueryValue::Int`.  When you write `mpack!(3.5)`, the literal `3.5` has an
/// unconstrained float type; Rust defaults it to `f64` — giving
/// `QueryValue::F64`.  If you need an explicit type, use a suffix:
/// `mpack!(0f64)` forces F64, `mpack!(1i64)` forces Int.
///
/// # Escape hatch `@`
///
/// Any `QueryValue` that cannot be expressed as a literal (e.g. `Dec`,
/// `Big`, `Bin`, `Set`, or a runtime variable) can be injected with `@`:
///
/// ```ignore
/// use rust_decimal::Decimal;
/// use shamir_types::types::value::QueryValue;
///
/// let price = QueryValue::Dec(Decimal::new(1099, 2)); // 10.99
/// let result = mpack!({ "price": @price, "qty": 5 });
/// ```
///
/// # Example
///
/// ```ignore
/// let v = mpack!({
///     "user": {
///         "name": "Alice",
///         "age":  30,
///         "scores": [10, 20, 30],
///         "active": true,
///         "balance": null,
///     }
/// });
/// ```
#[macro_export]
macro_rules! mpack {
    // -----------------------------------------------------------------------
    // Escape hatch: `@expr` — take the expression verbatim (must be QueryValue).
    // -----------------------------------------------------------------------
    (@ $expr:expr) => {
        $expr
    };

    // -----------------------------------------------------------------------
    // Scalars
    // -----------------------------------------------------------------------
    (null) => {
        $crate::types::value::QueryValue::Null
    };

    (true) => {
        $crate::types::value::QueryValue::Bool(true)
    };

    (false) => {
        $crate::types::value::QueryValue::Bool(false)
    };

    // Numeric and string literals.
    //
    // We cannot distinguish integer vs float literals in a single `$n:literal`
    // arm via macro pattern-matching alone.  Instead, we call a helper that
    // relies on Rust's type-inference / default-type rules:
    //   - an unsuffixed integer literal defaults to i64 (matched first),
    //   - an unsuffixed float literal defaults to f64.
    // This gives the same semantics as `serde_json::json!` for numbers.
    //
    // Negative literals: in macro_rules the unary minus `-` is a separate token
    // from the digit literal.  We match `- $n:literal` explicitly so that
    // `mpack!(-7)` and `mpack!(-2.5)` work without requiring the caller to
    // write `mpack!(@ -7)`.
    (- $n:literal) => {
        $crate::macros::mpack::__mpack_into_qv_neg($n)
    };

    ($other:literal) => {
        $crate::macros::mpack::__mpack_into_qv($other)
    };

    // -----------------------------------------------------------------------
    // Array  [ elem, elem, … ]
    //
    // Uses internal `@array` accumulator rules (mirroring serde_json::json!)
    // so that `@expr` escape hatches inside arrays are handled correctly.
    // A simple `$($elem:tt),+` pattern cannot consume multi-token `@(expr)`
    // elements, so we use an accumulator that drains tokens one at a time.
    // -----------------------------------------------------------------------
    ([]) => {
        $crate::types::value::QueryValue::List(::std::vec![])
    };

    ([$($tt:tt)+]) => {{
        #[allow(clippy::vec_init_then_push)]
        let result = {
            let mut __vec: ::std::vec::Vec<$crate::types::value::QueryValue> =
                ::std::vec::Vec::new();
            mpack!(@array __vec [] [$($tt)+]);
            __vec
        };
        $crate::types::value::QueryValue::List(result)
    }};

    // -----------------------------------------------------------------------
    // Internal array accumulator.
    // Pattern: @array <vec> [<current-elem-tokens>] [<remaining>]
    // -----------------------------------------------------------------------

    // Done — remaining is empty, no pending element.
    (@array $vec:ident [] []) => {};

    // Done — remaining has only a trailing comma (already consumed element).
    (@array $vec:ident [] [,]) => {};

    // Flush a pending element followed by a comma, then continue.
    (@array $vec:ident [$($elem:tt)+] [, $($rest:tt)*]) => {
        $vec.push(mpack!($($elem)+));
        mpack!(@array $vec [] [$($rest)*]);
    };

    // Flush the last pending element (end of input, no trailing comma).
    (@array $vec:ident [$($elem:tt)+] []) => {
        $vec.push(mpack!($($elem)+));
    };

    // Element starts with `@` — consume `@ $val:expr` greedily.
    // This arm must come BEFORE the general single-tt munch so that `@`
    // triggers expression-level parsing for the escape hatch.
    // After the expr we expect either `,` or end-of-input.
    (@array $vec:ident [] [@ $val:expr , $($rest:tt)*]) => {
        $vec.push(mpack!(@ $val));
        mpack!(@array $vec [] [$($rest)*]);
    };
    (@array $vec:ident [] [@ $val:expr]) => {
        $vec.push(mpack!(@ $val));
    };

    // Pending element starts with `@` — same logic.
    // (Handles the case where accumulation already started; in practice the
    //  accumulator always flushes before starting a new `@` element, but
    //  guard it anyway for soundness.)
    (@array $vec:ident [$($so_far:tt)+] [@ $val:expr , $($rest:tt)*]) => {
        // Flush whatever was accumulated (shouldn't normally happen).
        $vec.push(mpack!($($so_far)+));
        $vec.push(mpack!(@ $val));
        mpack!(@array $vec [] [$($rest)*]);
    };
    (@array $vec:ident [$($so_far:tt)+] [@ $val:expr]) => {
        $vec.push(mpack!($($so_far)+));
        $vec.push(mpack!(@ $val));
    };

    // Munch one token into the current element accumulator.
    (@array $vec:ident [$($so_far:tt)*] [$tt:tt $($rest:tt)*]) => {
        mpack!(@array $vec [$($so_far)* $tt] [$($rest)*]);
    };

    // -----------------------------------------------------------------------
    // Object  { "key": value, … }
    //
    // Uses internal `@object` accumulator rules (mirroring serde_json::json!)
    // to handle trailing commas and heterogeneous value types correctly.
    // -----------------------------------------------------------------------
    ({}) => {{
        $crate::types::value::QueryValue::Map(
            $crate::types::common::new_map()
        )
    }};

    ({ $($tt:tt)+ }) => {{
        let mut __map = $crate::types::common::new_map();
        mpack!(@object __map () ($($tt)+) ($($tt)+));
        $crate::types::value::QueryValue::Map(__map)
    }};

    // -----------------------------------------------------------------------
    // Internal object accumulator — adapted from serde_json::json! internals.
    // Pattern: @object <map> (<key-so-far>) (<remaining tokens>) (<full copy>)
    // -----------------------------------------------------------------------

    // Done — the remaining token list is empty.
    (@object $map:ident () () ()) => {};

    // Insert current key/value; more items follow (after a comma).
    (@object $map:ident [$($key:tt)+] ($value:expr) , $($rest:tt)*) => {
        let _ = $map.insert(($($key)+).to_string(), $value);
        mpack!(@object $map () ($($rest)*) ($($rest)*));
    };

    // Insert current key/value — end of input (no trailing comma).
    (@object $map:ident [$($key:tt)+] ($value:expr)) => {
        let _ = $map.insert(($($key)+).to_string(), $value);
    };

    // Insert current key/value — trailing comma only (end of input).
    (@object $map:ident [$($key:tt)+] ($value:expr) ,) => {
        let _ = $map.insert(($($key)+).to_string(), $value);
    };

    // Value is an array `[…]`, followed by a comma.
    (@object $map:ident ($($key:tt)+) (: [$($array:tt)*] , $($rest:tt)*) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!([$($array)*])) , $($rest)*);
    };
    // Value is an array `[…]` — end of input (optional trailing comma).
    (@object $map:ident ($($key:tt)+) (: [$($array:tt)*] $(,)?) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!([$($array)*])));
    };

    // Value is an object `{…}`, followed by a comma.
    (@object $map:ident ($($key:tt)+) (: {$($obj:tt)*} , $($rest:tt)*) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!({$($obj)*})) , $($rest)*);
    };
    // Value is an object `{…}` — end of input (optional trailing comma).
    (@object $map:ident ($($key:tt)+) (: {$($obj:tt)*} $(,)?) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!({$($obj)*})));
    };

    // Value is an escape `@expr`, followed by a comma.
    (@object $map:ident ($($key:tt)+) (: @ $val:expr , $($rest:tt)*) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!(@ $val)) , $($rest)*);
    };
    // Value is an escape `@expr` — end of input (optional trailing comma).
    (@object $map:ident ($($key:tt)+) (: @ $val:expr $(,)?) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!(@ $val)));
    };

    // Value is a single `tt` token, followed by a comma.
    (@object $map:ident ($($key:tt)+) (: $value:tt , $($rest:tt)*) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!($value)) , $($rest)*);
    };

    // Value is a single `tt` token — end of input (no comma).
    (@object $map:ident ($($key:tt)+) (: $value:tt) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!($value)));
    };

    // Value is a single `tt` token with trailing comma only.
    (@object $map:ident ($($key:tt)+) (: $value:tt ,) $copy:tt) => {
        mpack!(@object $map [$($key)+] (mpack!($value)));
    };

    // Munch one key token at a time until we hit `:`.
    (@object $map:ident ($($key:tt)*) ($tt:tt $($rest:tt)*) $copy:tt) => {
        mpack!(@object $map ($($key)* $tt) ($($rest)*) ($($rest)*));
    };
}

// ---------------------------------------------------------------------------
// Private helper — converts a literal into QueryValue via type inference.
//
// The trait `MpackIntoValue` has two impls:
//   - `i64`  → QueryValue::Int
//   - `f64`  → QueryValue::F64
//   - `&str` → QueryValue::Str
//
// When the macro writes `__mpack_into_qv(42)`, Rust infers the literal's
// type as `i64` (the integer default) → Int.
// When the macro writes `__mpack_into_qv(3.14)`, Rust infers `f64` → F64.
// When the macro writes `__mpack_into_qv("hi")`, the literal is `&str` → Str.
//
// This helper is `#[doc(hidden)]` and lives in `macros::mpack` so it is
// accessible via the crate-qualified path used inside `mpack!`.
// ---------------------------------------------------------------------------

/// Sealed trait used internally by the `mpack!` macro to dispatch numeric and
/// string literals to the correct `QueryValue` variant.
///
/// Not part of the public API; subject to change without notice.
#[doc(hidden)]
pub trait MpackIntoValue {
    fn __into_qv(self) -> crate::types::value::QueryValue;
}

impl MpackIntoValue for i64 {
    #[inline]
    fn __into_qv(self) -> crate::types::value::QueryValue {
        crate::types::value::QueryValue::Int(self)
    }
}

impl MpackIntoValue for f64 {
    #[inline]
    fn __into_qv(self) -> crate::types::value::QueryValue {
        crate::types::value::QueryValue::F64(self)
    }
}

impl MpackIntoValue for &str {
    #[inline]
    fn __into_qv(self) -> crate::types::value::QueryValue {
        crate::types::value::QueryValue::Str(self.to_string())
    }
}

/// Converts a literal to `QueryValue` via `MpackIntoValue` trait dispatch.
///
/// Called by the `mpack!` macro — not part of the public API.
#[doc(hidden)]
#[inline]
pub fn __mpack_into_qv<T: MpackIntoValue>(v: T) -> crate::types::value::QueryValue {
    v.__into_qv()
}

/// Negates a literal before converting to `QueryValue`.
///
/// Used by `mpack!(- $n:literal)` to handle negative numeric literals —
/// the unary minus is a separate token in macro_rules so we need a dedicated
/// helper.  Called by the `mpack!` macro — not part of the public API.
#[doc(hidden)]
pub trait MpackNegIntoValue {
    fn __neg_into_qv(self) -> crate::types::value::QueryValue;
}

impl MpackNegIntoValue for i64 {
    #[inline]
    fn __neg_into_qv(self) -> crate::types::value::QueryValue {
        crate::types::value::QueryValue::Int(-self)
    }
}

impl MpackNegIntoValue for f64 {
    #[inline]
    fn __neg_into_qv(self) -> crate::types::value::QueryValue {
        crate::types::value::QueryValue::F64(-self)
    }
}

#[doc(hidden)]
#[inline]
pub fn __mpack_into_qv_neg<T: MpackNegIntoValue>(v: T) -> crate::types::value::QueryValue {
    v.__neg_into_qv()
}
