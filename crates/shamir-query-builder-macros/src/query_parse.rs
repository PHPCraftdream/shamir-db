//! Parser and code-generator for the `q!` macro.
//!
//! ## Statement types (first keyword selects)
//!
//! ```text
//! q!( from <table> ...)           → ReadQuery  (unchanged)
//! q!( insert into <table> ...)    → InsertOp
//! q!( update <table> ...)         → UpdateOp
//! q!( delete from <table> ...)    → DeleteOp
//! q!( upsert <table> ...)         → SetOp (upsert)
//! ```
//!
//! ## Read clause order (fixed)
//!
//! ```text
//! q!( from <table | repo.table | "table">
//!     [where <filter-expr>]
//!     [group_by <field>, ...]
//!     [having <filter-expr>]
//!     [select [distinct] <select-items>]
//!     [order_by <field> (asc|desc), ...]
//!     [limit <N>]
//!     [offset <N>]
//! )
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, LitInt, LitStr, Token};

use crate::filter_lower;

// ── custom keywords ────────────────────────────────────────────────

mod kw {
    syn::custom_keyword!(from);
    syn::custom_keyword!(select);
    syn::custom_keyword!(distinct);
    syn::custom_keyword!(group_by);
    syn::custom_keyword!(having);
    syn::custom_keyword!(order_by);
    syn::custom_keyword!(limit);
    syn::custom_keyword!(offset);
    syn::custom_keyword!(asc);
    syn::custom_keyword!(desc);
    // write statement keywords
    syn::custom_keyword!(insert);
    syn::custom_keyword!(into);
    syn::custom_keyword!(values);
    syn::custom_keyword!(update);
    syn::custom_keyword!(set);
    syn::custom_keyword!(delete);
    syn::custom_keyword!(upsert);
    syn::custom_keyword!(key);
    syn::custom_keyword!(value);
}

// ── AST ────────────────────────────────────────────────────────────

struct QueryMacro {
    table: TableArg,
    where_clause: Option<Expr>,
    group_by_fields: Vec<Ident>,
    having_clause: Option<Expr>,
    select_items: Vec<SelectItemAst>,
    select_distinct: bool,
    order_by_items: Vec<(Ident, OrderDir)>,
    limit: Option<LitInt>,
    offset: Option<LitInt>,
}

enum TableArg {
    /// Single ident or string literal → `Query::from(...)`.
    Simple(TableName),
    /// `repo.table` → `Query::with_repo("repo", "table")`.
    Repo { repo: Ident, table: Ident },
}

enum TableName {
    Ident(Ident),
    Lit(LitStr),
}

enum OrderDir {
    Asc,
    Desc,
}

/// AST node for a single select item.
enum SelectItemAst {
    /// `*`
    All,
    /// `field` or `field as alias` or `a.b` or `a.b as alias`
    Field {
        segments: Vec<Ident>,
        alias: Option<Ident>,
    },
    /// `count(*)` [as alias]
    CountAll { alias: Option<Ident> },
    /// `count(field) as alias` / `sum(field) as alias` / etc.
    BuiltinAgg {
        func: String,
        field_segments: Vec<Ident>,
        alias: Ident,
    },
    /// `agg_fn("name", field) as alias`
    AggFn {
        name: LitStr,
        field_segments: Vec<Ident>,
        alias: Ident,
    },
    /// `func("ns/name", [args]) as alias`
    Func {
        name: LitStr,
        args: Expr,
        alias: Ident,
    },
}

// ── Write AST types ───────────────────────────────────────────────

/// A single doc-map literal: `{ "key" => expr, ... }`.
struct DocMapAst {
    pairs: Vec<(LitStr, Expr)>,
}

/// `q!(insert into <table> values <doc> [, <doc>]*)`.
struct InsertMacro {
    table: TableArg,
    docs: Vec<DocMapAst>,
}

/// `q!(update <table> set <doc> [where <expr>])`.
struct UpdateMacro {
    table: TableArg,
    set_doc: DocMapAst,
    where_clause: Option<Expr>,
}

