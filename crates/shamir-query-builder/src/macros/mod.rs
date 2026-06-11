//! Declarative macros for concise query building: `doc!` and `vals!`.

/// Build a [`Doc`](crate::write::Doc) from key-value pairs.
///
/// Trailing comma is allowed.
///
/// **Note:** inside the `shamir-query-builder` crate itself (and other
/// crates that import this macro into a module that also uses `#[doc]`
/// attributes), the bare name `doc` can clash with the built-in `#[doc]`
/// attribute path. In that case use the function form
/// [`write::doc()`](crate::write::doc) + `.set(...)` — it is equally
/// expressive. From downstream crates `doc!` works as shown.
///
/// # Example
///
/// ```ignore
/// use shamir_query_builder::{doc, val::col};
///
/// let d = doc! {
///     "name" => "Alice",
///     "email_lower" => col("email"),
/// };
/// ```
#[macro_export]
macro_rules! doc {
    ( $( $key:expr => $val:expr ),* $(,)? ) => {{
        #[allow(unused_mut)]
        let mut __doc = $crate::write::doc();
        $( __doc = __doc.set($key, $val); )*
        __doc
    }};
}

/// Build a `Vec<FilterValue>` from literal values.
///
/// Each element is wrapped in [`val::lit`](crate::val::lit).
/// Trailing comma is allowed.
///
/// # Example
///
/// ```ignore
/// use shamir_query_builder::vals;
///
/// let v = vals![1, 2, 3];
/// ```
#[macro_export]
macro_rules! vals {
    ( $( $val:expr ),* $(,)? ) => {
        ::std::vec![ $( $crate::val::lit($val) ),* ]
    };
}

/// Build a `TMap<String, FilterValue>` for sub-batch parameter bindings.
///
/// # Usage
/// ```ignore
/// bind! {
///     "user_id" => val::lit(42),
///     "thread_id" => val::qref("@messages[0].thread_id"),
/// }
/// ```
#[macro_export]
macro_rules! bind {
    ($($key:expr => $val:expr),* $(,)?) => {{
        let mut map = shamir_collections::new_map();
        $(map.insert($key.into(), $val);)*
        map
    }};
}

/// Build a [`SubscribeOp`](shamir_query_types::subscribe::SubscribeOp) declaratively.
///
/// # Example
///
/// ```ignore
/// use shamir_query_builder::{subscribe, filter};
///
/// let op = subscribe! {
///     source: ("main", "messages"),
///     where: filter!(thread_id == 42),
///     on: put,
///     deliver: keys,
/// };
/// ```
#[macro_export]
macro_rules! subscribe {
    // Internal: event mask mapping
    (@event any) => { shamir_query_types::subscribe::EventMask::All };
    (@event put) => { shamir_query_types::subscribe::EventMask::Put };
    (@event delete) => { shamir_query_types::subscribe::EventMask::Delete };

    (
        source: ($repo:expr, $table:expr),
        where: $filter:expr
        $(, on: $event:ident)?
        $(, initial: $init:expr)?
        $(, from_version: $ver:expr)?
        $(, deliver: records)?
        $(,)?
    ) => {{
        let __src = $crate::batch::subscribe::SourceBuilder::table(
            shamir_query_types::TableRef::with_repo($repo, $table)
        )
        .filter($filter)
        $(.events($crate::subscribe!(@event $event)))?
        .build();

        #[allow(unused_mut)]
        let mut __sub = $crate::batch::subscribe::Subscribe::source(__src);
        $(__sub = if $init { __sub.with_initial() } else { __sub };)?
        $(__sub = __sub.from_version($ver);)?
        __sub.build()
    }};

    (
        source: ($repo:expr, $table:expr),
        where: $filter:expr
        $(, on: $event:ident)?
        $(, initial: $init:expr)?
        $(, from_version: $ver:expr)?
        , deliver: keys
        $(,)?
    ) => {{
        let __src = $crate::batch::subscribe::SourceBuilder::table(
            shamir_query_types::TableRef::with_repo($repo, $table)
        )
        .filter($filter)
        $(.events($crate::subscribe!(@event $event)))?
        .build();

        #[allow(unused_mut)]
        let mut __sub = $crate::batch::subscribe::Subscribe::source(__src)
            .deliver_keys();
        $(__sub = if $init { __sub.with_initial() } else { __sub };)?
        $(__sub = __sub.from_version($ver);)?
        __sub.build()
    }};

    (
        source: ($repo:expr, $table:expr),
        where: $filter:expr
        $(, on: $event:ident)?
        $(, initial: $init:expr)?
        $(, from_version: $ver:expr)?
        , deliver: batch($batch:expr, $bind:expr)
        $(,)?
    ) => {{
        let __src = $crate::batch::subscribe::SourceBuilder::table(
            shamir_query_types::TableRef::with_repo($repo, $table)
        )
        .filter($filter)
        $(.events($crate::subscribe!(@event $event)))?
        .build();

        #[allow(unused_mut)]
        let mut __sub = $crate::batch::subscribe::Subscribe::source(__src)
            .deliver_batch(shamir_query_types::batch::SubBatchOp {
                batch: $batch,
                bind: $bind,
            });
        $(__sub = if $init { __sub.with_initial() } else { __sub };)?
        $(__sub = __sub.from_version($ver);)?
        __sub.build()
    }};

    (
        source: ($repo:expr, $table:expr),
        where: $filter:expr
        $(, on: $event:ident)?
        $(, initial: $init:expr)?
        $(, from_version: $ver:expr)?
        , deliver: call($call:expr)
        $(,)?
    ) => {{
        let __src = $crate::batch::subscribe::SourceBuilder::table(
            shamir_query_types::TableRef::with_repo($repo, $table)
        )
        .filter($filter)
        $(.events($crate::subscribe!(@event $event)))?
        .build();

        #[allow(unused_mut)]
        let mut __sub = $crate::batch::subscribe::Subscribe::source(__src)
            .deliver_call($call);
        $(__sub = if $init { __sub.with_initial() } else { __sub };)?
        $(__sub = __sub.from_version($ver);)?
        __sub.build()
    }};
}

#[cfg(test)]
mod tests;
