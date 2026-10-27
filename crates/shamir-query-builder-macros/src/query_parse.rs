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
//! q!( call <fn>(args...) )        → CallOp
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

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, LitInt, LitStr, Token};

pub(crate) use crate::query_parse_lower::q_macro;

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
    syn::custom_keyword!(call);
}

// ── AST ────────────────────────────────────────────────────────────

pub(crate) struct QueryMacro {
    pub(crate) table: TableArg,
    pub(crate) where_clause: Option<Expr>,
    pub(crate) group_by_fields: Vec<FieldPathAst>,
    pub(crate) having_clause: Option<Expr>,
    pub(crate) select_items: Vec<SelectItemAst>,
    pub(crate) select_distinct: bool,
    pub(crate) order_by_items: Vec<(FieldPathAst, OrderDir)>,
    pub(crate) limit: Option<LitInt>,
    pub(crate) offset: Option<LitInt>,
}

pub(crate) enum TableArg {
    /// Single ident or string literal → `Query::from(...)`.
    Simple(TableName),
    /// `repo.table` → `Query::with_repo("repo", "table")`.
    Repo { repo: Ident, table: Ident },
}

pub(crate) enum TableName {
    Ident(Ident),
    Lit(LitStr),
}

pub(crate) enum FieldPathAst {
    Idents(Vec<Ident>),
    Lit(LitStr),
}

pub(crate) enum OrderDir {
    Asc,
    Desc,
}

/// AST node for a single select item.
pub(crate) enum SelectItemAst {
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
pub(crate) struct DocMapAst {
    pub(crate) pairs: Vec<(LitStr, Expr)>,
}

/// `q!(insert into <table> values <doc> [, <doc>]*)`.
pub(crate) struct InsertMacro {
    pub(crate) table: TableArg,
    pub(crate) docs: Vec<DocMapAst>,
}

/// `q!(update <table> set <doc> [where <expr>])`.
pub(crate) struct UpdateMacro {
    pub(crate) table: TableArg,
    pub(crate) set_doc: DocMapAst,
    pub(crate) where_clause: Option<Expr>,
}

/// `q!(delete from <table> where <expr>)`.
pub(crate) struct DeleteMacro {
    pub(crate) table: TableArg,
    pub(crate) where_clause: Expr,
}

/// `q!(upsert <table> key <doc> value <doc>)`.
pub(crate) struct UpsertMacro {
    pub(crate) table: TableArg,
    pub(crate) key_doc: DocMapAst,
    pub(crate) value_doc: DocMapAst,
}

/// `q!(call <fn_name>(arg1, arg2, ...))` or `q!(call <repo>.<fn_name>(...))`.
pub(crate) struct CallMacro {
    /// Function name — ident or string literal.
    pub(crate) name: CallName,
    /// Repository qualifier — the ident before the dot in
    /// `call my_repo.daily(...)`; defaults to `"main"`.
    pub(crate) repo: Option<Ident>,
    /// Positional arguments (each is an expression → `FilterValue`).
    pub(crate) args: Vec<Expr>,
}

pub(crate) enum CallName {
    Ident(Ident),
    Lit(LitStr),
}

/// Top-level AST: the first keyword selects the variant.
pub(crate) enum QMacro {
    Read(Box<QueryMacro>),
    Insert(InsertMacro),
    Update(UpdateMacro),
    Delete(DeleteMacro),
    Upsert(UpsertMacro),
    Call(CallMacro),
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
        } else if input.peek(kw::call) {
            Ok(QMacro::Call(input.parse()?))
        } else {
            Err(input
                .error("q!: expected `from`, `insert`, `update`, `delete`, `upsert`, or `call`"))
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
            let expr = parse_filter_expr(input, "where")?;
            Some(expr)
        } else {
            None
        };