/// `q!(delete from <table> where <expr>)`.
struct DeleteMacro {
    table: TableArg,
    where_clause: Expr,
}

/// `q!(upsert <table> key <doc> value <doc>)`.
struct UpsertMacro {
    table: TableArg,
    key_doc: DocMapAst,
    value_doc: DocMapAst,
}

/// Top-level AST: the first keyword selects the variant.
enum QMacro {
    Read(Box<QueryMacro>),
    Insert(InsertMacro),
    Update(UpdateMacro),
    Delete(DeleteMacro),
    Upsert(UpsertMacro),
}

// ── parser ─────────────────────────────────────────────────────────

impl Parse for QMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(kw::from) {
            Ok(QMacro::Read(Box::new(input.parse()?)))
        } else if input.peek(kw::insert) {
            Ok(QMacro::Insert(input.parse()?))
        } else if input.peek(kw::update) {
            Ok(QMacro::Update(input.parse()?))
        } else if input.peek(kw::delete) {
            Ok(QMacro::Delete(input.parse()?))
        } else if input.peek(kw::upsert) {
            Ok(QMacro::Upsert(input.parse()?))
        } else {
            Err(input.error("q!: expected `from`, `insert`, `update`, `delete`, or `upsert`"))
        }
    }
}

impl Parse for QueryMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // -- from <table> or <repo.table> (required) --
        input.parse::<kw::from>()?;
        let table = parse_table_arg(input)?;

        // -- where <expr> (optional) --
        let where_clause = if input.peek(Token![where]) {
            input.parse::<Token![where]>()?;
            let expr = parse_filter_expr(input)?;
            Some(expr)
        } else {
            None
        };

        // -- group_by <field>, ... (optional) --
        let mut group_by_fields = Vec::new();
        if input.peek(kw::group_by) {
            input.parse::<kw::group_by>()?;
            loop {
                group_by_fields.push(input.parse::<Ident>()?);
                if input.peek(Token![,]) && !peek_clause_keyword_after_comma(input) {
                    input.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
        }

        // -- having <expr> (optional, after group_by) --
        let having_clause = if input.peek(kw::having) {
            input.parse::<kw::having>()?;
            let expr = parse_filter_expr(input)?;
            Some(expr)
        } else {
            None
        };

        // -- select [distinct] <items> (optional) --
        let mut select_items = Vec::new();
        let mut select_distinct = false;
        if input.peek(kw::select) {
            input.parse::<kw::select>()?;
            if input.peek(kw::distinct) {
                input.parse::<kw::distinct>()?;
                select_distinct = true;
            }
            loop {
                select_items.push(parse_select_item(input)?);
                if input.peek(Token![,]) && !peek_clause_keyword_after_comma(input) {
                    input.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
        }

        // -- order_by <field> (asc|desc), ... (optional) --
        let mut order_by_items = Vec::new();
        if input.peek(kw::order_by) {
            input.parse::<kw::order_by>()?;
            loop {
                let field = input.parse::<Ident>()?;
                let dir = if input.peek(kw::desc) {
                    input.parse::<kw::desc>()?;
                    OrderDir::Desc
                } else if input.peek(kw::asc) {
                    input.parse::<kw::asc>()?;
                    OrderDir::Asc
                } else {
                    return Err(input.error("order_by: expected `asc` or `desc` after field name"));
                };
                order_by_items.push((field, dir));
                if input.peek(Token![,]) && !peek_clause_keyword_after_comma(input) {
                    input.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
        }

        // -- limit <N> (optional) --
        let limit = if input.peek(kw::limit) {
            input.parse::<kw::limit>()?;
            Some(input.parse::<LitInt>()?)
        } else {
            None
        };

        // -- offset <N> (optional) --
        let offset = if input.peek(kw::offset) {
            input.parse::<kw::offset>()?;
            Some(input.parse::<LitInt>()?)
        } else {
            None
        };

        if !input.is_empty() {
            return Err(input.error(
                "q!: unexpected tokens after query; clauses must appear \
                 in order: from, where, group_by, having, select, \
                 order_by, limit, offset",
            ));
        }

        Ok(QueryMacro {
            table,
            where_clause,
            group_by_fields,
            having_clause,
            select_items,
            select_distinct,
            order_by_items,
            limit,
            offset,
        })
    }
}

/// Parse `from <table>` argument: ident, string literal, or repo.table.
fn parse_table_arg(input: ParseStream) -> syn::Result<TableArg> {
    if input.peek(LitStr) {
        let lit = input.parse::<LitStr>()?;
        return Ok(TableArg::Simple(TableName::Lit(lit)));
    }

    let first = input.parse::<Ident>()?;

    // Check for `repo.table` (dotted pair).
    if input.peek(Token![.]) {
        // Fork to make sure the thing after `.` is an ident, not a keyword
        let fork = input.fork();
        fork.parse::<Token![.]>()?;
        if fork.peek(Ident) {
            // Consume from actual stream
            input.parse::<Token![.]>()?;
            let second = input.parse::<Ident>()?;
            return Ok(TableArg::Repo {
                repo: first,
                table: second,
            });
        }
    }

    Ok(TableArg::Simple(TableName::Ident(first)))
}

/// Parse a single select item.
///
/// Forms:
/// - `*`
/// - `ident` or `a.b` [as alias]
/// - `count(*)` [as alias]
/// - `count|sum|avg|min|max(field)` as alias  (alias required)
/// - `agg_fn("name", field)` as alias
/// - `func("ns/name", [args])` as alias
fn parse_select_item(input: ParseStream) -> syn::Result<SelectItemAst> {
    // `*` wildcard
    if input.peek(Token![*]) {
        input.parse::<Token![*]>()?;
        return Ok(SelectItemAst::All);
    }

    // Check if this is a function-like call: ident(...)
    let fork = input.fork();
    if fork.peek(Ident) {
        let id: Ident = fork.parse()?;
        let id_str = id.to_string();

        // Check for function call form: ident(...)
        if fork.peek(syn::token::Paren) {
            match id_str.as_str() {
                "count" => {
                    // Advance past ident in real stream
                    input.parse::<Ident>()?;
                    let content;
                    syn::parenthesized!(content in input);

                    // count(*) or count(field)
                    if content.peek(Token![*]) {
                        content.parse::<Token![*]>()?;
                        let alias = parse_optional_as(input)?;
                        return Ok(SelectItemAst::CountAll { alias });
                    }

                    // count(field)
                    let field_segs = parse_dotted_ident_from(&content)?;
                    let alias = parse_required_as(input, "count")?;
                    return Ok(SelectItemAst::BuiltinAgg {
                        func: "count".to_owned(),
                        field_segments: field_segs,
                        alias,
                    });
                }
                "sum" | "avg" | "min" | "max" => {
                    input.parse::<Ident>()?;
                    let content;
                    syn::parenthesized!(content in input);
                    let field_segs = parse_dotted_ident_from(&content)?;
                    let alias = parse_required_as(input, &id_str)?;
                    return Ok(SelectItemAst::BuiltinAgg {
                        func: id_str,
                        field_segments: field_segs,
                        alias,
                    });
                }
                "agg_fn" => {
                    input.parse::<Ident>()?;
                    let content;
                    syn::parenthesized!(content in input);
                    let name: LitStr = content.parse()?;
                    content.parse::<Token![,]>()?;
                    let field_segs = parse_dotted_ident_from(&content)?;
                    let alias = parse_required_as(input, "agg_fn")?;
                    return Ok(SelectItemAst::AggFn {
                        name,
                        field_segments: field_segs,
                        alias,
                    });
                }
                "func" => {
                    input.parse::<Ident>()?;
                    let content;
                    syn::parenthesized!(content in input);
                    let name: LitStr = content.parse()?;
                    content.parse::<Token![,]>()?;
                    let args: Expr = content.parse()?;
                    let alias = parse_required_as(input, "func")?;
                    return Ok(SelectItemAst::Func { name, args, alias });
                }
                _ => {
                    // Not a known function — fall through to field parsing
                }
            }
        }
    }

    // Plain field: ident or a.b [as alias]
    let mut segments = vec![input.parse::<Ident>()?];
    while input.peek(Token![.]) {
        // Make sure it's not a clause keyword after the dot
        let fork = input.fork();
        fork.parse::<Token![.]>()?;
        if fork.peek(Ident) {
            input.parse::<Token![.]>()?;
            segments.push(input.parse::<Ident>()?);
        } else {
            break;
        }
    }

    let alias = parse_optional_as(input)?;
    Ok(SelectItemAst::Field { segments, alias })
}

/// Parse `as <ident>` if present.
fn parse_optional_as(input: ParseStream) -> syn::Result<Option<Ident>> {
    if input.peek(Token![as]) {
        input.parse::<Token![as]>()?;
        Ok(Some(input.parse::<Ident>()?))
    } else {
        Ok(None)
    }
}

/// Parse `as <ident>` — required for aggregates.
fn parse_required_as(input: ParseStream, func_name: &str) -> syn::Result<Ident> {
    if input.peek(Token![as]) {
        input.parse::<Token![as]>()?;
        Ok(input.parse::<Ident>()?)
    } else {
        Err(input.error(format!(
            "q!: aggregate `{func_name}(...)` requires `as <alias>` after it"
        )))
    }
}

/// Parse dotted ident segments from a sub-stream (inside parens).
fn parse_dotted_ident_from(input: ParseStream) -> syn::Result<Vec<Ident>> {
    let mut segs = vec![input.parse::<Ident>()?];
    while input.peek(Token![.]) {
        input.parse::<Token![.]>()?;
        segs.push(input.parse::<Ident>()?);
    }
    Ok(segs)
}

/// Parse a filter expression that ends when we hit a clause keyword at
/// the top level or end-of-input. We collect tokens into a buffer, then
/// parse that buffer as a `syn::Expr`.
fn parse_filter_expr(input: ParseStream) -> syn::Result<Expr> {
    // Collect raw token trees, stopping at clause keywords.
    let mut tokens = TokenStream2::new();

    while !input.is_empty() {
        // At depth 0, check for clause keywords that terminate the where expr.
        if is_clause_keyword(input) {
            break;
        }

        // Track paren/bracket/brace depth so keywords inside groups
        // don't terminate the expression.
        if input.peek(syn::token::Paren) {
            let content;
            let paren = syn::parenthesized!(content in input);
            let inner = content.parse::<proc_macro2::TokenStream>()?;
            // Re-emit as a parenthesised group.
            let mut group = proc_macro2::Group::new(proc_macro2::Delimiter::Parenthesis, inner);
            group.set_span(paren.span.join());
            tokens.extend(std::iter::once(proc_macro2::TokenTree::Group(group)));
            continue;
        }
        if input.peek(syn::token::Bracket) {
            let content;
            let bracket = syn::bracketed!(content in input);
            let inner = content.parse::<proc_macro2::TokenStream>()?;
            let mut group = proc_macro2::Group::new(proc_macro2::Delimiter::Bracket, inner);
            group.set_span(bracket.span.join());
            tokens.extend(std::iter::once(proc_macro2::TokenTree::Group(group)));
            continue;
        }
        if input.peek(syn::token::Brace) {
            let content;
            let brace = syn::braced!(content in input);
            let inner = content.parse::<proc_macro2::TokenStream>()?;
            let mut group = proc_macro2::Group::new(proc_macro2::Delimiter::Brace, inner);
            group.set_span(brace.span.join());
            tokens.extend(std::iter::once(proc_macro2::TokenTree::Group(group)));
            continue;
        }

        // Default: consume one token tree.
        let tt: proc_macro2::TokenTree = input.parse()?;
        tokens.extend(std::iter::once(tt));
    }

    if tokens.is_empty() {
        return Err(input.error("q!: expected a filter expression after `where`"));
    }

    syn::parse2::<Expr>(tokens)
}

/// Check (without consuming) whether the next token(s) form a clause keyword.
fn is_clause_keyword(input: ParseStream) -> bool {
    input.peek(kw::select)
        || input.peek(kw::group_by)
        || input.peek(kw::having)
        || input.peek(kw::order_by)
        || input.peek(kw::limit)
        || input.peek(kw::offset)
}

/// After a comma in a field-list, check if the NEXT token after the comma
/// would be a clause keyword (meaning the list is over and we should stop).
///
/// We can't lookahead past the comma with `syn::parse::ParseStream::peek`,
/// so we use `fork()`.
fn peek_clause_keyword_after_comma(input: ParseStream) -> bool {
    let fork = input.fork();
    if fork.parse::<Token![,]>().is_err() {
        return false;
    }
    is_clause_keyword(&fork) || fork.is_empty() || fork.peek(kw::asc) || fork.peek(kw::desc)
}

// ── write statement parsers ────────────────────────────────────────

/// Parse a brace-delimited doc map: `{ "key" => expr, ... }`.
fn parse_doc_map(input: ParseStream) -> syn::Result<DocMapAst> {
    let content;
    syn::braced!(content in input);
    let mut pairs = Vec::new();
    while !content.is_empty() {
        let key: LitStr = content.parse()?;
        content.parse::<Token![=>]>()?;
        let val: Expr = content.parse()?;
        pairs.push((key, val));
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        } else {
            break;
        }
    }
    Ok(DocMapAst { pairs })
}

impl Parse for InsertMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<kw::insert>()?;
        input.parse::<kw::into>()?;
        let table = parse_table_arg(input)?;
        input.parse::<kw::values>()?;
        let mut docs = vec![parse_doc_map(input)?];
        while input.peek(Token![,]) && input.peek2(syn::token::Brace) {
            input.parse::<Token![,]>()?;
            docs.push(parse_doc_map(input)?);
        }
        if !input.is_empty() {
            return Err(input.error("q!(insert ...): unexpected tokens after values"));
        }
        Ok(InsertMacro { table, docs })
    }
}

impl Parse for UpdateMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<kw::update>()?;
        let table = parse_table_arg(input)?;
        input.parse::<kw::set>()?;
        let set_doc = parse_doc_map(input)?;
        let where_clause = if input.peek(Token![where]) {
            input.parse::<Token![where]>()?;
            let expr = parse_filter_expr(input)?;
            Some(expr)
        } else {
            None
        };
        if !input.is_empty() {
            return Err(input.error("q!(update ...): unexpected tokens after where clause"));
        }
        Ok(UpdateMacro {
            table,
            set_doc,
            where_clause,
        })
    }
}

