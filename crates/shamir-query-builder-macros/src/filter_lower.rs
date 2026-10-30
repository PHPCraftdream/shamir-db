//! Expression-lowering logic shared by `filter!` and the `where` clause of `q!`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, BinOp, Expr};

use crate::MacroInput;

// ── public entry for the `filter!` proc-macro ──────────────────────

pub(crate) fn filter_macro(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as MacroInput<Expr>);
    match lower_expr(&input.value, &input.root) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// ── public helper: lower a `syn::Expr` (used by q! as well) ───────

pub(crate) fn lower_expr(expr: &Expr, root: &TokenStream2) -> syn::Result<TokenStream2> {
    lower(expr, root)
}

// ── recursive lowering ─────────────────────────────────────────────

fn lower(expr: &Expr, root: &TokenStream2) -> syn::Result<TokenStream2> {
    match expr {
        // ── parenthesised: (expr) ──────────────────────────────
        Expr::Paren(p) => lower(&p.expr, root),

        // ── binary: a op b ─────────────────────────────────────
        Expr::Binary(bin) => lower_binary(bin, root),

        // ── unary: !expr ───────────────────────────────────────
        Expr::Unary(u) => lower_unary(u, root),

        // ── group (compiler-inserted transparent parens) ───────
        Expr::Group(g) => lower(&g.expr, root),

        // ── predicate call at filter position ──────────────────
        Expr::Call(call) => lower_predicate_call(call, root),

        _ => Err(syn::Error::new_spanned(
            expr,
            "filter!: unsupported expression; expected comparisons \
             (==, !=, >, >=, <, <=), logical operators (&&, ||, !), \
             predicate calls (like, is_null, between, ...), \
             or parenthesised groups",
        )),
    }
}

// ── binary expressions ─────────────────────────────────────────────

fn lower_binary(bin: &syn::ExprBinary, root: &TokenStream2) -> syn::Result<TokenStream2> {
    match &bin.op {
        // Logical combinators — recurse both sides.
        BinOp::And(_) => {
            let lhs = lower(&bin.left, root)?;
            let rhs = lower(&bin.right, root)?;
            Ok(quote! {
                #root::filter::and([#lhs, #rhs])
            })
        }
        BinOp::Or(_) => {
            let lhs = lower(&bin.left, root)?;
            let rhs = lower(&bin.right, root)?;
            Ok(quote! {
                #root::filter::or([#lhs, #rhs])
            })
        }

        // Comparison operators — LHS is a field path, RHS is verbatim.
        BinOp::Eq(_) => lower_cmp(bin, root, quote! { eq }),
        BinOp::Ne(_) => lower_cmp(bin, root, quote! { ne }),
        BinOp::Gt(_) => lower_cmp(bin, root, quote! { gt }),
        BinOp::Ge(_) => lower_cmp(bin, root, quote! { gte }),
        BinOp::Lt(_) => lower_cmp(bin, root, quote! { lt }),
        BinOp::Le(_) => lower_cmp(bin, root, quote! { lte }),

        _ => Err(syn::Error::new_spanned(
            bin.op,
            "filter!: unsupported operator; use ==, !=, >, >=, <, <=, &&, ||",
        )),
    }
}

fn lower_cmp(
    bin: &syn::ExprBinary,
    root: &TokenStream2,
    func: TokenStream2,
) -> syn::Result<TokenStream2> {
    let field = field_path(&bin.left)?;
    let rhs = &bin.right;
    Ok(quote! {
        #root::filter::#func(#field, #rhs)
    })
}

// ── unary: !expr → filter::not ─────────────────────────────────────

fn lower_unary(u: &syn::ExprUnary, root: &TokenStream2) -> syn::Result<TokenStream2> {
    if let syn::UnOp::Not(_) = &u.op {
        let inner = lower(&u.expr, root)?;
        Ok(quote! {
            #root::filter::not(#inner)
        })
    } else {
        Err(syn::Error::new_spanned(
            u,
            "filter!: only the `!` unary operator is supported (negation)",
        ))
    }
}

// ── predicate-call lowering ────────────────────────────────────────

