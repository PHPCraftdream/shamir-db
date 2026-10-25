//! Lowering half of the `q!` macro — see [`crate::query_parse`] for the parse half and the module docs.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Ident;

use crate::filter_lower;
use crate::query_parse::{
    CallMacro, CallName, DeleteMacro, DocMapAst, InsertMacro, OrderDir, QMacro, QueryMacro,
    SelectItemAst, TableArg, TableName, UpdateMacro, UpsertMacro,
};

// ── code generation ────────────────────────────────────────────────

impl QueryMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        // -- from --
        let mut chain = match &self.table {
            TableArg::Simple(name) => {
                let table_expr = match name {
                    TableName::Ident(id) => {
                        let s = id.to_string();
                        quote! { #s }
                    }
                    TableName::Lit(lit) => quote! { #lit },
                };
                quote! {
                    ::shamir_query_builder::query::Query::from(#table_expr)
                }
            }
            TableArg::Repo { repo, table } => {
                let r = repo.to_string();
                let t = table.to_string();
                quote! {
                    ::shamir_query_builder::query::Query::with_repo(#r, #t)
                }
            }
        };

        // -- where --
        if let Some(expr) = &self.where_clause {
            let filter_ts = filter_lower::lower_expr(expr)?;
            chain = quote! { #chain.where_(#filter_ts) };
        }

        // -- group_by --
        if !self.group_by_fields.is_empty() {
            let fields: Vec<String> = self
                .group_by_fields
                .iter()
                .map(|id| id.to_string())
                .collect();
            chain = quote! { #chain.group_by_many([#( #fields ),*]) };
        }

        // -- having --
        if let Some(expr) = &self.having_clause {
            let filter_ts = filter_lower::lower_expr(expr)?;
            chain = quote! { #chain.having(#filter_ts) };
        }

        // -- select --
        if !self.select_items.is_empty() {
            let items: Vec<TokenStream2> = self
                .select_items
                .iter()
                .map(lower_select_item)
                .collect::<syn::Result<Vec<_>>>()?;
            chain = quote! { #chain.select([#( #items ),*]) };
        }

        // -- distinct --
        if self.select_distinct {
            chain = quote! { #chain.distinct() };
        }

        // -- order_by --
        for (field, dir) in &self.order_by_items {
            let s = field.to_string();
            match dir {
                OrderDir::Asc => {
                    chain = quote! { #chain.order_by_asc(#s) };
                }
                OrderDir::Desc => {
                    chain = quote! { #chain.order_by_desc(#s) };
                }
            }
        }

        // -- limit --
        if let Some(lit) = &self.limit {
            chain = quote! { #chain.limit(#lit) };
        }

        // -- offset --
        if let Some(lit) = &self.offset {
            chain = quote! { #chain.offset(#lit) };
        }

        // Terminal .build()
        chain = quote! { #chain.build() };

        Ok(chain)
    }
}

// ── write code generation ──────────────────────────────────────────

/// Emit tokens for a `DocMapAst` → `::shamir_query_builder::write::doc().set(k, v)...`.
///
/// `Doc::set` is fallible (RV-QUERY-BUILDER-20D9732A), so each `.set` is
/// followed by an unwrap that panics with the typed error — the macro layer
/// keeps its infallible ergonomics while the setter itself never panics.
///
/// **Note:** over-deep literal values (nesting beyond `MAX_FILTER_DEPTH`)
/// therefore PANIC at macro construction.
fn lower_doc_map(doc: &DocMapAst) -> TokenStream2 {
    let mut chain = quote! { ::shamir_query_builder::write::doc() };
    for (key, val) in &doc.pairs {
        chain = quote! {
            #chain.set(#key, #val).unwrap_or_else(|e| {
                ::core::panic!("q! doc construction failed: {e}")
            })
        };
    }
    chain
}

/// Emit a table constructor prefix for write builders.
fn write_table_tokens(
    table: &TableArg,
    builder_path: TokenStream2,
    simple_fn: TokenStream2,
) -> TokenStream2 {
    match table {
        TableArg::Simple(name) => {
            let table_expr = match name {
                TableName::Ident(id) => {
                    let s = id.to_string();
                    quote! { #s }
                }
                TableName::Lit(lit) => quote! { #lit },
            };
            quote! { #builder_path::#simple_fn(#table_expr) }
        }
        TableArg::Repo { repo, table } => {
            let r = repo.to_string();
            let t = table.to_string();
            quote! { #builder_path::with_repo(#r, #t) }
        }
    }
}

impl InsertMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { ::shamir_query_builder::write::Insert },
            quote! { into },
        );
        for doc in &self.docs {
            let doc_ts = lower_doc_map(doc);
            chain = quote! { #chain.row(#doc_ts) };
        }
        Ok(quote! { #chain.build() })
    }
}

impl UpdateMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { ::shamir_query_builder::write::Update },
            quote! { table },
        );
        let doc_ts = lower_doc_map(&self.set_doc);
        chain = quote! { #chain.set(#doc_ts) };
        if let Some(expr) = &self.where_clause {
            let filter_ts = filter_lower::lower_expr(expr)?;
            chain = quote! { #chain.where_(#filter_ts) };
        }
        Ok(quote! { #chain.build() })
    }
}

impl DeleteMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { ::shamir_query_builder::write::Delete },
            quote! { from_table },
        );
        let filter_ts = filter_lower::lower_expr(&self.where_clause)?;
        chain = quote! { #chain.where_(#filter_ts) };
        Ok(quote! { #chain.build() })
    }
}

impl UpsertMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { ::shamir_query_builder::write::Upsert },
            quote! { table },
        );
        let key_ts = lower_doc_map(&self.key_doc);
        let val_ts = lower_doc_map(&self.value_doc);
        chain = quote! { #chain.key(#key_ts).value(#val_ts) };
        Ok(quote! { #chain.build() })
    }
}

impl CallMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        let name_expr = match &self.name {
            CallName::Ident(id) => {
                let s = id.to_string();
                quote! { #s }
            }
            CallName::Lit(lit) => quote! { #lit },
        };

        let args = &self.args;

        Ok(quote! {
            ::shamir_query_types::call::CallOp {
                call: ::std::string::String::from(#name_expr),
                params: ::std::vec![#(
                    ::std::convert::Into::<::shamir_query_types::filter::FilterValue>::into(#args)
                ),*],
                repo: ::std::string::String::from("main"),
            }
        })
    }
}

impl QMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        match self {
            QMacro::Read(qm) => qm.to_tokens(),
            QMacro::Insert(im) => im.to_tokens(),
            QMacro::Update(um) => um.to_tokens(),
            QMacro::Delete(dm) => dm.to_tokens(),
            QMacro::Upsert(um) => um.to_tokens(),
            QMacro::Call(cm) => cm.to_tokens(),
        }
    }
}

/// Lower a single `SelectItemAst` node into a token stream.
fn lower_select_item(item: &SelectItemAst) -> syn::Result<TokenStream2> {
    match item {
        SelectItemAst::All => Ok(quote! {
            ::shamir_query_builder::select::all()
        }),

        SelectItemAst::Field { segments, alias } => {
            let field = segments_to_field_path(segments);
            match alias {
                Some(a) => {
                    let alias_str = a.to_string();
                    Ok(quote! {
                        ::shamir_query_builder::select::field_as(#field, #alias_str)
                    })
                }
                None => Ok(quote! {
                    ::shamir_query_builder::select::field(#field)
                }),
            }
        }

        SelectItemAst::CountAll { alias } => {
            let alias_str = alias
                .as_ref()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "count".to_owned());
            Ok(quote! {
                ::shamir_query_builder::select::count_all(#alias_str)
            })
        }

        SelectItemAst::BuiltinAgg {
            func,
            field_segments,
            alias,
        } => {
            let field = segments_to_field_path(field_segments);
            let alias_str = alias.to_string();
            let func_ident = syn::Ident::new(func, proc_macro2::Span::call_site());
            Ok(quote! {
                ::shamir_query_builder::select::#func_ident(#field, #alias_str)
            })
        }

        SelectItemAst::AggFn {
            name,
            field_segments,
            alias,
        } => {
            let field = segments_to_field_path(field_segments);
            let alias_str = alias.to_string();
            Ok(quote! {
                ::shamir_query_builder::select::agg_fn(#name, #field, #alias_str)
            })
        }

        SelectItemAst::Func { name, args, alias } => {
            let alias_str = alias.to_string();
            Ok(quote! {
                ::shamir_query_builder::select::func(#alias_str, #name, #args)
            })
        }
    }
}

/// Convert a vector of ident segments into a field path token stream.
fn segments_to_field_path(segments: &[Ident]) -> TokenStream2 {
    if segments.len() == 1 {
        let s = segments[0].to_string();
        quote! { #s }
    } else {
        let strings: Vec<String> = segments.iter().map(|id| id.to_string()).collect();
        quote! { [#( #strings ),*] }
    }
}

pub fn q_macro(input: TokenStream) -> TokenStream {
    let qm = syn::parse_macro_input!(input as QMacro);
    match qm.to_tokens() {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
