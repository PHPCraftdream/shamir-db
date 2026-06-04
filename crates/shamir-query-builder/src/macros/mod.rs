//! Declarative macros for concise query building: `doc!` and `vals!`.

/// Build a [`Doc`](crate::write::Doc) from key-value pairs.
///
/// Trailing comma is allowed.
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

#[cfg(test)]
mod tests;