/// Known predicate names and their arities / lowering strategy.
fn lower_predicate_call(call: &syn::ExprCall, root: &TokenStream2) -> syn::Result<TokenStream2> {
    // Extract the callee name (must be a simple path like `like`, `is_null`, etc.)
    let name = match call.func.as_ref() {
        Expr::Path(p) if p.path.segments.len() == 1 => &p.path.segments[0].ident,
        _ => {
            return Err(syn::Error::new_spanned(
                &call.func,
                "filter!: predicate call must be a simple name \
                 (like, ilike, regex, is_null, is_not_null, exists, not_exists, \
                  contains, contains_any, contains_all, in_, not_in, between, \
                  fts, vector_similarity, computed, computed_with_args)",
            ));
        }
    };

    let args = &call.args;

    match name {
        // ── pattern matching: field + pattern ──────────────────
        n if n == "like" || n == "ilike" || n == "regex" => {
            if args.len() != 2 {
                return Err(syn::Error::new_spanned(
                    call,
                    format!("filter!: `{name}` expects 2 arguments: (field, pattern)"),
                ));
            }
            let field = field_path(&args[0])?;
            let pat = &args[1];
            let func_ident = name;
            Ok(quote! {
                #root::filter::#func_ident(#field, #pat)
            })
        }

        // ── unary field predicates ─────────────────────────────
        n if n == "is_null" || n == "is_not_null" || n == "exists" || n == "not_exists" => {
            if args.len() != 1 {
                return Err(syn::Error::new_spanned(
                    call,
                    format!("filter!: `{name}` expects 1 argument: (field)"),
                ));
            }
            let field = field_path(&args[0])?;
            let func_ident = name;
            Ok(quote! {
                #root::filter::#func_ident(#field)
            })
        }

        // ── contains: field + value ────────────────────────────
        n if n == "contains" => {
            if args.len() != 2 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `contains` expects 2 arguments: (field, value)",
                ));
            }
            let field = field_path(&args[0])?;
            let val = &args[1];
            Ok(quote! {
                #root::filter::contains(#field, #val)
            })
        }

        // ── contains_any / contains_all: field + array ─────────
        n if n == "contains_any" || n == "contains_all" => {
            if args.len() != 2 {
                return Err(syn::Error::new_spanned(
                    call,
                    format!("filter!: `{name}` expects 2 arguments: (field, [values...])"),
                ));
            }
            let field = field_path(&args[0])?;
            let vals = &args[1];
            let func_ident = name;
            Ok(quote! {
                #root::filter::#func_ident(#field, #vals)
            })
        }

        // ── in_ / not_in: field + array ────────────────────────
        n if n == "in_" || n == "not_in" => {
            if args.len() != 2 {
                return Err(syn::Error::new_spanned(
                    call,
                    format!("filter!: `{name}` expects 2 arguments: (field, [values...])"),
                ));
            }
            let field = field_path(&args[0])?;
            let vals = &args[1];
            let func_ident = name;
            Ok(quote! {
                #root::filter::#func_ident(#field, #vals)
            })
        }

        // ── between: field + lo + hi ───────────────────────────
        n if n == "between" => {
            if args.len() != 3 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `between` expects 3 arguments: (field, lo, hi)",
                ));
            }
            let field = field_path(&args[0])?;
            let lo = &args[1];
            let hi = &args[2];
            Ok(quote! {
                #root::filter::between(#field, #lo, #hi)
            })
        }

        // ── fts: field + query + mode ──────────────────────────
        n if n == "fts" => {
            if args.len() != 3 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `fts` expects 3 arguments: (field, query, mode)",
                ));
            }
            let field = field_path(&args[0])?;
            let query = &args[1];
            let mode = &args[2];
            Ok(quote! {
                #root::filter::fts(#field, #query, #mode)
            })
        }

        // ── vector_similarity: field + vec_expr + k ────────────
        n if n == "vector_similarity" => {
            if args.len() != 3 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `vector_similarity` expects 3 arguments: (field, vec_expr, k)",
                ));
            }
            let field = field_path(&args[0])?;
            let vec_expr = &args[1];
            let k = &args[2];
            Ok(quote! {
                #root::filter::vector_similarity(#field, #vec_expr, #k)
            })
        }

        // ── computed: expr_op + field + cmp + value ──────────
        n if n == "computed" => {
            if args.len() != 4 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `computed` expects 4 arguments: (expr_op, field, cmp, value)",
                ));
            }
            let expr_op = &args[0];
            let field = field_path(&args[1])?;
            let cmp = &args[2];
            let val = &args[3];
            Ok(quote! {
                #root::filter::computed(#expr_op, #field, #cmp, #val)
            })
        }

        // ── computed_with_args: expr_op + field + expr_args + cmp + value
        n if n == "computed_with_args" => {
            if args.len() != 5 {
                return Err(syn::Error::new_spanned(
                    call,
                    "filter!: `computed_with_args` expects 5 arguments: \
                     (expr_op, field, expr_args, cmp, value)",
                ));
            }
            let expr_op = &args[0];
            let field = field_path(&args[1])?;
            let expr_args = &args[2];
            let cmp = &args[3];
            let val = &args[4];
            Ok(quote! {
                #root::filter::computed_with_args(
                    #expr_op, #field, #expr_args, #cmp, #val
                )
            })
        }

        _ => Err(syn::Error::new_spanned(
            &call.func,
            format!(
                "filter!: unknown predicate `{name}`; supported predicates: \
                 like, ilike, regex, is_null, is_not_null, exists, not_exists, \
                 contains, contains_any, contains_all, in_, not_in, between, \
                 fts, vector_similarity, computed, computed_with_args"
            ),
        )),
    }
}

// ── field-path extraction ──────────────────────────────────────────

/// Convert the LHS of a comparison into a field-path token stream.
///
/// - bare ident `status`       → `"status"`
/// - dotted `address.city`     → `["address", "city"]`
fn field_path(expr: &Expr) -> syn::Result<TokenStream2> {
    let mut segments = Vec::new();
    collect_field_segments(expr, &mut segments)?;

    Ok(segments_to_field_path(&segments))
}

pub(crate) fn ident_literal(ident: &syn::Ident) -> syn::LitStr {
    let value = ident.to_string();
    syn::LitStr::new(value.strip_prefix("r#").unwrap_or(&value), ident.span())
}

pub(crate) fn segments_to_field_path(segments: &[syn::Ident]) -> TokenStream2 {
    let literals: Vec<_> = segments.iter().map(ident_literal).collect();
    if literals.len() == 1 {
        let literal = &literals[0];
        quote! { #literal }
    } else {
        quote! { [#(#literals),*] }
    }
}

fn collect_field_segments(expr: &Expr, out: &mut Vec<syn::Ident>) -> syn::Result<()> {
    match expr {
        Expr::Path(p) if p.path.segments.len() == 1 => {
            out.push(p.path.segments[0].ident.clone());
            Ok(())
        }
        Expr::Field(f) => {
            collect_field_segments(&f.base, out)?;
            match &f.member {
                syn::Member::Named(ident) => {
                    out.push(ident.clone());
                    Ok(())
                }
                syn::Member::Unnamed(idx) => Err(syn::Error::new_spanned(
                    idx,
                    "filter!: tuple-index field access is not supported in field paths",
                )),
            }
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "filter!: expected a field name (ident) or dotted field path (e.g. `address.city`)",
        )),
    }
}