impl Parse for DeleteMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<kw::delete>()?;
        input.parse::<kw::from>()?;
        let table = parse_table_arg(input)?;
        if !input.peek(Token![where]) {
            return Err(
                input.error("q!(delete ...): `where` clause is required for delete statements")
            );
        }
        input.parse::<Token![where]>()?;
        let where_clause = parse_filter_expr(input)?;
        if !input.is_empty() {
            return Err(input.error("q!(delete ...): unexpected tokens after where clause"));
        }
        Ok(DeleteMacro {
            table,
            where_clause,
        })
    }
}

impl Parse for UpsertMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<kw::upsert>()?;
        let table = parse_table_arg(input)?;
        input.parse::<kw::key>()?;
        let key_doc = parse_doc_map(input)?;
        input.parse::<kw::value>()?;
        let value_doc = parse_doc_map(input)?;
        if !input.is_empty() {
            return Err(input.error("q!(upsert ...): unexpected tokens after value doc"));
        }
        Ok(UpsertMacro {
            table,
            key_doc,
            value_doc,
        })
    }
}

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
fn lower_doc_map(doc: &DocMapAst) -> TokenStream2 {
    let mut chain = quote! { ::shamir_query_builder::write::doc() };
    for (key, val) in &doc.pairs {
        chain = quote! { #chain.set(#key, #val) };
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

impl QMacro {
    fn to_tokens(&self) -> syn::Result<TokenStream2> {
        match self {
            QMacro::Read(qm) => qm.to_tokens(),
            QMacro::Insert(im) => im.to_tokens(),
            QMacro::Update(um) => um.to_tokens(),
            QMacro::Delete(dm) => dm.to_tokens(),
            QMacro::Upsert(um) => um.to_tokens(),
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
