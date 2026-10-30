//! Lowering half of the `q!` macro — see [`crate::query_parse`] for parsing.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Ident;

use crate::filter_lower::{ident_literal, segments_to_field_path};
use crate::query_parse::{
    CallMacro, CallName, DeleteMacro, DocMapAst, FieldPathAst, InsertMacro, OrderDir, QMacro,
    QueryMacro, SelectItemAst, TableArg, TableName, UpdateMacro, UpsertMacro,
};
use crate::{filter_lower, MacroInput};

// ── code generation ────────────────────────────────────────────────

impl QueryMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        // -- from --
        let mut chain = match &self.table {
            TableArg::Simple(name) => {
                let table_expr = match name {
                    TableName::Ident(id) => {
                        let s = unraw(id);
                        quote! { #s }
                    }
                    TableName::Lit(lit) => quote! { #lit },
                };
                quote! {
                    #root::query::Query::from(#table_expr)
                }
            }
            TableArg::Repo { repo, table } => {
                let r = unraw(repo);
                let t = unraw(table);
                quote! {
                    #root::query::Query::with_repo(#r, #t)
                }
            }
        };

        // -- where --
        if let Some(expr) = &self.where_clause {
            let filter_ts = filter_lower::lower_expr(expr, root)?;
            chain = quote! { #chain.where_(#filter_ts) };
        }

        // -- group_by --
        if !self.group_by_fields.is_empty() {
            let fields: Vec<TokenStream2> = self
                .group_by_fields
                .iter()
                .map(|field| match field {
                    FieldPathAst::Idents(names) => {
                        let names: Vec<_> = names.iter().map(ident_literal).collect();
                        quote! { vec![#(#names.to_owned()),*] }
                    }
                    FieldPathAst::Lit(lit) => quote! {
                        vec![#lit.to_owned()]
                    },
                })
                .collect();
            chain = quote! { #chain.group_by_many([#( #fields ),*]) };
        }

        // -- having --
        if let Some(expr) = &self.having_clause {
            let filter_ts = filter_lower::lower_expr(expr, root)?;
            chain = quote! { #chain.having(#filter_ts) };
        }

        // -- select --
        if !self.select_items.is_empty() {
            let items: Vec<TokenStream2> = self
                .select_items
                .iter()
                .map(|item| lower_select_item(item, root))
                .collect::<syn::Result<Vec<_>>>()?;
            chain = quote! { #chain.select([#( #items ),*]) };
        }

        // -- distinct --
        if self.select_distinct {
            chain = quote! { #chain.distinct() };
        }

        // -- order_by --
        let mut order_by = Vec::with_capacity(self.order_by_items.len());
        for (field, dir) in &self.order_by_items {
            let path = field_path_tokens(field);
            order_by.push(match dir {
                OrderDir::Asc => quote! { .order_by_asc(#path) },
                OrderDir::Desc => quote! { .order_by_desc(#path) },
            });
        }
        chain = quote! { #chain #(#order_by)* };

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

/// Emit tokens for a `DocMapAst` through the builder's `write::doc()`.
///
/// `Doc::set` is fallible (RV-QUERY-BUILDER-20D9732A), so each `.set` is
/// followed by an unwrap that panics with the typed error — the macro layer
/// keeps its infallible ergonomics while the setter itself never panics.
///
/// **Note:** over-deep literal values (nesting beyond `MAX_FILTER_DEPTH`)
/// therefore PANIC at macro construction.
fn lower_doc_map(doc: &DocMapAst, root: &TokenStream2) -> TokenStream2 {
    let entries: Vec<TokenStream2> = doc
        .pairs
        .iter()
        .map(|(key, val)| {
            quote! {
                .set(#key, #val).unwrap_or_else(|e| {
                    ::core::panic!("q! doc construction failed: {e}")
                })
            }
        })
        .collect();
    quote! { #root::write::doc() #(#entries)* }
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
                    let s = unraw(id);
                    quote! { #s }
                }
                TableName::Lit(lit) => quote! { #lit },
            };
            quote! { #builder_path::#simple_fn(#table_expr) }
        }
        TableArg::Repo { repo, table } => {
            let r = unraw(repo);
            let t = unraw(table);
            quote! { #builder_path::with_repo(#r, #t) }
        }
    }
}

impl InsertMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        let chain = write_table_tokens(
            &self.table,
            quote! { #root::write::Insert },
            quote! { into },
        );
        let rows: Vec<TokenStream2> = self
            .docs
            .iter()
            .map(|doc| {
                let doc_ts = lower_doc_map(doc, root);
                quote! { .row(#doc_ts) }
            })
            .collect();
        Ok(quote! { #chain #(#rows)* .build() })
    }
}

impl UpdateMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { #root::write::Update },
            quote! { table },
        );
        let doc_ts = lower_doc_map(&self.set_doc, root);
        chain = quote! { #chain.set(#doc_ts) };
        if let Some(expr) = &self.where_clause {
            let filter_ts = filter_lower::lower_expr(expr, root)?;
            chain = quote! { #chain.where_(#filter_ts) };
        }
        Ok(quote! { #chain.build() })
    }
}

impl DeleteMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { #root::write::Delete },
            quote! { from_table },
        );
        let filter_ts = filter_lower::lower_expr(&self.where_clause, root)?;
        chain = quote! { #chain.where_(#filter_ts) };
        Ok(quote! { #chain.build() })
    }
}

impl UpsertMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        let mut chain = write_table_tokens(
            &self.table,
            quote! { #root::write::Upsert },
            quote! { table },
        );
        let key_ts = lower_doc_map(&self.key_doc, root);
        let val_ts = lower_doc_map(&self.value_doc, root);
        chain = quote! { #chain.key(#key_ts).value(#val_ts) };
        Ok(quote! { #chain.build() })
    }
}

impl CallMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        let name_expr = match &self.name {
            CallName::Ident(id) => {
                let s = unraw(id);
                quote! { #s }
            }
            CallName::Lit(lit) => quote! { #lit },
        };

        let args = &self.args;
        let repo_expr = match &self.repo {
            Some(id) => {
                let r = unraw(id);
                quote! { #r }
            }
            None => quote! { "main" },
        };

        // The builder facade re-exports the query types used here.
        Ok(quote! {
            #root::CallOp {
                call: ::std::string::String::from(#name_expr),
                params: ::std::vec![#(
                    ::std::convert::Into::<#root::FilterValue>::into(#args)
                ),*],
                repo: ::std::string::String::from(#repo_expr),
            }
        })
    }
}

impl QMacro {
    pub(crate) fn to_tokens(&self, root: &TokenStream2) -> syn::Result<TokenStream2> {
        match self {
            QMacro::Read(qm) => qm.to_tokens(root),
            QMacro::Insert(im) => im.to_tokens(root),
            QMacro::Update(um) => um.to_tokens(root),
            QMacro::Delete(dm) => dm.to_tokens(root),
            QMacro::Upsert(um) => um.to_tokens(root),
            QMacro::Call(cm) => cm.to_tokens(root),
        }
    }
}

fn field_path_tokens(field: &FieldPathAst) -> TokenStream2 {
    match field {
        FieldPathAst::Lit(lit) => quote! { #lit },
        FieldPathAst::Idents(segments) => segments_to_field_path(segments),
    }
}

fn unraw(ident: &Ident) -> String {
    let value = ident.to_string();
    value.strip_prefix("r#").unwrap_or(&value).to_owned()
}

fn lower_select_item(item: &SelectItemAst, root: &TokenStream2) -> syn::Result<TokenStream2> {
    match item {
        SelectItemAst::All => Ok(quote! {
            #root::select::all()
        }),

        SelectItemAst::Field { segments, alias } => {
            let field = segments_to_field_path(segments);
            match alias {
                Some(a) => {
                    let alias_str = ident_literal(a);
                    Ok(quote! {
                        #root::select::field_as(#field, #alias_str)
                    })
                }
                None => Ok(quote! {
                    #root::select::field(#field)
                }),
            }
        }

        SelectItemAst::CountAll { alias } => {
            let alias_str = alias
                .as_ref()
                .map(ident_literal)
                .unwrap_or_else(|| syn::LitStr::new("count", proc_macro2::Span::call_site()));
            Ok(quote! {
                #root::select::count_all(#alias_str)
            })
        }

        SelectItemAst::BuiltinAgg {
            func,
            field_segments,
            alias,
        } => {
            let field = segments_to_field_path(field_segments);
            let alias_str = ident_literal(alias);
            let func_ident = syn::Ident::new(func, proc_macro2::Span::call_site());
            Ok(quote! {
                #root::select::#func_ident(#field, #alias_str)
            })
        }

        SelectItemAst::AggFn {
            name,
            field_segments,
            alias,
        } => {
            let field = segments_to_field_path(field_segments);
            let alias_str = ident_literal(alias);
            Ok(quote! {
                #root::select::agg_fn(#name, #field, #alias_str)
            })
        }

        SelectItemAst::Func { name, args, alias } => {
            let alias_str = ident_literal(alias);
            Ok(quote! {
                #root::select::func(#alias_str, #name, #args)
            })
        }
    }
}

pub(crate) fn q_macro(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as MacroInput<QMacro>);
    match input.value.to_tokens(&input.root) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Render a parsed [`QMacro`] to its expansion string (unit-test helper).
#[cfg(test)]
pub(crate) fn render(qm: &QMacro) -> syn::Result<String> {
    qm.to_tokens(&quote! { ::shamir_query_builder })
        .map(|ts| ts.to_string())
}
