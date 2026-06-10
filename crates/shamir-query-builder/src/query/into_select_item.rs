use shamir_query_types::read::SelectItem;

/// Anything convertible into a [`SelectItem`].
///
/// Implemented for `&str` / `String` (→ `SelectItem::Field`) and
/// `SelectItem` itself (passthrough), so both
/// `.select(["a", "b"])` and `.select([select::func(..)])` work.
pub trait IntoSelectItem {
    /// Convert into a select item.
    fn into_select_item(self) -> SelectItem;
}

impl IntoSelectItem for &str {
    fn into_select_item(self) -> SelectItem {
        crate::select::field(self)
    }
}

impl IntoSelectItem for String {
    fn into_select_item(self) -> SelectItem {
        crate::select::field(self)
    }
}

impl IntoSelectItem for SelectItem {
    fn into_select_item(self) -> SelectItem {
        self
    }
}
