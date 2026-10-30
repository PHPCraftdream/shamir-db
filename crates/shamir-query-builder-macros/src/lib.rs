//! Proc-macros for `shamir-query-builder`: `filter!` and `q!`.
//!
//! The public builder facade supplies the hygienic root path.

use syn::parse::{Parse, ParseStream};
use syn::Token;

struct MacroInput<T> {
    root: proc_macro2::TokenStream,
    value: T,
}

impl<T: Parse> Parse for MacroInput<T> {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut root = proc_macro2::TokenStream::new();
        while !input.is_empty() && !input.peek(Token![;]) {
            root.extend(std::iter::once(input.parse::<proc_macro2::TokenTree>()?));
        }
        input.parse::<Token![;]>()?;
        Ok(Self {
            root,
            value: input.parse()?,
        })
    }
}

mod filter_lower;
mod query_parse;
mod query_parse_lower;

#[cfg(test)]
mod tests;

#[doc(hidden)]
#[proc_macro]
pub fn __filter(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    filter_lower::filter_macro(input)
}

#[doc(hidden)]
#[proc_macro]
pub fn __q(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    query_parse::q_macro(input)
}
