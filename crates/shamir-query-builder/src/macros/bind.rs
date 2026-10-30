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
        let mut map: $crate::shamir_collections::TMap<
            ::std::string::String,
            $crate::shamir_query_types::filter::FilterValue,
        > = $crate::shamir_collections::new_map();
        $(map.insert($key.into(), $val);)*
        map
    }};
}
