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
