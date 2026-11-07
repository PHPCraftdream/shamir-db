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
/// **Note:** over-deep literal values (nesting beyond `MAX_FILTER_DEPTH`)
/// PANIC at construction — `doc!` unwraps the fallible `Doc::set`. Use
/// [`Doc::set`](crate::write::Doc::set) directly when the error must be
/// handled.
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
        $( __doc = match __doc.set($key, $val) {
            ::core::result::Result::Ok(d) => d,
            ::core::result::Result::Err(e) =>
                ::core::panic!("doc! construction failed: {e}"),
        }; )*
        __doc
    }};
}