        // -- group_by <field>, ... (optional) --
        let mut group_by_fields = Vec::new();
        if input.peek(kw::group_by) {
            input.parse::<kw::group_by>()?;
            loop {
                group_by_fields.push(parse_field_path(input)?);
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                    if input.is_empty()
                        || input.peek(kw::having)
                        || input.peek(kw::select)
                        || input.peek(kw::order_by)
                        || input.peek(kw::limit)
                        || input.peek(kw::offset)
                    {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        // -- having <expr> (optional, after group_by) --
        let having_clause = if input.peek(kw::having) {
            input.parse::<kw::having>()?;
            let expr = parse_filter_expr(input, "having")?;
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
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                    if input.is_empty()
                        || input.peek(kw::order_by)
                        || input.peek(kw::limit)
                        || input.peek(kw::offset)
                    {
                        break;
                    }
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
                let field = parse_field_path(input)?;
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
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                    if input.is_empty() || input.peek(kw::limit) || input.peek(kw::offset) {
                        break;
                    }
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

        // Reject duplicate select output keys (FB738525): the engine would
        // silently overwrite one aggregate with another sharing its alias.
        check_duplicate_select_keys(&select_items)?;

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

fn unraw_ident(ident: &Ident) -> String {
    let value = ident.to_string();
    value.strip_prefix("r#").unwrap_or(&value).to_owned()
}

fn parse_field_path(input: ParseStream) -> syn::Result<FieldPathAst> {
    if input.peek(LitStr) {
        return Ok(FieldPathAst::Lit(input.parse()?));
    }
    let mut segments = vec![input.parse::<Ident>()?];
    while input.peek(Token![.]) {
        let fork = input.fork();
        fork.parse::<Token![.]>()?;
        if !fork.peek(Ident) {
            break;
        }
        input.parse::<Token![.]>()?;
        segments.push(input.parse::<Ident>()?);
    }
    Ok(FieldPathAst::Idents(segments))
}

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
            let repo = unraw_ident(&first);
            let table = unraw_ident(&second);
            return Ok(TableArg::Repo {
                repo: Ident::new(&repo, first.span()),
                table: Ident::new(&table, second.span()),
            });
        }
    }

    Ok(TableArg::Simple(TableName::Ident(first)))
}

/// Output key a select item produces (alias, or the dotted field path).
fn select_item_key(item: &SelectItemAst) -> Option<String> {
    match item {
        SelectItemAst::All => None,
        SelectItemAst::Field { segments, alias } => Some(match alias {
            Some(a) => unraw_ident(a),
            None => segments
                .iter()
                .map(unraw_ident)
                .collect::<Vec<_>>()
                .join("."),
        }),
        SelectItemAst::CountAll { alias } => Some(match alias {
            Some(a) => unraw_ident(a),
            None => "count".to_owned(),
        }),
        SelectItemAst::BuiltinAgg { alias, .. }
        | SelectItemAst::AggFn { alias, .. }
        | SelectItemAst::Func { alias, .. } => Some(unraw_ident(alias)),
    }
}

/// Reject two select items producing the same output key.
fn check_duplicate_select_keys(items: &[SelectItemAst]) -> syn::Result<()> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if let Some(key) = select_item_key(item) {
            if seen.contains(&key) {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "q!: duplicate select output key `{key}`; give each \
                         aggregate a distinct `as` alias"
                    ),
                ));
            }
            seen.push(key);
        }
    }
    Ok(())
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
                        require_exhausted(&content, "count")?;
                        let alias = parse_optional_as(input)?;
                        return Ok(SelectItemAst::CountAll { alias });
                    }

                    // count(field)
                    let field_segs = parse_dotted_ident_from(&content)?;
                    require_exhausted(&content, "count")?;
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
                    require_exhausted(&content, &id_str)?;
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
                    require_exhausted(&content, "agg_fn")?;
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
                    require_exhausted(&content, "func")?;
                    let alias = parse_required_as(input, "func")?;
                    return Ok(SelectItemAst::Func { name, args, alias });
                }
                _ => {
                    // Unknown function-like select item: report precisely
                    // instead of falling through to the clause-order error.
                    return Err(syn::Error::new(
                        id.span(),
                        format!(
                            "q!: unknown select function `{id_str}`; \
                             supported: count, sum, avg, min, max, agg_fn, func"
                        ),
                    ));
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

/// Require a parenthesised select-function content stream to be fully
/// consumed (E24D5F62); otherwise report precise trailing-token error.
fn require_exhausted(input: ParseStream, id_str: &str) -> syn::Result<()> {
    if !input.is_empty() {
        return Err(input.error(format!(
            "q!: unexpected trailing tokens inside `{id_str}(...)`"
        )));
    }
    Ok(())
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
        if !input.peek(Ident) {
            return Err(input.error("expected field name after `.`"));
        }
        segs.push(input.parse::<Ident>()?);
    }
    Ok(segs)
}

/// Parse a filter expression that ends when we hit a clause keyword at
/// the top level or end-of-input. We collect tokens into a buffer, then
/// parse that buffer as a `syn::Expr`.
fn parse_filter_expr(input: ParseStream, clause: &str) -> syn::Result<Expr> {
    // Collect raw token trees, stopping at clause keywords.
    let mut tokens = TokenStream2::new();

    while !input.is_empty() {
        // At depth 0, check for clause keywords that terminate the expr.
        // A keyword only terminates if the tokens collected so far already
        // form a complete expression; otherwise it is part of the expression
        // itself (e.g. dotted segment `foo.select` or RHS `x == group_by`).
        if is_clause_keyword(input)
            && (tokens.is_empty() || syn::parse2::<Expr>(tokens.clone()).is_ok())
        {
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
        return Err(input.error(format!("q!: expected a filter expression after `{clause}`")));
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
        } else if !content.is_empty() {
            return Err(content.error(
                "q!: unexpected trailing tokens in doc map; expected \"key\" => value pairs separated by `,`",
            ));
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
            let expr = parse_filter_expr(input, "where")?;
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
        let where_clause = parse_filter_expr(input, "where")?;
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

impl Parse for CallMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<kw::call>()?;

        // Function name: string literal, bare ident, or `<repo>.<name>` —
        // the qualifier before the dot is the repository, the ident after
        // the dot is the procedure name (2C459E8E).
        let (name, repo) = if input.peek(LitStr) {
            (CallName::Lit(input.parse()?), None)
        } else {
            let first: Ident = input.parse()?;
            if input.peek(Token![.]) {
                let fork = input.fork();
                fork.parse::<Token![.]>()?;
                if fork.peek(Ident) {
                    input.parse::<Token![.]>()?;
                    let second: Ident = input.parse()?;
                    (CallName::Ident(second), Some(first))
                } else {
                    (CallName::Ident(first), None)
                }
            } else {
                (CallName::Ident(first), None)
            }
        };

        // Arguments in parentheses: (arg1, arg2, ...)
        let args = if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let mut args = Vec::new();
            while !content.is_empty() {
                args.push(content.parse::<Expr>()?);
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                } else if !content.is_empty() {
                    return Err(content.error(
                        "q!(call ...): unexpected trailing tokens in argument list; separate arguments with `,`",
                    ));
                } else {
                    break;
                }
            }
            args
        } else {
            Vec::new()
        };

        if !input.is_empty() {
            return Err(input.error("q!(call ...): unexpected tokens after arguments"));
        }

        Ok(CallMacro { name, repo, args })
    }
}
