/// Build a [`SubscribeOp`] declaratively.
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
    (@event any) => { $crate::shamir_query_types::subscribe::EventMask::All };
    (@event put) => { $crate::shamir_query_types::subscribe::EventMask::Put };
    (@event delete) => { $crate::shamir_query_types::subscribe::EventMask::Delete };

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
            $crate::shamir_query_types::TableRef::with_repo($repo, $table)
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
            $crate::shamir_query_types::TableRef::with_repo($repo, $table)
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
            $crate::shamir_query_types::TableRef::with_repo($repo, $table)
        )
        .filter($filter)
        $(.events($crate::subscribe!(@event $event)))?
        .build();

        #[allow(unused_mut)]
        let mut __sub = $crate::batch::subscribe::Subscribe::source(__src)
            .deliver_batch($crate::shamir_query_types::batch::SubBatchOp {
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
            $crate::shamir_query_types::TableRef::with_repo($repo, $table)
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
